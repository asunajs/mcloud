// 纯密码学原语 —— 与 TS 原版实现**逐字节**对齐。
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/crypto.rs。
//
// # 为什么在 core 而不是 io 层
//
// 协议层必须能用签名头（MD5）、设备指纹（DES/RSA），但它**不允许依赖 io**
// （否则 HTTP 客户端会被拖进依赖树，"对单个函数做无网测试"就不再可能）。
// 密码学是**无 IO 的纯计算**，所以归属 core。
//
// # 对拍基准
//
// 本模块的全部行为由 test/golden/*.json 锁定，那些文件由
// scripts/gen-*.cjs **直接调用 TS 原版所用的 crypto-js + node:crypto** 产出。
// 也就是说：**预期的密文不是手算的，是原实现真跑出来的**。
//
// # 三个"想当然会错"的坑（已实测，勿改）
//
//  1. **RSA PKCS#1 v1.5 加密是随机的**（Type 02 填充含随机非零字节），
//     同一明文加密三次结果全不同 → 只能做"解回明文"的单向验证，不能字节级对拍。
//  2. **gzip 的 OS 字节 = 19 是归一化而非魔改**：Node 的 CompressionStream("gzip")
//     产出的 OS 字节本来就是 19；写它是为了抹平各运行时差异。
//     Go 的 compress/gzip 默认是 255，**必须显式改**。
//  3. **encryptDes 的补零按 JS 字符串长度（UTF-16 码元数）算**，不是按字节数。
//     纯 ASCII 下两者等价；含 CJK 时会算出非 8 倍数字节。本模块**保持 ASCII 行为**，
//     不"顺手修好"，否则生成的 smid 与线上不一致。
package core

import (
	"bytes"
	"compress/gzip"
	"crypto/aes"
	"crypto/cipher"
	"crypto/des"
	"crypto/hmac"
	"crypto/md5"
	"crypto/rsa"
	"crypto/sha1"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"
)

// ─────────────────────────────────────────────────────────────────────────────
// 常量（与 TS 源码逐字对齐）
// ─────────────────────────────────────────────────────────────────────────────

// SignSaltPair 是签名盐表的一行。
type SignSaltPair struct {
	Key  uint8
	Salt string
}

// SignSaltMap 对应 core/src/auth/sign.ts 的 SALT_MAP。签名盐值只有两个，用 u8 索引。
//
// ⚠ 顺序有意义：[SignSalt] 的"未知 key 回落到第一个"依赖它。
var SignSaltMap = []SignSaltPair{
	{1, "seedMdYYLIZfbCxg"},
	{2, "sekaMdYYLIZfbCfm"},
}

// RefreshTokenKey 是 core/src/utils/encrypt.ts 的默认密钥。
//
// **24 字节 → AES-192**（不是 AES-256，这个坑很常见）。
const RefreshTokenKey = "2olBaQGYnEKoStYomsd1n7ax"

// AESFixedIV 是 runtime/src/crypto.ts: aesEncrypt 的固定 IV。
var AESFixedIV = [16]byte{
	'0', '1', '0', '2', '0', '3', '0', '4',
	'0', '5', '0', '6', '0', '7', '0', '8',
}

// SklandPublicKeyB64 是 shared/src/smid/index.ts 的 skland 公钥（SPKI DER 的 base64，1024 位）。
const SklandPublicKeyB64 = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQC8KHAcHbkCn5rxGgGJE+07tY+pt86D/oZ7sA51FaEBv2jgno2TI9zHJVYKJynmiKpixgwUcv93EfWIrU/p/UCs5Vu+odS3I4UBp3R7IZ1A0W01FkumAHYW2PQpMm8ueQKPLUq/idkpG/9b2JDv/qU+Ks36nbUPwlW4CjdfrV+V9QIDAQAB"

const (
	// gzipOSIndex 是 gzip 头里 OS 字段的下标。
	gzipOSIndex = 9
	// gzipOSValue 是 gzip 头里 OS 字段必须被改成的值（浏览器/Node 的实际取值）。
	gzipOSValue = 19
)

// ─────────────────────────────────────────────────────────────────────────────
// 随机源 —— 让"随机"可注入，从而整条链路可复现
// ─────────────────────────────────────────────────────────────────────────────

