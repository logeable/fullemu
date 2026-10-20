//! 管理教学文件系统的磁盘格式、目录项和文件内容。
//!
//! 磁盘布局和 inode 思路参考 rCore EasyFS；本模块自行编码磁盘格式，并通过
//! `BlockDevice` 访问扇区。文件系统对象拥有块设备，所有磁盘操作都经由该对象完成。

use alloc::{boxed::Box, vec::Vec};

use crate::block::{BlockDevice, SECTOR_SIZE};

const SUPERBLOCK_SECTOR: u64 = 0;
const SUPERBLOCK_MAGIC: &[u8; 8] = b"FULLEMU\0";
const FORMAT_VERSION: u32 = 2;
const INODE_COUNT: u64 = 128;
const DISK_INODE_SIZE: usize = 128;
const INODE_TYPE_OFFSET: usize = 8;
const INODE_DIRECT_OFFSET: usize = 16;
const DIRECT_BLOCK_COUNT: usize = 14;
const ROOT_INODE_ID: InodeId = InodeId(0);
const DIRECTORY_ENTRY_SIZE: usize = 32;
const DIRECTORY_NAME_SIZE: usize = DIRECTORY_ENTRY_SIZE - core::mem::size_of::<u32>();
const BITS_PER_SECTOR: u64 = (SECTOR_SIZE * 8) as u64;
const MAX_FILE_SIZE: usize = SECTOR_SIZE * DIRECT_BLOCK_COUNT;

const MAGIC_RANGE: core::ops::Range<usize> = 0..8;
const VERSION_OFFSET: usize = 8;
const SECTOR_SIZE_OFFSET: usize = 12;
const DEVICE_SECTOR_COUNT_OFFSET: usize = 16;
const MOUNT_COUNT_OFFSET: usize = 24;
const INODE_BITMAP_SECTORS_OFFSET: usize = 32;
const INODE_AREA_SECTORS_OFFSET: usize = 40;
const DATA_BITMAP_SECTORS_OFFSET: usize = 48;
const DATA_AREA_SECTORS_OFFSET: usize = 56;
const INODE_COUNT_OFFSET: usize = 64;
const ROOT_INODE_ID_OFFSET: usize = 72;
const DATA_START_SECTOR_OFFSET: usize = 80;

/// inode 在当前文件系统卷中的编号。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InodeId(u32);

impl InodeId {
    /// 返回稳定的卷内编号。
    pub(crate) const fn index(self) -> u32 {
        self.0
    }
}

/// inode 表示的对象类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InodeKind {
    /// 普通文件。
    File,
    /// 目录。
    Directory,
}

impl InodeKind {
    const fn disk_value(self) -> u8 {
        match self {
            Self::File => 1,
            Self::Directory => 2,
        }
    }

    fn from_disk(value: u8) -> Result<Self, FileSystemError> {
        match value {
            1 => Ok(Self::File),
            2 => Ok(Self::Directory),
            _ => Err(FileSystemError::InvalidInode),
        }
    }
}

/// inode 中可供上层查看的基本属性。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Metadata {
    /// 文件或目录的字节长度。
    pub(crate) size: usize,
    /// inode 表示普通文件还是目录。
    pub(crate) kind: InodeKind,
}

/// 目录项的短期副本，不暴露磁盘缓冲区。
#[derive(Clone, Copy)]
pub(crate) struct DirectoryEntry {
    /// 被目录项引用的 inode。
    pub(crate) inode_id: InodeId,
    /// 目标 inode 类型。
    pub(crate) kind: InodeKind,
    /// UTF-8 文件名的固定缓冲区。
    pub(crate) name: [u8; DIRECTORY_NAME_SIZE],
    /// 文件名的有效字节数。
    pub(crate) name_length: usize,
}

/// 打开文件时使用的访问和创建选项。
#[derive(Clone, Copy)]
pub(crate) struct OpenOptions {
    /// 是否允许读取文件内容。
    pub(crate) readable: bool,
    /// 是否允许写入文件内容。
    pub(crate) writable: bool,
    /// 文件不存在时是否创建普通文件。
    pub(crate) create: bool,
    /// 文件已存在且同时请求创建时是否报错。
    pub(crate) exclusive: bool,
    /// 打开时是否清空普通文件内容。
    pub(crate) truncate: bool,
    /// 写入位置是否总取文件末尾。
    pub(crate) append: bool,
    /// 是否要求目标必须为目录。
    pub(crate) directory_only: bool,
}

/// 进程打开文件描述符所引用的状态。
#[derive(Clone, Copy)]
pub(crate) struct OpenFile {
    /// 文件系统 inode。
    pub(crate) inode_id: InodeId,
    /// inode 类型。
    pub(crate) kind: InodeKind,
    /// 当前读写位置；目录描述符把它解释为目录项序号。
    pub(crate) position: usize,
    /// 此描述符是否允许读取。
    pub(crate) readable: bool,
    /// 此描述符是否允许写入。
    pub(crate) writable: bool,
    /// 普通文件写入是否追加到末尾。
    pub(crate) append: bool,
}

/// 每个进程独立拥有的文件描述符表；0、1、2 由 syscall 层保留给控制台。
pub(crate) struct FileDescriptorTable {
    slots: Vec<Option<OpenFile>>,
}

impl FileDescriptorTable {
    pub(crate) const fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// 复制文件描述符及当前位置；本阶段 fork 后两边的位置独立变化。
    pub(crate) fn duplicate(&self) -> Result<Self, ()> {
        let mut slots = Vec::new();
        slots.try_reserve_exact(self.slots.len()).map_err(|_| ())?;
        slots.extend_from_slice(&self.slots);
        Ok(Self { slots })
    }

    pub(crate) fn insert(&mut self, file: OpenFile) -> Result<usize, ()> {
        if let Some(index) = self.slots.iter().position(Option::is_none) {
            self.slots[index] = Some(file);
            return Ok(index + 3);
        }
        self.slots.try_reserve(1).map_err(|_| ())?;
        self.slots.push(Some(file));
        Ok(self.slots.len() + 2)
    }

    pub(crate) fn get(&self, descriptor: usize) -> Option<OpenFile> {
        self.slots
            .get(descriptor.checked_sub(3)?)
            .copied()
            .flatten()
    }

