# findings-261004-core

- 基线: HEAD 2ca9cf7
- 范围: crates/mcloud-core/, crates/mcloud-crypto/, crates/mcloud-store/（39 文件 / ≈1.37 万行）
- 方法: 只读。读完 crypto 全量（aes/des/gzip/hash/password/rand/rsa/sign/slide/tn + 3 个 tests/golden*）、
  store 全量（clock/json_store/`json_store::tests`）、core 的 event/run/log/cancel/schedule/settings/user/error/smid；
  对照 `/Users/catlair/Documents/workspace/mcloud-sign/packages/core/src/{utils/encrypt.ts,auth/sign.ts}`；
  读了 `data/` 现网数据（只读，未触碰 :3000）；核对了 tokio-1.53.1 `sync/notify.rs` 源码。
  跑过的命令只有 `git / grep / sed / python3 / ls / du`，**未跑 cargo、未改任何源码**。

## 摘要
1. 落盘链路的最高风险不在并发，而在**掉电**：`write_atomic` 无 fsync，且空文件被当成"默认值"、
   紧随其后的第一次写入会把 `.bak` 覆盖掉 ⇒ 有把账号/凭据**静默清零且备份同时失效**的路径（co-1）。
2. `history.json` 现在每条运行自带 200 行日志（×200 条历史），已实测 276 KB，与 store 模块文档
   自述的"KB 量级、微秒级"成本模型不符，且没有任何测试盯着它（co-2）。
3. 权限收紧清单漏了 4 个同样会落盘的文件（含任务参数），且现有测试只覆盖清单内那 9 个（co-3）。
4. 脱敏在"文本层"不做键名归一化，`verify_code=` / `api_key=` 这类 snake_case 文本键会逃逸（co-4）。
5. crypto 与 TS 参考**逐条一致**（签名盐值/拼接顺序/回落、AES 动态 IV 载荷格式、DES 两种填充语义）；
   `rand.rs` 生产源是 OS CSPRNG；未见秘密进 `Debug`（`PasswordHash` / `SharedRandom` 都手写脱敏）。

## 发现

### co-1 [高] `write_atomic` 无 fsync，且空文件被当作"默认值" + 首次写入即覆盖 `.bak`
- 位置: `crates/mcloud-store/src/json_store.rs:329`（写）、`:368`（读）、`:331`（备份）
- 证据: `write_atomic` 只做 `f.write_all(bytes)`（`:346`）→ `std::fs::rename(&tmp, path)`（`:353`），
  **全程没有 `File::sync_all`/`sync_data`，也没有对父目录 fsync**；注释 `:27-30` 的"rename 原子 ⇒ 不会留下半截 JSON"
  只对**进程被杀**成立，对**掉电/内核崩溃**不成立（rename 可见而数据页未回写的零长度文件是已知形态）。
  更糟的是读侧：`read_json` 对空文件直接 `Ok(T::default())`（`:368-370`）⇒ 一个 0 字节的
  `accounts.json` 会被当成"还没有账号"，服务正常启动、界面显示"账号为空"，**不报任何错**。
  而写侧在覆盖前会 `std::fs::copy(path, path.with_extension("bak"))`（`:331`）—— 于是崩溃后
  **第一次**写账号（例如一次登录/一次刷新）就把那份 0 字节文件拷成 `accounts.bak`，
  **把最后一份好备份也毁掉**，然后写入新数据。备份机制在这条路径上是负的。
- 复现: `grep -n "sync_all\|sync_data" crates/mcloud-store/src/json_store.rs`（0 命中）
  ；`sed -n '329,355p;363,371p' crates/mcloud-store/src/json_store.rs`
- 影响: 掉电/内核崩溃后可丢**全部账号与凭据**（`accounts.json`）、会话（`sessions.json`）、
  口令（`users.json`），且现场无告警、`.bak` 已被同一路径覆盖。属"能撤也要靠人工备份"的级别。
- 建议: 写完后 `f.sync_all()` 再 rename（必要时对目录 fsync）；把"空文件"从"默认值"改成
  显式告警或拒绝启动；`.bak` 的拷贝改成"仅当新内容已 fsync 成功"才做，或改名为 `.bak.<时间戳>` 保留多份。
- 状态: 仅静态（未跑 cargo，未能构造掉电） · 置信度 中（机制在代码里确定；触发取决于文件系统与掉电时机）