// Random 是密码学安全的随机源，也是 **core 内唯一允许的随机入口**。
//
// 理由：设备指纹（smid）构造混用了 randomUUID 与 Math.random，若直接取全局
// 随机数，则 fixture 回放与黄金向量对拍**无法复现**（每次请求体都不同）。
// 收敛到一个接口后，测试注入固定序列即可让全链路字节级稳定。
//
// 生产实现（基于 OS CSPRNG）放在 internal/io —— 本层不许有 IO。
type Random interface {
	// FillBytes 用随机字节填满 dst。
	FillBytes(dst []byte)
}

// UUIDv4 生成一个 UUID v4 字符串（对齐 rt.crypto.randomUUID()）。
func UUIDv4(r Random) string {
	var b [16]byte
	r.FillBytes(b[:])
	// 版本 4 + variant RFC 4122，与 node:crypto randomUUID 的语义一致
	b[6] = (b[6] & 0x0f) | 0x40
	b[8] = (b[8] & 0x3f) | 0x80
	h := hex.EncodeToString(b[:])
	return h[0:8] + "-" + h[8:12] + "-" + h[12:16] + "-" + h[16:20] + "-" + h[20:32]
}

// Below 取 [0, n) 内的随机数，对齐 JS 的 Math.floor(Math.random() * n)。
//
// 用**拒绝采样**避免取模偏差 —— 不要简化成 `v % n`：那会让小的余数
// 出现得比大的多，而这里产出的值会进设备指纹。
func Below(r Random, n uint32) uint32 {
	if n == 0 {
		panic("core.Below(0) 无意义")
	}
	const maxU32 = math.MaxUint32
	limit := maxU32 - (maxU32 % n) - 1
	for {
		var b [4]byte
		r.FillBytes(b[:])
		v := uint32(b[0])<<24 | uint32(b[1])<<16 | uint32(b[2])<<8 | uint32(b[3])
		if v <= limit {
			return v % n
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. 哈希 / MAC
// ─────────────────────────────────────────────────────────────────────────────

// MD5Hex 对齐 runtime/src/crypto.ts: md5（小写 hex）。
func MD5Hex(data []byte) string {
	sum := md5.Sum(data)
	return hex.EncodeToString(sum[:])
}

// SHA256Hex 对齐 runtime/src/crypto.ts: sha256（小写 hex）。
func SHA256Hex(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

// SHA1HexUpper 返回**大写** hex。
//
// 对齐 PC 客户端 common/utils.js: encryptByHash：
// CryptoJS.SHA1(HASHTEXT + password).toString(Hex).toUpperCase()。
// 大小写不是风格问题 —— 它直接进入请求体 dycpwd，服务端按十六进制解析。
func SHA1HexUpper(data []byte) string {
	sum := sha1.Sum(data)
	return strings.ToUpper(hex.EncodeToString(sum[:]))
}

// HmacSHA256Base64 输出 **base64**（不是 hex！对齐 shared/src/push 的钉钉加签）。
func HmacSHA256Base64(key, message []byte) string {
	mac := hmac.New(sha256.New, key)
	mac.Write(message)
	return base64.StdEncoding.EncodeToString(mac.Sum(nil))
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. 编码助手
// ─────────────────────────────────────────────────────────────────────────────

// B64Encode 是标准 base64 编码（带 = 填充）。
//
// 对外公开是因为**协议层需要它拼 URL**：App 端的滑块验证码 H5 页面把整组请求头
// 塞进 `?data=<base64(JSON)>`，那是纯计算，不该为此让协议层多背一个 base64 依赖。
func B64Encode(data []byte) string {
	return base64.StdEncoding.EncodeToString(data)
}

// B64Decode 是 [B64Encode] 的逆运算。
func B64Decode(s string) ([]byte, error) {
	out, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("base64 解码失败: %v", err))
	}
	return out, nil
}

// jsPadKey16 复刻 key.substring(0, 16).padEnd(16, "\0")。
//
// ⚠ JS 的 substring 按 UTF-16 码元计数。本项目**所有密钥都是 ASCII**，
// 因此按字节截断/补零等价；非 ASCII 密钥会与 JS 不一致
// （原实现从未使用，仅记录 —— 不要"顺手修好"）。
func jsPadKey16(key string) [16]byte {
	var out [16]byte
	copy(out[:], key) // copy 只搬 min(16, len(key))，其余保持零值
	return out
}

// jsPadKey8 复刻 key.substring(0, 8).padEnd(8, "\0")（ASCII 口令场景）。
func jsPadKey8(key string) [8]byte {
	var out [8]byte
	copy(out[:], key)
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. PKCS7 补位（标准库没有，且**必须自己校验**）
// ─────────────────────────────────────────────────────────────────────────────

// pkcs7Pad 按 RFC 5652 补位：**总是**补 1..blockSize 个字节（长度是整数倍时补一整块）。
func pkcs7Pad(data []byte, blockSize int) []byte {
	n := blockSize - len(data)%blockSize
	out := make([]byte, len(data)+n)
	copy(out, data)
	for i := len(data); i < len(out); i++ {
		out[i] = byte(n)
	}
	return out
}

// pkcs7Unpad 去补位，**并校验**。
//
// ⚠ 为什么要自己判：Rust 的 `decrypt_padded_mut::<Pkcs7>` 会拒绝非法填充，
// Go 标准库没有等价物（`cipher` 只提供裸分组/模式）。不判的后果不是"少一个检查"，
// 而是**用错密钥时静默产出半个凭据**，而不是报错 —— 这类失败最难查。
func pkcs7Unpad(data []byte, blockSize int) ([]byte, error) {
	if len(data) == 0 || len(data)%blockSize != 0 {
		return nil, NewParse("AES 明文填充非法")
	}
	pad := int(data[len(data)-1])
	if pad == 0 || pad > blockSize || pad > len(data) {
		return nil, NewParse("AES 明文填充非法")
	}
	for _, b := range data[len(data)-pad:] {
		if int(b) != pad {
			return nil, NewParse("AES 明文填充非法")
		}
	}
	return data[:len(data)-pad], nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. AES-CBC/PKCS7（固定 IV）—— runtime/src/crypto.ts: aesEncrypt
// ─────────────────────────────────────────────────────────────────────────────

func aesCBCPKCS7Encrypt(key, iv, data []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("AES key/iv 不合法: %v", err))
	}
	padded := pkcs7Pad(data, block.BlockSize())
	out := make([]byte, len(padded))
	cipher.NewCBCEncrypter(block, iv).CryptBlocks(out, padded)
	return out, nil
}

func aesCBCPKCS7Decrypt(key, iv, data []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("AES key/iv 不合法: %v", err))
	}
	bs := block.BlockSize()
	if len(data) == 0 || len(data)%bs != 0 {
		return nil, NewParse("AES 明文填充非法")
	}
	pt := make([]byte, len(data))
	cipher.NewCBCDecrypter(block, iv).CryptBlocks(pt, data)
	return pkcs7Unpad(pt, bs)
}

// AESCBCFixedIVEncryptHex 是 AES-CBC/PKCS7，密钥截断/补零到 16，固定 IV，输出 **hex**。
//
// 对齐 runtime/src/crypto.ts: aesEncrypt。
func AESCBCFixedIVEncryptHex(key, message string) (string, error) {
	k := jsPadKey16(key)
	ct, err := aesCBCPKCS7Encrypt(k[:], AESFixedIV[:], []byte(message))
	if err != nil {
		return "", err
	}
	return hex.EncodeToString(ct), nil
}

// AESCBCFixedIVDecryptHex 是固定 IV 那条路的**反向**：hex 密文 → UTF-8 明文。
//
// **仅测试与调试台使用**（与 [RSAPKCS1v15Decrypt] 同理：生产链路不需要解）。
// 存在的理由很具体：上报出去的 smid 载荷是单向加密的，光有加密函数就永远
// 证明不了"发出去的那一份里装的确实是绑定给这个账号的特征"。
func AESCBCFixedIVDecryptHex(key, cipherHex string) (string, error) {
	k := jsPadKey16(key)
	ct, err := hex.DecodeString(cipherHex)
	if err != nil {
		return "", NewParse(fmt.Sprintf("AES 密文不是合法 hex: %v", err))
	}
	pt, err := aesCBCPKCS7Decrypt(k[:], AESFixedIV[:], ct)
	if err != nil {
		return "", err
	}
	if !utf8.Valid(pt) {
		return "", NewParse("AES 明文不是 UTF-8")
	}
	return string(pt), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. AES-ECB/PKCS7 —— PC 客户端 common/utils.js / App 端 MCLoginCryptoUtil
// ─────────────────────────────────────────────────────────────────────────────

func aesECBPKCS7Encrypt(key, data []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("AES 密钥不合法: %v", err))
	}
	bs := block.BlockSize()
	padded := pkcs7Pad(data, bs)
	out := make([]byte, len(padded))
	// 裸分组密码逐块加密 = ECB（块间无链接）。显式写出"每块独立"，
	// 而不是靠某个 helper 的默认行为 —— ECB 就是这件事本身。
	for i := 0; i < len(padded); i += bs {
		block.Encrypt(out[i:i+bs], padded[i:i+bs])
	}
	return out, nil
}