    pub(crate) fn update(&mut self, descriptor: usize, file: OpenFile) -> Result<(), ()> {
        let Some(slot) = descriptor
            .checked_sub(3)
            .and_then(|index| self.slots.get_mut(index))
        else {
            return Err(());
        };
        if slot.is_none() {
            return Err(());
        }
        *slot = Some(file);
        Ok(())
    }

    pub(crate) fn close(&mut self, descriptor: usize) -> Result<(), ()> {
        let Some(slot) = descriptor
            .checked_sub(3)
            .and_then(|index| self.slots.get_mut(index))
        else {
            return Err(());
        };
        if slot.take().is_none() {
            return Err(());
        }
        Ok(())
    }

    pub(crate) fn clear(&mut self) {
        self.slots = Vec::new();
    }

    pub(crate) fn contains_inode(&self, inode_id: InodeId) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|file| file.inode_id == inode_id)
    }
}

/// 文件系统操作失败的原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileSystemError {
    /// 块设备容量不足以容纳文件系统元数据和数据区。
    DeviceTooSmall,
    /// 设备读请求失败。
    DeviceRead,
    /// 设备写请求失败。
    DeviceWrite,
    /// 非空扇区不是本项目的文件系统卷，拒绝覆盖。
    UnknownVolume,
    /// 磁盘超级块格式版本尚不支持。
    UnsupportedVersion,
    /// 磁盘记录的扇区大小与块设备契约不一致。
    InvalidSectorSize,
    /// 磁盘记录的容量与当前设备容量不一致。
    DeviceCapacityChanged,
    /// 挂载计数溢出。
    MountCountOverflow,
    /// 超级块区域长度或起始位置与当前布局算法不一致。
    InvalidLayout,
    /// 根 inode 位图或 inode 内容无效。
    InvalidRootInode,
    /// inode 位图或 inode 内容无效。
    InvalidInode,
    /// 当前目录 inode 不是目录。
    NotDirectory,
    /// 当前 inode 不是普通文件。
    NotFile,
    /// 目录或文件名不符合当前磁盘格式限制。
    InvalidName,
    /// 文件名超过当前目录项可存放的字节数。
    NameTooLong,
    /// 目录中已经存在同名项。
    AlreadyExists,
    /// 指定的 inode 没有分配。
    InodeNotFound,
    /// inode 位图或数据位图没有可分配项。
    NoSpace,
    /// 文件偏移超出当前实现支持的直接块范围。
    FileTooLarge,
    /// 写入偏移大于当前文件长度；本阶段不创建稀疏文件。
    InvalidOffset,
    /// 写入扇区后读回校验不一致。
    SectorWriteMismatch,
    /// 文件系统运行时对象尚未初始化。
    NotMounted,
    /// 已经存在已挂载文件系统。
    AlreadyMounted,
    /// 路径没有对应目录项。
    NoEntry,
    /// 路径中间组件不是目录。
    PathNotDirectory,
    /// 目标是目录，但当前接口只删除普通文件。
    IsDirectory,
}

impl FileSystemError {
    /// 返回适合启动日志的错误说明。
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::DeviceTooSmall => "块设备容量不足以容纳文件系统",
            Self::DeviceRead => "读取文件系统扇区失败",
            Self::DeviceWrite => "写入文件系统扇区失败",
            Self::UnknownVolume => "磁盘不是空白卷或 fullemu 文件系统",
            Self::UnsupportedVersion => "文件系统格式版本不受支持",
            Self::InvalidSectorSize => "文件系统记录的扇区大小无效",
            Self::DeviceCapacityChanged => "文件系统记录的设备容量与当前设备不一致",
            Self::MountCountOverflow => "文件系统挂载计数溢出",
            Self::InvalidLayout => "文件系统磁盘区域布局无效",
            Self::InvalidRootInode => "根目录 inode 无效",
            Self::InvalidInode => "inode 内容无效",
            Self::NotDirectory => "inode 不是目录",
            Self::NotFile => "inode 不是普通文件",
            Self::InvalidName => "文件名无效或超过磁盘格式限制",
            Self::NameTooLong => "文件名超过磁盘格式限制",
            Self::AlreadyExists => "目录中已经存在同名项",
            Self::InodeNotFound => "inode 未分配",
            Self::NoSpace => "文件系统没有可分配空间",
            Self::FileTooLarge => "文件超过当前直接块寻址范围",
            Self::InvalidOffset => "写入位置不能越过当前文件末尾",
            Self::SectorWriteMismatch => "扇区写入后校验不一致",
            Self::NotMounted => "文件系统尚未挂载",
            Self::AlreadyMounted => "文件系统已经挂载",
            Self::NoEntry => "路径不存在",
            Self::PathNotDirectory => "路径中间组件不是目录",
            Self::IsDirectory => "当前操作不支持目录",
        }
    }
}

/// 文件系统卷在磁盘上的连续区域布局。
#[derive(Clone, Copy)]
struct FileSystemLayout {
    inode_bitmap_start: u64,
    inode_bitmap_sectors: u64,
    inode_area_start: u64,
    inode_area_sectors: u64,
    data_bitmap_start: u64,
    data_bitmap_sectors: u64,
    data_area_start: u64,
    data_area_sectors: u64,
}

impl FileSystemLayout {
    fn calculate(total_sectors: u64) -> Result<Self, FileSystemError> {
        let inode_bitmap_sectors = div_ceil(INODE_COUNT, BITS_PER_SECTOR);
        let inode_area_bytes = INODE_COUNT
            .checked_mul(DISK_INODE_SIZE as u64)
            .ok_or(FileSystemError::DeviceTooSmall)?;
        let inode_area_sectors = div_ceil(inode_area_bytes, SECTOR_SIZE as u64);
        let inode_bitmap_start = 1;
        let inode_area_start = inode_bitmap_start + inode_bitmap_sectors;
        let data_bitmap_start = inode_area_start + inode_area_sectors;
        let fixed_sectors = data_bitmap_start;
        let mut data_bitmap_sectors = 0;

        loop {
            let Some(data_area_sectors) =
                total_sectors.checked_sub(fixed_sectors + data_bitmap_sectors)
            else {
                return Err(FileSystemError::DeviceTooSmall);
            };
            if data_area_sectors == 0 {
                return Err(FileSystemError::DeviceTooSmall);
            }
            let required_bitmap_sectors = div_ceil(data_area_sectors, BITS_PER_SECTOR);
            if required_bitmap_sectors == data_bitmap_sectors {
                return Ok(Self {
                    inode_bitmap_start,
                    inode_bitmap_sectors,
                    inode_area_start,
                    inode_area_sectors,
                    data_bitmap_start,
                    data_bitmap_sectors,
                    data_area_start: data_bitmap_start + data_bitmap_sectors,
                    data_area_sectors,
                });
            }
            data_bitmap_sectors = required_bitmap_sectors;
        }
    }
}