### co-2 [中] `history.json` 的写放大远超 store 自述成本模型：每条运行自带 200 行日志 × 200 条历史
- 位置: `crates/mcloud-core/src/run.rs:228`（`logs` 字段）· `crates/mcloud-store/src/json_store.rs:54`（`HISTORY_LIMIT`）
  · `crates/mcloud-store/src/json_store.rs:574`（`persist_runs`）· 成本模型写在 `:19-20`
- 证据: `RunRecord.logs: Vec<RunLogLine>`（`run.rs:228`，上限 `RUN_LOG_LIMIT = 200`，`run.rs:201`）
  由 runner 填（`crates/mcloud-engine/src/runner.rs:517` `record.logs = collector.take();`）。
  `upsert_run` 每次都对**整份** `db.runs`（上限 200 条）做 `write_json`，而 `write_atomic` 还要
  先把旧文件整份 `copy` 成 `.bak`。实测现网数据（只读）：`data/history.json` = **276275 字节**，
  111 条记录、共 271 行日志；按上限 200 条 × 200 行外推约 4~5 MB，
  每次 upsert（每次运行至少 2 次：起、完）要在**db 写锁内**写 + 拷贝共约两倍体积。
  而 `:19-20` 仍写着"这些文件都在 KB 量级（账号、定时、编排），一次写盘是微秒级"，
  `docs/conventions.md` §6 的"阻塞 IO 例外"正是引用这句。
  已有的 `worst_case_log_write_stays_bounded`（`json_store/tests.rs:784`）只量**单 run 的日志文件**，
  不量 `history.json`。
- 复现: `python3 -c "import json,os;p='data/history.json';d=json.load(open(p));print(os.path.getsize(p),len(d),sum(len(r.get('logs',[])) for r in d))"`
- 影响: 记录攒满后每次运行的落盘成本涨一个量级，且**发生在写锁内**（`:729` 起的所有 persist 都在锁内），
  会拖慢同一进程里的所有读；模块文档与 §6 例外的前提被静默推翻。
- 建议: 把 `RunRecord.logs` 从 `history.json` 里挪出去（`logs/<run>.json` 已经有一份，`LogStore`），
  或给 history 落盘单独定预算并在测试里钉住 `history.json` 的字节上界。
- 状态: 已实测（现网文件体积 + 代码路径） · 置信度 高

### co-3 [中] `harden_existing_permissions` 清单漏 4 个会落盘的文件，且现有测试只覆盖清单内的 9 个
- 位置: `crates/mcloud-store/src/json_store.rs:251`（清单）对比 `:138`/`:140`/`:142`/`:146`（文件常量）
- 证据: `let names = [...]` 只列 ACCOUNTS/RUNS/SCHEDULES/PIPELINES/STATE/USERS/SESSIONS/SMID/PC_DEVICE，
  **不含** `SETTINGS`(`settings.json`)、`TASK_CONFIG`(`task-config.json`)、
  `KOULING_POOL`(`kouling-pool.json`)、`EXCHANGE_SUBSCRIPTIONS`(`exchange-subscriptions.json`)。
  注释 `:239` 自称"清单覆盖**所有会落盘的表**"，与事实不符。
  回归测试 `legacy_loose_credentials_and_sessions_are_tightened_on_open`（`json_store/tests.rs:701`）
  的 `secrets` 数组只有 4 项（USERS/SESSIONS/SMID/PC_DEVICE），
  `existing_loose_permissions_are_tightened_on_open`（`:653`）只测 `accounts.json`
  ⇒ 这 4 个文件的旧权限**没有任何测试**盯着。
- 复现: `sed -n '251,262p' crates/mcloud-store/src/json_store.rs; sed -n '707,712p' crates/mcloud-store/src/json_store/tests.rs`
- 影响: 老数据目录升级后 `task-config.json`（任务默认参数，可能含账号口令/口令池）仍对同机其他用户可读，
  且**不会自愈**（harden 只在 `open` 时对清单内文件执行）。新装实例不受影响。
- 建议: 把 4 个常量加进 `names`；更稳的写法是遍历数据目录里的 `*.json` + `pc_device.txt`，
  这样以后再加表也不会漏；同时把测试改成"目录里所有文件都必须 0600"。
- 状态: 仅静态 · 置信度 高

