import Foundation
import Testing

@testable import SumpterCore

/// 回归:分页列表端点走 SQLite 投影快路径,不解 `payload_json`,因此行里没有
/// `codexMetadata` / `clientDeclared`,只有服务端算好的 `projectName` /
/// `projectSource`。客户端若只按前两者推导,翻页拿到的行会全部显示「未识别项目」——
/// 而 SSE 推送的同一批事件是好的(那条路径带完整字段),表现为「详情识别到项目,
/// 列表仍显示未识别」。
@Suite struct ProjectProjectionListRowTests {
    @Test func projectedRowIsAttributedWithoutCodexOrDeclared() {
        let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: nil,
            declared: nil,
            projectedName: "automode-proxy",
            projectedSource: "client_declared"
        )
        #expect(context?.name == "automode-proxy")
        #expect(context?.source == .clientDeclared)
    }

    /// 合成桶名要翻成中文再展示,不能把机器 token 直接显示给用户。
    @Test func projectedUnidentifiedBucketIsLocalized() {
        let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: nil,
            declared: nil,
            projectedName: "unidentified_project",
            projectedSource: "missing_workspace_metadata"
        )
        #expect(context?.name == RuntimeEventPresentation.unidentifiedProjectName)
        #expect(context?.source == .missingWorkspaceMetadata)
    }

    /// 服务端投影优先于客户端自行推导,避免两边算出不同结果。
    @Test func projectedValueWinsOverClientSideDerivation() {
        let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: nil,
            declared: ClientDeclaredMetadata(project: "client-loses"),
            projectedName: "server-wins",
            projectedSource: "workspace_local"
        )
        #expect(context?.name == "server-wins")
        #expect(context?.source == .workspaceLocal)
    }

    /// 没有投影值时(SSE 推送、单事件详情、旧 daemon)仍按完整字段推导。
    @Test func fallsBackToClientSideWhenProjectionAbsent() {
        let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: nil,
            declared: ClientDeclaredMetadata(project: "declared-only")
        )
        #expect(context?.name == "declared-only")
        #expect(context?.source == .clientDeclared)
    }

    /// 未知来源 token 不得崩,降级为「来源未记录」。
    @Test func unknownProjectedSourceDegradesGracefully() {
        let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: nil,
            declared: nil,
            projectedName: "some-project",
            projectedSource: "a_source_from_a_newer_daemon"
        )
        #expect(context?.source == .missingWorkspaceMetadata)
    }

    /// 列表行摘要(用户看到「未识别项目」的那一行)也必须吃投影值。
    @Test func listRowSummaryUsesProjection() {
        let summary = RuntimeEventPresentation.projectAttribution(
            eventKind: "client",
            metadata: nil,
            declared: nil,
            projectedName: "automode-proxy",
            projectedSource: "client_declared"
        )
        #expect(summary == "项目: automode-proxy（客户端声明）")
    }

    @Test func localUserFormatsWorkspaceAsNameAndLocalUser() {
        let summary = RuntimeEventPresentation.projectAttribution(
            eventKind: "client",
            metadata: nil,
            declared: nil,
            projectedName: "sumpter",
            projectedSource: "workspace_local",
            projectedLocalUser: "kkl"
        )
        #expect(summary == "sumpter 本地(kkl)")
    }

    @Test func codexSourceWorkspacePathFormatsAsLocalUser() throws {
        let metadata = try JSONDecoder().decode(
            CodexMetadata.self,
            from: Data(#"""
            {
              "workspaces": {".../claude/sumpter": {}},
              "sourceWorkspacePaths": ["/Users/kkl/Documents/claude/sumpter"]
            }
            """#.utf8)
        )
        let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: metadata,
            declared: nil
        )
        #expect(context?.name == "sumpter")
        #expect(context?.source == .workspaceLocal)
        #expect(context?.localUser == "kkl")
        #expect(context?.source.displayLabel(localUser: context?.localUser) == "本地(kkl)")
        #expect(
            RuntimeEventPresentation.projectAttribution(
                eventKind: "client",
                metadata: metadata
            ) == "sumpter 本地(kkl)"
        )
    }

    @Test func internalFeatureProjectionIsNotPresentedAsAProject() {
        let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: nil,
            declared: nil,
            projectedName: "internal_feature",
            projectedSource: "internal_feature",
            attributionScope: "internal_feature"
        )
        #expect(context?.name == "后台功能")
        #expect(context?.source == .internalFeature)
    }
}