/// 文件系统卷的基本信息。
#[derive(Clone, Copy)]
pub(crate) struct FileSystemInfo {
    /// 块设备的逻辑扇区总数。
    pub(crate) sector_count: u64,
    /// 此卷被挂载的累计次数。
    pub(crate) mount_count: u64,
    /// inode 区可容纳的 inode 数量。
    pub(crate) inode_count: u64,
    /// 数据区可分配的扇区数量。
    pub(crate) data_sector_count: u64,
    /// 本次启动是否创建了全新的文件系统卷。
    pub(crate) was_formatted: bool,
}

/// 已挂载文件系统；对象持有块设备，inode 通过编号引用磁盘记录。
pub(crate) struct FileSystem<D: BlockDevice> {
    device: D,
    layout: FileSystemLayout,
    info: FileSystemInfo,
}

impl<D: BlockDevice> FileSystem<D> {
    /// 只在超级块和全部元数据区为空时格式化设备。
    pub(crate) fn format(mut device: D) -> Result<Self, FileSystemError> {
        let layout = FileSystemLayout::calculate(device.sector_count())?;
        let mut superblock = [0; SECTOR_SIZE];
        read_sector(&mut device, SUPERBLOCK_SECTOR, &mut superblock)?;
        if superblock.iter().any(|byte| *byte != 0) {
            return Err(FileSystemError::UnknownVolume);
        }
        verify_format_metadata_is_empty(&mut device, layout)?;
        initialize_volume(&mut device, layout)?;
        let info = file_system_info(device.sector_count(), 1, layout, true);
        Ok(Self {
            device,
            layout,
            info,
        })
    }

    /// 校验已有文件系统并挂载；不会格式化未知卷。
    pub(crate) fn mount(mut device: D) -> Result<Self, FileSystemError> {
        let sector_count = device.sector_count();
        let layout = FileSystemLayout::calculate(sector_count)?;
        let mut superblock = [0; SECTOR_SIZE];
        read_sector(&mut device, SUPERBLOCK_SECTOR, &mut superblock)?;
        let previous_mount_count = decode_superblock(&superblock, sector_count, layout)?;
        validate_root_inode(&mut device, layout)?;
        let mount_count = previous_mount_count
            .checked_add(1)
            .ok_or(FileSystemError::MountCountOverflow)?;
        encode_superblock(&mut superblock, sector_count, mount_count, layout);
        write_and_verify_sector(&mut device, SUPERBLOCK_SECTOR, &superblock)?;
        let info = file_system_info(sector_count, mount_count, layout, false);
        Ok(Self {
            device,
            layout,
            info,
        })
    }

    /// 对全空设备格式化，对其他设备执行严格挂载。
    ///
    /// 这是启动策略接口；实际格式化和挂载分别由 [`Self::format`]、[`Self::mount`] 完成。
    pub(crate) fn mount_or_format(device: D) -> Result<Self, FileSystemError> {
        let mut probe = [0; SECTOR_SIZE];
        let mut device = device;
        read_sector(&mut device, SUPERBLOCK_SECTOR, &mut probe)?;
        if probe.iter().all(|byte| *byte == 0) {
            Self::format(device)
        } else {
            Self::mount(device)
        }
    }

    /// 返回卷信息。
    pub(crate) const fn info(&self) -> FileSystemInfo {
        self.info
    }

