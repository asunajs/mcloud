# app-task 单条玩法：从 FAIL 到翻账（排查修复流程）

> 状态：**已用三次真机修复验过** —— 409/1005（打卡 key 发错）、1020（旧路 404 ⇒ 换活路）、
> 1017（缺一发页面初始化上报）✓（见 `docs/app-task-status.md` §5-9 / §5-10 / §5-11）。
> 口径：这份是**源**；同一份内容装在 `.dsh/skills/apptask-fix/SKILL.md`（那份在 `.gitignore` 里，
> 所以**改这里要同步那一份** ✗）。原始任务：用户 2026-10-10「记住为skill」「项目也存一份」✓。

**判据只有一条**：上游的 `state` / 读数**变了**才算做成 ✓ —— "请求发出去了、上游回 200"**不算** ✗。

## 〇、第 0 步：**先确定"这次是哪个号"**（2026-10-11 用户口径，硬要求 ✓）

**用户原话：「记住抓包前需要确定账号」。** 踩过一次，代价是一条错结论 ✓。

* 开抓前 / **读 `state` 前**，把"这次抓的是哪个号"钉下来 ✓，并让**读状态的号**与它**一致** ✓。
* 取证（从 HAR 里搜，任一条能自证即可 ✓）：邮件收件人 `to:"<号>@139.com"` ✓、`uin` ✓、`rmkey` ✓
  （`userNumber` 多为**密文** ✗）。
* ⚠ **不许默认**"抓包号 = 我一直在测的那个号" ✗ —— 2026-10-11 就是这条：
  用户在小号（159****7542）上真机把 1029 做成了（面板「已完成 · 第 4 步」✓），
  agent 却去读**移动**（178****4979）的列表 ✗（此前 16 次候选都在移动上试，就默认同号 ✗），
  于是写下"两趟真机走完都没翻账" ✗ —— 用户拿面板截图当场打回 ✓。
* 细则见抓包仓 `docs/capture-guidelines.md` §1.4 ✓。

## 一、先判它属于哪一类（读 run 明细，别猜）

```bash
MCLOUD_DATA_DIR="$PWD/target/lines/<线名>-data" ./target/lines/<线名>/debug/mcloud \
  run app-task <账号> --body '{"onlyTaskIds":"<id>"}'
python3 - <<'PY'
import json; runs=json.load(open('target/lines/<线名>-data/history.json'))
rid=max(r['run_id'] for r in runs); r=[x for x in runs if x['run_id']==rid][0]
d=(r.get('details') or {}).get('reported') or {}
print(rid, r.get('outcome'))
for s in d.get('steps',[]):
    print(s.get('name'), s.get('stateBefore'),'->',s.get('stateAfter'), 'clicked=',s.get('clicked'))
    for n in (s.get('problems') or []): print('   ', n)
PY
```

| 长相 | 含义 |
|---|---|
| `WAIT → WAIT` + problems 里有具体错误 | 动作**没被上游认**（真因就在那一句里） |
| `WAIT → WAIT` + 只写"本次做了…没记上" | **做了但没记账** ⇒ 缺一发、或 key / 步数不对 |
| `hard_failed` | 链路里有一枪硬失败 ⇒ 先修那一枪 |

## 二、取证三招（三次破局都靠它们）

1. **取原始条目** —— 本仓只解析一部分字段，破局信息常在没解析的那几格里
   （`currstep=1`、`button.email.link` 都是这么看到的）。
   临时在 `parse_task_list_v2` 里按 id `eprintln!("{raw_item}")`，跑一趟、看完**立刻删** ✗。
2. **H5 页面自己的 JS** —— 这一族的"接口真相"在页面包里：
   `curl -s https://img.mcloud.139.com/portal/<page>/public/js/<name>.<hash>.js`，然后
   `grep -oE "/ycloud/[a-zA-Z0-9/_.-]+"` / `grep -oE ".{0,150}<关键词>.{0,250}"`。
   端点、请求体键、`#ssoToken#` 这类占位、回调触发条件（`getHashQueryString("taskToken")`）都在里面 ✓。
3. **真机发一发看上游回什么** —— 业务码本身就是证据（`code=500 msg=无法获取中国移动任务信息`
   这种话能把"猜"直接挡掉 ✓）。

**证据优先级**：抓包（真机实拍 / HAR）> `mcloud-sign`（TS 参考实现）> 推断。
⚠ "没有抓包"**不是**不做的理由：TS 有实现就照 TS 写；只有真写不了的分支才留空，并注明为什么 ✓。

## 三、常见真因清单

1. **打卡 key / 步数**：默认 `task`；`409 → task2`、`478/481 → randomCloudTask`。
   `currstep == 0` 才有"先打卡"那一发；`currstep != 0` 只发"补的那一发"。
   ⚠ 同一个 id **两发的 key 可能不同**（409 = `task` 然后 `task2`）⇒ 别共用一个函数 ✗。
2. **缺一发"页面初始化上报"**：`POST /ycloud/signin/page/doTaskPost`（页面 JS 里叫 `initTaskStatus`），
   `activityid` 跟**本条任务的市场名**走（`sign_in_3` / `newsign_139mail`）。1017 就是缺它 ✓。
3. **旧 TS 的路径已经死了**：先看 `../mcloud-capture` 的读数，别照抄 404 的老路
   （`mail.10086.cn/mailsms/sms/backup` 今天 404 ⇒ 换到活主机
   `html5.mail.10086.cn/mw2/file/disk?func=disk:*` + App 自己的字面量形状 ✓）。