### co-4 [中] 脱敏的键名在文本层不归一化：`verify_code=` / `api_key=` 这类 snake_case 文本键逃逸
- 位置: `crates/mcloud-core/src/event.rs:319`（文本层）对比 `:508`（JSON 键名层）
- 证据: JSON 那一层用 `normalize_key` 去掉 `_`/`-` 后**相等**匹配（`:507-510`），
  所以 `{"verify_code":"7788"}` 会被整格替换 ✓；
  但文本层 `for key in SENSITIVE_KEYS { find_ascii_ci(&out, key, ...) }`（`:319-322`）
  用的是清单里的**字面串**，而清单里写的是无下划线形态（`:466-490` 的 `"verifycode"` / `"apikey"` /
  `"refreshtoken"` / `"sessionid"` / `"jwttoken"` / `"accesstoken"`）。
  ⇒ 文本 `verify_code=7788`、`api_key=xxx` **一条都匹配不上**（清单里的字面串不是它们的子串：
  `python3 -c "print('verifycode' in 'verify_code=7788', 'apikey' in 'api_key=xxx')"` → `False False`）；
  与之相对，`access_token=` 因为清单里有更短的 `"token"` 而被侥幸拦住 ——
  这正是"有的漏有的不漏"的成因，也说明这不是刻意取舍。
  已有测试 `redacts_password_family_keys`（`:723`）只测 JSON 形态，抓不到这条。
- 复现: `grep -n "for key in SENSITIVE_KEYS" -A3 crates/mcloud-core/src/event.rs; grep -n "verifycode\|\"apikey\"" crates/mcloud-core/src/event.rs`
- 影响: 任务日志/进度文案（`RunEvent::Log.message` / `Progress.label` 的自由文本）里以
  `key=value` 形态出现的校验码、API key 会随 SSE、进程日志、webhook、历史落盘四条路出去。
  这正是 `:296-307` 那段注释说要堵住的那类洞，只是漏了"归一化"这一半。
- 建议: 文本层也按 `normalize_key` 匹配（或对每个 key 同时试"原串/去下划线/去连字符"三种形态）；
  在 `:723` 那条测试里补 `verify_code=` / `api_key=` 的文本形态。
- 状态: 仅静态 · 置信度 高

### co-5 [中] `append_logs` 的落盘顺序可与内存顺序倒挂，且唯一的相关测试只验内存
- 位置: `crates/mcloud-store/src/json_store.rs:927`（快照）· `:943`（gate）
- 证据: 快照 `bucket.clone()`（`:938`）在 db 写锁内取，db 锁在块结束即释放（`:939`），
  `write_gate` 直到 `:943` 才获取 ⇒ 两个并发 `append_logs` 同一 run 时，
  A 拿到子集快照 S1、B 拿到超集 S2，若 B 先拿到 gate 写完、A 后写，磁盘上留下的是 S1。
  而 `:202-206` 注释声称这把锁让"落盘顺序与内存修改顺序一致"。
  测试 `concurrent_log_appends_keep_every_line`（`json_store/tests.rs:166`）只断言
  `s.logs(...)`（内存）一条不少（`:200-205`），**没有 reopen、没有读盘上的文件**
  ⇒ 这条倒挂在测试里完全不可见。
- 复现: `sed -n '923,948p' crates/mcloud-store/src/json_store.rs; sed -n '196,206p' crates/mcloud-store/src/json_store/tests.rs`
- 影响: 崩溃/重启后该 run 的日志比内存少若干行（丢一段历史日志），静默发生。
- 建议: 把 `write_gate` 的获取提到 db 写锁之前（或让快照与落盘在同一个临界区内完成）；
  在 `:166` 那条测试末尾加一次 `JsonStore::open(&d)` 并比对盘上行数。
- 状态: 仅静态 · 置信度 中

### co-6 [低] 黄金向量 `hmacSha256B64` 段既无实现也无测试（而仓里真有一份手写 HMAC）
- 位置: `test/golden/crypto-vectors.json`（段名）· `crates/mcloud-crypto/tests/golden.rs:16`（`Vectors` 结构体）
- 证据: `Vectors`（`golden.rs:16-44`）没有 hmac 字段；`crates/mcloud-crypto/src/` 里也没有 HMAC
  （`grep -rn hmac crates/mcloud-crypto/` 0 命中）。而 `crates/mcloud-app/src/push.rs:587`
  为了不给依赖表加 `hmac` 包，**手写**了 RFC 2104 的 `hmac_sha256_base64`
  （含 `key.len() > 64` 时先 sha256 的分支），它只有一条自造的期望值
  （`crates/mcloud-app/src/push/tests.rs:284`，注释说是"独立的 Python hmac"算的）。
  `test/golden/README.md` 也自认"`hmacSha256B64` 等段当前未消费"。
