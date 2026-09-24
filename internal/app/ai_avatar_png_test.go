package app

// 随机噪点 PNG 生成器的用例：这张图是 AI 头像那条链的**入场券**，
// 它坏了的表现不是"报错"，而是"上游回一句看不懂的原因"。
//
// # 这一族断言的口径
//
// 我们**没有**任何一张 `merge` 的实拍（`docs/capture/` 里 rabactive 一条都没有），
// 所以这里不能断言"上游收这张图"。能断言的是**"这是一张合法的 200×200 RGB PNG，
// 且像素内容与随机源严格对应"** —— 也就是"如果上游肯收一张 PNG，收的一定是这种"。
//
// # 为什么用标准库**解码**回验，而不是逐字节比对一张基线图
//
// 基线图会把"我们自己的编码器写错了"和"标准库换了"两件事混成一条红灯。
// 这里让 `image/png` 与 `compress/zlib` 当**独立第三方**：
//   - `png.Decode` 会校验每个数据块的 CRC（覆盖"类型 + 数据"）与 zlib 流尾的 ADLER32，
//     所以"CRC 算错了范围"这类错法在这里就红，不需要我把实现抄一遍到测试里；
//   - 解出来的**像素**与独立重算的期望逐字节比对，抓的是"行过滤器字节占了随机源一个格子"
//     这一类 off-by-one（把 `rng.FillBytes(raw[start+1:…])` 改成 `raw[start:…]` ⇒ 本条变红）。
//
// ⚠ 反过来**不**断言的两件事，都写在实现的文件头：上游认不认这张图的内容、
// 161 KB 的 body 会不会被拒。那两格要的是实拍，不是这里再编一层断言。

import (
	"bytes"
	"compress/zlib"
	"encoding/base64"
	stdadler "hash/adler32"
	"hash/crc32"
	"image/png"
	"io"
	"strings"
	"testing"
)

// countingRandom 是"第 n 个字节等于 n mod 256"的随机源。
//
// 用它而不是仓库里现成的 [seqRng]：测试需要**独立重算**出每一个像素字节，
// 而"按调用顺序连续递增"让重算变成一行算术。⚠ 它同时把"实现有没有把行首
// 过滤器那一个字节也算进随机流"变成可判的 —— 差一格，后面所有像素全错位。
type countingRandom struct{ n uint64 }

func (r *countingRandom) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = byte(r.n)
		r.n++
	}
}

const (
	// noiseStride 是 PNG 未压缩数据里一行的字节数：1 字节过滤器 + 200 像素 ×3 通道。
	noiseStride = 1 + aiAvatarNoiseWidth*3
	// noiseRawLen / noisePNGLen / noiseBase64Len 三格的算式写在 [TestNoiseBase64IsBare] 里，
	// 它们是"体积"这一格的现状基准 —— 谁改压缩方式都会在这里被叫住问一句。
	noiseRawLen     = aiAvatarNoiseHeight * noiseStride
	noisePNGLen     = 120273
	noiseBase64Len  = 160364
	noiseStoredLen  = 120216
	noiseMaxBlock   = 65535
	noiseBlockCount = 2
)

// expectedNoiseRows 独立重算"IDAT 解压之后应该是什么"。
func expectedNoiseRows(t *testing.T) []byte {
	t.Helper()
	if noiseRawLen != 120200 {
		t.Fatalf("夹具本身先算错了：200×200 RGB 每行带 1 字节过滤器应是 120200 字节，算得 %d", noiseRawLen)
	}
	raw := make([]byte, noiseRawLen)
	var n uint64
	for row := 0; row < aiAvatarNoiseHeight; row++ {
		start := row * noiseStride
		for i := start + 1; i < start+noiseStride; i++ {
			raw[i] = byte(n)
			n++
		}
	}
	return raw
}

// ─────────────────────────────────────────────────────────────────────────────
// 自己走一遍块表（只用来定位"哪一块的哪一格"，合法性本身交给标准库）
// ─────────────────────────────────────────────────────────────────────────────

type noiseChunk struct {
	kind string
	data []byte
}

