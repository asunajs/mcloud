package app

// AI 头像那条链的**入场券**：一张随机噪点 PNG（纯函数，不碰网络）。
//
// 对应旧实现的 `utils/png-generator.ts` + `tasks/ai-avatar/task.ts:38-40`
// （`docs/business-feature-inventory.md` §2.3 `ai-avatar`）。
//
// # 为什么这张图非得本地生成
//
// `POST /market/rabactive/ai/image/merge` 收的是**一张图片的 base64**，上游拿它去"合成"
// 一张 AI 头像。旧实现传进去的内容就是一片随机噪点 —— 也就是说
// **图上画的是什么，上游与用户都不在乎**：它要的只是"有一个合法的图片输入"。
// 所以这一族没有"挑一张好图"这件事，只有"造一张能过校验的图"。
//
// ⚠ "上游对图片内容有没有任何识别"没有样本（本仓零实拍）。这里按旧实现照做：
// 传噪点。要核它：拿一张真照片与一张噪点各跑一次 `merge`，比对 `status` 与产物。
//
// # 为什么自己写编码器而不用 `image/png`
//
// 标准库能编 PNG，但它编出来的**字节**由它的过滤器选择决定（同一份像素，
// 不同 Go 版本的 `image/png` 可能给出不同 IDAT）。本仓的纪律是"线格式进了门禁就不动"，
// 而这里要的恰恰是**可逐字节断言**的一张图：随机源一旦固定，整张 PNG 的字节必须固定，
// 否则"请求体与抓包一致"那类断言在这条链上永远做不出来。
// 自己写只有三段：`IHDR`(13 字节) + `IDAT`(zlib **stored** 块) + `IEND`。
// stored 块是 deflate 里唯一"原样复制输入"的模式，因此**没有压缩器行为可漂移**。
//
// 代价：一份 200×200 的图 base64 后约 160 KB（比 deflate 压缩大一个量级），
// 旧实现也是这样发的（`deflateSync` 名字虽叫 deflate，里面走的正是 stored）。
// ⚠ 这一格 body 大小**上游在乎不在乎**没有样本 ⇒ 不为了省流量换成真压缩：
// 换了就同时换了两个变量（字节形态与体积），将来对不上时无法归因。
//
// # 为什么输出是**裸 base64**、不带 `data:image/png;base64,`
//
// 旧生成器返回带前缀的 data URI，`task.ts:39` 立刻用一个正则把前缀去掉。
// 也就是"上游收到的从来没有前缀" ⇒ 这里直接产裸串。
// 带前缀发出去会怎样：零样本（可能只是解析失败，也可能被当成更长的 base64 收下）。

import (
	"encoding/base64"
	"encoding/binary"
	"hash/crc32"

	"github.com/catlair/mcloud/internal/core"
)

// 图片尺寸：旧实现调用点写死 `generateRandomNoisePNG(200, 200)`（`task.ts:39`），
// 而生成器自己的默认值是 300×300 —— **默认值从来没有被用上**。
// 这里只留被实际用过的那一份，避免"两个数都不知道哪个算数"。
const (
	aiAvatarNoiseWidth  = 200
	aiAvatarNoiseHeight = 200
)

// aiAvatarNoiseBase64 产一张随机噪点 PNG 的**裸 base64**（无 data URI 前缀）。
//
// `rng` 是唯一的随机入口（[core.Random]）：像素全走 `FillBytes`，
// 所以固定随机源 ⇒ 固定字节 ⇒ 这条链的请求体可以被逐字断言。
//
// 尺寸是常量而不是参数（见上面那两个数）：传 0 或负数在这里是一条永远不会红的路径，
// 与其为它写一个测不到的分支，不如不给它存在的机位。
func aiAvatarNoiseBase64(rng core.Random) string {
	return base64.StdEncoding.EncodeToString(aiAvatarNoisePNG(rng))
}

// aiAvatarNoisePNG 是那张图的字节：签名 + IHDR + IDAT + IEND。
//
// ⚠ 刻意**不**导出：外面只需要 base64 那一格。导出它等于给"直接发二进制图"留一个入口，
// 而 `merge` 的 body 只认 `imageBase64` 字符串。
func aiAvatarNoisePNG(rng core.Random) []byte {
	raw := make([]byte, aiAvatarNoiseHeight*(1+aiAvatarNoiseWidth*3))
	stride := 1 + aiAvatarNoiseWidth*3
	for row := 0; row < aiAvatarNoiseHeight; row++ {
		start := row * stride
		// 每行首字节是 PNG 行过滤器号，`0` = None（不预测、原样存）。
		// 只有取 0 时"IDAT 里就是像素本身"这件事才成立，stored 块也因此才有意义。
		raw[start] = 0
		rng.FillBytes(raw[start+1 : start+stride])
	}

	header := make([]byte, 13)
	binary.BigEndian.PutUint32(header[0:], aiAvatarNoiseWidth)
	binary.BigEndian.PutUint32(header[4:], aiAvatarNoiseHeight)
	header[8] = 8  // bit depth
	header[9] = 2  // color type: RGB（没有 alpha —— 有 alpha 会多一列，行跨度就不是 1+3w 了）
	header[10] = 0 // compression: deflate（PNG 只认这一种，与 IDAT 内部用 stored 不冲突）
	header[11] = 0 // filter method
	header[12] = 0 // interlace: 非隔行

	out := append([]byte{}, pngSignature...)
	out = append(out, pngChunk("IHDR", header)...)
	out = append(out, pngChunk("IDAT", zlibStored(raw))...)
	out = append(out, pngChunk("IEND", nil)...)
	return out
}