func aesECBPKCS7Decrypt(key, data []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("AES 密钥不合法: %v", err))
	}
	bs := block.BlockSize()
	if len(data) == 0 || len(data)%bs != 0 {
		return nil, NewParse(fmt.Sprintf("AES-ECB 密文长度非法: %d", len(data)))
	}
	out := make([]byte, len(data))
	for i := 0; i < len(data); i += bs {
		block.Decrypt(out[i:i+bs], data[i:i+bs])
	}
	return pkcs7Unpad(out, bs)
}

// AESECBPKCS7DecryptHex 是 AES-ECB/PKCS7 解密，密文是 **hex**（不是 base64），返回 UTF-8 明文。
//
// 对齐 PC 客户端 common/utils.js: decryptByAes(input, key)：
//
//	const keyWordArray = CryptoJS.enc.Utf8.parse(key);          // 16 字节 ASCII → AES-128
//	const encryptedWordArray = CryptoJS.enc.Hex.parse(input);   // 输入是 hex
//	CryptoJS.AES.decrypt(..., { mode: ECB, padding: Pkcs7 })
//
// 两个容易搞错的点：
//   - **输入是 hex 而不是 base64**：同一个文件里的 encryptByAes 输出 base64，
//     两者不是一对逆运算，照「加解密对称」的直觉写就会解不开；
//   - 密钥原样使用（不做截断/补零），与本模块 CBC 系列的处理方式不同。
func AESECBPKCS7DecryptHex(key, cipherHex string) (string, error) {
	ct, err := hex.DecodeString(strings.TrimSpace(cipherHex))
	if err != nil {
		return "", NewParse(fmt.Sprintf("密文不是合法 hex: %v", err))
	}
	pt, err := aesECBPKCS7Decrypt([]byte(key), ct)
	if err != nil {
		return "", err
	}
	if !utf8.Valid(pt) {
		return "", NewParse("AES 明文不是合法 UTF-8")
	}
	return string(pt), nil
}