- 复现: `python3 -c "import json;print(list(json.load(open('test/golden/crypto-vectors.json'))))"; grep -rn hmac crates/mcloud-crypto/ | wc -l`
- 影响: 一份权威向量摆在那里没用上，手写密码学代码的唯一对拍是自证式的；将来改 HMAC 无基线。
- 建议: 用 `hmacSha256B64` 段给那个手写实现加一条对拍（它现在依赖 `sha256_hex_bytes` 再解 hex 回字节，
  多一次编解码，正好值得被向量钉住）。
- 状态: 已实测 · 置信度 高

### co-7 [低] 死公共 API：4 个零引用函数，其中 `open_default` 的自述目的已被违背
- 位置: `crates/mcloud-store/src/lib.rs:26` · `crates/mcloud-core/src/error.rs:68` · `crates/mcloud-core/src/smid.rs:54`、`:112` ·（另 `hash.rs:44`）
- 证据: 对三个 crate 的 `pub fn` 做全仓引用统计，零引用（除声明行）的有且仅有这四个：
  `mcloud_store::open_default`、`mcloud_core::Error::is_auth_failure`、`SmidState::has_fingerprint`、
  `smid_storage_error`。其中 `open_default` 的文档写着"存在的意义是让调用方（`mcloud-app`）**不要**直接写
  `JsonStore::open`"，而实际调用方 `crates/mcloud-app/src/builder.rs:44` 正是 `JsonStore::open(...)`
  ⇒ 不仅没人用，注释所述约束也不成立。另外 `mcloud-crypto` 的 `hash::sha256_hex_upper`
  （`crates/mcloud-crypto/src/hash.rs:44`，`lib.rs:27` 导出）也只被自己的单测引用。
  附带的文档缺陷：`error.rs` 的 `is_auth_failure` 的文档（"上游返回的错误是否为「需要重新登录」…"）
  挂在了紧邻其上的 `is_cancelled`（`:60-64`）头上，而 `is_auth_failure` 自己没有文档。
- 复现: `grep -rn --include=*.rs -w open_default crates/`（只有声明行）; `grep -rn "JsonStore::open(" crates/mcloud-app/src/`
  ; `sed -n '58,70p' crates/mcloud-core/src/error.rs`
- 影响: 公共面比实际用到的宽，读者会以为存在"换后端的统一入口"和"凭证失效判定"两条被使用的通路。
- 建议: 删掉这四个（`is_cancelled` 是同一族的活口，注意别连它的文档一起删错），
  或把 `open_default` 真正接进 `builder.rs`；`sha256_hex_upper` 若确无计划用途一并删。
- 状态: 已实测（grep 计数） · 置信度 高

### co-8 [低] `AppSettings` 只在写路径校验，启动加载不校验 ⇒ 非法 `settings.json` 静默生效
- 位置: `crates/mcloud-store/src/json_store.rs:502`（加载）· `:1237`（保存才 validate）· `crates/mcloud-core/src/settings.rs:65`
- 证据: `load_all` 里 `let settings: AppSettings = self.read_json(...)?`（`:502`），
  **没有任何 `.validate()`**；`AppSettings::validate()` 的全仓调用点只有
  `mcloud-server/src/routes/security.rs:81`（PUT 路由）与 `json_store.rs:1237`（`save_settings`），
  都在**写**的那一侧。会话过期是按 `expires_at_ms: now_ms + ttl_ms` 算的
  （`crates/mcloud-server/src/routes/auth.rs:313`，`make_session`）。
  `AppSettings` 的范围是 `1..=2160` 小时、`4..=64` 位（`settings.rs:27,30`）。
  于是手写/外部工具写出的 `{"session_ttl_hours":0}` 能正常启动，
  之后 `session_ttl_ms() == 0` ⇒ 新建会话 `expires_at_ms == now`，`is_valid_at` 立刻为假
  （`user.rs:109-111`）⇒ **登录进去马上掉线**，而任何地方都不会报"设置非法"。
- 复现: `grep -rn "settings.validate()\|\.validate()" crates/mcloud-store/src/json_store.rs crates/mcloud-server/src/routes/security.rs`
  ; `sed -n '497,510p' crates/mcloud-store/src/json_store.rs`
- 影响: 一处手工编辑或旧版本写出的越界值会变成"登录即掉线"这类难查现象；
  范围下限 4 与文档默认 8 的差异也只有在写路径才被把关。
