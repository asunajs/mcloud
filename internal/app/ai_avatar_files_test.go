package app

// 「按图片 URL 找回网盘文件」那两枚纯函数的用例。
//
// 这一族错了不报错，只**少找 / 多找**：
//   - 少找 ⇒ 网盘里留下没人清的临时文件（今天本链不删，代价是攒垃圾）；
//   - 多找 ⇒ 将来接了清理就是**删错文件**（同名假设破了那一档，见实现文件头）。
//
// 所以这里的断言分两头：既钉"该撞中的撞中了"，也钉"不该撞中的一个都没撞中"。
// ⚠ 两头都只钉**匹配规则**，不钉"上游今天到底给不给 `.webp`"—— 那格零实拍，
// 登记在 `ai_avatar_files.go` 的扩展名注释里。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/upstream"
)

func str(s string) *string { return &s }

// TestImageFileNameCases 逐档钉"取哪一段、什么时候截断、什么算图片"。
func TestImageFileNameCases(t *testing.T) {
	cases := []struct {
		name string
		url  string
		want string
		ok   bool
		why  string
	}{
		{"空串", "", "", false, "没给 URL，与「给了但不是图片」同一处置"},
		{"裸 jpg", "https://img.example.com/a.jpg", "a.jpg", true, ""},
		{"jpeg", "https://img.example.com/a.jpeg", "a.jpeg", true, ""},
		{"大写扩展名", "https://img.example.com/a.PNG", "a.PNG", true, "旧实现的正则带 i"},
		{"⭐ 带查询串", "https://img.example.com/a.jpg?token=abc", "a.jpg", true,
			"先截 ? 再验扩展名；反过来写会把这张图判成非图片，那个临时文件永远找不回来"},
		{"带片段", "https://img.example.com/a.jpg#x", "a.jpg", true, ""},
		{"查询串加片段", "https://img.example.com/a.jpg?v=1#x", "a.jpg", true, ""},
		{"末尾斜杠", "https://img.example.com/dir/", "", false, "最后一段是空串"},
		{"无扩展名", "https://img.example.com/abcdef", "", false, ""},
		{"⚠ webp", "https://img.example.com/a.webp", "", false,
			"三档之外的扩展名判 false ⇒ 那一个文件**找不到**（不是找错）。这一格未验证"},
		{"扩展名后还有字符", "https://img.example.com/a.jpgx", "", false, "正则锚在结尾"},
		{"点在前缀里", "https://img.example.com/a.jpeg?v=1.2", "a.jpeg", true, ""},
		{"多段点", "https://img.example.com/a.b.jpg", "a.b.jpg", true, "取整个最后一段"},
		{"查询串里有斜杠", "https://img.example.com/a?b/c.jpg", "c.jpg", true, ""},
		{"⚠ 问号在段前", "https://img.example.com/a?b/c", "", false,
			"旧实现先取最后一段、再截 ?，所以它取到的是 `c`（不是图片 ⇒ 返回空）。这里保持一致，不去「修正」它"},
		{"整串没有斜杠", "a.jpg", "a.jpg", true, "LastIndexByte 找不到 ⇒ 从 0 起"},
		{"只有斜杠", "/", "", false, ""},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, ok := aiAvatarImageFileName(c.url)
			if ok != c.ok {
				t.Errorf("ok 应 %v，实得 %v（名字 %q）—— %s", c.ok, ok, got, c.why)
			}
			if got != c.want {
				t.Errorf("文件名应 %q，实得 %q —— %s", c.want, got, c.why)
			}
		})
	}
}

