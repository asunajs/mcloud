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
    if let Some(existing) = read_trimmed(path) {
        return Ok(existing);
    }
    let device_id = new_device_id(rng);
    write_atomically(path, &device_id)?;
    Ok(device_id)
}

/// 读文件里的值；不存在或只有空白都算"没有"。
///
/// 空白也算没有：一个被截断/清空的文件若被当成"已有设备 id"，
/// 就等于每次运行都换一个身份（见 [`load_or_create`] 的说明）。
fn read_trimmed(path: &Path) -> Option<String> {
    let existing = std::fs::read_to_string(path).ok()?;
    let trimmed = existing.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 原子落盘：先写 `<path>.tmp` 再改名。
fn write_atomically(path: &Path, value: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            Error::Internal(format!("创建设备指纹目录失败 {}: {e}", parent.display()))
        })?;
    }

    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{value}\n"))
        .map_err(|e| Error::Internal(format!("写设备指纹失败 {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| Error::Internal(format!("落盘设备指纹失败 {}: {e}", path.display())))?;
    Ok(())
}

// ==================== App 端（iOS）的设备指纹 ====================

/// App 端的 `deviceId`：32 位大写 HEX，**没有** `-ENDIN` 后缀。
///
/// 与 PC 的 [`new_device_id`] 只差那个后缀，但设备串第 6 段是整串比对的一部分，
/// 带上后缀会让上游看到一台不存在的设备形态。
pub fn new_app_device_id(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    hex::encode_upper(bytes)
}

/// App 端的 `deviceShort`：12 位大写 HEX（设备串第 7 段）。
///
/// # 它是**每次运行**生成的，不持久化（实测依据）
///
/// 同一台设备（`deviceId` 完全相同）在两个时间点的 `deviceShort` **不一样**：
///
/// | 观测时间 | `deviceId` | `deviceShort` |
/// | --- | --- | --- |
/// | 2026-08-12 | `67F4C56A…315C` | `8A708867DBC3` |
/// | 2026-09-18 | `67F4C56A…315C` | `EAB9BF94B8DE` |
///
/// 也就是说 **`deviceId` 稳定、`deviceShort` 会变** —— 所以后者**不能**当"每安装指纹"
/// 持久化（那会声称一个已被证伪的稳定性）。它随每次运行新生成。
///
/// **未确定的部分**：它到底按什么变（每次启动 / 每天 / 某个事件）只有两个观测点，
/// 定不下来。记录在 `docs/client-profiles.md` §5.4 的待确认项里。
pub fn new_app_device_short(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 6];
    rng.fill_bytes(&mut bytes);
    hex::encode_upper(bytes)
}

/// 读 App 端 `deviceId`；文件不存在就生成一个并写盘。
///
/// **只持久化 `deviceId`**，`deviceShort` 每次运行用 [`new_app_device_short`] 现生成
/// （理由见该函数的文档）。
pub fn load_or_create_app_device_id(path: &Path, rng: &mut impl Random) -> Result<String, Error> {
    if let Some(existing) = read_trimmed(path) {
        return Ok(existing);
    }
    let device_id = new_app_device_id(rng);
    write_atomically(path, &device_id)?;
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

    #[test]
    fn app_device_id_has_no_endin_suffix() {
        let id = new_app_device_id(&mut SeqRandom(0));
        assert_eq!(id, "000102030405060708090A0B0C0D0E0F");
        assert_eq!(id.len(), 32, "App 的 deviceId 是纯 32 位 HEX");
        assert!(
            !id.ends_with(DEVICE_ID_SUFFIX),
            "带 -ENDIN 的是 PC 的形态，App 端没有"
        );
    }

    #[test]
    fn app_device_short_is_12_hex() {
        let short = new_app_device_short(&mut SeqRandom(0));
        assert_eq!(short, "000102030405");
        assert_eq!(short.len(), 12);
        assert!(short.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn app_device_id_is_stable_across_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-app.txt");

        let first = load_or_create_app_device_id(&path, &mut SeqRandom(1)).unwrap();
        let second = load_or_create_app_device_id(&path, &mut SeqRandom(200)).unwrap();
        assert_eq!(first, second, "deviceId 必须稳定（实测跨月不变）");
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn app_device_short_is_not_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-app.txt");
        load_or_create_app_device_id(&path, &mut SeqRandom(1)).unwrap();

        // 同一个随机源连出两次，得到的 short 也必须不同 ——
        // 它**不落盘**，所以每次运行都是新的（实测：同一 deviceId 换过 short）
        let a = new_app_device_short(&mut SeqRandom(1));
        let b = new_app_device_short(&mut SeqRandom(2));
        assert_ne!(a, b);
        // 而文件里只有 deviceId 一行，没有 short 的位置
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.trim().split('\n').count(), 1, "{content:?}");
    }
}
