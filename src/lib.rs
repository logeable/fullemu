#![no_std]

//! 可在内核与宿主机测试间共用的基础数据格式解析器。

#[cfg(test)]
extern crate std;

pub mod boot;