- 建议: 在 `load_all` 读完后调用一次 `settings.validate()`，不通过就回默认值**并告警**
  （回默认值比拒绝启动更合适，与 `:83` 的 "老实例没有 settings.json 走默认" 同一条路）。
- 状态: 仅静态 · 置信度 高

### co-9 [低] 损坏的 `logs/*.json` 被静默吞掉，与其它表的"损坏即报错"不一致
- 位置: `crates/mcloud-store/src/json_store.rs:516`
- 证据: `if let Ok(lines) = self.read_json::<Vec<LogLine>>(&p) { logs.insert(...) }` ——
  解析失败连一行警告都没有；而同一个 `load_all` 里七个表用的是 `?`（`:495-509`），
  解析失败会让 `JsonStore::open` 整个失败、服务起不来（`read_json` 的错误文本还专门带文件名与前 200 字符，`:371-379`）。
- 复现: `sed -n '511,520p' crates/mcloud-store/src/json_store.rs`
- 影响: 单个 run 的日志文件损坏 ⇒ 那次运行的日志**无声消失**（前端只看到"没有日志"），
  排障时正是最需要它的时候；而与此同时 `history.json` 里 `RunRecord.logs` 还留着一份（≤200 行），
  两份数据不一致且没有任何提示。
- 建议: 至少 `tracing::warn!` 一行（文件名 + 错误），或与其它表统一成致命错误。
- 状态: 仅静态 · 置信度 高

### co-10 [低] cron 规范化在 core 与 engine 各有一份，校验与触发可能再次漂移
- 位置: `crates/mcloud-core/src/schedule.rs:115` · `crates/mcloud-engine/src/scheduler.rs:158`
- 证据: `Schedule::normalize_expr`（core，public，被 `validate_expr` 与创建路由使用）
  与 `mcloud_engine::scheduler::normalize_expr`（engine，私有，`decide` 里使用）
  是两段同构实现（都是"5 段补 `0 `，6 段原样，其余报错"）。
  而 core 那份的文档（`schedule.rs:111-114`）恰好写着"之前两边各写一份，
  结果 `validate_expr` 按段数放行 5 段、随后又用 `from_str` 直接解析而失败"——
  同一个坑的形状又回来了，只是这次两份实现暂时一致。
- 复现: `grep -rn "fn normalize_expr" --include=*.rs crates/`（两处）
- 影响: 只改一边（例如将来支持 `@daily` 之类的别名）就会出现"创建时通过、跑的时候行为不同"，
  而这种漂移不会让任何测试变红。
- 建议: engine 改成调用 `mcloud_core::Schedule::normalize_expr`（它已是 public），删掉那份私有副本。
- 状态: 仅静态 · 置信度 高

### co-11 [低] 更新定时时不带 `timezone` 会静默回落 `Asia/Shanghai`
- 位置: `crates/mcloud-server/src/routes/schedules.rs:157`（写 `Schedule.timezone`，字段定义在 `crates/mcloud-core/src/schedule.rs:46`）
- 证据: `upsert` 是整体覆盖式写入，但对"上次水位线/记账字段"是刻意**保留**的
  （`existing.as_ref().and_then(...)`，`routes/schedules.rs:185-195`），
  唯独 `timezone` 走 `input.timezone.unwrap_or_else(|| "Asia/Shanghai".into())`（`:157`）：
  客户端若不带 `timezone`（`ScheduleInput.timezone` 是 `#[serde(default)] Option<String>`，`:76-77`），
  已有定时的时区会被**悄悄改成东八区**，而 `last_fire_ms` 会被保留 ⇒ 触发点整体平移，
  并且没有任何提示（老前端 bundle 缓存、或用 CLI/curl 做局部更新都会踩到）。
- 复现: `sed -n '155,190p' crates/mcloud-server/src/routes/schedules.rs`
- 影响: 非东八区用户改个名字/改个 cron 表达式，任务就跑在错误钟点；水位线还留着旧时区的值。
- 建议: 与其它字段同口径 —— `input.timezone` 为 `None` 时沿用 `existing.timezone`
  （新行才用默认值），并把"新行"与"老行"两条路径分开写。
- 状态: 仅静态 · 置信度 中（该文件在本次声明的三个 crate 之外，按"跨模块"记录）

## 提问的其余结论（未列入发现，回答原问题）

