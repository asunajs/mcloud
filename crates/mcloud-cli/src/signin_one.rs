// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::args::*;
use crate::signin_output::*;
use crate::signin_run::*;
use crate::smid_cmd::resolve_device_id;
use crate::support::*;
use anyhow::Result;
use mcloud_app::{app_login, signin};
use mcloud_core::{Random, Scope, StateStore};
use mcloud_io::{ReqwestClient, SqliteStore, SystemClock};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::{AppDevice, AppProfile};

/// 跑**一个**账号的一次签到（含 `--receive` / `--revive` 两个写开关）。
///
/// 返回 `Ok(None)` = 人看的输出已经打完了；`Ok(Some(v))` = `--json` 模式的结果对象，
/// 交给调用方决定怎么合并（单账号直接打、多账号合成数组）。
///
/// `device_id` / `device_short` 由调用方给：它们代表**这台安装**，一次运行里不该变。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn signin_one(
    store: &SqliteStore,
    scope: Scope<'_>,
    target: &SigninTarget,
    device_id: &str,
    device_short: &str,
    args: &SigninArgs,
    rng: &mut impl Random,
) -> Result<Option<serde_json::Value>> {
    let account_id = &target.account_id;
    let phone_override = target.phone_override.as_deref();

    // 设备形态必须与登录时一致（同一个文件 → 同一个 `deviceId`）。
    // 漂移的表现是"凭据都在、却回 90001 未登录"，看起来像凭据坏了。
    let profile = AppProfile::new(AppDevice::ipad(device_id, device_short));
    // 回填库里存的 `userDomainId`（= `x-yun-uni` 头）。读的范围必须与写的一致，
    // 否则会出现"读到某个账号的用户域、却把它发给另一个账号"。
    let profile = app_login::profile_with_stored_domain_id(store, scope, profile, account_id)
        .await
        .map_err(core_err)?;

    let credential = match phone_override {
        Some(phone) => signin::SignInCredential::from_phone(store, scope, phone).await,
        None => signin::SignInCredential::from_account(store, scope, account_id).await,
    }
    .map_err(core_err)?;

    // `--phone` 只该输一次：把号码记下来，下次 `--account <下标>` 就行。
    //
    // # 为什么**只记手机号**，不记上面那个现拼出来的整串
    //
    // `authorization` 列的口径是"**服务端给的原文**"——登录响应里拿到的、或旧配置里
    // 存着的那一串。而这里手上的整串是**我们拼的**：前缀由 `AppProfile` 硬编码成
    // `mobile`（见 `mcloud_app::signin` 里那道前缀闸）。
    //
    // 把拼出来的东西写进那一列，等于**给它伪造一个"这是拿到的"的身份**：
    //   ① 它会被整串优先路径直接采信，而那条路**绕过前缀闸** —— 此后这个账号
    //      发出去的每个头都带着一个我们自己安上去的前缀，没人再会去核对；
    //   ② 一旦它是 `pc:` 端的账号（`platform` 列为空的老行拦不住），存下去的就是
    //      一串**永久错的**凭据，而现象只是"忽然回 90001 未登录"。
    //
    // 只记手机号就够了：下次走退路照旧现拼，前缀闸照旧生效。用户不再需要输第二遍，
    // 而"这串是从哪来的"这个问题永远不会被答错。
    //
    // 用 `fill_account_credentials`（**只填空缺**）而不是整体覆盖：库里已有整串的账号
    // 不该被这个动作碰到。
    //
    // 失败不致命（这次签到用的凭据已经拼好了），但**要说一声**：否则用户下次还得
    // 再输一遍，而他不知道为什么。
    if let Some(phone) = phone_override
        && let Err(err) = store
            .fill_account_credentials(scope, account_id, None, Some(phone))
            .await
    {
        eprintln!("⚠ 手机号没能记进库里（下次还得再输一遍）：{err}");
    }

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);

    // H5 那个 `deviceId`。**显式给的优先**；否则从库里取（没有就现上报一次换一个）——
    // 两条路都是纯 Rust，见 [`resolve_device_id`]。
    //
    // 解析失败**不当致命错误**：签到本身不带它也能签上（实测 2026-09-19 账号 B），
    // 所以降级成"不带"，只把原因打到 stderr。真正需要它的 `--receive` 会从上游
    // 拿到 `615` —— 那时这条提示就是"为什么是 615"的唯一线索。
    //
    // `--status-only` / `--raw` / `--records` 直接跳过：查个状态不该顺手去打一次第三方风控。
    let read_only = args.status_only || args.raw || args.records;
    let h5_device_id = if read_only {
        None
    } else {
        match resolve_device_id(
            &upstream,
            store,
            scope,
            account_id,
            args.device_id.as_deref(),
            rng,
        )
        .await
        {
            Ok(id) => Some(id),
            Err(err) => {
                eprintln!("⚠ 取 H5 deviceId 失败，这次不带它：{err}");
                None
            }
        }
    };

    // 下标只有"按列表跑"时才有意义；`--phone` 定位的账号记 0，那时不打下标 ——
    // 打一个 0 出来只会让人以为它是第一个账号。
    let who = if target.index == 0 {
        format!("账号 {}", credential.account_id())
    } else {
        format!("账号 {} ({})", target.index, credential.account_id())
    };
    let progress = format!(
        "{who}：以 iOS App 身份{}",
        if args.raw {
            "取 infoV3 原始响应"
        } else if args.records {
            "取豆的收支流水"
        } else if args.prizes {
            "查待领取奖品"
        } else if args.status_only {
            "查询签到状态"
        } else {
            "签到"
        }
    );
    // ⚠ `--json` 时这行走 **stderr**。
    //
    // stdout 上只能有 JSON —— 否则 `mcloud signin --json | jq` 会因为这一行直接报
    // 语法错，而"机器读的输出里混进一句人话"正是最难被发现的破坏方式：
    // 人肉看一眼觉得没问题，脚本那边已经全挂了。
    if args.json {
        eprintln!("{progress}");
    } else {
        println!("{progress}");
    }

    if args.records {
        // 两个账本各取一页。顺序固定（改名前的在前），这样人看的输出与 JSON 的键序一致。
        let books = [
            (signin::RECORD_BOOK_CLOUD, "云朵账本（改名前）", "cloud"),
            (signin::RECORD_BOOK_BEAN, "AI豆账本（改名后）", "bean"),
        ];
        let mut json = serde_json::Map::new();
        for (book, label, key) in books {
            // `--records-all` 就一页页翻到底；默认只取第一页。
            //
            // 退出条件三个都要：`--records-all` 没开、上游给了 `total` 且已收满、
            // 或者**这一页是空的**（`total` 缺失时唯一的终止依据）。
            let mut collected = Vec::new();
            let mut total = None;
            for page_number in 1..=RECORD_MAX_PAGES {
                let page = signin::cloud_records(
                    &upstream,
                    &profile,
                    &credential,
                    &signin::RecordsQuery {
                        ai_dou: book,
                        filter: signin::RECORD_FILTER_ALL,
                        page_number,
                        page_size: RECORD_PAGE_SIZE,
                    },
                    &SystemClock,
                    rng,
                )
                .await
                .map_err(core_err)?;
                let got = page.records.len() as i64;
                total = page.total.or(total);
                collected.extend(page.records);
                if !args.records_all || got == 0 {
                    break;
                }
                if total.is_some_and(|t| collected.len() as i64 >= t) {
                    break;
                }
            }
            let page = signin::CloudRecords {
                records: collected,
                total,
            };
            if !args.json {
                print_cloud_records(label, book, &page);
            }
            json.insert(
                key.to_string(),
                serde_json::json!({
                    "aiDou": book,
                    "total": page.total,
                    "records": page.records,
                }),
            );
        }
        if args.json {
            return Ok(Some(serde_json::Value::Object(json)));
        }
        return Ok(None);
    }

    if args.prizes {
        // **只读**，而且**不落库** —— 与 `--raw` 同策：奖品列表不是余额，记进
        // `cloud_stats` 只会往趋势图里塞噪声。
        //
        // 只取一页（`PRIZE_PAGE_SIZE`）：这条的用途是"看有没有待领的"，
        // 不是把历史全拉下来。要看更多页得自己翻 —— 现在没有那个开关。
        let page = signin::pending_prizes(
            &upstream,
            &profile,
            &credential,
            &SystemClock,
            rng,
            PRIZE_PAGE_SIZE,
        )
        .await
        .map_err(core_err)?;
        if args.json {
            return Ok(Some(serde_json::json!({
                "marketName": signin::PRIZE_MARKET_NAME,
                "status": signin::PRIZE_STATUS_PENDING,
                "pageSize": PRIZE_PAGE_SIZE,
                // 条目原样带出去 —— 我们只认得出几个字段，其余不替上游编名字。
                "records": page.records.iter().map(|item| item.raw.clone()).collect::<Vec<_>>(),
            })));
        }
        print_pending_prizes(&page);
        return Ok(None);
    }

    if args.raw {
        // 排障路径：**不解析、不落库**。
        //
        // 不落库是有意的 —— 原始响应没被解析成 `cloudTotal`，记一条"余额未知"
        // 的历史只会往趋势图里塞噪声。`--raw` 是拿来看的，不是拿来攒数据的。
        let raw = signin::status_raw(&upstream, &profile, &credential, &SystemClock, rng)
            .await
            .map_err(core_err)?;
        if args.json {
            return Ok(Some(raw));
        }
        // 人看的时候给缩进版；`to_string_pretty` 对 `Value` 不会失败，
        // 但真失败了也不该让整个命令挂掉 —— 退回紧凑形态，信息一点不少。
        println!(
            "{}",
            serde_json::to_string_pretty(&raw).unwrap_or_else(|_| raw.to_string())
        );
        return Ok(None);
    }

    if args.status_only {
        let status = signin::status(&upstream, &profile, &credential, &SystemClock, rng)
            .await
            .map_err(core_err)?;
        // 采集放在 `--json` 判断**之前**：落库与"这次怎么打印"无关，
        // json 模式同样该攒下余额历史，否则脚本调用的那些天在趋势图上是空的。
        // ⚠ 记的是 **`bean_total`（上游 `total`）**，不是 `cloud_total`（`beforeTotal`）——
        // 后者不随领取变化，记它等于让趋势图永远是一条直线。
        // 这一对数字有实拍对照，见 `mcloud_app::cloud::record_cloud` 的说明。
        let cloud_note = record_cloud_note(store, scope, account_id, status.bean_total).await;
        if args.json {
            return Ok(Some(signin_status_json(&status, None)));
        }
        print_signin_status(&status);
        if let Some(note) = cloud_note {
            println!("{note}");
        }
        return Ok(None);
    }

    let report = signin::sign_in(
        &upstream,
        &profile,
        &credential,
        &signin::SignInOptions {
            device_id: h5_device_id.as_deref(),
            force: args.force,
        },
        &SystemClock,
        rng,
    )
    .await
    .map_err(core_err)?;

    // 记 `after` 而不是 `before`：两者在余额上应当相等
    // （签到只是打卡，豆要另外领 —— 实测签到前后 `total` 不变），
    // 取 `after` 是"更晚读到的那次"。
    let cloud_note = record_cloud_note(store, scope, account_id, report.after.bean_total).await;

    // ⚠ 两个写开关（`--receive` / `--revive`）**必须跑在 `--json` 判断之前**。
    //
    // 放在后面的话，`--json --receive` 会**静默什么都不做** —— 而静默空操作比报错
    // 难查得多：定时任务会以为领过了，日志里却一个字都没有。结果先收进 `extras`，
    // 两种输出各自消费（人看的输出**仍然排在签到结论之后**，见下面的循环）。
    let mut extras: Vec<(&'static str, WriteOutcome)> = Vec::new();

    if args.receive {
        // 领取会**再建一次会话**（`open_session` 在 `sign_in` 内部，没暴露出来）。
        // 自检场景下多两次请求无所谓 —— 见 `signin::receive_all` 的说明。
        //
        // 失败**不用 `?` 抛出去**：这个开关的用途恰恰是"看上游到底回什么码"，
        // 抛出去就只剩一句错误信息，反而看不到原始 code。
        let device = h5_device_id.as_deref().unwrap_or("");
        let device_note = match (device.is_empty(), args.device_id.is_some()) {
            (true, _) => "为空".to_string(),
            (false, true) => "用 --device-id 给的".to_string(),
            (false, false) => format!(
                "用库里/新上报的（{}…）",
                device.chars().take(8).collect::<String>()
            ),
        };
        let (head, json, tail) = if args.cloud_type.is_some() {
            // ── 显式模式（自检用）：我说领哪一笔就领哪一笔 ──────────────────
            let head = format!(
                "指定领取：再发一次 receiveV3（cloudId={:?}，cloudType={:?}，deviceId {}）",
                args.cloud_id, args.cloud_type, device_note
            );
            match signin::receive_cloud(
                &upstream,
                &profile,
                &credential,
                &signin::ReceiveTarget {
                    cloud_id: &args.cloud_id,
                    cloud_type: args.cloud_type,
                    device_id: device,
                },
                &SystemClock,
                rng,
            )
            .await
            {
                Ok(result) => (
                    head,
                    serde_json::json!({ "ok": true, "mode": "explicit", "received": result.receive }),
                    format!("  上游回的 result.receive = {} 豆", result.receive),
                ),
                Err(err) => (
                    head,
                    serde_json::json!({ "ok": false, "mode": "explicit", "error": err.to_string() }),
                    format!("  领取失败：{err}"),
                ),
            }
        } else {
            // ── 默认：按上游的账目**自己判断**该领哪些 ────────────────────────
            //
            // 这是日常该走的路。把"该领哪个"交给用户，等于让用户去猜 `cloudType`
            // 的含义，而猜错的表现是"领到 0 朵"——看起来像上游没发。
            let head = format!("按 infoV3 的账目自动分流领取（deviceId {device_note}）");
            match signin::receive_all(&upstream, &profile, &credential, device, &SystemClock, rng)
                .await
            {
                Ok(outcomes) => {
                    let json = serde_json::json!({
                        "ok": true,
                        "mode": "auto",
                        "items": outcomes.iter().map(receive_outcome_json).collect::<Vec<_>>(),
                    });
                    let lines: Vec<String> = outcomes.iter().map(receive_outcome_line).collect();
                    let tail = if lines.is_empty() {
                        "  账目里没有可处理的条目（`receiveList` 为空）".to_string()
                    } else {
                        lines.join("\n")
                    };
                    (head, json, tail)
                }
                Err(err) => (
                    head,
                    serde_json::json!({ "ok": false, "mode": "auto", "error": err.to_string() }),
                    format!("  领取失败：{err}"),
                ),
            }
        };
        extras.push((
            "receive",
            WriteOutcome {
                human: format!("{head}\n{tail}"),
                json,
            },
        ));
    }

    if args.revive {
        // ⚠⚠ **不可逆** —— H5 原话「关闭弹窗视为放弃机会，无法补发」。
        // 所以它单独一个开关，绝不跟着签到顺手跑。
        //
        // 与 `--receive` 同样的理由，失败**不用 `?` 抛出去**：
        // 这个开关的用途就是"看上游到底回什么码"。
        //
        // 注意 `device_id` 给的是 `Option`（不 `unwrap_or("")`）——
        // `None` = 照 H5 定义**不带**这个头，与"给了个空串"是两件事。
        // 注意这里**故意不回落**到库里的 `smid_device_id`：H5 的
        // `receiveRevivalReward` 定义里没有 `isDeviceId`，默认就该一个头都不带。
        // 只有显式 `--device-id` 时才补这个头（两个证据不一致，见上游层说明）。
        let device = args.device_id.as_deref();
        let head = format!(
            "再发一次 receiveRevivalReward（用复活卡/回馈卡，deviceid 头{}）",
            if device.is_some() { "已给" } else { "不带" }
        );
        let (json, tail) = match signin::receive_revival_reward(
            &upstream,
            &profile,
            &credential,
            device,
            &SystemClock,
            rng,
        )
        .await
        {
            Ok(reward) => (
                serde_json::json!({
                    "ok": true,
                    "cardType": reward.card_type,
                    "rewardClouds": reward.reward_clouds,
                    "totalClouds": reward.total_clouds,
                    "littleFirstTime": reward.little_first_time,
                }),
                format!(
                    "  用掉一张{}：本次 +{} 豆，累计 {}（littleFirstTime={}）",
                    if reward.card_type == 1 {
                        "复活卡"
                    } else {
                        "回馈卡"
                    },
                    reward.reward_clouds,
                    reward.total_clouds,
                    reward.little_first_time
                ),
            ),
            Err(err) => (
                serde_json::json!({ "ok": false, "error": err.to_string() }),
                format!("  失败：{err}"),
            ),
        };
        extras.push((
            "revive",
            WriteOutcome {
                human: format!("{head}\n{tail}"),
                json,
            },
        ));
    }

    if args.multiple {
        // **写请求，每月只有一次机会。** 门槛由 `claim_multiple` 自己判
        // （`curMonthBackup && !curMonthBackupSignAccept`）—— 不成立就**一个请求都不发**。
        //
        // 失败**不用 `?` 抛出去**：与 `--receive` / `--revive` 同一个理由 ——
        // 这个开关的用途就是"看上游到底回什么码"。
        let (json, tail) =
            match signin::claim_multiple(&upstream, &profile, &credential, &SystemClock, rng).await
            {
                Ok(outcome) => (
                    serde_json::json!({
                        // `ok` = "这次调用没出错"，**不等于"领到了"** ——
                        // 三种"按逻辑就没领"（已领过 / 没开备份 / 判不了）也是 `ok: true`。
                        // 领没领到看 `claimed`。
                        "ok": true,
                        "claimed": matches!(outcome, signin::MultipleOutcome::Claimed { .. }),
                        "outcome": multiple_outcome_json(&outcome),
                    }),
                    multiple_outcome_line(&outcome),
                ),
                Err(err) => (
                    serde_json::json!({ "ok": false, "claimed": false, "error": err.to_string() }),
                    format!("  失败：{err}"),
                ),
            };
        extras.push((
            "multiple",
            WriteOutcome {
                human: format!("领签到翻倍（`multiple`，每月一次）\n{tail}"),
                json,
            },
        ));
    }

    if args.backup_gift {
        // **写请求，每月只有一次机会。** 门槛由 `claim_backup_gift` 自己判
        // （`curMonth` + `state`）—— 不成立就**一个请求都不发**。
        //
        // 失败**不用 `?` 抛出去**：与 `--receive` / `--revive` / `--multiple` 同一个理由 ——
        // 这个开关的用途就是"看上游到底回什么码"。
        let (json, tail) =
            match signin::claim_backup_gift(&upstream, &profile, &credential, &SystemClock, rng)
                .await
            {
                Ok(outcome) => (
                    serde_json::json!({
                        // 与 `--multiple` 同一套约定：`ok` = "这次调用没出错"，
                        // **不等于"领到了"** —— 三种"按逻辑就没领"也是 `ok: true`。
                        "ok": true,
                        "claimed": matches!(outcome, signin::BackupGiftOutcome::Claimed { .. }),
                        "outcome": backup_gift_outcome_json(&outcome),
                    }),
                    backup_gift_outcome_line(&outcome),
                ),
                Err(err) => (
                    serde_json::json!({ "ok": false, "claimed": false, "error": err.to_string() }),
                    format!("  失败：{err}"),
                ),
            };
        extras.push((
            "backupGift",
            WriteOutcome {
                human: format!("领备份好礼（`backupgift`，每月一次）\n{tail}"),
                json,
            },
        ));
    }

    if args.json {
        let mut value = signin_status_json(&report.after, Some(&report));
        if !extras.is_empty() {
            let object = value
                .as_object_mut()
                .expect("signin_status_json 一定返回对象");
            for (key, extra) in &extras {
                object.insert((*key).to_string(), extra.json.clone());
            }
        }
        // 打不打由调用方定：多账号时它要把每个账号的结果合成数组再输出。
        return Ok(Some(value));
    }

    // 三种结局必须**分开报** —— 它们在日志里长得一样，但含义完全不同：
    // 定时任务连跑一个月，只有分开才知道它到底有没有在干活。
    if report.signed_now() {
        match report.sign_in_points() {
            Some(points) => println!("签到成功：+{points} 豆"),
            None => println!("签到成功"),
        }
    } else if report.was_already_signed() {
        if report.result.is_some() {
            println!(
                "今天本来就已签；按 --force 重发了一次 startSignIn，上游接受（协议自检通过）。"
            );
        } else {
            println!("今天已经签过了，本次没有重复提交。");
        }
    } else {
        // 既不是"刚签上"、也不是"本来就签过"，却还能走到这里 —— 不该发生。
        // 宁可打一句明确的异常，也不要静默按"成功"糊过去。
        println!("⚠ 本次既未签上、也没有「今天已签」的凭据，请检查 infoV3 的返回值。");
    }
    print_signin_status(&report.after);
    if let Some(note) = cloud_note {
        println!("{note}");
    }
    // 写开关的输出排在签到结论**之后** —— 它们是在上面已经发生过的动作，
    // 但"这次签到怎么样"才是这一屏的主角。
    for (_, extra) in &extras {
        println!("{}", extra.human);
    }
    Ok(None)
}