4. **开关型参数**：`encryptType=1` / `encryptResult=1` 这类"客户端能解"的前提会让响应变**密文**
   （实测 3296 个十六进制字符）⇒ 探针侧读不出任何字段，先去掉再试 ✓。
5. **两格入口别串台**：`button.app.link`（615 那一族）与 `button.email.link`（1017）是**两格**；
   且后者十来个条目都有 ⇒ **谁拿它做什么要逐条看**，不许按"有这一格"推广 ✗。
7. **打卡发几发，看 `currstep`**：那道"先打卡"的门（③-a）在 `currstep == 0` 时**自己会打一发**
   （默认 key `task` ✓）⇒ 你那一档**只补"门没打"的那一发** ✓：
   1017（`currstep = 1`）⇒ 补 ✓；604（`currstep = 0`）⇒ **不补** ✗，否则同一条任务连打两发。
   ⚠ 例外是 409：它的 runner 在 TS 里**又打了一发 `task2`** ⇒ 那条**就是要两发** ✓（"两发"是读出来的，不是推出来的）。
6. **归因要分开**：改了两处就别把功劳记一起。做法：只发 A+B（✗）→ 补上 C（✓）⇒ 才敢写"缺的是 C" ✓。

8. **"打卡没反应"和"打卡不算数"是两件事**：有的条目打卡会**真的动上游**（609 那次把 `currstep`
   从 0 推到 3 ✓）却**不翻账** ✗ ⇒ 别把"步数变了"当"做成了" ✓。判据永远只有 `state` / 读数 ✓。
9. **同一趟里放两条互为对照**：run 318 一趟跑 547 + 609、只发同一发打卡 —— 547 翻 ✓ / 609 不翻 ✗
   ⇒ 这一对**互为对照**，"打卡对这一族够不够"当场就有了答案 ✓（比分开跑两趟干净 ✓）。

## 四、最小修 + 测试

- 只改**这一条**要走的那条路，不顺手推广到别的条目（它们没有读数支撑 ✗）。
- 单测钉**形状**（URL / 体 / 头 / 顺序），再钉**失败档**（业务码打回算硬失败、拿不到参数算硬失败）；
  运行级测试钉"这一档共发几发、顺序如何"。

## 五、验收协议（用户口径：一个账号实现、另一个测试）

| 档 | 用途 |
|---|---|
| 账号 A | 实现那一趟（改完先在这里翻） |
| 账号 B | 验证那一趟（换个账号再翻一次） |
| 账号 C | 若早已 `FINISH` ⇒ "外部也会翻"也是读数 ✓ |

⚠ 月限任务**翻过就不能重试** ⇒ 要分离归因得在**别的账号**上做，别把验证账号用掉 ✓。

## 六、落点四件套（少一件都不算完）

1. 代码：`crates/mcloud-tasks/src/app_task*.rs` + `crates/mcloud-upstream/src/<族>.rs`
   （形状与出处写进文档注释，含"哪一格是拼装、哪一格是逐字"）。
2. `docs/app-task-status.md`：主表那一行改成"✅ 已实现 + 判据 + 验收读数"，并在 §5 末尾加一节
   （现象 → 真因 → 修法 → 归因 → 真机验收 → 测试数）。
3. `docs/task-log.md`：追加一行（下达 / 完成时刻 / 状态 / 落点 / **没做完的部分**）——
   追加前先取当前最大编号，别和别的写入线重号。
4. `../mcloud-capture`：实拍读数写进对应 `docs/capture/*.txt`（**只写键名与形状，值一个不落**），
   然后 `node scripts/capture-check.js` 必须 **4/4** ✓。

## 七、上线（三条硬坑）

```bash
export CARGO_HOME="$PWD/.cargo-home"; export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$PWD/target/lines/<线名>"        # ① 每线一个 target，别抢共享 target
cargo build -p mcloud-server --bin mcloud
rm -f target/debug/mcloud && ln target/lines/<线名>/debug/mcloud target/debug/mcloud   # ② 用 ln，别用 cp
MCLOUD_SKIP_LOCK_CHECK=1 bash scripts/dev-reload.sh --no-build                         # ③ 签名 + 重启
curl -s http://127.0.0.1:3000/api/health
python3 -c "b=open('target/debug/mcloud','rb').read(); print(b.count('新增的串'.encode()))"   # 别用 grep -a
```

- **`cp` 进 `target/debug/` 的可执行文件 exec 会卡在 `_dyld_start`**（外置盘实测；同内容**硬链接**
  或放子目录都正常 ✓）⇒ 一律 `ln` ✓。
- **共享 `target/` 会卡死**（rustc 子进程 0% CPU 十几分钟），期间 WebUI 不可用 ⇒ 起手就设自己的
  `CARGO_TARGET_DIR` ✓。
- 二进制里查串**用字节计数**（`grep -a` / `strings` 在这份二进制上一律漏报 ✗）；没调用方的
  `pub` 常量/函数会被链接器丢掉 ⇒ 查不到串**不等于**漏编 ✓。
- ⚠ 别对全仓跑 `cargo fmt --all`：会顺手改到**别的写入线**的未提交文件 ✗；只格式化本次动的文件 ✓。

## 八、收尾

`cargo test -p mcloud-tasks --lib`、`-p mcloud-upstream --lib` 全绿；
`cargo clippy -p … --all-targets -- -D warnings` 与 `cargo fmt --all -- --check` 干净；
然后按台账口径写状态：**已上线**（跑在 `:3000` 的就是它 ✓）/ **已落盘**（没重启 ✗）/ **待确认** / **未做**。