    /// 查询 inode 的类型和长度。
    pub(crate) fn metadata(&mut self, inode_id: InodeId) -> Result<Metadata, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        Ok(Metadata {
            size: inode.size,
            kind: inode.kind,
        })
    }

    /// 在目录中按名称查找 inode。
    pub(crate) fn lookup(
        &mut self,
        directory_id: InodeId,
        name: &str,
    ) -> Result<Option<InodeId>, FileSystemError> {
        let name_bytes = validate_name(name)?;
        let directory = self.read_inode(directory_id)?;
        if directory.kind != InodeKind::Directory {
            return Err(FileSystemError::NotDirectory);
        }

        let mut offset = 0;
        let mut entry = [0; DIRECTORY_ENTRY_SIZE];
        while offset < directory.size {
            self.read_inode_data(directory_id, offset, &mut entry)?;
            if let Some((entry_name, inode_id)) = decode_directory_entry(&entry)? {
                if entry_name == name_bytes {
                    // 校验目录项指向已分配且格式有效的 inode，避免把坏磁盘编号交给调用方。
                    self.read_inode(inode_id)?;
                    return Ok(Some(inode_id));
                }
            }
            offset += DIRECTORY_ENTRY_SIZE;
        }
        Ok(None)
    }

    /// 按根目录相对路径查找 inode，用于删除前检查进程是否仍持有该文件。
    pub(crate) fn lookup_path(&mut self, path: &str) -> Result<InodeId, FileSystemError> {
        self.resolve_path(path)
    }

    /// 在目录中创建普通文件。
    pub(crate) fn create_file(
        &mut self,
        directory_id: InodeId,
        name: &str,
    ) -> Result<InodeId, FileSystemError> {
        self.create_inode(directory_id, name, InodeKind::File)
    }

    /// 在目录中创建子目录。
    pub(crate) fn create_directory(
        &mut self,
        directory_id: InodeId,
        name: &str,
    ) -> Result<InodeId, FileSystemError> {
        self.create_inode(directory_id, name, InodeKind::Directory)
    }

    /// 创建目录路径；相对路径从卷根目录开始解析。
    pub(crate) fn create_directory_path(&mut self, path: &str) -> Result<InodeId, FileSystemError> {
        let (parent_id, name) = self.resolve_parent(path)?;
        self.create_directory(parent_id, name)
    }

    /// 按路径打开文件或目录，并按选项创建、截断或追加。
    pub(crate) fn open_path(
        &mut self,
        path: &str,
        options: OpenOptions,
    ) -> Result<OpenFile, FileSystemError> {
        let inode_id = if path == "." || path.bytes().all(|byte| byte == b'/') {
            if options.create || options.truncate {
                return Err(FileSystemError::IsDirectory);
            }
            ROOT_INODE_ID
        } else {
            let (parent_id, name) = self.resolve_parent(path)?;
            match self.lookup(parent_id, name)? {
                Some(_) if options.create && options.exclusive => {
                    return Err(FileSystemError::AlreadyExists);
                }
                Some(inode_id) => inode_id,
                None if options.create => self.create_file(parent_id, name)?,
                None => return Err(FileSystemError::NoEntry),
            }
        };
        let mut inode = self.read_inode(inode_id)?;
        if options.directory_only && inode.kind != InodeKind::Directory {
            return Err(FileSystemError::NotDirectory);
        }
        if inode.kind == InodeKind::Directory && (options.writable || options.truncate) {
            return Err(FileSystemError::IsDirectory);
        }
        if options.truncate && inode.kind == InodeKind::File {
            for block in inode
                .direct_blocks
                .iter()
                .copied()
                .filter(|block| *block != 0)
            {
                self.release_data_sector(block)?;
            }
            inode = DiskInode::empty(InodeKind::File);
            self.write_inode(inode_id, &inode)?;
        }
        Ok(OpenFile {
            inode_id,
            kind: inode.kind,
            position: if options.append { inode.size } else { 0 },
            readable: options.readable,
            writable: options.writable,
            append: options.append,
        })
    }

    /// 删除普通文件并释放其数据扇区；目录必须为空且由后续接口另行删除。
    pub(crate) fn unlink_file(&mut self, path: &str) -> Result<(), FileSystemError> {
        let (parent_id, name) = self.resolve_parent(path)?;
        let inode_id = self
            .lookup(parent_id, name)?
            .ok_or(FileSystemError::NoEntry)?;
        let inode = self.read_inode(inode_id)?;
        if inode.kind == InodeKind::Directory {
            return Err(FileSystemError::IsDirectory);
        }

        let mut parent = self.read_inode(parent_id)?;
        let slot_count = parent.size / DIRECTORY_ENTRY_SIZE;
        let mut found = false;
        for slot_index in 0..slot_count {
            let offset = slot_index * DIRECTORY_ENTRY_SIZE;
            let mut entry = [0; DIRECTORY_ENTRY_SIZE];
            self.read_inode_data(parent_id, offset, &mut entry)?;
            if decode_directory_entry(&entry)?.is_some_and(|(_, entry_id)| entry_id == inode_id) {
                let block_index = offset / SECTOR_SIZE;
                let block_offset = offset % SECTOR_SIZE;
                let sector_id = parent.direct_blocks[block_index];
                let mut sector = [0; SECTOR_SIZE];
                read_sector(&mut self.device, sector_id, &mut sector)?;
                sector[block_offset..block_offset + DIRECTORY_ENTRY_SIZE].fill(0);
                write_and_verify_sector(&mut self.device, sector_id, &sector)?;
                found = true;
                break;
            }
        }
        if !found {
            return Err(FileSystemError::InvalidInode);
        }

        for block in inode
            .direct_blocks
            .iter()
            .copied()
            .filter(|block| *block != 0)
        {
            self.release_data_sector(block)?;
        }
        self.release_inode(inode_id)?;
        // 删除尾部空槽可以让目录长度随删除收缩，但保留已分配目录块，避免移动其他目录项。
        while parent.size >= DIRECTORY_ENTRY_SIZE {
            let offset = parent.size - DIRECTORY_ENTRY_SIZE;
            let mut entry = [0; DIRECTORY_ENTRY_SIZE];
            self.read_inode_data(parent_id, offset, &mut entry)?;
            if decode_directory_entry(&entry)?.is_some() {
                break;
            }
            parent.size -= DIRECTORY_ENTRY_SIZE;
        }
        self.write_inode(parent_id, &parent)
    }

    /// 返回目录中记录的槽位数量；已删除项仍占用槽位序号。
    pub(crate) fn directory_slot_count(
        &mut self,
        directory_id: InodeId,
    ) -> Result<usize, FileSystemError> {
        let directory = self.read_inode(directory_id)?;
        if directory.kind != InodeKind::Directory {
            return Err(FileSystemError::NotDirectory);
        }
        if directory.size % DIRECTORY_ENTRY_SIZE != 0 {
            return Err(FileSystemError::InvalidInode);
        }
        Ok(directory.size / DIRECTORY_ENTRY_SIZE)
    }

    /// 读取指定目录槽位；删除后留下的空槽返回 `None`。
    pub(crate) fn directory_entry(
        &mut self,
        directory_id: InodeId,
        slot_index: usize,
    ) -> Result<Option<DirectoryEntry>, FileSystemError> {
        let offset = slot_index
            .checked_mul(DIRECTORY_ENTRY_SIZE)
            .ok_or(FileSystemError::InvalidInode)?;
        let directory = self.read_inode(directory_id)?;
        if directory.kind != InodeKind::Directory {
            return Err(FileSystemError::NotDirectory);
        }
        if offset + DIRECTORY_ENTRY_SIZE > directory.size {
            return Ok(None);
        }
        let mut encoded = [0; DIRECTORY_ENTRY_SIZE];
        self.read_inode_data(directory_id, offset, &mut encoded)?;
        let Some((name, inode_id)) = decode_directory_entry(&encoded)? else {
            return Ok(None);
        };
        let inode = self.read_inode(inode_id)?;
        let mut name_buffer = [0; DIRECTORY_NAME_SIZE];
        name_buffer[..name.len()].copy_from_slice(name);
        Ok(Some(DirectoryEntry {
            inode_id,
            kind: inode.kind,
            name: name_buffer,
            name_length: name.len(),
        }))
    }

    /// 从文件的指定偏移读取数据；到达文件末尾时返回实际读取长度 0。
    pub(crate) fn read_at(
        &mut self,
        inode_id: InodeId,
        offset: usize,
        destination: &mut [u8],
    ) -> Result<usize, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        if inode.kind != InodeKind::File {
            return Err(FileSystemError::NotFile);
        }
        if offset >= inode.size || destination.is_empty() {
            return Ok(0);
        }
        let amount = core::cmp::min(destination.len(), inode.size - offset);
        self.read_inode_data(inode_id, offset, &mut destination[..amount])?;
        Ok(amount)
    }

    /// 从当前文件末尾或已有内容范围内写入数据，并返回实际写入长度。
    ///
    /// 本阶段不支持稀疏文件；偏移不能越过文件末尾，单文件上限为 14 个数据扇区。
    pub(crate) fn write_at(
        &mut self,
        inode_id: InodeId,
        offset: usize,
        source: &[u8],
    ) -> Result<usize, FileSystemError> {
        let mut inode = self.read_inode(inode_id)?;
        if inode.kind != InodeKind::File {
            return Err(FileSystemError::NotFile);
        }
        let end = offset
            .checked_add(source.len())
            .ok_or(FileSystemError::FileTooLarge)?;
        if end > MAX_FILE_SIZE {
            return Err(FileSystemError::FileTooLarge);
        }
        if offset > inode.size {
            return Err(FileSystemError::InvalidOffset);
        }

        let mut written = 0;
        while written < source.len() {
            let file_offset = offset + written;
            let block_index = file_offset / SECTOR_SIZE;
            let block_offset = file_offset % SECTOR_SIZE;
            let amount = core::cmp::min(SECTOR_SIZE - block_offset, source.len() - written);
            let data_sector = self.ensure_inode_data_block(inode_id, &mut inode, block_index)?;
            let mut sector = [0; SECTOR_SIZE];
            read_sector(&mut self.device, data_sector, &mut sector)?;
            sector[block_offset..block_offset + amount]
                .copy_from_slice(&source[written..written + amount]);
            write_and_verify_sector(&mut self.device, data_sector, &sector)?;
            inode.size = core::cmp::max(inode.size, file_offset + amount);
            self.write_inode(inode_id, &inode)?;
            written += amount;
        }
        Ok(written)
    }

    fn create_inode(
        &mut self,
        directory_id: InodeId,
        name: &str,
        kind: InodeKind,
    ) -> Result<InodeId, FileSystemError> {
        let name_bytes = validate_name(name)?;
        let directory = self.read_inode(directory_id)?;
        if directory.kind != InodeKind::Directory {
            return Err(FileSystemError::NotDirectory);
        }
        if self.lookup(directory_id, name)?.is_some() {
            return Err(FileSystemError::AlreadyExists);
        }

        let inode_id = self.allocate_inode(kind)?;
        let mut entry = [0; DIRECTORY_ENTRY_SIZE];
        entry[..core::mem::size_of::<u32>()].copy_from_slice(&inode_id.0.to_le_bytes());
        entry[core::mem::size_of::<u32>()..core::mem::size_of::<u32>() + name_bytes.len()]
            .copy_from_slice(name_bytes);
        let result = self.append_directory_entry(directory_id, &entry);
        if result.is_err() {
            self.release_inode(inode_id)?;
        }
        result.map(|()| inode_id)
    }

    fn append_directory_entry(
        &mut self,
        directory_id: InodeId,
        entry: &[u8; DIRECTORY_ENTRY_SIZE],
    ) -> Result<(), FileSystemError> {
        let mut directory = self.read_inode(directory_id)?;
        if directory.kind != InodeKind::Directory {
            return Err(FileSystemError::NotDirectory);
        }
        if directory.size % DIRECTORY_ENTRY_SIZE != 0 {
            return Err(FileSystemError::InvalidInode);
        }
        let slot_count = directory.size / DIRECTORY_ENTRY_SIZE;
        let mut slot_index = slot_count;
        let mut existing_entry = [0; DIRECTORY_ENTRY_SIZE];
        for candidate in 0..slot_count {
            self.read_inode_data(
                directory_id,
                candidate * DIRECTORY_ENTRY_SIZE,
                &mut existing_entry,
            )?;
            if decode_directory_entry(&existing_entry)?.is_none() {
                slot_index = candidate;
                break;
            }
        }
        let offset = slot_index * DIRECTORY_ENTRY_SIZE;
        let block_index = offset / SECTOR_SIZE;
        if block_index >= DIRECT_BLOCK_COUNT {
            return Err(FileSystemError::NoSpace);
        }
        let block_offset = offset % SECTOR_SIZE;
        let data_sector =
            self.ensure_inode_data_block(directory_id, &mut directory, block_index)?;
        let mut sector = [0; SECTOR_SIZE];
        read_sector(&mut self.device, data_sector, &mut sector)?;
        sector[block_offset..block_offset + DIRECTORY_ENTRY_SIZE].copy_from_slice(entry);
        write_and_verify_sector(&mut self.device, data_sector, &sector)?;
        if slot_index == slot_count {
            directory.size += DIRECTORY_ENTRY_SIZE;
            self.write_inode(directory_id, &directory)?;
        }
        Ok(())
    }

    fn read_inode_data(
        &mut self,
        inode_id: InodeId,
        offset: usize,
        destination: &mut [u8],
    ) -> Result<(), FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        let mut copied = 0;
        while copied < destination.len() {
            let file_offset = offset + copied;
            let block_index = file_offset / SECTOR_SIZE;
            let block_offset = file_offset % SECTOR_SIZE;
            let amount = core::cmp::min(SECTOR_SIZE - block_offset, destination.len() - copied);
            let data_sector = *inode
                .direct_blocks
                .get(block_index)
                .ok_or(FileSystemError::FileTooLarge)?;
            if data_sector == 0 {
                return Err(FileSystemError::InvalidInode);
            }
            let mut sector = [0; SECTOR_SIZE];
            read_sector(&mut self.device, data_sector, &mut sector)?;
            destination[copied..copied + amount]
                .copy_from_slice(&sector[block_offset..block_offset + amount]);
            copied += amount;
        }
        Ok(())
    }

    fn ensure_inode_data_block(
        &mut self,
        inode_id: InodeId,
        inode: &mut DiskInode,
        block_index: usize,
    ) -> Result<u64, FileSystemError> {
        let Some(slot) = inode.direct_blocks.get(block_index) else {
            return Err(FileSystemError::FileTooLarge);
        };
        if *slot == 0 {
            let sector = self.allocate_data_sector()?;
            inode.direct_blocks[block_index] = sector;
            self.write_inode(inode_id, inode)?;
            return Ok(sector);
        }
        Ok(*slot)
    }

    fn allocate_inode(&mut self, kind: InodeKind) -> Result<InodeId, FileSystemError> {
        for inode_index in 1..INODE_COUNT {
            let inode_id = InodeId(inode_index as u32);
            if !self.inode_is_allocated(inode_id)? {
                self.set_inode_allocated(inode_id, true)?;
                let inode = DiskInode::empty(kind);
                if let Err(error) = self.write_inode(inode_id, &inode) {
                    let _ = self.set_inode_allocated(inode_id, false);
                    return Err(error);
                }
                return Ok(inode_id);
            }
        }
        Err(FileSystemError::NoSpace)
    }

    fn release_inode(&mut self, inode_id: InodeId) -> Result<(), FileSystemError> {
        self.set_inode_allocated(inode_id, false)?;
        self.write_inode(inode_id, &DiskInode::empty(InodeKind::File))
    }

    fn release_data_sector(&mut self, sector_id: u64) -> Result<(), FileSystemError> {
        let Some(data_index) = sector_id.checked_sub(self.layout.data_area_start) else {
            return Err(FileSystemError::InvalidInode);
        };
        if data_index >= self.layout.data_area_sectors
            || !self.data_sector_is_allocated(data_index)?
        {
            return Err(FileSystemError::InvalidInode);
        }
        self.set_data_sector_allocated(data_index, false)
    }

    fn allocate_data_sector(&mut self) -> Result<u64, FileSystemError> {
        for bit_index in 0..self.layout.data_area_sectors {
            if !self.data_sector_is_allocated(bit_index)? {
                self.set_data_sector_allocated(bit_index, true)?;
                let sector_id = self.layout.data_area_start + bit_index;
                let zero_sector = [0; SECTOR_SIZE];
                if let Err(error) =
                    write_and_verify_sector(&mut self.device, sector_id, &zero_sector)
                {
                    let _ = self.set_data_sector_allocated(bit_index, false);
                    return Err(error);
                }
                return Ok(sector_id);
            }
        }
        Err(FileSystemError::NoSpace)
    }

    fn inode_is_allocated(&mut self, inode_id: InodeId) -> Result<bool, FileSystemError> {
        if u64::from(inode_id.0) >= INODE_COUNT {
            return Ok(false);
        }
        let byte_index = u64::from(inode_id.0) / 8;
        let bit_index = inode_id.0 % 8;
        let mut sector = [0; SECTOR_SIZE];
        read_sector(
            &mut self.device,
            self.layout.inode_bitmap_start + byte_index / SECTOR_SIZE as u64,
            &mut sector,
        )?;
        Ok(sector[(byte_index % SECTOR_SIZE as u64) as usize] & (1 << bit_index) != 0)
    }

    fn set_inode_allocated(
        &mut self,
        inode_id: InodeId,
        allocated: bool,
    ) -> Result<(), FileSystemError> {
        if u64::from(inode_id.0) >= INODE_COUNT {
            return Err(FileSystemError::InodeNotFound);
        }
        let byte_index = u64::from(inode_id.0) / 8;
        let bit_index = inode_id.0 % 8;
        let sector_id = self.layout.inode_bitmap_start + byte_index / SECTOR_SIZE as u64;
        let mut sector = [0; SECTOR_SIZE];
        read_sector(&mut self.device, sector_id, &mut sector)?;
        let byte = &mut sector[(byte_index % SECTOR_SIZE as u64) as usize];
        if allocated {
            *byte |= 1 << bit_index;
        } else {
            *byte &= !(1 << bit_index);
        }
        write_and_verify_sector(&mut self.device, sector_id, &sector)
    }

    fn data_sector_is_allocated(&mut self, data_index: u64) -> Result<bool, FileSystemError> {
        let byte_index = data_index / 8;
        let bit_index = (data_index % 8) as u8;
        let mut sector = [0; SECTOR_SIZE];
        read_sector(
            &mut self.device,
            self.layout.data_bitmap_start + byte_index / SECTOR_SIZE as u64,
            &mut sector,
        )?;
        Ok(sector[(byte_index % SECTOR_SIZE as u64) as usize] & (1 << bit_index) != 0)
    }

    fn set_data_sector_allocated(
        &mut self,
        data_index: u64,
        allocated: bool,
    ) -> Result<(), FileSystemError> {
        let byte_index = data_index / 8;
        let bit_index = (data_index % 8) as u8;
        let sector_id = self.layout.data_bitmap_start + byte_index / SECTOR_SIZE as u64;
        let mut sector = [0; SECTOR_SIZE];
        read_sector(&mut self.device, sector_id, &mut sector)?;
        let byte = &mut sector[(byte_index % SECTOR_SIZE as u64) as usize];
        if allocated {
            *byte |= 1 << bit_index;
        } else {
            *byte &= !(1 << bit_index);
        }
        write_and_verify_sector(&mut self.device, sector_id, &sector)
    }

    fn read_inode(&mut self, inode_id: InodeId) -> Result<DiskInode, FileSystemError> {
        if !self.inode_is_allocated(inode_id)? {
            return Err(FileSystemError::InodeNotFound);
        }
        let byte_offset = u64::from(inode_id.0) * DISK_INODE_SIZE as u64;
        let sector_id = self.layout.inode_area_start + byte_offset / SECTOR_SIZE as u64;
        let record_offset = (byte_offset % SECTOR_SIZE as u64) as usize;
        let mut sector = [0; SECTOR_SIZE];
        read_sector(&mut self.device, sector_id, &mut sector)?;
        DiskInode::decode(&sector[record_offset..record_offset + DISK_INODE_SIZE])
    }

    fn write_inode(&mut self, inode_id: InodeId, inode: &DiskInode) -> Result<(), FileSystemError> {
        if u64::from(inode_id.0) >= INODE_COUNT {
            return Err(FileSystemError::InodeNotFound);
        }
        let byte_offset = u64::from(inode_id.0) * DISK_INODE_SIZE as u64;
        let sector_id = self.layout.inode_area_start + byte_offset / SECTOR_SIZE as u64;
        let record_offset = (byte_offset % SECTOR_SIZE as u64) as usize;
        let mut sector = [0; SECTOR_SIZE];
        read_sector(&mut self.device, sector_id, &mut sector)?;
        inode.encode(&mut sector[record_offset..record_offset + DISK_INODE_SIZE]);
        write_and_verify_sector(&mut self.device, sector_id, &sector)
    }

    fn resolve_path(&mut self, path: &str) -> Result<InodeId, FileSystemError> {
        if path.as_bytes().contains(&0) {
            return Err(FileSystemError::InvalidName);
        }
        let mut current = ROOT_INODE_ID;
        let mut components = path
            .split('/')
            .filter(|component| !component.is_empty())
            .peekable();
        while let Some(component) = components.next() {
            if component == "." || component == ".." {
                return Err(FileSystemError::InvalidName);
            }
            current = self
                .lookup(current, component)?
                .ok_or(FileSystemError::NoEntry)?;
            if components.peek().is_some() && self.read_inode(current)?.kind != InodeKind::Directory
            {
                return Err(FileSystemError::PathNotDirectory);
            }
        }
        Ok(current)
    }

    fn resolve_parent<'a>(&mut self, path: &'a str) -> Result<(InodeId, &'a str), FileSystemError> {
        let without_trailing_slashes = path.trim_end_matches('/');
        let Some(name_start) = without_trailing_slashes.rfind('/') else {
            validate_name(without_trailing_slashes)?;
            return Ok((ROOT_INODE_ID, without_trailing_slashes));
        };
        let name = &without_trailing_slashes[name_start + 1..];
        validate_name(name)?;
        let parent_path = &without_trailing_slashes[..name_start];
        let parent = if parent_path.is_empty() {
            ROOT_INODE_ID
        } else {
            self.resolve_path(parent_path)?
        };
        if self.read_inode(parent)?.kind != InodeKind::Directory {
            return Err(FileSystemError::PathNotDirectory);
        }
        Ok((parent, name))
    }
}