- **crypto 与 TS 参考一致性**：`sign.rs` 与 `mcloud-sign/packages/core/src/auth/sign.ts` 逐条一致
  （盐值表 `seedMdYYLIZfbCxg`/`sekaMdYYLIZfbCfm`、未知 key 回落第 1 个、`salt+id+ts+nonce+other+salt` 拼接、MD5；
  头名 `x-request-id/x-timestamp/x-nonce/x-signature`；`randomUUID` 对应 `SeqRandom::uuid_v4` 的版本/变体位）。
  `aes::encrypt_with_dynamic_iv` 与 `utils/encrypt.ts` 的 `base64(IV(16) ‖ CBC 密文)` 完全同形、
  密钥按字节长度分派（24 字节 → AES-192 ✓，`encrypt.ts` 的 `Utf8.parse` 等价）。
  DES 的双填充语义（加密补零且对齐不补块 / 解密按 PKCS7）在 `des.rs:1-27` 有明确出处，并由 18+ 条向量钉住。
- **`rand.rs` 是密码学安全随机**：生产实现 `OsRandom::fill_bytes` 走 `getrandom`（`rand.rs:154-163`），
  失败时**重试而不是降级**；`SeqRandom` 明确只用于测试/回放；`SharedRandom` 中毒时兜底也换 `OsRandom`（`:118`）。
  `password.rs` 的盐与会话令牌用 `OsRng`（`:65`、`:107-110`），不是时间戳。
- **秘密进 `Debug`/日志**：未发现泄漏点。`PasswordHash`（`user.rs:41-42`）与 `SharedRandom`（`rand.rs:130-135`）
  手写 `Debug` 脱敏，`JsonStore` 的 `Debug` 只打目录与计数（`json_store.rs:679-694`），
  `CancelToken` 只打是否已取消（`cancel.rs:86-92`）。
  `AuthInfo`/`Account` 仍 `derive(Debug)`（含 `raw`/`token`），但全仓没有把它们 `{:?}` 进日志的调用点
  （`grep -rn "{:?}" crates/ | grep -iv test` 无命中）—— 属"没有闸门但当前也没人走"的形态。
- **`cancel.rs` 的取消语义核对过、没问题**：`cancelled()` 的 `notified()` 与 `notify_waiters` 之间的经典丢唤醒，
  tokio-1.53.1 在 `Notified` 首次 poll 时比对创建时快照的 `notify_waiters_calls`
  （`~/.cargo/registry/src/*/tokio-1.53.1/src/sync/notify.rs:1117-1124`）会立即返回 Ready，
  循环里的"先看标志位"（`cancel.rs:74-78`）因此是双保险。取消是**协作式**的（`cancel.rs:1-13` 有明确理由）。
- **黄金向量是否真被读取**：6 个文件都有代码读（不是只有注释）
  —— `crypto-vectors.json`（`tests/golden.rs:171`、`src/tn.rs:125`、`upstream/src/smid/tests.rs:16`）、
  `password-hash-vectors.json`（`tests/golden_password.rs:74`）、
  `app-signin-vectors.json`（`crypto/tests/golden.rs:449` + `upstream/src/app_signin/tests.rs:15`）、
  `auth-vectors.json`（`upstream/src/auth/tests.rs:23`）、
  `live-room-vectors.json`（多处）、`live-redflower-vectors.json`（`upstream/src/live_flower/tests.rs:15`）。
  本次三个 crate 里唯一没被读的**段**是 `crypto-vectors.json` 的 `hmacSha256B64`（见 co-6）。
  另：`app-exchange-vectors.json` **文件不存在**，只在注释里被引用过（`upstream/src/exchange.rs:35` 已注明）。

## 未覆盖 / 时间盒外
- `crates/mcloud-core/src/{account.rs,kouling.rs,push.rs,traits.rs,exchange.rs,owner.rs,pipeline.rs,pc_device.rs,testkit.rs}`
  只做了针对性 grep（派生的 `Debug`、`.validate()` 调用点、`/ gate`），未逐行读。
- `mcloud-crypto/src/slide.rs` 只读到 200 行（几何/Sobel 评分那半段没逐行核），也未与参考实现逐值对拍。
- 时区/跨日/错过补偿的**判定逻辑**在 `crates/mcloud-engine/src/scheduler.rs`（`decide` / `parse_timezone`，
  `GRACE_MS = 60_000`），不在本次声明的三个 crate 内；我只核到"它明确拒绝不支持的时区、
  只支持固定偏移（无 DST）并在文档里写明"，未展开审。
- 未跑任何 cargo（另一条线在编译），所以所有"仅静态"条目都没有运行期证据。
- 未测多进程并发（代码明确声明只保证进程内，`json_store.rs:32-35`）。