func walkNoisePNG(t *testing.T, data []byte) []noiseChunk {
	t.Helper()
	if !bytes.HasPrefix(data, pngSignature) {
		t.Fatalf("PNG 签名不对，前 8 字节 %v", data[:min(8, len(data))])
	}
	var chunks []noiseChunk
	for offset := 8; offset+12 <= len(data); {
		length := int(be32(data[offset : offset+4]))
		if offset+8+length+4 > len(data) {
			t.Fatalf("块 %s 声明长度 %d 越界（其后只剩 %d 字节）—— 长度字段是**大端**，写反了会报这一条",
				string(data[offset+4:offset+8]), length, len(data)-(offset+8))
		}
		kind := string(data[offset+4 : offset+8])
		body := data[offset+8 : offset+8+length]
		// CRC 覆盖**类型 + 数据**、不含长度。这是 PNG 最容易记错的一格，
		// 而 `png.Decode` 同样会抓到 —— 这里单独断言一次，只为错误消息直指这一格。
		want := crc32.ChecksumIEEE(data[offset+4 : offset+8+length])
		if got := be32(data[offset+8+length : offset+12+length]); got != want {
			t.Fatalf("块 %s 的 CRC 没有覆盖「类型+数据」：算得 %08x，实得 %08x", kind, want, got)
		}
		chunks = append(chunks, noiseChunk{kind: kind, data: append([]byte{}, body...)})
		offset += 12 + length
	}
	return chunks
}

func kindsOf(chunks []noiseChunk) string {
	kinds := make([]string, 0, len(chunks))
	for _, chunk := range chunks {
		kinds = append(kinds, chunk.kind)
	}
	return strings.Join(kinds, ",")
}

func be32(data []byte) uint32 {
	return uint32(data[0])<<24 | uint32(data[1])<<16 | uint32(data[2])<<8 | uint32(data[3])
}

// ─────────────────────────────────────────────────────────────────────────────
// 合法性：由第三方说了算
// ─────────────────────────────────────────────────────────────────────────────

// TestNoisePNGIsDecodableByStdlib ⭐
//
// 反向改坏点（每一条都会让本条变红）：
//   - `header[9]` 从 2 改成 6（带 alpha）⇒ 每行多一列，像素比对失败；
//   - 行过滤器从 0 改成 1（Sub）⇒ 解出来的像素与期望错位；
//   - [pngChunk] 的 CRC 范围去掉类型 ⇒ `png.Decode` 报 invalid checksum。
func TestNoisePNGIsDecodableByStdlib(t *testing.T) {
	data := aiAvatarNoisePNG(&countingRandom{})
	img, err := png.Decode(bytes.NewReader(data))
	if err != nil {
		t.Fatalf("标准库解不开这张图：%v（字节数 %d）", err, len(data))
	}
	if bounds := img.Bounds(); bounds.Dx() != 200 || bounds.Dy() != 200 {
		t.Fatalf("尺寸应是 200×200，实得 %dx%d", bounds.Dx(), bounds.Dy())
	}
	want := expectedNoiseRows(t)
	for row := 0; row < aiAvatarNoiseHeight; row++ {
		for col := 0; col < aiAvatarNoiseWidth; col++ {
			expect := want[row*noiseStride+1+col*3]
			got, _, _, a := img.At(col, row).RGBA()
			if uint8(got>>8) != expect {
				t.Fatalf("像素 (%d,%d) 的红通道应为 %d，实得 %d —— 整体错位一格通常是行过滤器字节占了随机源",
					col, row, expect, uint8(got>>8))
			}
			if a != 0xffff {
				t.Fatalf("color type 应是 RGB（不透明），(%d,%d) 的 alpha 实得 %d", col, row, a)
			}
		}
	}
}

// TestNoisePNGChunkTable：块序列只有 IHDR / IDAT / IEND，且 IHDR 那 13 字节每一格
// 都按实现里注释的值钉住（尺寸取**常量**而不是硬编码第二个真相）。
func TestNoisePNGChunkTable(t *testing.T) {
	chunks := walkNoisePNG(t, aiAvatarNoisePNG(&countingRandom{}))
	if got := kindsOf(chunks); got != "IHDR,IDAT,IEND" {
		t.Fatalf("块顺序应是 IHDR,IDAT,IEND，实得 %s（%d 块）", got, len(chunks))
	}
	header := chunks[0].data
	if len(header) != 13 {
		t.Fatalf("IHDR 必须 13 字节，实得 %d", len(header))
	}
	if w := be32(header[0:4]); w != aiAvatarNoiseWidth {
		t.Errorf("IHDR 宽度 %d vs 常量 %d", w, aiAvatarNoiseWidth)
	}
	if h := be32(header[4:8]); h != aiAvatarNoiseHeight {
		t.Errorf("IHDR 高度 %d vs 常量 %d", h, aiAvatarNoiseHeight)
	}
	// 下面这四格错了都不会让**我们自己的编码**出错，只会让解码方按别的行跨度读像素
	// —— 所以它们必须在这里钉死，而不是指望上一条（那条靠标准库，会替我们归因）。
	if header[8] != 8 || header[9] != 2 {
		t.Errorf("bit depth 应 8、color type 应 2(RGB)，实得 %d/%d", header[8], header[9])
	}
	if header[10] != 0 || header[11] != 0 || header[12] != 0 {
		t.Errorf("compression/filter/interlace 应全 0，实得 %d/%d/%d",
			header[10], header[11], header[12])
	}
	if len(chunks[2].data) != 0 {
		t.Errorf("IEND 是空块，实得 %d 字节", len(chunks[2].data))
	}
	if len(chunks[1].data) != noiseStoredLen {
		t.Errorf("IDAT 数据应是 %d 字节（zlib stored），实得 %d", noiseStoredLen, len(chunks[1].data))
	}
}

