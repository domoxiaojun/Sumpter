import Testing

@testable import SumpterCore

private func row(
    _ name: String,
    source: String? = nil,
    kinds: [String] = ["claude_code"],
    attempts: Int = 1
) -> ClaudeAttributionHint.ProjectRow {
    ClaudeAttributionHint.ProjectRow(
        name: name, projectSource: source, clientKinds: kinds, attempts: attempts)
}

@Suite struct ClaudeAttributionHintTests {
    @Test func promptsWhenClaudeCodeFallsIntoUnidentifiedProject() {
        #expect(ClaudeAttributionHint.shouldPrompt(projects: [row("unidentified_project")]))
    }

    @Test func promptsWhenSourceIsMissingWorkspaceMetadata() {
        #expect(
            ClaudeAttributionHint.shouldPrompt(projects: [
                row("whatever", source: "missing_workspace_metadata")
            ]))
    }

    /// Codex 不走客户端声明这条路,它的未识别行不该催用户配 CC 的 wrapper。
    @Test func staysQuietWhenOnlyCodexIsUnidentified() {
        #expect(
            !ClaudeAttributionHint.shouldPrompt(projects: [
                row("unidentified_project", kinds: ["codex"])
            ]))
    }

    /// 已经有 client_declared 行 = 用户配好了,剩下的未识别行是历史事件,提示是噪音。
    @Test func staysQuietOnceAnyRowIsClientDeclared() {
        #expect(
            !ClaudeAttributionHint.shouldPrompt(projects: [
                row("unidentified_project"),
                row("automode-proxy", source: "client_declared"),
            ]))
    }

    @Test func staysQuietWhenEverythingIsAttributed() {
        #expect(
            !ClaudeAttributionHint.shouldPrompt(projects: [
                row("automode-proxy", source: "workspace_local", kinds: ["codex"])
            ]))
    }

    @Test func staysQuietOnEmptyAnalytics() {
        #expect(!ClaudeAttributionHint.shouldPrompt(projects: []))
    }

    /// 0 次尝试的行只是筛选残留,不构成"确实有未归因请求"。
    @Test func staysQuietWhenUnidentifiedRowHasNoAttempts() {
        #expect(
            !ClaudeAttributionHint.shouldPrompt(projects: [
                row("unidentified_project", attempts: 0)
            ]))
    }

    // MARK: - 三态判定(安全页面引导用)

    @Test func stateIsConfiguredOnceAnyRowIsClientDeclared() {
        #expect(
            ClaudeAttributionHint.state(projects: [
                row("unidentified_project"),
                row("automode-proxy", source: "client_declared"),
            ]) == .configured)
    }

    @Test func stateIsUnconfiguredWhenClaudeCodeIsUnattributed() {
        #expect(ClaudeAttributionHint.state(projects: [row("unidentified_project")]) == .unconfigured)
    }

    /// unknown 的成因之一:窗口里的未识别行是 Codex 的,不能据此说 CC 没配。
    @Test func stateIsUnknownWhenOnlyCodexIsUnidentified() {
        #expect(
            ClaudeAttributionHint.state(projects: [
                row("unidentified_project", kinds: ["codex"])
            ]) == .unknown)
    }

    /// unknown 的成因之二:分析数据还没到(打开安全页时统计可能压根没拉过)。
    @Test func stateIsUnknownOnEmptyAnalytics() {
        #expect(ClaudeAttributionHint.state(projects: []) == .unknown)
    }

    /// shouldPrompt 现在是三态的 Bool 投影,只有 unconfigured 才提示 —— 统计页行为不能变。
    @Test func shouldPromptIsProjectionOfUnconfiguredState() {
        let cases: [[ClaudeAttributionHint.ProjectRow]] = [
            [row("unidentified_project")],
            [row("whatever", source: "missing_workspace_metadata")],
            [row("automode-proxy", source: "client_declared")],
            [row("unidentified_project", kinds: ["codex"])],
            [row("unidentified_project", attempts: 0)],
            [],
        ]
        for projects in cases {
            #expect(
                ClaudeAttributionHint.shouldPrompt(projects: projects)
                    == (ClaudeAttributionHint.state(projects: projects) == .unconfigured))
        }
    }

    // MARK: - 引导文案(与 WebUI 侧 CC_ATTRIBUTION_GUIDE 同口径)

    @Test func guideLabelsEveryState() {
        for state in [ClaudeAttributionHint.State.configured, .unconfigured, .unknown] {
            #expect(!ClaudeAttributionHint.Guide.statusLabel(state).isEmpty)
            #expect(!ClaudeAttributionHint.Guide.statusDetail(state).isEmpty)
        }
    }

    /// 少一条命令用户就配不完或退不回来,所以逐条钉死而不是只数个数。
    @Test func guideCarriesInstallAndRollbackCommands() {
        let steps = ClaudeAttributionHint.Guide.steps.map(\.command)
        #expect(steps.contains("./cc-project-attribution.sh status"))
        #expect(steps.contains("./cc-project-attribution.sh install"))
        let rollback = ClaudeAttributionHint.Guide.rollback.map(\.command)
        #expect(rollback.contains("./cc-project-attribution.sh restore"))
        #expect(rollback.contains("./cc-project-attribution.sh uninstall"))
    }

    /// 「新开终端」那一步没有命令,但必须有说明 —— 它正是最容易被跳过的一步。
    @Test func guideStepWithoutCommandStillExplainsItself() {
        let commandless = ClaudeAttributionHint.Guide.steps.filter { $0.command.isEmpty }
        #expect(commandless.count == 1)
        #expect(commandless.allSatisfy { !$0.note.isEmpty && !$0.title.isEmpty })
    }

    /// 三个陷阱都是实测踩到的:漏掉任一条会让用户以为配置失败,或直接把 CC 弄到起不来。
    @Test func guideKeepsAllThreePitfalls() {
        let pitfalls = ClaudeAttributionHint.Guide.pitfalls
        #expect(pitfalls.count == 3)
        #expect(pitfalls.contains { $0.title.contains("ASCII") })
        #expect(pitfalls.contains { $0.title.contains("settings.json") })
        #expect(pitfalls.contains { $0.title.contains("进程级") })
        #expect(pitfalls.allSatisfy { !$0.detail.isEmpty })
    }

    @Test func guideMatrixCoversBothPlatforms() {
        let matrix = ClaudeAttributionHint.Guide.platformMatrix
        #expect(!matrix.isEmpty)
        #expect(matrix.allSatisfy { !$0.label.isEmpty && !$0.macOS.isEmpty && !$0.linux.isEmpty })
    }

    /// 归因是"在哪台机器配"最容易搞错的地方(daemon 常在远程),这句必须留着。
    @Test func guideSaysWhereToRunAndKeepsPrivacyCaveat() {
        #expect(ClaudeAttributionHint.Guide.whereToRun.contains("daemon"))
        #expect(ClaudeAttributionHint.Guide.privacy.contains("CLAUDE.md"))
    }

    @Test func grokUnidentifiedTrafficIsUnconfigured() {
        #expect(
            GrokAttributionHint.state(projects: [
                row("unidentified_project", kinds: ["grok_build"]),
            ]) == .unconfigured
        )
        #expect(
            GrokAttributionHint.state(projects: [
                row("sumpter", source: "workspace_local", kinds: ["grok_build"]),
            ]) == .configured
        )
        #expect(GrokAttributionHint.command.contains("grok-project-attribution.sh install"))
    }
}
