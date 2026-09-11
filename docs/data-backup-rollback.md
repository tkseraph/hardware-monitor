# 历史数据备份与回滚流程

日期：2026-09-12。适用范围：R1/R2/R3 涉及的 schema 迁移与任何会改动历史数据库的操作。

## 约束（贯穿全程）

- 不删除真实历史库，不在真库注入合成数据。
- 任何迁移先在**一致性副本**上验证，通过后才考虑切换。
- 不能只复制正在写入的 `.db` 主文件 —— 必须连同 WAL 状态一起处理，或用 SQLite 在线备份 API。
- 不搬动、不覆盖正在运行的实例（含挂测实例 PID 9144，数据目录 `/tmp/monitor-longrun`）正在使用的库。

## 数据库位置

- 真实库：`<app_data_dir>/monitor.db`（默认 `~/Library/Application Support/<bundle-id>/`）。
- 挂测/隔离库：由 `MONITOR_DATA_DIR` 指定的目录下的 `monitor.db`。

## 备份（迁移前必做）

优先使用 SQLite 在线备份（对打开的库也一致）：

```bash
sqlite3 /path/to/monitor.db ".timeout 5000" ".backup '/path/to/monitor.backup-YYYYMMDD-HHMMSS.db'"
```

等价地，若实例已停止、无 WAL 写入，可复制三件套：

```bash
cp -p monitor.db monitor.db-wal monitor.db-shm /path/to/backup-dir/   # -wal/-shm 存在才复制
```

验证备份完整性（对副本，不对真库）：

```bash
sqlite3 /path/to/backup.db "PRAGMA integrity_check;"   # 期望 ok
```

## 迁移验证顺序

1. 停止写入目标库的实例（或确认其数据目录指向别处）。
2. 对**副本**运行迁移，记录行数、各粒度桶计数、schema_version 前后值。
3. 在副本上跑守恒校验：迁移前后样本数 / 加权和一致；无新生成的“合成”值。
4. 副本通过后才对真库操作；真库迁移前再做一次在线备份。
5. 迁移失败：放弃目标库，从最近备份恢复副本重试；不回退到已知会丢数据的旧删除逻辑。

## 回滚

- 保留迁移前备份至少一个发布周期；备份不删。
- 回滚 = 停止写入 → 用备份副本替换目标库 → 重启实例。不做跨 schema 的数据合并。
- 无法恢复的旧损失标记为“未知”，禁止合成修复值。

## 挂测实例注意

挂测实例（数据目录 `/tmp/monitor-longrun`）是连续观察对象。更换其数据目录、版本或重启后，**重新起算**，不把不同目录/版本的运行拼成一段“连续成功”。
