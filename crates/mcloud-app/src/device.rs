//! 设备指纹的**本地持久化**。
//!
//! # 为什么是文件，不是数据库
//!
//! 扫码登录的第一件事就是把 `dID` 拼进二维码 —— 那一刻数据库**可能还没建**
//! （全新安装、甚至还没决定库放哪）。而 `dID` 一旦漂移，服务端就按 `dID + sID`
//! 定位不到会话，实测表现是轮询一直停在 `200059541 未找到设备的二维码信息`：
//! 二维码扫了、手机上点了确认，这边却毫无反应 —— 这是最难查的一类故障。
//!
//! 所以它必须是最早存在、且永不改变的那一份本地状态，单独一个文件最合适。
//!
//! # 形状的证据
//!
//! 客户端 `apiServer.js: getClientDeviceInfo()`：原生接口拿不到时
//! `windowMachine = Utils.getGUID32() + '-ENDIN'`，而 `getGUID32()` 是
//! `MD5(随机 UUID)` 的大写十六进制。
//!
//! 也就是说这是"按安装随机生成"的值 —— **不需要真实硬件信息**，
//! 我们自己生成完全成立（见 `docs/client-profiles.md` §5.1）。

use mcloud_core::{Error, Random};
use std::path::Path;

/// 设备 id 的后缀。客户端实测值形如 `<32 位大写 HEX>-ENDIN`。
pub const DEVICE_ID_SUFFIX: &str = "-ENDIN";

/// 生成一个新的设备 id：32 位大写十六进制 + `-ENDIN`。
pub fn new_device_id(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    format!("{}{DEVICE_ID_SUFFIX}", hex::encode_upper(bytes))
}

/// 读设备 id；文件不存在就生成一个并写盘。
///
/// 写入是**先写临时文件再改名**：直接写目标文件时，如果进程在写到一半时退出，
/// 留下的是一个被截断的 id —— 而它下次会被当成"已经存在的设备 id"读进去，
/// 于是这台机器的身份永远变了（服务端再也定位不到会话）。
pub fn load_or_create(path: &Path, rng: &mut impl Random) -> Result<String, Error> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let device_id = new_device_id(rng);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            Error::Internal(format!("创建设备指纹目录失败 {}: {e}", parent.display()))
        })?;
    }

    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{device_id}\n"))
        .map_err(|e| Error::Internal(format!("写设备指纹失败 {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| Error::Internal(format!("落盘设备指纹失败 {}: {e}", path.display())))?;

    Ok(device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    #[test]
    fn device_id_matches_the_captured_shape() {
        let id = new_device_id(&mut SeqRandom(0));
        // 实测：32 位大写 HEX + "-ENDIN"
        assert_eq!(id, "000102030405060708090A0B0C0D0E0F-ENDIN");
        assert_eq!(id.len(), 32 + DEVICE_ID_SUFFIX.len());
        assert!(id.ends_with("-ENDIN"));
    }

    #[test]
    fn device_id_is_stable_across_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.txt");

        let first = load_or_create(&path, &mut SeqRandom(1)).unwrap();
        // 第二次用**完全不同的随机源**：读到的必须还是同一个 id，
        // 否则"刷新二维码导致设备漂移"这个故障就复现了
        let second = load_or_create(&path, &mut SeqRandom(200)).unwrap();
        assert_eq!(first, second);
        assert!(path.is_file());
    }

    #[test]
    fn empty_or_blank_file_is_treated_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.txt");
        std::fs::write(&path, "   \n").unwrap();

        let id = load_or_create(&path, &mut SeqRandom(3)).unwrap();
        assert!(id.ends_with("-ENDIN"), "{id}");
        // 空白文件必须被真正重写，而不是每次都重新生成一个
        let again = load_or_create(&path, &mut SeqRandom(9)).unwrap();
        assert_eq!(id, again);
    }

    #[test]
    fn missing_parent_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/device.txt");
        let id = load_or_create(&path, &mut SeqRandom(0)).unwrap();
        assert!(path.is_file(), "父目录应被自动创建");
        assert!(id.ends_with("-ENDIN"));
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.txt");
        load_or_create(&path, &mut SeqRandom(0)).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name != "device.txt")
            .collect();
        assert!(leftovers.is_empty(), "不该留下临时文件: {leftovers:?}");
    }
}
