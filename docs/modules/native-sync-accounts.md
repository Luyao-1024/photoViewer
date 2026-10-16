# 多账户客户端接入

服务端负责登录、会话和隔离协议；唯一来源为[用户认证契约](../../../syncserver/docs/contracts/authentication.md)和[服务端实施记录](../../../syncserver/docs/design/multi-user.md)。本页只记录 PhotoViewer 的配置、凭证、本地映射和交互，不另写协议规范。

## 设置与本地数据

添加服务器时填写地址、用户名和密码，点击「登录并验证」，再选择服务器区域、本地根和上传相册。密码仅用于当次认证，平台凭证服务保存返回的会话，SQLite 只保存凭证引用与服务器验证过的用户身份。用户身份保存在既有 native_pinned_scope JSON 中；旧 JSON 缺省无账户身份，数据库版本固定为 1，不清除原有任务、dirty、媒体和进度。

同一服务器的不同账户可建立独立任务，每个账户使用不同且不重叠的本地根。已有 zone 映射按 endpoint、账户命名空间和 zone 判断，不能因两个用户名使用同一地址而错误拒绝，也不能把一个账户的同一区域重复映射到多个根。上传范围仍须明确选择，空选择只接收。

每次建立任务传输客户端，先核对真实登录身份、namespace、数据库 generation 与已保存连接。即便系统凭证被替换成另一账户的有效会话，也必须在接收其数据前拒绝，不能推进旧任务的 ingest/applied 或覆写文件。资源请求在同源校验后携带当前会话，仍验证签名能力、摘要和角色。

## 重新登录与退出

任务显示已保存的用户名，提供「登录并继续」和「退出登录」。重新登录只接受该任务原账户；其他用户名若对应不同用户 ID 或 namespace 会拒绝。登录草稿和失败不改动原配置。会话到期、管理员重设密码后可重新登录，保留 dirty、待确认操作和已接收进度。

退出会撤销当前服务器会话、删除本地会话凭证并暂停该任务；其他账户或同账户其他会话的任务继续工作。已由别处撤销/过期的会话也可以在本地退出。重新登录成功后继续任务，不用清同步历史。

旧单账户服务器升级时，管理员显式创建原数据接管者（命令由服务端说明）。在原任务选择「登录并继续」使用接管者登录；只有原 namespace/generation 一致时允许从无用户身份的连接升级，进度、媒体和文件不重建。不能让另一新账户继承这条本地关系。

## 验证范围

`tests/native_sync_accounts.rs` 使用临时服务器、实际管理员 CLI、合成照片、DbActor 和持久内存凭证适配，不访问真实密钥环或用户图库。覆盖同地址 Alice/Bob、同用户另一登录会话接收、错误会话注入后进度保持、不同账户重新登录被拒绝、退出不影响其他任务，以及旧 API 关系显式接管后不重复上传。

`tests/ux_native_sync_settings.rs` 经完整窗口真实控件覆盖用户名/密码登录、相册范围、取消/无效草稿、退出及再次登录。共用凭证测试适配只替换 Secret Service 边界，不用最终数据库状态伪造登录或照片成功。

```sh
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus tools/with-at-spi.sh \
  xvfb-run -a -s "-screen 0 1920x1080x24" \
  env SYNC_SERVER_BIN=../syncserver/target/debug/syncserver \
  cargo test --locked --test native_sync_accounts --test ux_native_sync_settings \
    --test native_sync_assets --test native_sync_probe -- --test-threads=1
```

本轮只运行相关聚焦用例和必要静态检查，不执行全量测试、不推送、不修改阿里云部署或创建真实用户。线上服务是否支持账户登录以服务端部署版本为准。

## 本轮结果

本地 **50 个相关 Rust 用例通过，0 失败、0 忽略**：账户与旧关系升级集成 2、资源旅程 9、连接探测 6、真实 GTK 控件旅程 1、协议/actor 指定单元用例 32。实际使用本次构建的相邻 SyncServer 二进制，无缺失服务端的静默跳过。两仓库的 fmt、严格 Clippy 和二进制构建通过；服务端的独立账户与 CLI 验证结果见其[实施记录](../../../syncserver/docs/design/multi-user.md)。

日志保存在本地忽略的 `target/multiuser-client-final-{fmt,clippy,build,unit,focused}.log`。未运行全量测试、远端 CI；按用户指令仅创建本地提交，不推送，未改动阿里云部署及真实账户。提交阶段不额外运行测试，以上结果来自开发验证阶段。此前截图相册的线上验证属于旧单账户阶段，不计作本轮线上多用户证据。
