import Foundation

/// 统计页「项目 Token 排行」里 Claude Code 请求未归因时的提示判定(纯逻辑,可单测)。
///
/// 判定只看分析数据本身,不去读 `~/.zshrc` 或 `~/.claude/settings.json`——用户可能用
/// direnv、per-project settings 或自己的 wrapper 配好了,读 shell 配置只会误判。真正
/// 的信号是「确实有 Claude Code 请求落进了未识别项目」。
public enum ClaudeAttributionHint {
    /// 未归因项目行的固定名字(与 Rust 侧 `project_base` 的取值一致)。
    public static let unidentifiedProject = "unidentified_project"
    /// Codex 结构化 workspace 与客户端声明都缺位时的来源标签。
    public static let missingSource = "missing_workspace_metadata"
    /// 客户端声明归因成功时的来源标签。
    public static let declaredSource = "client_declared"

    /// 提示所需的最小行投影,避免 SumpterCore 依赖 AdminWire。
    public struct ProjectRow: Equatable, Sendable {
        public let name: String
        public let projectSource: String?
        public let clientKinds: [String]
        public let attempts: Int

        public init(name: String, projectSource: String?, clientKinds: [String], attempts: Int) {
            self.name = name
            self.projectSource = projectSource
            self.clientKinds = clientKinds
            self.attempts = attempts
        }
    }

    /// 归因配置状态。安全页面的引导要区分「已配好」「确实没配」「说不清」——统计页那个
    /// Bool 只需要知道该不该提示，不够用。两者共用同一内核，避免两处口径漂移。
    /// 与 WebUI 侧 `ccAttributionState`(helpers.js) 同口径。
    public enum State: String, Sendable {
        /// 已出现 `client_declared` 行：链路通了。
        case configured
        /// 无 `client_declared`，且确实有 Claude Code 请求落进未识别项目。
        case unconfigured
        /// 窗口内没有 CC 未归因流量，或分析数据还没到：说不清，不能断言"没配"。
        case unknown
    }

    public static func state(projects: [ProjectRow]) -> State {
        if projects.contains(where: { $0.projectSource == declaredSource }) {
            return .configured
        }
        let hasUnattributedClaudeCode = projects.contains { row in
            row.attempts > 0
                && isUnidentified(row)
                && row.clientKinds.contains("claude_code")
        }
        return hasUnattributedClaudeCode ? .unconfigured : .unknown
    }

    /// 是否该显示「Claude Code 项目归因未配置」提示。
    ///
    /// 成立条件:存在未识别项目行，且该行确实由 Claude Code 贡献。
    /// 已经有任何一行是 `client_declared` 时不再提示——说明用户已经配好了，
    /// 剩下的未识别行是配置生效之前的历史事件，提示只会是噪音。
    public static func shouldPrompt(projects: [ProjectRow]) -> Bool {
        state(projects: projects) == .unconfigured
    }

    private static func isUnidentified(_ row: ProjectRow) -> Bool {
        row.name == unidentifiedProject || row.projectSource == missingSource
    }

    /// 提示正文。不含具体路径:配置器在两个产品里的位置不同(发布包 vs clone 的仓库)。
    public static let message = """
        这些 Claude Code 请求没有项目归因。CC 默认不上行工作目录，需要在**跑 CC 的机器**上
        装一个 wrapper，让它随请求带上项目名。会话统计不受影响。
        """

    /// 给用户复制的命令。install 前建议先跑 status 体检，所以给成两条。
    public static let command = """
        ./cc-project-attribution.sh status
        ./cc-project-attribution.sh install
        """

    // MARK: - 安全页面的完整引导

    /// 安全页面的完整引导内容。与统计页的小提示卡(`message` / `command`)分工不同:那边是
    /// "发现症状后的一句话 + 命令",这里是"从零配完"的全流程。文案与 WebUI 侧
    /// `CC_ATTRIBUTION_GUIDE`(helpers.js) 同步,关键串由两侧测试各自钉死。
    ///
    /// 放 SumpterCore 而不是视图里:长文案可单测、不随 UI 重排丢失,且与判定同处一地。
    public enum Guide {
        public struct Step: Identifiable, Sendable {
            public let id: Int
            public let title: String
            /// 空串表示这一步没有命令(例如"新开终端验证")。
            public let command: String
            public let note: String
        }

        public struct MatrixRow: Identifiable, Sendable {
            public let id: Int
            public let label: String
            public let macOS: String
            public let linux: String
        }

        public struct Pitfall: Identifiable, Sendable {
            public let id: Int
            public let title: String
            public let detail: String
        }

        public struct RollbackCommand: Identifiable, Sendable {
            public let id: Int
            public let command: String
            public let note: String
        }

        public static let title = "Claude Code 项目归因"
        public static let subtitle = "让统计能按项目区分 Claude Code 请求。只影响项目维度，会话维度零配置就有。"

        public static let why = """
            Claude Code 不把工作目录放进请求（cwd / project_dir 只给本机 statusLine 和 hook 用），\
            所以默认所有 CC 请求都堆在「未识别项目」里。要分项目，就让 CC 把项目名随请求带上：\
            Sumpter认 X-Kekulv-Project / X-Kekulv-Workspace / X-Kekulv-Git-Remote 三个入站 header，\
            读完即从出站剥离。会话维度不受影响 —— CC 无条件发 X-Claude-Code-Session-Id。
            """

