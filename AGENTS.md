# Agent 协作说明

本仓库是一个使用 Rust 从零构建的教学操作系统。修改或新增代码前，先阅读 [`docs/development-principles.md`](docs/development-principles.md)。该文档是项目目标、架构边界和 Rust 代码约定的准则来源。

每项改动都应足够小，能在教学中讲清楚。优先采用明确的接口和易读的控制流程。当改动跨越子系统边界时，更新相关接口说明，并在改动摘要中说明依赖关系。只有在功能已经实现并验证后，才能声称支持相应的 Linux ABI 行为。