// TestNoiseIDATDecodesToPixelRows：IDAT 解压回来必须**逐字节等于**那 120200 行数据。
//
// 这是"stored 块原样复制输入"的定义性断言。它比的是**解压结果**，
// 所以哪天换成真压缩这一条仍然成立 —— 钉住字节形态的是 [TestZlibStoredFraming]。
func TestNoiseIDATDecodesToPixelRows(t *testing.T) {
	idat := walkNoisePNG(t, aiAvatarNoisePNG(&countingRandom{}))[1].data
	reader, err := zlib.NewReader(bytes.NewReader(idat))
	if err != nil {
		t.Fatalf("IDAT 不是合法 zlib 流：%v", err)
	}
	got, err := io.ReadAll(reader)
	if err != nil {
		t.Fatalf("解压 IDAT 失败：%v", err)
	}
	// Close 会校验流尾那四字节 ADLER32 —— 上面 `png.Decode` 也校验，
	// 但那一处失败的消息是"invalid checksum"，指不到 ADLER 这一格。
	if err := reader.Close(); err != nil {
		t.Fatalf("zlib 收尾校验没过（ADLER32 与数据对不上）：%v", err)
	}
	want := expectedNoiseRows(t)
	if !bytes.Equal(got, want) {
		t.Fatalf("解压结果与期望逐字节不符：实得 %d 字节，期望 %d 字节", len(got), len(want))
	}
	for row := 0; row < aiAvatarNoiseHeight; row++ {
		if got[row*noiseStride] != 0 {
			t.Fatalf("第 %d 行的过滤器号应是 0（None），实得 %d", row, got[row*noiseStride])
		}
	}
}

// TestZlibStoredFraming 钉 stored 块的**字节形态**：zlib 头、块数、
// BFINAL 只在末块、LEN/NLEN 小端且互补、ADLER32 与标准库一致。
//
// 为什么单独测这个纯函数而不只靠 PNG 解码：`png.Decode` 只要求"能解开"，
// 而 BTYPE 写成 01（固定 Huffman）时它同样可能解开一堆错内容**不报错**；
// 反过来"NLEN 写成大端"是必然解不开 —— 那一档由上面那条抓。
func TestZlibStoredFraming(t *testing.T) {
	data := bytes.Repeat([]byte{0x5A}, 70000) // 65535 + 4465 ⇒ 恰好两块
	stream := zlibStored(data)
	if stream[0] != 0x78 || stream[1] != 0x01 {
		t.Fatalf("zlib 头应是 78 01（0x7801 = 31×991 才过 FCHECK），实得 %02x %02x",
			stream[0], stream[1])
	}
	first := stream[2]
	if first&0x03 != 0 {
		t.Errorf("stored 块的 BTYPE 应是 00，实得 %02b", first&0x03)
	}
	if first&0x01 != 0 {
		t.Errorf("非末块的 BFINAL 必须是 0，实得 %02x", first)
	}
	if blockLen := uint16(stream[3]) | uint16(stream[4])<<8; blockLen != noiseMaxBlock {
		t.Errorf("LEN 应是**小端**的 %d，实得 %d", noiseMaxBlock, blockLen)
	}
	if nlen := uint16(stream[5]) | uint16(stream[6])<<8; nlen != ^uint16(noiseMaxBlock) {
		t.Errorf("NLEN 应是 LEN 按位取反，实得 %#04x", nlen)
	}
	second := noiseMaxBlock + 5 + 2 // 末块头的下标
	if stream[second]&0x01 != 1 {
		t.Errorf("末块的 BFINAL 必须是 1，实得 %02x", stream[second])
	}
	if blockLen := uint16(stream[second+1]) | uint16(stream[second+2])<<8; blockLen != 4465 {
		t.Errorf("末块 LEN 应是 70000-65535=4465，实得 %d", blockLen)
	}
	if want := stdAdler(data); be32(stream[len(stream)-4:]) != want {
		t.Errorf("ADLER32 与 hash/adler32 不一致：本包算 %08x，标准库 %08x",
			be32(stream[len(stream)-4:]), want)
	}
	reader, err := zlib.NewReader(bytes.NewReader(stream))
	if err != nil {
		t.Fatalf("造出来的不是合法 zlib：%v", err)
	}
	out, err := io.ReadAll(reader)
	if err != nil {
		t.Fatalf("解压失败：%v", err)
	}
	if !bytes.Equal(out, data) {
		t.Errorf("stored 块解压回来不是原样：%d 字节 vs %d 字节", len(out), len(data))
	}
}