// AESECBPKCS7EncryptHex 是 AES-ECB/PKCS7 加密，输出 **hex** —— [AESECBPKCS7DecryptHex] 的逆运算。
//
// 与解密不同，这个方向在 PC 客户端里不存在（PC 只解密响应）；
// 它是 **App 端**才有的：MCLoginCryptoUtil 走
// CCCrypt(op=kCCEncrypt, alg=kCCAlgorithmAES,
// options=kCCOptionPKCS7Padding|kCCOptionECBMode, keyLength=16)，再 hexStringFromData:。
//
// 密钥**原样使用**（ASCII 16 字节 → AES-128），不做截断/补零。
func AESECBPKCS7EncryptHex(key, plaintext string) (string, error) {
	ct, err := aesECBPKCS7Encrypt([]byte(key), []byte(plaintext))
	if err != nil {
		return "", err
	}
	return hex.EncodeToString(ct), nil
}

// ShiftTail10ToFront 把 App 端登录体的**末 10 位移到最前**（MCLoginCryptoUtil +privateEncrypt:）。
//
// 反汇编实参（-[MCLoginCryptoUtil privateEncrypt:isCurrentEnvironment:]）：
//
//	p    = MAX(len, 10) - 10;                 // MAX 是编译出的 csel
//	tail = [e substringFromIndex:p];          // 末 10 个字符
//	head = [e substringToIndex:p];            // 其余
//	out  = [NSString stringWithFormat:@"%@%@", tail, head];
//
// 长度不足 10 时 p == 0，两段拼回来等于原串 —— **恒等变换**，不是错误路径。
// 所以本函数对短串原样返回，不报错（照抄客户端行为，别"顺手校验长度"）。
func ShiftTail10ToFront(s string) string {
	rs := []rune(s)
	if len(rs) <= 10 {
		return s
	}
	cut := len(rs) - 10
	return string(rs[cut:]) + string(rs[:cut])
}

