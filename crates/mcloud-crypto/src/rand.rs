//! 可注入的随机源。
//!
//! # 为什么"随机"也要是一个 trait
//!
//! 设备指纹（smid）的整条链路上到处是随机：uid、smid 字段里的 uuid、
//! `vpw` / `trees`、电池电量、RSA 的 PKCS#1 v1.5 填充。
//! 如果这些直接取全局随机数，那么**同一份输入永远得不到同一份输出** ——
//! fixture 回放与黄金向量对拍都无从谈起（每次请求体都不同）。
//!
//! 把随机收敛成 trait 之后，测试注入一个"按固定序列吐字节"的假源，
//! 整条链路就变成字节级可复现的纯函数。生产实现（[`SystemRandom`] 之类）
//! 放在有 IO 权限的组装层，本 crate 保持零 IO。
//!
//! # 语义约定：这是 CSPRNG
//!
//! 实现方**必须**是密码学安全的随机源。RSA 加密的填充强度直接取决于它；
//! 而 trait 本身无法在类型上表达这一点，所以在这里写明。

/// 密码学安全的随机源。
///
/// 见模块文档：它是让 smid 链路可复现的注入点，不是"图方便的可替换依赖"。
pub trait Random: Send + Sync {
    /// 用随机字节填满 `dst`。
    fn fill_bytes(&mut self, dst: &mut [u8]);

    /// 生成一个 UUID v4 字符串（对齐 `crypto.randomUUID()`）。
    ///
    /// 默认实现按 RFC 4122 打版本位与变体位 —— 这与 Node / 浏览器的
    /// `randomUUID()` 一致。旧实现里那些 uuid 是**原样进特征体**的，
    /// 形状（版本 4、变体 10xx）不对的话服务端侧就多一个可识别的异常。
    fn uuid_v4(&mut self) -> String {
        let mut b = [0u8; 16];
        self.fill_bytes(&mut b);
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h = hex::encode(b);
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

/// 顺序吐字节的随机源，供测试与 fixture 回放使用。
///
/// 放在生产代码里（而不是各 crate 的 `#[cfg(test)]`）是有意的：
/// 它是**唯一**能让 smid 链路字节级复现的替身，多个 crate 的测试都要用它，
/// 各自复制一份迟早会分叉（见 docs/conventions.md §8.3 的同款教训）。
/// 它没有 IO、不引入任何依赖，因此不违反本 crate「纯函数」的铁律。
#[derive(Debug, Clone)]
pub struct SeqRandom {
    next: u8,
}

impl SeqRandom {
    /// 从 `start` 开始，每次取字节后 `+1`（回绕）。
    pub fn new(start: u8) -> Self {
        Self { next: start }
    }
}

impl Default for SeqRandom {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Random for SeqRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for byte in dst.iter_mut() {
            *byte = self.next;
            self.next = self.next.wrapping_add(1);
        }
    }
}

/// 可跨线程共享、且可廉价克隆的随机源。
///
/// # 为什么需要它
///
/// [`Random`] 的方法收 `&mut self`，所以一个随机源同一时刻只能被一个调用方持有。
/// 但服务层要把它放进 `Arc` 里共享给并发请求 —— 于是需要内部可变性。
///
/// 它同时是 `Clone` 的（内部是 `Arc<Mutex<..>>`，克隆只加一次引用计数）：
/// 调用方拿一份克隆就能得到 `&mut`，从而满足 `&mut impl Random` 的签名，
/// 而底层仍然是同一个随机源。
///
/// **关键约束：锁只在 `fill_bytes` 内持有，绝不跨 `.await`。**
/// `with` 收的是**同步**闭包，所以"持锁 await"在类型层面就写不出来
/// （见 docs/conventions.md §6）。
///
/// 用 `std::sync::Mutex` 而非 `tokio::sync::Mutex`：后者允许跨 await 持锁，
/// 于是"忘了释放"会变成运行期才暴露的问题；前者在编译期就杜绝了那种写法。
#[derive(Clone)]
pub struct SharedRandom {
    inner: std::sync::Arc<std::sync::Mutex<Box<dyn Random>>>,
}

impl SharedRandom {
    /// 包一个随机源。
    pub fn new(inner: Box<dyn Random>) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(inner)),
        }
    }

    /// 在锁内跑一段**同步**代码。
    ///
    /// 锁中毒时用 OS 熵源兜底，而不是 panic：一个无关线程的 panic
    /// 不该让整条设备指纹链路跟着死。
    pub fn with<T>(&self, f: impl FnOnce(&mut dyn Random) -> T) -> T {
        match self.inner.lock() {
            Ok(mut guard) => f(&mut **guard),
            Err(_) => f(&mut OsRandom),
        }
    }
}