// TestImageFileNameDoesNotLeakQueryIntoTheName ⭐ 单拎出来，因为它是"名字对不上"的头号来源。
//
// 网盘侧的 `name` 从来不带查询串；这里若把 `a.jpg?token=abc` 整串交出去，
// 表现是匹配**永远失败**且不报错（查询串还可能是凭据形态的串，那更不该往下传）。
func TestImageFileNameDoesNotLeakQueryIntoTheName(t *testing.T) {
	name, ok := aiAvatarImageFileName("https://img.example.com/photo.jpg?authorization=Basic%20x")
	if !ok || name != "photo.jpg" {
		t.Fatalf("应只取出 photo.jpg，实得 %q/%v", name, ok)
	}
	for _, forbidden := range []string{"Basic", "authorization", "?", "%"} {
		// 反向改坏点：去掉 `aiAvatarImageFileName` 里那句 `IndexAny(last, "?#")` ⇒ 本条红。
		if strings.Contains(strings.ToLower(name), strings.ToLower(forbidden)) {
			t.Errorf("文件名里不该出现 %q（实得 %q）", forbidden, name)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 作品 id → 待清理的网盘文件 id
// ─────────────────────────────────────────────────────────────────────────────

// avatarWork 造一条作品（两格 URL 传空串表示"上游没给这一格"，即 nil 指针）。
func avatarWork(id string, imgURL, qrURL string) upstream.AiAvatarWork {
	work := upstream.AiAvatarWork{WorkID: id}
	if imgURL != "" {
		work.ImgURL = str(imgURL)
	}
	if qrURL != "" {
		work.QrImgURL = str(qrURL)
	}
	return work
}

func cloudFiles(pairs ...string) []upstream.CloudFile {
	out := make([]upstream.CloudFile, 0, len(pairs)/2)
	for i := 0; i+1 < len(pairs); i += 2 {
		out = append(out, upstream.CloudFile{Name: pairs[i], FileID: pairs[i+1]})
	}
	return out
}

func TestCollectAiAvatarFileIDs(t *testing.T) {
	const host = "https://img.example.com/"
	cases := []struct {
		name      string
		uploadIDs []string
		works     []upstream.AiAvatarWork
		files     []upstream.CloudFile
		want      []string
	}{
		{
			// 这一条是整族的**主用途**：一次生成产两张（原图 + 合成图）。
			name:      "两张都撞中",
			uploadIDs: []string{"U1"},
			works:     []upstream.AiAvatarWork{avatarWork("U1", host+"origin.jpg", host+"merged.jpg")},
			files:     cloudFiles("origin.jpg", "F-ORIGIN", "merged.jpg", "F-MERGED", "别的.jpg", "F-OTHER"),
			want:      []string{"F-MERGED", "F-ORIGIN"},
		},
		{
			name:      "只看在上传 id 里出现过的作品",
			uploadIDs: []string{"U1"},
			works: []upstream.AiAvatarWork{
				avatarWork("U1", host+"a.jpg", ""),
				avatarWork("U2", host+"b.jpg", ""), // 别人家的作品，名字再像也不算
			},
			files: cloudFiles("a.jpg", "F-A", "b.jpg", "F-B"),
			want:  []string{"F-A"},
		},
		{
			name:      "两格 URL 同图时按 fileId 去重",
			uploadIDs: []string{"U1"},
			works:     []upstream.AiAvatarWork{avatarWork("U1", host+"same.jpg", host+"same.jpg")},
			files:     cloudFiles("same.jpg", "F-SAME"),
			want:      []string{"F-SAME"},
		},
		{
			// ⭐ 这一格钉的是 `seen`：同一个 `fileId` 在列表里出现两次
			// （分页重叠、或网盘真给了两条）时只能登记一遍 ——
			// 登记两遍的代价落在**将来接了清理**那一刻：同一个文件删两次。
			name:      "同一个 fileId 出现两次只算一次",
			uploadIDs: []string{"U1", "U2"},
			works: []upstream.AiAvatarWork{
				avatarWork("U1", host+"x.jpg", ""), avatarWork("U2", host+"x.jpg", ""),
			},
			files: cloudFiles("x.jpg", "F-X", "x.jpg", "F-X"),
			want:  []string{"F-X"},
		},
		{
			// 同名但**不同** fileId ⇒ 两个都算（网盘允许同名重复条目，少一个就是漏清）。
			// 这一格反向钉上一条：去重只能按 `fileId`，不能按名字。
			name:      "同名不同 id 的两个都算",
			uploadIDs: []string{"U1"},
			works:     []upstream.AiAvatarWork{avatarWork("U1", host+"x.jpg", "")},
			files:     cloudFiles("x.jpg", "F-X1", "x.jpg", "F-X2"),
			want:      []string{"F-X1", "F-X2"},
		},
		{
			name:      "缺 qrImgUrl 那一格不算错",
			uploadIDs: []string{"U1"},
			works:     []upstream.AiAvatarWork{avatarWork("U1", "", host+"only-qr.jpg")},
			files:     cloudFiles("only-qr.jpg", "F-QR"),
			want:      []string{"F-QR"},
		},
		{
			name:      "非图片 URL 被跳过",
			uploadIDs: []string{"U1"},
			works:     []upstream.AiAvatarWork{avatarWork("U1", host+"page.html", host+"real.jpg")},
			files:     cloudFiles("page.html", "F-HTML", "real.jpg", "F-REAL"),
			want:      []string{"F-REAL"},
		},
		{
			name:      "名字对不上就是找不到（不做前缀放宽）",
			uploadIDs: []string{"U1"},
			works:     []upstream.AiAvatarWork{avatarWork("U1", host+"a.jpg", "")},
			files:     cloudFiles("a.jpg.png", "F-NEAR", "prefix-a.jpg", "F-NEAR2"),
			want:      []string{},
		},
		{
			name:      "没有上传 id 时一个都不找",
			uploadIDs: nil,
			works:     []upstream.AiAvatarWork{avatarWork("U1", host+"a.jpg", "")},
			files:     cloudFiles("a.jpg", "F-A"),
			want:      []string{},
		},
		{
			name:      "网盘列表为空",
			uploadIDs: []string{"U1"},
			works:     []upstream.AiAvatarWork{avatarWork("U1", host+"a.jpg", "")},
			files:     nil,
			want:      []string{},
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got := collectAiAvatarFileIDs(c.uploadIDs, c.works, c.files)
			if len(got) != len(c.want) {
				t.Fatalf("应得 %v，实得 %v", c.want, got)
			}
			for i := range got {
				if got[i] != c.want[i] {
					t.Fatalf("第 %d 格应 %q，实得 %q（全部 %v）", i, c.want[i], got[i], got)
				}
			}
			if got == nil {
				t.Error("该交回空切片而不是 nil（它会进 SummaryLine 与将来的清理请求）")
			}
		})
	}
}

// TestCollectAiAvatarFileIDsSorts ⭐ 返回**排序**，不受上游返回顺序影响。
//
// 旧实现交回的是 Set 的插入序 ⇒ 它取决于 `getWorkList` 这次先回哪一条。
// 这里给两份**内容相同、顺序相反**的输入：不排序的话两串会不同，
// 而"两次跑同一份响应得到同一串"是 `SummaryLine` 与幂等断言的前提。
func TestCollectAiAvatarFileIDsSorts(t *testing.T) {
	const host = "https://img.example.com/"
	works := []upstream.AiAvatarWork{
		avatarWork("U1", host+"one.jpg", host+"two.jpg"),
		avatarWork("U2", host+"three.jpg", ""),
	}
	files := cloudFiles("one.jpg", "F-ONE", "two.jpg", "F-TWO", "three.jpg", "F-THREE")
	forward := collectAiAvatarFileIDs([]string{"U1", "U2"}, works, files)
	reversed := collectAiAvatarFileIDs([]string{"U2", "U1"}, works,
		[]upstream.CloudFile{files[2], files[1], files[0]})
	want := []string{"F-ONE", "F-THREE", "F-TWO"}
	for i, got := range [][]string{forward, reversed} {
		if len(got) != 3 || got[0] != want[0] || got[1] != want[1] || got[2] != want[2] {
			t.Errorf("第 %d 份输入应得 %v（排序后），实得 %v", i+1, want, got)
		}
	}
}