// ShiftHead10ToBack 是 [ShiftTail10ToFront] 的逆运算（MCLoginCryptoUtil +privateDecrypt:）。
//
// 客户端在 len < 11 时返回空串；长度 ≤ 10 时本函数返回原串 ——
// 对长度恰为 10 的输入两条路径同解，所以这里照抄"恒等变换"那支即可。
func ShiftHead10ToBack(s string) string {
	rs := []rune(s)
	if len(rs) <= 10 {
		return s
	}
	return string(rs[10:]) + string(rs[:10])
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. AES-CBC/PKCS7（动态 IV）—— core/src/utils/encrypt.ts
// ─────────────────────────────────────────────────────────────────────────────

// AESDynamicIVEncrypt 用指定 IV 加密，输出 Base64(IV ‖ CT)。
//
// 密钥**原样使用不截断**（[RefreshTokenKey] 24 字节 → AES-192）。
func AESDynamicIVEncrypt(key, plaintext string, iv [16]byte) (string, error) {
	ct, err := aesCBCPKCS7Encrypt([]byte(key), iv[:], []byte(plaintext))
	if err != nil {
		return "", err
	}
	out := make([]byte, 0, 16+len(ct))
	out = append(out, iv[:]...)
	out = append(out, ct...)
	return B64Encode(out), nil
}

// AESDynamicIVEncryptRandom 用随机 IV 加密（生产路径）。IV 从注入的 [Random] 取，因此测试可复现。
func AESDynamicIVEncryptRandom(key, plaintext string, r Random) (string, error) {
	var iv [16]byte
	r.FillBytes(iv[:])
	return AESDynamicIVEncrypt(key, plaintext, iv)
}

// AESDynamicIVDecrypt 解密 Base64(IV ‖ CT)，返回 UTF-8 明文。
func AESDynamicIVDecrypt(key, dataB64 string) (string, error) {
	raw, err := B64Decode(dataB64)
	if err != nil {
		return "", err
	}
	if len(raw) < 16 {
		return "", NewParse("AES 动态 IV 密文长度不足 16 字节")
	}
	pt, err := aesCBCPKCS7Decrypt([]byte(key), raw[:16], raw[16:])
	if err != nil {
		return "", err
	}
	if !utf8.Valid(pt) {
		return "", NewParse("AES 明文不是合法 UTF-8")
	}
	return string(pt), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. DES-ECB —— shared/src/smid/index.ts
// ─────────────────────────────────────────────────────────────────────────────

// desZeroPad 复刻 encryptDes 的补零：padLen = 8 - (len % 8)，**padLen === 8 时不补**。
//
// 等价于"补到 8 的倍数，已是倍数则不补"。空串保持空串（→ 空密文）。
func desZeroPad(message []byte) []byte {
	rem := len(message) % 8
	if rem == 0 {
		return message
	}
	out := make([]byte, len(message), len(message)+8-rem)
	copy(out, message)
	return append(out, make([]byte, 8-rem)...)
}

func desECBBlocks(key, data []byte, encrypt bool) ([]byte, error) {
	block, err := des.NewCipher(key)
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("DES 密钥非法: %v", err))
	}
	// ⚠ 与 Rust 的 chunks_exact(8) 一致：末尾不足 8 字节的残块**被丢弃**。
	out := make([]byte, 0, len(data))
	buf := make([]byte, 8)
	for i := 0; i+8 <= len(data); i += 8 {
		if encrypt {
			block.Encrypt(buf, data[i:i+8])
		} else {
			block.Decrypt(buf, data[i:i+8])
		}
		out = append(out, buf...)
	}
	return out, nil
}