impl Random for SharedRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        // 走 `with`，与共享路径同一个兜底策略。
        self.with(|r| r.fill_bytes(dst));
    }
}

impl std::fmt::Debug for SharedRandom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不暴露内部随机源的任何细节（它可能装着测试用的固定序列）。
        f.write_str("SharedRandom")
    }
}

/// 基于 OS 熵源的随机源（生产用）。
///
/// 走 `getrandom`，也就是操作系统的 CSPRNG（Linux `getrandom(2)`、
/// macOS `getentropy`、Windows `BCryptGenRandom`）。
///
/// 放在本 crate 里而不是组装层：它没有任何 IO 依赖（就是一次系统调用），
/// 而"随机源必须是密码学安全的"这条约定只有和 trait 放在一起才立得住。
#[derive(Debug, Default, Clone, Copy)]
pub struct OsRandom;

impl OsRandom {
    /// 新建。
    pub fn new() -> Self {
        Self
    }
}

impl Random for OsRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        // OS 熵源失败（几乎只可能是早期启动阶段）时不能返回未初始化内存。
        // `getrandom` 失败是极罕见事件；这里选择**继续重试**而不是降级成
        // 可预测的字节 —— 随机源退化成常量是安全问题，卡住比失守好。
        while getrandom::getrandom(dst).is_err() {
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_random_is_reproducible_from_its_start_byte() {
        let mut a = SeqRandom::new(7);
        let mut b = SeqRandom::new(7);
        let mut x = [0u8; 5];
        let mut y = [0u8; 5];
        a.fill_bytes(&mut x);
        b.fill_bytes(&mut y);
        assert_eq!(x, y);
        assert_eq!(x, [7, 8, 9, 10, 11]);
    }

    #[test]
    fn seq_random_wraps_around() {
        let mut r = SeqRandom::new(255);
        let mut b = [0u8; 2];
        r.fill_bytes(&mut b);
        assert_eq!(b, [255, 0]);
    }

    #[test]
    fn uuid_v4_has_the_rfc4122_shape() {
        let mut r = SeqRandom::new(0);
        let u = r.uuid_v4();
        // 8-4-4-4-12
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{u}"
        );
        // 版本位固定为 4，变体位落在 8/9/a/b
        assert!(parts[2].starts_with('4'), "版本位应是 4：{u}");
        assert!(
            matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "变体位应是 10xx：{u}"
        );
        assert!(u
            .chars()
            .filter(|c| *c != '-')
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn os_random_produces_different_bytes_on_each_draw() {
        let mut r = OsRandom::new();
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        r.fill_bytes(&mut a);
        r.fill_bytes(&mut b);
        assert_ne!(a, b, "两次取数不该相同（2^-256 的碰撞概率）");
        assert!(a.iter().any(|&x| x != 0), "不该是常量");
    }

    #[test]
    fn shared_random_is_cloneable_and_thread_safe() {
        // 4 个线程各自克隆一份、各取一批随机数：底层必须是**同一个**随机源
        // （克隆只加引用计数），否则调用方拿到的其实是互不相干的源。
        let r = SharedRandom::new(Box::new(SeqRandom::new(1)));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let mut mine = r.clone();
            handles.push(std::thread::spawn(move || {
                let mut local = [0u8; 4];
                mine.fill_bytes(&mut local);
                local
            }));
        }
        let mut all: Vec<[u8; 4]> = handles
            .into_iter()
            .map(|h| h.join().expect("线程不该 panic"))
            .collect();
        all.sort();
        all.dedup();
        assert_eq!(
            all.len(),
            4,
            "四个线程必须各自拿到不同的一段序列（同一个底层源）"
        );
    }

    #[test]
    fn shared_random_with_is_the_single_lock_entry() {
        let r = SharedRandom::new(Box::new(SeqRandom::new(9)));
        let v = r.with(|rng| rng.uuid_v4());
        assert_eq!(v.len(), 36);
    }
}