        public static let whereToRun = """
            先分清三台机器：WebUI/浏览器所在设备只负责打开管理页；daemon 所在主机负责接收请求和保存统计；\
            Claude Code 所在主机才需要安装 wrapper。配置命令必须在跑 CC 的主机执行，不是打开 WebUI 或运行 daemon 的机器。\
            若 CC 与 daemon 不同机，先 SSH/进入 CC 主机，并确认 ANTHROPIC_BASE_URL 指向 daemon 的可达地址；\
            daemon 只监听 127.0.0.1 时要使用 SSH 隧道或安全内网地址，不要直接暴露无认证监听。每台跑 CC 的机器各配一次。
            """

        public static let privacy = """
            header 会被代理从出站剥离，上游中转站看不到。但同一请求的 body 本来就带工作目录绝对路径、\
            CLAUDE.md 全文和 git status —— 配这三个 header 不增不减外泄面，只决定能否按项目统计。
            """

        public static func statusLabel(_ state: State) -> String {
            switch state {
            case .configured: "已生效"
            case .unconfigured: "未配置"
            case .unknown: "暂无法判定"
            }
        }

        public static func statusDetail(_ state: State) -> String {
            switch state {
            case .configured:
                "统计里已出现「客户端声明」来源的项目行，归因链路是通的。"
            case .unconfigured:
                "有 Claude Code 请求落进「未识别项目」，且没有任何一行来自客户端声明。"
            case .unknown:
                "当前时间窗口内没有 Claude Code 流量，或分析数据还没取到 —— 无法判定。"
            }
        }

        public static let steps: [Step] = [
            Step(
                id: 1,
                title: "只读体检",
                command: "./cc-project-attribution.sh status",
                note: "看当前 shell、要改哪个 rc 文件、有没有 settings.json 覆盖陷阱。不改任何文件。"
            ),
            Step(
                id: 2,
                title: "安装 wrapper",
                command: "./cc-project-attribution.sh install",
                note:
                    "想先预演就加 --dry-run。装前自动给 rc 打时间戳备份；改动是一段带标记的 source 块，可精确移除。"
            ),
            Step(
                id: 3,
                title: "新开终端验证",
                command: "",
                note:
                    "wrapper 是 shell 函数，只对之后启动的 shell 生效。新开一个终端窗口，进任意项目发一条消息，回本页看状态变成「已生效」。"
            ),
        ]

        public static let platformMatrix: [MatrixRow] = [
            MatrixRow(
                id: 1, label: "配置器位置",
                macOS: "App 内 Resources/（源码构建则在 platforms/linux/scripts/）",
                linux: "部署包解包后 scripts/（安装后 /opt/kekulv/scripts/）"),
            MatrixRow(
                id: 2, label: "默认 shell",
                macOS: "通常 zsh → ~/.zshrc", linux: "视发行版，zsh 或 bash 都常见"),
            MatrixRow(
                id: 3, label: "bash 用哪个 rc",
                macOS: "~/.bash_profile（登录 shell 不读 .bashrc）", linux: "~/.bashrc"),
            MatrixRow(
                id: 4, label: "CC 与 daemon",
                macOS: "通常同机", linux: "CC 常在别的机器上连远程 daemon"),
            MatrixRow(
                id: 5, label: "fish",
                macOS: "fish-snippet 手动粘贴（未实测）", linux: "同左"),
        ]

        public static let pitfalls: [Pitfall] = [
            Pitfall(
                id: 1,
                title: "值必须是纯 ASCII",
                detail:
                    "CC 见到含非 ASCII 的 ANTHROPIC_CUSTOM_HEADERS 会直接报错退出 —— 不是归因缺失，是整个会话起不来。中文目录名会让 claude 在该项目完全不可用。配置器已内置 ASCII 守卫，跳过不安全的值而不是硬塞。"
            ),
            Pitfall(
                id: 2,
                title: "别写进 settings.json 的 env",
                detail:
                    "那里的值会覆盖进程环境变量，且不做插值（$PWD、${CLAUDE_PROJECT_DIR} 全部字面传出）。一旦写死，shell wrapper 永久失效，且只能固定一个项目名。配置器检出该键会拒绝安装。"
            ),
            Pitfall(
                id: 3,
                title: "进程级，启动时读一次",
                detail:
                    "归因的是「启动 CC 时所在的项目」，会话内 cd 不更新。--print / SDK / CI 这些非交互场景不走 shell 函数，需要自行显式设环境变量。"
            ),
        ]

        public static let rollback: [RollbackCommand] = [
            RollbackCommand(
                id: 1, command: "./cc-project-attribution.sh restore",
                note: "还原 rc 到装前（取最新备份，并先把当前 rc 另存为 .kekulv-prerestore-*）"),
            RollbackCommand(
                id: 2, command: "./cc-project-attribution.sh uninstall",
                note: "移除 wrapper，保留备份"),
        ]
    }
}
