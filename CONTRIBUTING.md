# Contributing

## 开发流程

1. Fork & clone
2. 创建特性分支
3. TDD：先写失败测试
4. 实现到通过
5. cargo fmt + cargo clippy
6. PR 提交

## 模块说明

- `core/`：数据层（DB、扫描、元数据），与 UI 解耦
- `ui/`：GTK widgets
- `platform/`：XDG 集成