var pngSignature = []byte{137, 80, 78, 71, 13, 10, 26, 10}

// pngChunk 拼一个 PNG 数据块：长度(4BE) + 类型(4) + 数据 + CRC(4BE)。
//
// CRC 覆盖的是**类型 + 数据**，不含长度 —— 这是 PNG 规范里最容易记错的一格，
// 也是"图看起来合法但解不开"的经典来源（[crc32.ChecksumIEEE] 用的多项式正是 PNG 要的
// IEEE 反射式，与 zlib 同源）。
func pngChunk(kind string, data []byte) []byte {
	chunk := make([]byte, 4+4+len(data)+4)
	binary.BigEndian.PutUint32(chunk[0:], uint32(len(data)))
	copy(chunk[4:], kind)
	copy(chunk[8:], data)
	sum := crc32.ChecksumIEEE(chunk[4 : 8+len(data)])
	binary.BigEndian.PutUint32(chunk[8+len(data):], sum)
	return chunk
}

// zlibStored 把数据编成一个**只含 stored 块**的 zlib 流。
//
// 三处细节，每一处错了都会得到"zlib 工具说不是 zlib"的产物：
//
//   - `CMF=0x78`、`FLG=0x01`：这两字节的乘积校验（`(CMF*256+FLG) % 31 == 0`）必须过，
//     `0x01` 恰好让 `0x7801 = 30721 = 31×991`。改 FLG 之前先重算这一格。
//   - stored 块头：`BFINAL(1bit) + BTYPE=00(2bit)` 在最左，随后 **LEN 与 NLEN 都是小端**
//     （与 PNG 其余部分的"一律大端"相反，这是 deflate 的历史遗留，写反了表现为
//     "解出来是一堆乱码而不是报错"）。NLEN 是 LEN 按位取反。
//   - 块大小上限 65535：200×200 那份是 120200 字节 ⇒ 必然两块，
//     所以"只写一块"的简化版**测不出来**，这里按多块写。
//
// ADLER32 覆盖的是**未压缩**的那份数据（PNG 解码器拿它校验像素，不是校验 deflate 流）。
//
// ⚠ 输入为空时这里**不产任何块**（于是 `BFINAL` 也没置位）⇒ 那不是合法 zlib 流。
// 本包只在固定尺寸的噪点图上用它（120200 字节，恒非空），所以不为一格走不到的路径
// 加一个永远不会红的分支。**以后若要拿它编别的图，先补空输入那一档。**
func zlibStored(data []byte) []byte {
	const maxStoredBlock = 65535
	out := make([]byte, 0, 2+len(data)+5*(len(data)/maxStoredBlock+1)+4)
	out = append(out, 0x78, 0x01)
	for offset := 0; offset < len(data); offset += maxStoredBlock {
		end := offset + maxStoredBlock
		if end > len(data) {
			end = len(data)
		}
		block := data[offset:end]
		header := []byte{0, 0, 0, 0, 0}
		if end == len(data) {
			header[0] = 1 // BFINAL=1（只有最后一块置位）
		}
		binary.LittleEndian.PutUint16(header[1:], uint16(len(block)))
		binary.LittleEndian.PutUint16(header[3:], ^uint16(len(block)))
		out = append(out, header...)
		out = append(out, block...)
	}
	adler := make([]byte, 4)
	binary.BigEndian.PutUint32(adler, adler32(data))
	return append(out, adler...)
}

// adler32 是 zlib 尾部那四个字节的算法本体（模 65521，`a` 从 1 起）。
//
// 为什么不用标准库：Go 把 `adler32` 藏在 `compress/flate` 内部（`internal/byteorder`
// 时代之后没有对外包），自己写是 6 行 —— 而它错了的表现是"PNG 校验器拒绝这张图"，
// 所以它必须在本文件里看得见、测得了。
func adler32(data []byte) uint32 {
	const modulus = 65521
	var a, b uint32 = 1, 0
	for _, value := range data {
		a = (a + uint32(value)) % modulus
		b = (b + a) % modulus
	}
	return b<<16 | a
}
