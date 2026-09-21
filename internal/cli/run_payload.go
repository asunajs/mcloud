package cli

import (
	"fmt"

	"github.com/catlair/mcloud/internal/app"
)

// runVersion 打印版本与运行时信息。
//
// `--json` 的响应体与 `GET /api/version` **是同一个结构体**（app.VersionPayload），
// 不是"两份长得像的输出" —— 所以前端与脚本可以互换使用。
func runVersion(out *output, asJSON bool) error {
	payload := app.VersionPayloadOf()
	if asJSON {
		return out.json(payload)
	}
	out.human("%s %s (时区 %s)", payload.Name, payload.Version, payload.Timezone)
	return nil
}

// runCapabilities 打印能力注册表。
//
// 同样地，`--json` 与 `GET /api/capabilities` 同源。
func runCapabilities(out *output, asJSON bool) error {
	payload := app.CapabilitiesPayloadOf()
	if asJSON {
		return out.json(payload)
	}

	out.human("能力注册表（%d 项）\n", len(payload.All))
	for _, capability := range payload.All {
		line := fmt.Sprintf("  %-16s %s  [%s · risk=%s]",
			capability.Code, capability.Title, capability.Group, capability.Risk)
		if capability.Hidden {
			line += "  (独立页面)"
		}
		out.human("%s", line)
	}

	// 渲染成 `sign, cloud-snapshot`，而不是 Go 的 `[a b]` —— 那是给人读的一行。
	out.human("\n通用任务列表可见: %s", joinComma(payload.Visible))
	return nil
}

func joinComma(items []string) string {
	joined := ""
	for index, item := range items {
		if index > 0 {
			joined += ", "
		}
		joined += item
	}
	return joined
}