// DESECBNoPaddingEncryptBase64 是 DES-ECB/**NoPadding** 加密，输出 base64。
//
// 对齐 shared/src/smid/index.ts: encryptDes。
// 空串 → 空 base64（不产出任何块），与原实现一致。
func DESECBNoPaddingEncryptBase64(key, message string) (string, error) {
	k := jsPadKey8(key)
	padded := desZeroPad([]byte(message))
	ct, err := desECBBlocks(k[:], padded, true)
	if err != nil {
		return "", err
	}
	return B64Encode(ct), nil
}

// DESECBPKCS7Decrypt 是 DES-ECB/**PKCS7** 解密，base64 进、UTF-8 出。
//
// 对齐 runtime/src/crypto.ts: desDecrypt。
func DESECBPKCS7Decrypt(key, cipherB64 string) (string, error) {
	k := jsPadKey8(key)
	ct, err := B64Decode(cipherB64)
	if err != nil {
		return "", err
	}
	if len(ct)%8 != 0 {
		return "", NewParse("DES 密文长度不是 8 的倍数")
	}
	pt, err := desECBBlocks(k[:], ct, false)
	if err != nil {
		return "", err
	}
	// 手工去 PKCS7（DES-ECB 没有现成 mode 包装，所以自己收尾）
	if len(pt) == 0 {
		return "", NewParse("DES 密文为空")
	}
	pad := int(pt[len(pt)-1])
	if pad == 0 || pad > 8 || pad > len(pt) {
		return "", NewParse("DES PKCS7 填充非法")
	}
	pt = pt[:len(pt)-pad]
	if !utf8.Valid(pt) {
		return "", NewParse("DES 明文不是合法 UTF-8")
	}
	return string(pt), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. RSA PKCS#1 v1.5 —— runtime/src/node.ts: rsaEncrypt
// ─────────────────────────────────────────────────────────────────────────────

// rsaRngAdapter 把 core 的 [Random] 适配成 rsa 需要的 io.Reader。
type rsaRngAdapter struct{ r Random }

func (a rsaRngAdapter) Read(p []byte) (int, error) {
	a.r.FillBytes(p)
	return len(p), nil
}

// RSAPKCS1v15EncryptBase64 是 RSA PKCS#1 v1.5 公钥加密，输出 base64。
//
// publicKeySPKIDER 是 **SPKI DER**（即原实现里 base64 解开后的字节）。
//
// ⚠ **输出不可对拍**：PKCS#1 v1.5 加密填充含随机非零字节，同一明文每次结果都不同。
// 正确性靠"用私钥解回明文"验证。
func RSAPKCS1v15EncryptBase64(publicKeySPKIDER, message []byte, r Random) (string, error) {
	parsed, err := x509.ParsePKIXPublicKey(publicKeySPKIDER)
	if err != nil {
		return "", NewInternal(fmt.Sprintf("RSA 公钥解析失败: %v", err))
	}
	pub, ok := parsed.(*rsa.PublicKey)
	if !ok {
		return "", NewInternal("RSA 公钥解析失败: 不是 RSA 公钥")
	}
	ct, err := rsa.EncryptPKCS1v15(rsaRngAdapter{r}, pub, message)
	if err != nil {
		return "", NewInternal(fmt.Sprintf("RSA 加密失败: %v", err))
	}
	return B64Encode(ct), nil
}

// RSAPKCS1v15EncryptBase64WithB64Key 是传入 base64 的 SPKI 公钥（如 [SklandPublicKeyB64]）的便捷版本。
func RSAPKCS1v15EncryptBase64WithB64Key(publicKeyB64 string, message []byte, r Random) (string, error) {
	der, err := B64Decode(publicKeyB64)
	if err != nil {
		return "", err
	}
	return RSAPKCS1v15EncryptBase64(der, message, r)
}

// RSAPKCS1v15Decrypt 是 RSA PKCS#1 v1.5 私钥解密（**仅测试与调试台使用**，生产链路拿不到私钥）。
//
// ⚠ 传 nil 作 random = **关闭盲化**。这在本函数的用途（验证密文里装的是什么）下
// 是可接受的，但它意味着**不要**把它接到任何处理真实凭据的路径上。
func RSAPKCS1v15Decrypt(privateKeyPKCS8DER []byte, cipherB64 string) ([]byte, error) {
	parsed, err := x509.ParsePKCS8PrivateKey(privateKeyPKCS8DER)
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("RSA 私钥解析失败: %v", err))
	}
	priv, ok := parsed.(*rsa.PrivateKey)
	if !ok {
		return nil, NewInternal("RSA 私钥解析失败: 不是 RSA 私钥")
	}
	ct, err := B64Decode(cipherB64)
	if err != nil {
		return nil, err
	}
	pt, err := rsa.DecryptPKCS1v15(nil, priv, ct)
	if err != nil {
		return nil, NewParse(fmt.Sprintf("RSA 解密失败: %v", err))
	}
	return pt, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. gzip（OS 字节归一为 19）