// TestAdler32MatchesStdlib 逐档比对自己的实现与 `hash/adler32`。
//
// 特别跑一段**越过模数边界**的输入：`a`/`b` 只在累加超过 65521 时才取模，
// 短输入测不出"忘了取模"，那一档表现为长输入时 ADLER 偏大、PNG 解码器拒图。
func TestAdler32MatchesStdlib(t *testing.T) {
	cases := [][]byte{
		{},
		{0},
		[]byte("a"),
		bytes.Repeat([]byte{0xff}, 65520),
		bytes.Repeat([]byte{0xff}, 65521),
		bytes.Repeat([]byte{0xff}, 200000),
		{1, 2, 3, 255, 254, 253},
	}
	for i, data := range cases {
		if got, want := adler32(data), stdAdler(data); got != want {
			t.Errorf("第 %d 档（%d 字节）：本包 %08x vs 标准库 %08x", i, len(data), got, want)
		}
	}
}

// TestNoiseBase64IsBare ⭐ 上游收到的从来没有 `data:image/...;base64,` 前缀
// （旧生成器产 data URI，调用点立刻用正则去掉 —— 见 `ai_avatar_png.go` 文件头）。
//
// 顺带钉住体积这一格的**现状**：161 KB 是本链今天已知的最大请求体，
// 而"上游在乎不在乎大小"零样本 ⇒ 换压缩方式可以是**决定**，不能是**顺手**。
func TestNoiseBase64IsBare(t *testing.T) {
	raw := aiAvatarNoisePNG(&countingRandom{})
	text := aiAvatarNoiseBase64(&countingRandom{})
	// 裸 base64 的字母表里没有 `:` `;` `,`，所以"带 data URI 前缀"必然被这三格抓到，
	// 不必去猜前缀的具体写法（旧生成器那一句正则见过的是 `data:image/png;base64,`）。
	if strings.HasPrefix(text, "data:") || strings.ContainsAny(text, ":;,") {
		t.Errorf("不该带 data URI 前缀，实得前 40 字符 %q", head(text, 40))
	}
	// 120200 = 200×(1+200×3)；zlib stored = 2 + (5+65535) + (5+54665) + 4 = 120216；
	// PNG = 8 + 25(IHDR) + (12+120216)(IDAT) + 12(IEND) = 120273；base64 = 120273/3×4 = 160364。
	if len(raw) != noisePNGLen {
		t.Errorf("PNG 字节数应是 %d（上面那行算式），实得 %d", noisePNGLen, len(raw))
	}
	if len(text) != noiseBase64Len {
		t.Errorf("base64 字符数应是 %d，实得 %d", noiseBase64Len, len(text))
	}
	if strings.Contains(text, "=") {
		t.Errorf("字节数是 3 的整倍数，不该有 padding（有就说明上面的总长变了）")
	}
	back, err := base64.StdEncoding.DecodeString(text)
	if err != nil {
		t.Fatalf("不是合法 StdEncoding base64：%v", err)
	}
	if !bytes.Equal(back, raw) {
		t.Errorf("同一个随机源两次产图字节不一致（随机源没接上，或中间被改过）")
	}
}

// TestNoisePNGFollowsTheRandomSource 换随机源必须换字节。
//
// 抓的是"把随机流写死 / 复用了一份缓冲区"这一类 —— 那会让**每天五张图全都一样**，
// 而上游照样回成功（图片内容据旧实现不参与判断），所以这条错法在界面上永远看不见。
func TestNoisePNGFollowsTheRandomSource(t *testing.T) {
	first := aiAvatarNoisePNG(&countingRandom{})
	second := aiAvatarNoisePNG(&countingRandom{n: 7})
	if bytes.Equal(first, second) {
		t.Fatal("换随机源后字节完全相同 ⇒ 像素不是从随机源来的")
	}
	// 同一份随机源 ⇒ 逐字节相同（"请求体可逐字断言"这一条的前提）。
	if !bytes.Equal(aiAvatarNoisePNG(&countingRandom{}), first) {
		t.Error("同一随机流起点两次产图不一致")
	}
}

// stdAdler 走标准库 `hash/adler32`（它只对外给 hash.Hash32，没有 Sum32 快捷函数）。
func stdAdler(data []byte) uint32 {
	hash := stdadler.New()
	hash.Write(data)
	return hash.Sum32()
}

func head(text string, n int) string {
	if len(text) <= n {
		return text
	}
	return text[:n]
}
