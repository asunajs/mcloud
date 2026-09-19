// 本模块单元测试的公共 helper（确定性随机源、夹具取值器）；主题用例见同目录其它文件。
use super::*;

use mcloud_core::{HttpRequest, HttpResponse};

use serde_json::json;

use std::cell::RefCell;

/// 确定性随机源。
struct SeqRandom(u8);

impl Random for SeqRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for byte in dst.iter_mut() {
            *byte = self.0;
            self.0 = self.0.wrapping_add(1);
        }
    }
}

/// 按脚本逐次返回响应的假客户端，并记录发出去的请求。
struct ScriptedClient {
    responses: RefCell<Vec<Value>>,
    seen: RefCell<Vec<HttpRequest>>,
}

impl ScriptedClient {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: RefCell::new(responses),
            seen: RefCell::new(Vec::new()),
        }
    }

    fn sent(&self) -> usize {
        self.seen.borrow().len()
    }
}

impl HttpClient for ScriptedClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        self.seen.borrow_mut().push(request);
        let mut queue = self.responses.borrow_mut();
        if queue.is_empty() {
            return Err(Error::Internal("脚本已用完".into()));
        }
        let body = queue.remove(0);
        Ok(HttpResponse::new(200, serde_json::to_vec(&body).unwrap()))
    }
}

/// 假解密器：登录成功的 `data` 在这个测试里直接就是明文 JSON。
struct PlainDecryptor;

impl LoginPayloadDecryptor for PlainDecryptor {
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value, Error> {
        serde_json::from_str(ciphertext).map_err(|e| Error::Parse(e.to_string()))
    }
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-18T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn user_info() -> Value {
    json!({
        "account": "19900000000",
        "authToken": "SYNTH001|1|RCS|1893456000000|AAAA",
        "userId": "1990000000000000001"
    })
}

mod qr;
mod session;
mod sms;
