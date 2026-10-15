# 服务端协议入口与客户端适配边界

协议唯一来源是相邻的 SyncServer 仓库。本页只提供入口与客户端适配决策，不维护第二份接口、字段、状态码或默认限额规范。服务端契约发生变化时，先核对服务端版本，再调整客户端 DTO 和映射；不能在客户端文档中自行修改协议。

## 服务端契约

| 主题 | 唯一契约来源 |
|---|---|
| 功能与验收约束 | [服务端规格](../../../syncserver/spac.md) |
| 实现范围与支持能力 | [实现说明](../../../syncserver/docs/design/v0.1-server.md)、[兼容矩阵](../../../syncserver/docs/design/protocol-compatibility.md) |
| 用户、密码登录、会话与隔离 | [认证契约](../../../syncserver/docs/contracts/authentication.md) |
| 身份、认证与部署配置 | [安全与运维](../../../syncserver/docs/design/security-operations.md)、[多用户管理](../../../syncserver/docs/design/multi-user.md) |
| 首次清单、快照、游标与增量 | [快照与增量契约](../../../syncserver/docs/contracts/snapshots.md) |
| 记录读写、版本条件与幂等结果 | [记录契约](../../../syncserver/docs/contracts/records.md) |
| 照片记录、两角色与 generation | [照片记录契约](../../../syncserver/docs/contracts/photo-records.md) |
| 资源会话、摘要、上传与下载 | [资源契约](../../../syncserver/docs/contracts/assets.md) |
| 上行租约、进度门槛、阶段与成功确认 | [上行协调契约](../../../syncserver/docs/contracts/upload-coordination.md) |
| 合成协议样例 | [服务端 fixtures](../../../syncserver/docs/contracts/fixtures.json) |
| 运维指标 | [指标契约](../../../syncserver/docs/contracts/metrics.md) |

这些链接暂用本地相对路径，要求两个仓库并列检出。正式发布时统一替换为指定服务端版本的仓库网页链接；发布前不混用漂移的网页协议与本地版本，也不复制正文作为替代。

## 客户端拥有的接入方案

- [整体接入与模块职责](native-sync.md)：任务生命周期、能力探测、本地范围与独立上行协调。
- [本地数据库和发布方案](native-sync-storage.md)：dirty、确认基线、已发出操作恢复、资源待办、发布事务和读取保护。
- [实施阶段与本地验收](native-sync-plan.md)：实现进度、差异和客户端验证边界。

客户端只允许绝对根地址；默认要求 HTTPS，literal loopback 的开发服务可用 HTTP。通过能力探测固定 namespace、zone 和数据库代际，签名资源地址必须与已配置服务同源，禁止跨源重定向和凭证转发。这是 PhotoViewer 的连接策略，不是对服务端协议的重新定义。

同一原生任务绑定一个本地根和一个服务端 zone；上传范围由客户端显式配置决定。当前产品只开放 Sync/Bidirectional；Backup、隐藏照片展示和其他缺少本地模型的能力不能仅因服务端支持而自动启用。未知或未适配内容进入可见的冲突/待办，不假装已经应用。

`native/protocol.rs`、`receive.rs`、`assets.rs`、`lease.rs`、`modify.rs` 是对上述服务端契约的 Rust 适配。线上字段所属记录、摘要编码、错误和版本条件均以服务端为准。客户端方案只解释它们如何映射到本地媒体、数据库与交互，不另行列一份 wire 规范。


多账户的客户端映射、凭证生命周期和 UI 接入见[账户接入方案](native-sync-accounts.md)，此页不复制服务器登录接口定义。
