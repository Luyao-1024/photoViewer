# 原生云端同步使用与实机验证

PhotoViewer 提供 Native SyncServer 双向同步。服务端契约仍只由[服务端协议链接索引](native-sync-protocol.md)维护；本文记录客户端操作、范围和本地验证方法。

## 使用

在设置的「云端同步」中启用同步，选择「添加服务器」，输入 HTTPS 地址、API 密钥和本地照片根目录，点击「验证连接」。探测成功后选择服务器已配置的区域，勾选允许上传的物理相册，再保存。密钥保存到系统密钥环，SQLite 只保存引用。空相册选择表示仅接收云端照片；不会自动上传整个根目录。新增的扫描工作在后台进行，已索引且 mtime/size 未变化的文件不重新读取正文计算摘要。

已保存任务每 15 秒检查一次；设置中的「立即同步」和照片页顶部下拉可以主动触发。暂停任务后保留配置与历史；继续后自动检查。修改相册范围先编辑后接受，取消、无效草稿及重复保存同样的范围不会停止任务。修改被接受后，旧配置停止发起后续传输，已开始的本地发布先安全结算，再应用最新配置。

每张照片在自己的原图、缩略图与本地缓存就绪后入库，不等待整个相册。浏览图库使用本地文件。收藏及原图修改通过数据库中的版本和 dirty 字段同步；离线或失败保留待办。未知提交结果先通过原操作回执核实，不会把超时当作未提交。

「管理云端照片」提供显式云端删除、已删除照片的新身份重新上传，以及冲突的版本选择。云端删除保留本地照片；另一设备收到完整删除投影后保留本地文件并停止自动重传。选择「使用本地版本」前保存已校验的竞争云端资源，选择「使用云端版本」前保存本地原图和决策信息。恢复副本在应用数据目录旁的 `sync-recovery`，界面显示具体目录。隐藏照片仍需要支持该表示的客户端取消隐藏，不会公开为本地普通照片。

关闭总开关前需要确认：同步历史会清除，连接、范围及本地/云端照片保留。移除任务需要确认，会移除该任务及其凭证，也保留两端照片。重新接入已有云端内容遇到同路径、不同身份时保留冲突，必须明确选择版本。

当前仅支持原生 Sync/Bidirectional；Backup、原图懒下载、分享、上传断点续传不在当前范围内。下载中断会复用已经完成并校验的资源，未完成的单个资源重新下载。

## 聚焦验证

先构建相邻服务器，设置 `SYNC_SERVER_BIN` 指向实际二进制。GTK 用例在 Sandbox 中操作完整窗口并驱动真实控件，不访问真实相册或系统凭证。

```sh
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus tools/with-at-spi.sh \
  xvfb-run -a -s "-screen 0 1920x1080x24" \
  env SYNC_SERVER_BIN=../syncserver/target/debug/syncserver \
  cargo test --locked --test ux_native_sync_settings --test native_sync_assets \
    --test db_migrations --test native_sync_receive -- --test-threads=1

cargo test --locked --lib -- core::sync::store::receive::tests \
  core::sync::device::tests core::sync::store::runtime::tests \
  core::sync::service::tests --test-threads=1
```

真实上传/下载、双端冲突及恢复备份、删除传播、新身份重建由 `native_sync_assets` 的真实 actor/文件/服务器旅程覆盖。快照、墓碑与固定水位的协议用例在 `native_sync_receive`。schema 9/10 升级保留原生任务与 dirty；schema 8 以前的同步历史按已定迁移规则移除，图库用户数据保留。

## 显式实机连接工具

`examples/connect_native.rs` 不属于测试套件，只在明确提供 `--configure-library` 时运行。它会写入实际客户端连接、把指定相册上传到服务器，并在图库外创建第二个接收目录。执行前关闭使用同一数据库的应用并备份数据库与设置。连接 JSON 只包含 `baseURL`、`apiKeyFile` 及 `zones`；密钥文件必须只对用户可读，不放入仓库或报告。

```sh
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
  cargo run --locked --example connect_native -- \
  --configure-library \
  --connection /path/to/private/connection.json \
  --album /path/to/Pictures/截图 \
  --evidence /path/outside/library/verification
```

工具核对每份原图的 SHA-256、两端再次同步无重复写入，并执行收藏更改的上传/接收及恢复。原始照片字节不改变。完成后启用已配置任务；报告与第二客户端数据保留在证据目录。密钥只进入系统密钥环和进程内存。

实机结果单独记录在[本次实现说明](native-sync-integration.md)，不把本地模拟、聚焦测试、远端 CI 或真实公网验证混为一项。
