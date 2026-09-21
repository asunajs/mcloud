// 旧数据解析单元测试的公共夹具与 helper；主题用例见同目录其它文件。
use super::*;

use base64::Engine as _;

/// 夹具由 `scripts/gen-legacy-fixtures.cjs` 从旧项目真实数据脱敏生成。
/// 用 `include_str!` 而不是运行时读盘：夹具缺失就是编译失败，不会静默跳过断言。
mod fixture {
    pub const CONFIG: &str = include_str!("../../../../../test/fixtures/legacy/asign.json");
    pub const STATE: &str = include_str!("../../../../../test/fixtures/legacy/state.json");
    pub const HISTORY: &str = include_str!("../../../../../test/fixtures/legacy/history.json");
    pub const PIPELINES: &str = include_str!("../../../../../test/fixtures/legacy/pipelines.json");
    pub const SCHEDULES: &str = include_str!("../../../../../test/fixtures/legacy/schedules.json");
    pub const EXCHANGE: &str = include_str!("../../../../../test/fixtures/legacy/exchange.json");
    pub const WEBUI: &str = include_str!("../../../../../test/fixtures/legacy/webui.json");
    pub const META: &str = include_str!("../../../../../test/fixtures/legacy/$meta.json");
}

fn load<T: DeserializeOwned>(text: &str) -> T {
    serde_json::from_str(text).expect("夹具应是合法 JSON 且字段与新定义一致")
}

/// 造一个能通过 `parse_auth` 的 auth，用来在测试里构造任意手机号的账号。
fn auth_for(phone: &str) -> String {
    let payload = format!("2:{phone}:ak|bk|ck|1893456000000|fk");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(payload)
    )
}

fn account(phone: &str, nickname: Option<&str>) -> LegacyAccountConfig {
    LegacyAccountConfig {
        auth: auth_for(phone),
        nickname: nickname.map(str::to_string),
        phone: None,
    }
}

fn fixture_views() -> Vec<LegacyAccountView> {
    describe_legacy_accounts(&load::<LegacyConfig>(fixture::CONFIG).caiyun)
}

mod fixtures;
mod identity;
mod normalize;
mod references;