static mut MOUNTED_FILE_SYSTEM: Option<FileSystem<Box<dyn BlockDevice>>> = None;

/// 初始化并长期保存唯一挂载点，返回卷信息供启动日志使用。
pub(crate) fn initialize(
    device: impl BlockDevice + 'static,
) -> Result<FileSystemInfo, FileSystemError> {
    // 安全性：文件系统只在单 hart 启动阶段初始化一次，用户态陷入处理尚未开始。
    let mounted = unsafe { &mut *core::ptr::addr_of_mut!(MOUNTED_FILE_SYSTEM) };
    if mounted.is_some() {
        return Err(FileSystemError::AlreadyMounted);
    }
    let erased_device: Box<dyn BlockDevice> = Box::new(device);
    let file_system = FileSystem::mount_or_format(erased_device)?;
    let info = file_system.info();
    *mounted = Some(file_system);
    Ok(info)
}

/// 在单 hart 的 syscall 陷入期间独占访问已挂载文件系统。
///
/// 调用方不得在回调中触发可能重入文件系统的路径；S-mode 陷入期间本 hart 的中断关闭。
pub(crate) fn with_mounted<R>(
    operation: impl FnOnce(&mut FileSystem<Box<dyn BlockDevice>>) -> R,
) -> Result<R, FileSystemError> {
    // 安全性：文件系统访问仅来自启动或 S-mode syscall，单 hart 陷入路径不会并发或重入访问该对象。
    let mounted = unsafe { &mut *core::ptr::addr_of_mut!(MOUNTED_FILE_SYSTEM) };
    let file_system = mounted.as_mut().ok_or(FileSystemError::NotMounted)?;
    Ok(operation(file_system))
}

