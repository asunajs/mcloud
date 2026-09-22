package cli

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"

	"golang.org/x/term"
)

// output 是一次运行的两个流。
//
// # 为什么进度行**永远**走 stderr
//
// `--json` 时 stdout 必须是**干净的一份 JSON**，否则 `mcloud --json | jq`
// 当场语法错，而人在终端上看不出任何不对（两个流都混着打）。
// 不"只在 --json 时"才走 stderr 的理由是：那样这条性质只在半边被测到，
// 而默认那条路（人看的）恰恰没人会去检查有没有串行。
type output struct {
	stdout io.Writer
	stderr io.Writer
}

// human 打给人看的一行。`--json` 时**不该**调用它 —— 结构化输出走 [json]。
func (o *output) human(format string, args ...any) {
	fmt.Fprintf(o.stdout, format+"\n", args...)
}

// note 是一条"说一声但不影响退出码"的提示，走 stderr。
//
// 对位 Rust 的那条 `ℹ 库里还没有用户…`：它是**正常中间态**的说明，
// 不是失败，但也不能不说 —— 否则用户下一步在网页端看到"账号一个都没有"，
// 那时已经想不起来是这一步的事。
func (o *output) note(format string, args ...any) {
	fmt.Fprintf(o.stderr, format+"\n", args...)
}

// json 把值以两空格缩进打到 stdout。
//
// 缩进不是装饰：Rust 侧 `to_string_pretty` 就是 pretty，两边输出可以逐字对拍。
// ⚠ 必须关 HTML 转义（同 core.marshalJSON 的坑 #32）：`json.MarshalIndent`
// 默认把 `<>&` 转成 `\u003c…`，而 serde 不会 —— 开着它谈"逐字对拍"是假的。
func (o *output) json(value any) error {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(value); err != nil {
		return fmt.Errorf("序列化输出失败: %w", err)
	}
	_, err := o.stdout.Write(buf.Bytes())
	return err
}

// rawJSON 打印一段**已经序列化好**的 JSON（迁移报告那条路）。
func (o *output) rawJSON(encoded []byte) error {
	if !json.Valid(encoded) {
		return errors.New("内部错误：产出的不是合法 JSON")
	}
	var indented bytes.Buffer
	if err := json.Indent(&indented, encoded, "", "  "); err != nil {
		return fmt.Errorf("序列化输出失败: %w", err)
	}
	_, err := fmt.Fprintln(o.stdout, indented.String())
	return err
}

// readSecret 读一行敏感输入（口令 / 验证码 / 滑块答案）。
//
// # 两条输入路径，与 Rust 侧一致
//
//   - **有终端**：关掉回显再读（`golang.org/x/term` 对位 `rpassword`）；
//   - **没有终端**（管道、CI、脚本）：读标准输入 —— 否则 `echo 'pw' | mcloud user init`
//     会像 Rust 早期那样报 `Device not configured`，自动化就没了路。
//
// 两条路都**不接受命令行参数**：参数进 shell 历史与 `ps` 输出，
// 那是凭据泄漏最常见的一条路。返回值只该被哈希、被比较，
// **永远不该**出现在打印、日志或错误串里。
func (d *Deps) readSecret(prompt string) (string, error) {
	fmt.Fprint(d.Stderr, prompt)

	if file, ok := d.Stdin.(*os.File); ok && term.IsTerminal(int(file.Fd())) {
		// 终端路径不经过 d.reader：term.ReadPassword 直接读 fd，
		// 而预读进 bufio 缓冲区的那段会把后面的输入一起吞掉。
		fmt.Fprint(d.Stderr, "\r")
		raw, err := term.ReadPassword(int(file.Fd()))
		fmt.Fprintln(d.Stderr)
		if err != nil {
			return "", fmt.Errorf("读取输入失败: %w", err)
		}
		return trimLineEnd(string(raw)), nil
	}

	line, err := d.reader().ReadString('\n')
	if err != nil && line == "" {
		return "", fmt.Errorf("读取标准输入失败: %w", err)
	}
	return trimLineEnd(line), nil
}

// readPasswordTwice 读两次口令并要求一致。
//
// 一致与否只报"不一致"，**不回显任何一次输入的内容**。
func (d *Deps) readPasswordTwice() (string, error) {
	first, err := d.readSecret("设置口令: ")
	if err != nil {
		return "", err
	}
	again, err := d.readSecret("再输一次: ")
	if err != nil {
		return "", err
	}
	if first != again {
		return "", errors.New("两次输入的口令不一致")
	}
	return first, nil
}

// trimLineEnd 去掉行尾换行符。
//
// ⚠ 用 [strings.TrimRight] 而不是 [strings.TrimSpace]：口令里的空格是有意义的，
// "pw " 与 "pw" 是两个口令，顺手 trim 一下会让用户设的口令变成另一个。
func trimLineEnd(s string) string {
	return strings.TrimRight(s, "\r\n")
}
