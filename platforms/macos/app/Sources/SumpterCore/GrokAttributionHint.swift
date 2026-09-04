import Foundation

/// Grok Build 项目归因提示。判定口径对齐 Claude Code：看分析行里有没有
/// `grok_build` 已归因到 workspace_local / client_declared，而不是去读 shell。
public enum GrokAttributionHint {
    public static func state(projects: [ClaudeAttributionHint.ProjectRow]) -> ClaudeAttributionHint.State {
        if projects.contains(where: { row in
            (row.projectSource == ClaudeAttributionHint.declaredSource
                || row.projectSource == "workspace_local")
                && row.clientKinds.contains("grok_build")
        }) {
            return .configured
        }
        let hasUnattributed = projects.contains { row in
            row.attempts > 0
                && (row.name == ClaudeAttributionHint.unidentifiedProject
                    || row.projectSource == ClaudeAttributionHint.missingSource)
                && row.clientKinds.contains("grok_build")
        }
        return hasUnattributed ? .unconfigured : .unknown
    }

    public static func shouldPrompt(projects: [ClaudeAttributionHint.ProjectRow]) -> Bool {
        state(projects: projects) == .unconfigured
    }

    public static let message = """
        这些 Grok Build 请求没有项目归因。Grok 的工作目录只写到本机会话元数据，不进推理请求，\
        需要在**跑 grok 的机器**上装 wrapper，让请求带上项目名。
        """

    public static let command = """
        ./grok-project-attribution.sh status
        ./grok-project-attribution.sh install
        """

    public enum Guide {
        public static let title = "Grok Build 项目归因"
        public static let subtitle = "让统计能按项目区分 Grok Build 请求。只影响项目维度。"

        public static let why = """
            Grok Build 的 cwd / git 走本机 GCS metadata，不进推理请求，所以默认所有 Grok 请求\
            都堆在「未识别项目」里。wrapper 按启动目录写入 X-Sumpter-Project / Workspace / Git-Remote / User，\
            读完即从出站剥离。会话 ID 仍用 x-grok-session-id。
            """

        public static let whereToRun = """
            配置必须在启动 grok 的那台机器执行，不是只运行 sidecar/daemon 的机器。\
            每台跑 Grok Build 的主机各装一次；装完要新开终端。
            """

        public static let privacy = """
            X-Sumpter-* header 会被代理从出站剥离。GROK_CONFIG overlay 只注入这四个归因 header，\
            不改 ~/.grok/config.toml。
            """

        public static func statusLabel(_ state: ClaudeAttributionHint.State) -> String {
            ClaudeAttributionHint.Guide.statusLabel(state)
        }

        public static func statusDetail(_ state: ClaudeAttributionHint.State) -> String {
            switch state {
            case .configured:
                "统计里已出现 Grok Build 的本地项目行，归因链路是通的。"
            case .unconfigured:
                "有 Grok Build 请求落进「未识别项目」，且没有任何一行来自 Grok 的工作区声明。"
            case .unknown:
                "当前时间窗口内没有 Grok Build 流量，或分析数据还没取到 —— 无法判定。"
            }
        }

        public static let steps: [ClaudeAttributionHint.Guide.Step] = [
            .init(
                id: 1,
                title: "只读体检",
                command: "./grok-project-attribution.sh status",
                note: "看当前 shell、要改哪个 rc 文件、GROK_CONFIG_PATH 会不会挡住 overlay。不改任何文件。"
            ),
            .init(
                id: 2,
                title: "安装 wrapper",
                command: "./grok-project-attribution.sh install",
                note: "想先预演就加 --dry-run。装前自动给 rc 打时间戳备份；改动是一段带标记的 source 块。"
            ),
            .init(
                id: 3,
                title: "新开终端验证",
                command: "",
                note: "wrapper 是 grok() shell 函数。新开终端后再启动 grok，进项目发一条消息，回本页看状态变成「已生效」。"
            ),
        ]

        public static let rollback: [ClaudeAttributionHint.Guide.RollbackCommand] = [
            .init(id: 1, command: "./grok-project-attribution.sh restore", note: "还原 rc 到装前"),
            .init(id: 2, command: "./grok-project-attribution.sh uninstall", note: "移除 wrapper，保留备份"),
        ]
    }
}