#[derive(Clone, Copy)]
struct DiskInode {
    size: usize,
    kind: InodeKind,
    direct_blocks: [u64; DIRECT_BLOCK_COUNT],
}

impl DiskInode {
    const fn empty(kind: InodeKind) -> Self {
        Self {
            size: 0,
            kind,
            direct_blocks: [0; DIRECT_BLOCK_COUNT],
        }
    }

    fn decode(record: &[u8]) -> Result<Self, FileSystemError> {
        let size =
            usize::try_from(read_u64(record, 0)).map_err(|_| FileSystemError::InvalidInode)?;
        if size > MAX_FILE_SIZE {
            return Err(FileSystemError::InvalidInode);
        }
        let kind = InodeKind::from_disk(record[INODE_TYPE_OFFSET])?;
        let mut direct_blocks = [0; DIRECT_BLOCK_COUNT];
        for (index, block) in direct_blocks.iter_mut().enumerate() {
            *block = read_u64(record, INODE_DIRECT_OFFSET + index * 8);
        }
        if size > 0
            && direct_blocks[..div_ceil(size as u64, SECTOR_SIZE as u64) as usize].contains(&0)
        {
            return Err(FileSystemError::InvalidInode);
        }
        Ok(Self {
            size,
            kind,
            direct_blocks,
        })
    }