// ─────────────────────────────────────────────────────────────────────────────

// GzipWithOS19 是 gzip 压缩，并**强制把 OS 字节写成 19**。
//
// 对齐 shared/src/smid/index.ts: gzipCompressObject。该行是"归一化"而非魔改：
// Node 的 CompressionStream("gzip") 本来就产出 19，写这一行是为了抹平 txiki.js 的差异。
//
// ⚠ Go 这边不需要"事后改字节"（Rust 的 flate2 没暴露该字段，所以它改了 buf[9]）：
// compress/gzip 的 Header.OS 是公开的，默认 255，显式设成 19 即可。
// 两者语义相同，但**判据要一致**：向量要求的是"输出的第 10 个字节 == 19"。
func GzipWithOS19(data []byte) ([]byte, error) {
	var buf bytes.Buffer
	zw := gzip.NewWriter(&buf)
	zw.Header.OS = gzipOSValue
	if _, err := zw.Write(data); err != nil {
		return nil, NewInternal(fmt.Sprintf("gzip 写入失败: %v", err))
	}
	if err := zw.Close(); err != nil {
		return nil, NewInternal(fmt.Sprintf("gzip 收尾失败: %v", err))
	}
	return buf.Bytes(), nil
}

// GzipWithOS19Base64 是 gzip 压缩后 base64（smid 载荷用的就是这个）。
func GzipWithOS19Base64(data []byte) (string, error) {
	gz, err := GzipWithOS19(data)
	if err != nil {
		return "", err
	}
	return B64Encode(gz), nil
}

