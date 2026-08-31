import Testing

@testable import SumpterApp

@Suite struct ClaudeAttributionInstallerTests {
    @Test func parsesHealthyInstallation() throws {
        let status = try ClaudeAttributionInstaller.parseStatus(
            """
            shell:            zsh
            rc:               /Users/test/.zshrc
            rc 内标记块:      已安装
            snippet:          /Users/test/.config/sumpter/claude-project-attribution.sh
            settings.json 键: 无(正确)
            备份:
              /Users/test/.zshrc.sumpter-bak-20260826-120000
            """)

        #expect(status.condition == .installed)
        #expect(status.shell == "zsh")
        #expect(status.rcPath == "/Users/test/.zshrc")
        #expect(status.backupCount == 1)
        #expect(status.canRemove)
        #expect(status.canRestore)
    }

    @Test func treatsOrphanedSnippetAsNotInstalled() throws {
        let status = try ClaudeAttributionInstaller.parseStatus(
            """
            shell:            zsh
            rc:               /Users/test/.zshrc
            rc 内标记块:      未安装
            snippet:          /Users/test/.config/sumpter/claude-project-attribution.sh
            settings.json 键: 无(正确)
            备份:
              (无)
            """)

        #expect(status.condition == .notInstalled)
        #expect(status.canRemove)
        #expect(!status.canRestore)
    }

    @Test func detectsMissingSnippetAndSettingsConflict() throws {
        let incomplete = try ClaudeAttributionInstaller.parseStatus(
            """
            shell:            bash
            rc:               /Users/test/.bash_profile
            rc 内标记块:      已安装
            snippet:          /Users/test/.config/sumpter/claude-project-attribution.sh (不存在)
            settings.json 键: 无(正确)
            备份:
              (无)
            """)
        #expect(incomplete.condition == .needsRepair)

        let blocked = try ClaudeAttributionInstaller.parseStatus(
            """
            shell:            zsh
            rc:               /Users/test/.zshrc (不存在)
            rc 内标记块:      未安装
            snippet:          /Users/test/.config/sumpter/claude-project-attribution.sh (不存在)
            settings.json 键: 存在(会覆盖 wrapper,必须删)
            备份:
              (无)
            """)
        #expect(blocked.condition == .blockedBySettings)
        #expect(blocked.rcPath == "/Users/test/.zshrc")
    }

    @Test func rejectsUnexpectedStatusOutput() {
        #expect(throws: ClaudeAttributionInstallerError.invalidStatusOutput) {
            try ClaudeAttributionInstaller.parseStatus("unexpected")
        }
    }
}