    fn encode(&self, record: &mut [u8]) {
        record.fill(0);
        write_u64(record, 0, self.size as u64);
        record[INODE_TYPE_OFFSET] = self.kind.disk_value();
        for (index, block) in self.direct_blocks.iter().enumerate() {
            write_u64(record, INODE_DIRECT_OFFSET + index * 8, *block);
        }
    }
}

fn validate_name(name: &str) -> Result<&[u8], FileSystemError> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || name == "."
        || name == ".."
        || bytes.iter().any(|byte| *byte == 0 || *byte == b'/')
    {
        return Err(FileSystemError::InvalidName);
    }
    if bytes.len() >= DIRECTORY_NAME_SIZE {
        return Err(FileSystemError::NameTooLong);
    }
    Ok(bytes)
}

fn decode_directory_entry(
    entry: &[u8; DIRECTORY_ENTRY_SIZE],
) -> Result<Option<(&[u8], InodeId)>, FileSystemError> {
    let name_bytes = &entry[4..];
    let Some(name_end) = name_bytes.iter().position(|byte| *byte == 0) else {
        return Err(FileSystemError::InvalidName);
    };
    if name_end == 0 {
        return Ok(None);
    }
    let inode_id = InodeId(u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]));
    Ok(Some((&name_bytes[..name_end], inode_id)))
}

fn file_system_info(
    sector_count: u64,
    mount_count: u64,
    layout: FileSystemLayout,
    was_formatted: bool,
) -> FileSystemInfo {
    FileSystemInfo {
        sector_count,
        mount_count,
        inode_count: INODE_COUNT,
        data_sector_count: layout.data_area_sectors,
        was_formatted,
    }
}