// Gunzip 是 gzip 解压。与 [GzipWithOS19] 配成往返，**仅测试与调试台使用**。
//
// 为什么要有它：smid 的 data 字段解出来是一串 gzip 的 base64，而"上报体里带的
// 字段值对不对"只能解开来看。没有它，那一整段的断言就只能停在
// "算出了一个 hex 串"—— 那等于没测。
func Gunzip(data []byte) ([]byte, error) {
	zr, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		return nil, NewParse(fmt.Sprintf("gzip 解压失败: %v", err))
	}
	defer func() { _ = zr.Close() }()
	out, err := io.ReadAll(zr)
	if err != nil {
		return nil, NewParse(fmt.Sprintf("gzip 解压失败: %v", err))
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 10. 请求签名头 —— core/src/auth/sign.ts
// ─────────────────────────────────────────────────────────────────────────────

// SignSalt 取签名盐值；未知 saltKey 回落到第一个（对齐原实现的 `|| SALT_MAP[1]`）。
func SignSalt(saltKey uint8) string {
	if saltKey == 2 {
		return SignSaltMap[1].Salt
	}
	return SignSaltMap[0].Salt
}

// GenerateSignature 计算 md5(salt + id + timestamp + nonce + other + salt)。
//
// ⚠ 字段**顺序固定**：顺序错了哈希必错，而"哈希对了"不足以说明顺序对。
func GenerateSignature(id, timestamp, nonce string, saltKey uint8, other string) string {
	salt := SignSalt(saltKey)
	var b strings.Builder
	b.Grow(len(salt)*2 + len(id) + len(timestamp) + len(nonce) + len(other))
	b.WriteString(salt)
	b.WriteString(id)
	b.WriteString(timestamp)
	b.WriteString(nonce)
	b.WriteString(other)
	b.WriteString(salt)
	return MD5Hex([]byte(b.String()))
}

// ─────────────────────────────────────────────────────────────────────────────
// 11. 设备指纹序列化 getTn —— shared/src/smid/index.ts
// ─────────────────────────────────────────────────────────────────────────────

// GetTn 把一个 JSON 对象按键名排序后，把值按原实现的规则拼成一串（用于 smid 的 tn）。
//
// 规则（逐条对齐 TS）：
//   - number 且**非整数** → Math.floor(v * 10000)
//   - number 且整数 → v * 10000
//   - object（非 null）→ 递归
//   - 其余（含 null）→ JS String(v)，注意 null → "null" 而非空串
//
// ⚠⚠ 调用方**必须**用 json.Decoder.UseNumber() 解码后再传进来。
// 用默认的 float64 解码会丢掉"整数/小数"这个区分 —— 而它决定要不要 floor。
// 见 TestGetTnNeedsUseNumberToKeepIntFloatDistinction。
//
// 已知边界（原实现如此，向量未覆盖，勿擅自"修好"）：
//   - 键排序：JS 按 UTF-16 码元，这里按 UTF-8 字节；非 BMP 字符下顺序可能不同；
//   - 极大/极小数字：JS 会输出 1e+21 这类指数形式，本实现不会；
//   - **数组整体贡献空串**（与 Rust 一致，不是遗漏 —— 见 tnFragment）。
func GetTn(value any) string {
	obj, ok := value.(map[string]any)
	if !ok {
		return ""
	}
	keys := make([]string, 0, len(obj))
	for k := range obj {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var b strings.Builder
	for _, k := range keys {
		b.WriteString(tnFragment(obj[k]))
	}
	return b.String()
}

// tnFragment 处理单个值，对应 Rust 里 `match &obj[k] { ... }` 的每个分支。
func tnFragment(v any) string {
	switch t := v.(type) {
	case json.Number:
		f, err := t.Float64()
		if err != nil {
			// 对应 Rust 的 `n.as_f64().unwrap_or(0.0)`
			f = 0
		}
		return formatTnNumber(f, isIntegralLiteral(t.String()))
	case float64:
		// ⚠ 走到这里说明调用方没开 UseNumber：整数/小数已经分不出来了。
		// 按"非整数"处理（floor），与 Rust 在 f64 上的行为一致。
		return formatTnNumber(t, false)
	case string:
		return t
	case bool:
		if t {
			return "true"
		}
		return "false"
	case nil:
		return "null"
	case map[string]any:
		return GetTn(t)
	case []any:
		// ⚠ 数组在这里贡献**空串** —— 递归进去时顶层不是 object，GetTn 直接返回 ""。
		// 这是照抄原实现的行为，不是"忘了处理"；改掉会让 tn 与线上不一致。
		return GetTn(t)
	}
	return ""
}

// formatTnNumber 对应 Rust 的 `format!("{scaled}")`。
//
// 用 'f' + 精度 -1 = 最短的、不带指数的十进制表示，与 Rust f64 的 Display 一致。
func formatTnNumber(f float64, integral bool) string {
	scaled := f * 10000
	if !integral {
		scaled = math.Floor(scaled)
	}
	return strconv.FormatFloat(scaled, 'f', -1, 64)
}

// isIntegralLiteral 判断 JSON 数字字面量是否是整数。
//
// 对应 serde_json 的 `n.is_i64() || n.is_u64()`：serde_json 把带 `.`/`e` 的
// 字面量一律解析成 f64，所以"字面量里有没有这些字符"就是那个判据。
func isIntegralLiteral(lit string) bool {
	return !strings.ContainsAny(lit, ".eE")
}

// GetTnMD5 是 smid 的 tn 字段：md5(getTn(obj))。
func GetTnMD5(value any) string {
	return MD5Hex([]byte(GetTn(value)))
}