fn initialize_volume(
    device: &mut impl BlockDevice,
    layout: FileSystemLayout,
) -> Result<(), FileSystemError> {
    let mut inode_bitmap = [0; SECTOR_SIZE];
    inode_bitmap[0] = 1;
    write_and_verify_sector(device, layout.inode_bitmap_start, &inode_bitmap)?;

    for sector_id in 0..layout.inode_area_sectors {
        let mut inode_sector = [0; SECTOR_SIZE];
        if sector_id == 0 {
            DiskInode::empty(InodeKind::Directory).encode(&mut inode_sector[..DISK_INODE_SIZE]);
        }
        write_and_verify_sector(device, layout.inode_area_start + sector_id, &inode_sector)?;
    }

    let empty_sector = [0; SECTOR_SIZE];
    for sector_id in 0..layout.data_bitmap_sectors {
        write_and_verify_sector(device, layout.data_bitmap_start + sector_id, &empty_sector)?;
    }

    let mut superblock = [0; SECTOR_SIZE];
    encode_superblock(&mut superblock, device.sector_count(), 1, layout);
    write_and_verify_sector(device, SUPERBLOCK_SECTOR, &superblock)
}

fn verify_format_metadata_is_empty(
    device: &mut impl BlockDevice,
    layout: FileSystemLayout,
) -> Result<(), FileSystemError> {
    let mut sector = [0; SECTOR_SIZE];
    for sector_id in 1..layout.data_area_start {
        read_sector(device, sector_id, &mut sector)?;
        if sector.iter().any(|byte| *byte != 0) {
            return Err(FileSystemError::UnknownVolume);
        }
    }
    Ok(())
}

fn validate_root_inode(
    device: &mut impl BlockDevice,
    layout: FileSystemLayout,
) -> Result<(), FileSystemError> {
    let mut inode_bitmap = [0; SECTOR_SIZE];
    read_sector(device, layout.inode_bitmap_start, &mut inode_bitmap)?;
    if inode_bitmap[0] & 1 == 0 {
        return Err(FileSystemError::InvalidRootInode);
    }
    let mut inode_sector = [0; SECTOR_SIZE];
    read_sector(device, layout.inode_area_start, &mut inode_sector)?;
    DiskInode::decode(&inode_sector[..DISK_INODE_SIZE])
        .map_err(|_| FileSystemError::InvalidRootInode)
        .and_then(|root| {
            if root.kind == InodeKind::Directory {
                Ok(())
            } else {
                Err(FileSystemError::InvalidRootInode)
            }
        })
}

fn decode_superblock(
    sector: &[u8; SECTOR_SIZE],
    device_sector_count: u64,
    expected_layout: FileSystemLayout,
) -> Result<u64, FileSystemError> {
    if &sector[MAGIC_RANGE] != SUPERBLOCK_MAGIC {
        return Err(FileSystemError::UnknownVolume);
    }
    if read_u32(sector, VERSION_OFFSET) != FORMAT_VERSION {
        return Err(FileSystemError::UnsupportedVersion);
    }
    if read_u32(sector, SECTOR_SIZE_OFFSET) as usize != SECTOR_SIZE {
        return Err(FileSystemError::InvalidSectorSize);
    }
    if read_u64(sector, DEVICE_SECTOR_COUNT_OFFSET) != device_sector_count {
        return Err(FileSystemError::DeviceCapacityChanged);
    }
    if read_u64(sector, INODE_BITMAP_SECTORS_OFFSET) != expected_layout.inode_bitmap_sectors
        || read_u64(sector, INODE_AREA_SECTORS_OFFSET) != expected_layout.inode_area_sectors
        || read_u64(sector, DATA_BITMAP_SECTORS_OFFSET) != expected_layout.data_bitmap_sectors
        || read_u64(sector, DATA_AREA_SECTORS_OFFSET) != expected_layout.data_area_sectors
        || read_u64(sector, INODE_COUNT_OFFSET) != INODE_COUNT
        || read_u64(sector, ROOT_INODE_ID_OFFSET) != u64::from(ROOT_INODE_ID.0)
        || read_u64(sector, DATA_START_SECTOR_OFFSET) != expected_layout.data_area_start
    {
        return Err(FileSystemError::InvalidLayout);
    }
    Ok(read_u64(sector, MOUNT_COUNT_OFFSET))
}

fn encode_superblock(
    sector: &mut [u8; SECTOR_SIZE],
    sector_count: u64,
    mount_count: u64,
    layout: FileSystemLayout,
) {
    sector.fill(0);
    sector[MAGIC_RANGE].copy_from_slice(SUPERBLOCK_MAGIC);
    write_u32(sector, VERSION_OFFSET, FORMAT_VERSION);
    write_u32(sector, SECTOR_SIZE_OFFSET, SECTOR_SIZE as u32);
    write_u64(sector, DEVICE_SECTOR_COUNT_OFFSET, sector_count);
    write_u64(sector, MOUNT_COUNT_OFFSET, mount_count);
    write_u64(
        sector,
        INODE_BITMAP_SECTORS_OFFSET,
        layout.inode_bitmap_sectors,
    );
    write_u64(sector, INODE_AREA_SECTORS_OFFSET, layout.inode_area_sectors);
    write_u64(
        sector,
        DATA_BITMAP_SECTORS_OFFSET,
        layout.data_bitmap_sectors,
    );
    write_u64(sector, DATA_AREA_SECTORS_OFFSET, layout.data_area_sectors);
    write_u64(sector, INODE_COUNT_OFFSET, INODE_COUNT);
    write_u64(sector, ROOT_INODE_ID_OFFSET, u64::from(ROOT_INODE_ID.0));
    write_u64(sector, DATA_START_SECTOR_OFFSET, layout.data_area_start);
}

fn read_sector(
    device: &mut impl BlockDevice,
    sector_id: u64,
    destination: &mut [u8; SECTOR_SIZE],
) -> Result<(), FileSystemError> {
    device
        .read_sector(sector_id, destination)
        .map_err(|_| FileSystemError::DeviceRead)
}

fn write_and_verify_sector(
    device: &mut impl BlockDevice,
    sector_id: u64,
    expected: &[u8; SECTOR_SIZE],
) -> Result<(), FileSystemError> {
    device
        .write_sector(sector_id, expected)
        .map_err(|_| FileSystemError::DeviceWrite)?;
    let mut actual = [0; SECTOR_SIZE];
    read_sector(device, sector_id, &mut actual)?;
    if actual != *expected {
        return Err(FileSystemError::SectorWriteMismatch);
    }
    Ok(())
}

fn div_ceil(value: u64, divisor: u64) -> u64 {
    value / divisor + u64::from(value % divisor != 0)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(value)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(value)
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
