import XCTest
@testable import SumpterCore

/// 覆盖运行事件展示逻辑:断开判定、消息友好化、池/状态/耗时文案。
final class RuntimeEventPresentationTests: XCTestCase {
    func testProjectAttributionDoesNotAssignNotificationsOrUpstreamAttempts() throws {
        XCTAssertEqual(
            RuntimeEventPresentation.projectAttribution(eventKind: "client", metadata: nil),
            "未识别项目 · 来源未记录"
        )
        let empty = try JSONDecoder().decode(CodexMetadata.self, from: Data("{}".utf8))
        XCTAssertEqual(
            RuntimeEventPresentation.projectAttribution(eventKind: "client", metadata: empty),
            "未识别项目 · 来源未记录"
        )
        XCTAssertNil(RuntimeEventPresentation.projectAttribution(eventKind: "upstream", metadata: empty))
        XCTAssertNil(RuntimeEventPresentation.projectAttribution(eventKind: "notify", metadata: nil))

        let workspace = try JSONDecoder().decode(
            CodexMetadata.self,
            from: Data(#"{"workspaces":{"/workspace/automode-proxy":{}}}"#.utf8)
        )
        XCTAssertEqual(
            RuntimeEventPresentation.projectAttribution(eventKind: "client", metadata: workspace),
            "项目: automode-proxy"
        )
    }

    /// Claude Code 不上行 workspace,项目只能来自 X-Sumpter-* 声明。归因必须落到声明值、
    /// 来源与 Codex 的本地项目区分开,且 Codex 的结构化 workspace 在同时存在时优先。
    func testProjectAttributionFallsBackToClientDeclaredHeaders() throws {
        let declared = ClientDeclaredMetadata(
            project: "automode-proxy",
            workspace: ".../.claude/automode-proxy",
            gitRemote: "https://github.com/domoxiaojun/sumpter.git"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.projectAttribution(
                eventKind: "client",
                metadata: nil,
                declared: declared
            ),
            "项目: automode-proxy"
        )
        let context = try XCTUnwrap(
            RuntimeEventPresentation.projectContext(
                eventKind: "client",
                metadata: nil,
                declared: declared
            )
        )
        XCTAssertEqual(context.name, "automode-proxy")
        XCTAssertEqual(context.source, .workspaceLocal)
        XCTAssertEqual(context.source.rawValue, "workspace_local")
        XCTAssertEqual(context.source.label, "本地项目")
        XCTAssertEqual(context.source.displayLabel(localUser: "kkl"), "本地(kkl)")
        XCTAssertEqual(
            context.detail,
            ".../.claude/automode-proxy · https://github.com/domoxiaojun/sumpter.git"
        )

        let workspace = try JSONDecoder().decode(
            CodexMetadata.self,
            from: Data(#"{"workspaces":{"/workspace/codex-wins":{}}}"#.utf8)
        )
        let both = try XCTUnwrap(
            RuntimeEventPresentation.projectContext(
                eventKind: "client",
                metadata: workspace,
                declared: declared
            )
        )
        XCTAssertEqual(both.name, "codex-wins")
        XCTAssertEqual(both.source, .workspaceLocal)

        XCTAssertEqual(
            RuntimeEventPresentation.projectAttribution(
                eventKind: "client",
                metadata: nil,
                declared: ClientDeclaredMetadata()
            ),
            "未识别项目 · 来源未记录"
        )
        XCTAssertNil(
            RuntimeEventPresentation.projectAttribution(
                eventKind: "upstream",
                metadata: nil,
                declared: declared
            )
        )
    }

    func testCodexMetadataSummaryAndJSONExposeSubagentEvidence() throws {
        let data = Data(#"""
        {
          "threadID":"thread-child",
          "agentName":"/root/worker",
          "turnID":"turn-1",
          "parentThreadID":"thread-parent",
          "subagentHeader":"collab_spawn",
          "subagentKind":"thread_spawn",
          "isSubagent":true,
          "workspaces":{"/workspace":{"hasChanges":true}},
          "toolNamespacesInfo":{"shell":{"functions":{"run":{"name":"run","deferred":true}}}}
        }
        """#.utf8)
        let metadata = try JSONDecoder().decode(CodexMetadata.self, from: data)
        XCTAssertEqual(metadata.agentName, "/root/worker")
        XCTAssertEqual(RuntimeEventPresentation.codexSummary(metadata), "Codex · 子代理(thread_spawn) · /root/worker")
        let json = try XCTUnwrap(RuntimeEventPresentation.codexJSON(metadata))
        XCTAssertTrue(json.contains("thread-child"))
        XCTAssertTrue(json.contains("thread-parent"))
        XCTAssertTrue(json.contains("collab_spawn"))
        XCTAssertTrue(json.contains(#""agentName" : "/root/worker""#))
        XCTAssertTrue(json.contains("toolNamespacesInfo"))
    }

    func testCodexMetadataSummaryKeepsNonIdentityFieldsVisible() throws {
        let data = Data(#"{"agentName":"/root","originator":"codex_cli_rs"}"#.utf8)
        let metadata = try JSONDecoder().decode(CodexMetadata.self, from: data)
        XCTAssertFalse(metadata.isEmpty)
        XCTAssertEqual(RuntimeEventPresentation.codexSummary(metadata), "Codex · 未发现子代理证据 · /root")
        let json = try XCTUnwrap(RuntimeEventPresentation.codexJSON(metadata))
        XCTAssertTrue(json.contains("codex_cli_rs"))
        XCTAssertFalse(json.contains("workspaces"), "空集合编码应与 Rust wire 一样省略")
    }

    func testCodexGuardianAndContextFieldsSurviveExport() throws {
        let data = Data(#"{"subagentHeader":"guardian","windowNumber":0,"contextWindowID":"context","forkedFromOrdinalExclusive":42,"turnTrigger":"user_input","historyIngestRequested":false}"#.utf8)
        let metadata = try JSONDecoder().decode(CodexMetadata.self, from: data)
        XCTAssertEqual(metadata.windowNumber, 0)
        XCTAssertEqual(metadata.contextWindowID, "context")
        XCTAssertEqual(metadata.forkedFromOrdinalExclusive, 42)
        XCTAssertEqual(metadata.turnTrigger, "user_input")
        XCTAssertEqual(metadata.historyIngestRequested, false)
        XCTAssertEqual(RuntimeEventPresentation.codexSummary(metadata), "Codex · Guardian 安全审查")
        XCTAssertEqual(try JSONDecoder().decode(CodexMetadata.self, from: JSONEncoder().encode(metadata)), metadata)
    }

    func testCodexMetadataRemainsOptionalForLegacyEvents() throws {
        let event = try JSONDecoder().decode(RuntimeEvent.self, from: Data(#"""
        {
          "id":"legacy", "timestamp":0, "kind":"client", "statusCode":200,
          "durationMS":1, "failover":false
        }
        """#.utf8))
        XCTAssertNil(event.codexMetadata)
        XCTAssertNil(event.grokMetadata)
    }

    func testGrokMetadataSummaryAndJSONExposeSamplingHeaders() throws {
        let data = Data(#"""
        {
          "sessionID":"sess-1",
          "convID":"conv-1",
          "requestID":"req-1",
          "clientIdentifier":"grok-shell",
          "clientVersion":"0.2.119",
          "clientMode":"interactive",
          "turnIndex":"3"
        }
        """#.utf8)
        let metadata = try JSONDecoder().decode(GrokMetadata.self, from: data)
        XCTAssertEqual(RuntimeEventPresentation.grokSummary(metadata), "Grok · grok-shell · 0.2.119 · interactive · 会话 sess-1 · 对话 conv-1")
        let json = try XCTUnwrap(RuntimeEventPresentation.grokJSON(metadata))
        XCTAssertTrue(json.contains("sess-1"))
        XCTAssertTrue(json.contains("grok-shell"))
        XCTAssertEqual(metadata.turnIndex, "3")
    }

    func testEmptyCodexMetadataFromTraceparentIsNotIdentity() throws {
        let metadata = try JSONDecoder().decode(
            CodexMetadata.self,
            from: Data(#"{"sources":["headers"],"redactedFields":["traceparent"]}"#.utf8)
        )
        XCTAssertFalse(metadata.hasRequestIdentity)
        XCTAssertNil(RuntimeEventPresentation.codexSummary(metadata))
    }

    func testFriendlyMessageBasics() {
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "upstream", statusCode: 502, failover: false, message: "timeout"),
            "响应超时"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 200, failover: false, message: nil),
            ""
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "upstream", statusCode: 200, failover: true, message: nil),
            "故障转移"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "upstream", statusCode: 200, failover: false, message: "pinned 1.2.3.4"),
            ""
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 400, failover: false, message: "自定义消息原样透传"),
            "自定义消息原样透传"
        )
    }

    func testFriendlyMessageClientDisconnect() {
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 499, failover: false, message: "client_disconnected: whatever"),
            "客户端断开/取消"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 400,
                failover: false,
                message: nil,
                outcome: .cancelled,
                failureKind: .clientCancelled
            ),
            "客户端断开/取消"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "upstream",
                statusCode: 502,
                failover: false,
                message: nil,
                outcome: .failed,
                failureKind: .connectionFailed
            ),
            "连接失败（未收到响应头）"
        )
    }

    /// 现行词表全 token 覆盖(docs/architecture.md §5.1)。
    /// 清单与 crates/sumpter-proxy/tests/engine.rs 的 `MESSAGE_TOKEN_PREFIXES` 互钉:
    /// 引擎新增 token 而这里没映射,对应断言会以「原样透传」失败暴露。
    func testFriendlyMessageVocabulary() {
        func friendly(
            _ message: String?,
            kind: String = "upstream",
            status: Int = 200,
            failover: Bool = false,
            outcome: RuntimeEventOutcome? = nil,
            phase: RuntimeEventPhase? = nil,
            streamTrace: StreamTrace? = nil
        ) -> String {
            RuntimeEventPresentation.friendlyMessage(
                kind: kind, statusCode: status, failover: failover, message: message,
                outcome: outcome, phase: phase, streamTrace: streamTrace
            )
        }
        XCTAssertEqual(friendly("timeout", status: 502), "响应超时")
        XCTAssertEqual(friendly("connection failed: tcp connect error", status: 502), "连接失败")
        XCTAssertEqual(friendly("invalid response: bad header", status: 502), "上游响应无法解析")
        XCTAssertEqual(friendly("stream interrupted: timeout", kind: "client", status: 502), "流中断(吐字超时)")
        XCTAssertEqual(friendly("stream interrupted: connection reset", status: 200), "流中断(上游断流)")
        XCTAssertEqual(friendly("upstream_retryable_status", kind: "client", status: 503), "上游持续返回可重试错误")
        XCTAssertEqual(friendly("inbound_auth_required", kind: "client", status: 401), "入站认证失败")
        XCTAssertEqual(friendly("openai_tools_unsupported", status: 400), "openai 协议入口不支持工具调用,已跳过")
        XCTAssertEqual(friendly("body is not JSON", kind: "client", status: 400), "请求体不是合法 JSON")
        XCTAssertEqual(friendly("body is not an object", kind: "client", status: 400), "请求体不是合法 JSON")
        XCTAssertEqual(friendly("inbound_convert_failed: missing model", kind: "client", status: 400), "OpenAI 兼容请求无法转换")
        XCTAssertEqual(friendly("no pool accepts model gpt-9", kind: "client", status: 400), "没有匹配该模型的池")
        XCTAssertEqual(friendly("pool not found: x", kind: "client", status: 400), "目标池不存在")
        XCTAssertEqual(friendly("no enabled endpoint in pool primary", kind: "client", status: 400), "目标池无可用入口")
        XCTAssertEqual(friendly("feature rule not found: r1", kind: "client", status: 400), "分流规则不存在")
        XCTAssertEqual(friendly("passthrough responses", outcome: .succeeded), "请求成功")
        XCTAssertEqual(
            friendly("bridge openai-responses; 命中路由规则", outcome: .succeeded, streamTrace: StreamTrace(chunkCount: 1)),
            "流式输出完成"
        )
        XCTAssertEqual(
            friendly(
                "passthrough chat",
                outcome: .succeeded,
                phase: .completed,
                streamTrace: StreamTrace(chunkCount: 2)
            ),
            "流式输出完成"
        )
        XCTAssertEqual(friendly("bridge openai"), "openai 桥接")
        XCTAssertEqual(friendly("bridge openai-responses"), "openai-responses 桥接")
        XCTAssertEqual(friendly("deferred_rounds 3", kind: "client"), "上游重跑 3 轮")
        XCTAssertEqual(friendly("unmatched_no_tools", kind: "client"), "无工具请求(用途未识别)")
    }

    func testFriendlyMessageCombinedTokens() {
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 200,
                failover: false,
                message: "bridge openai; deferred_rounds 2"
            ),
            "openai 桥接 · 上游重跑 2 轮"
        )
        // 信息 token + 错误 token 组合:失败解释已在,不叠加状态码合成段。
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "upstream",
                statusCode: 502,
                failover: false,
                message: "connection failed: boom"
            ),
            "连接失败"
        )
    }

    func testForwardingModeDisplayUsesRecordedEngineTokens() {
        XCTAssertEqual(
            RuntimeEventPresentation.forwardingModeDisplay("passthrough responses; unmatched_no_tools"),
            "Responses 原生适配"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.forwardingModeDisplay("bridge openai-responses"),
            "Responses ↔ Anthropic 桥接"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.forwardingModeDisplay("bridge openai"),
            "Chat Completions ↔ Anthropic 桥接"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.forwardingModeDisplay("passthrough images-edits"),
            "Images Edits 原生适配"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.forwardingModeDisplay("passthrough alpha-search"),
            "Codex Alpha Search 原生适配"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.forwardingModeDisplay("passthrough completions"),
            "Legacy Completions 原生适配"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.forwardingModeDisplay("passthrough claude-count-tokens"),
            "Claude Token Count 原生适配"
        )
        XCTAssertNil(RuntimeEventPresentation.forwardingModeDisplay(nil))
        XCTAssertNil(RuntimeEventPresentation.forwardingModeDisplay("deferred_rounds 3"))
    }

    func testProtocolPathDisplaysRecordedSourceTargetAndMode() {
        XCTAssertEqual(
            RuntimeEventPresentation.protocolPath(
                sourceFormat: .openaiResponses,
                targetFormat: .anthropic,
                routeMode: .translated
            ),
            "OpenAI Responses → Anthropic Messages · 协议转换"
        )
    }

    func testToolOnlySuccessfulEventStillShowsDiagnosticsSection() {
        let toolOnly = RuntimeEvent(
            kind: "client",
            statusCode: 200,
            durationMS: 1_000,
            toolCalls: ["collaboration.spawn_agent"],
            outcome: .succeeded,
            phase: .completed
        )
        XCTAssertTrue(RuntimeEventPresentation.hasFailureToolOrStreamDiagnostics(toolOnly))

        let plainSuccess = RuntimeEvent(
            kind: "client",
            statusCode: 200,
            durationMS: 1_000,
            outcome: .succeeded,
            phase: .completed
        )
        XCTAssertFalse(RuntimeEventPresentation.hasFailureToolOrStreamDiagnostics(plainSuccess))
    }

    func testOutcomeDisplayDoesNotInferFailureWhileStreaming() {
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(nil, statusCode: 0, inFlight: true),
            "传输中"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(nil, statusCode: 200, inFlight: true),
            "传输中"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(nil, statusCode: 0),
            "最终结果未上报"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(nil, statusCode: 0, upstreamStatusCode: 200),
            "最终结果未上报"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(nil, statusCode: 0, upstreamStatusCode: 499),
            "最终结果未上报"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 0,
                failover: false,
                message: nil,
                upstreamStatusCode: 503
            ),
            ""
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 0,
                failover: false,
                message: nil,
                upstreamStatusCode: 499
            ),
            ""
        )
    }

    func testOutcomeDisplayTreatsNotifyWithoutOutcomeAsNotApplicable() {
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(nil, statusCode: 200, eventKind: "notify"),
            "不适用（通知事件）"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(.succeeded, statusCode: 200, eventKind: "notify"),
            "成功",
            "通知若未来显式记录最终结果，应优先展示该协议事实"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.outcomeDisplay(nil, statusCode: 200),
            "最终结果未上报",
            "HTTP 成功不能代替明确的最终执行结果"
        )
    }

    func testCurrentMessagesArePreservedWithoutHistoricalClientRewrites() {
        let raw = "passthrough responses; unmatched_no_tools"
        XCTAssertEqual(
            RuntimeEventPresentation.messageForDisplay(raw, clientKind: .codex),
            raw
        )
        XCTAssertEqual(
            RuntimeEventPresentation.messageForDisplay(raw, clientKind: .openaiCompat),
            raw
        )
        XCTAssertEqual(
            RuntimeEventPresentation.messageForDisplay(raw, clientKind: .claudeCode),
            raw
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 200,
                failover: false,
                message: raw,
                outcome: .succeeded,
                phase: .completed,
                clientKind: .codex
            ),
            "请求成功"
        )
    }

    func testStructuredFailureSemanticsDistinguishEvery502Source() {
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 502,
                failover: false,
                message: "connection failed: tcp reset",
                outcome: .failed,
                failureKind: .connectionFailed
            ),
            "代理连接上游失败"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 502,
                failover: false,
                message: "timeout",
                outcome: .failed,
                failureKind: .responseTimeout,
                timeoutMS: 200_000
            ),
            "首响应超时（有效阈值 200 秒）"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "upstream",
                statusCode: 502,
                failover: false,
                message: nil,
                outcome: .failed,
                failureKind: .upstreamHTTPStatus,
                upstreamStatusCode: 502
            ),
            "上游返回 HTTP 502(网关错误)"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.statusDisplay(
                200,
                outcome: .failed,
                failureKind: .streamInterrupted
            ),
            "200 · 流中断"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.statusDisplay(
                200,
                outcome: .failed,
                failureKind: .upstreamResponseFailed
            ),
            "200 · 上游协议失败"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.statusDisplay(
                200,
                outcome: .failed,
                failureKind: .upstreamResponseIncomplete
            ),
            "200 · 响应未完成"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 200,
                failover: false,
                message: nil,
                outcome: .failed,
                failureKind: .upstreamResponseIncomplete
            ),
            "上游响应未完整完成"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(
                kind: "client",
                statusCode: 200,
                failover: false,
                message: nil,
                outcome: .failed,
                failureKind: .upstreamResponseFailed
            ),
            "上游以失败状态结束响应"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.messageSeverity(
                statusCode: 200,
                failover: false,
                message: nil,
                outcome: .failed,
                failureKind: .streamInterrupted
            ),
            .warning
        )
        XCTAssertEqual(
            RuntimeEventPresentation.statusDisplay(200, outcome: .cancelled),
            "200 · 已取消",
            "取消不能遮蔽已经收到的上游 HTTP 状态"
        )
        XCTAssertEqual(RuntimeEventPresentation.statusDisplay(499, outcome: .cancelled), "取消")
        XCTAssertEqual(
            RuntimeEventPresentation.failureKindDisplay(.upstreamResponseIncomplete),
            "上游响应未完整"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.failureKindDisplay(.upstreamResponseFailed),
            "上游响应失败"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.failureKindDisplay(.clientRequestRejected),
            "客户端请求被拒绝"
        )
        XCTAssertEqual(RuntimeFailureKind.clientRequestRejected.rawValue, "client_request_rejected")
        XCTAssertEqual(
            RuntimeEventPresentation.statusDisplay(499, outcome: .failed, failureKind: .upstreamHTTPStatus),
            "499",
            "显式的上游 HTTP 499 不是客户端取消"
        )
    }

    func testRuntimeEventKeepsHttpStatusIndependentFromFinalResult() throws {
        let legacy = #"{"id":"OLD","timestamp":1.5,"kind":"client","statusCode":200,"durationMS":5,"failover":false}"#.data(using: .utf8)!
        let oldEvent = try JSONDecoder().decode(RuntimeEvent.self, from: legacy)
        XCTAssertNil(oldEvent.outcome)
        XCTAssertNil(oldEvent.failureKind)
        XCTAssertNil(oldEvent.effectiveModel)
        XCTAssertFalse(oldEvent.isSucceeded)
        XCTAssertFalse(oldEvent.isFailed)

        let current = RuntimeEvent(
            id: "REQUEST-1",
            timestamp: Date(timeIntervalSinceReferenceDate: 2),
            kind: "client",
            clientModel: "client-model",
            upstreamModel: "upstream-model",
            effectiveModel: "route-model",
            statusCode: 200,
            durationMS: 10,
            toolCalls: ["shell_command", "mcp:files/read"],
            streamTrace: nil,
            outcome: .failed,
            failureDetail: "peer reset",
            failureKind: .streamInterrupted,
            failurePhase: .responseStream,
            requestID: "REQUEST-1",
            upstreamStatusCode: 200
        )
        let decoded = try JSONDecoder().decode(RuntimeEvent.self, from: JSONEncoder().encode(current))
        XCTAssertEqual(decoded, current)
        XCTAssertEqual(decoded.effectiveModel, "route-model")
        XCTAssertEqual(decoded.toolCalls ?? [], ["shell_command", "mcp:files/read"])
        XCTAssertTrue(decoded.isFailed)
        XCTAssertFalse(decoded.isSucceeded)

        let completedWithLegacyZero = RuntimeEvent(
            kind: "client",
            statusCode: 0,
            durationMS: 1_000,
            phase: .completed,
            upstreamStatusCode: 200
        )
        XCTAssertEqual(completedWithLegacyZero.effectiveHTTPStatusCode, 0)
        XCTAssertFalse(completedWithLegacyZero.isSucceeded)
        XCTAssertFalse(completedWithLegacyZero.isFailed)

        let failedWithLegacyZero = RuntimeEvent(
            kind: "client",
            statusCode: 0,
            durationMS: 1_000,
            phase: .completed,
            upstreamStatusCode: 503
        )
        XCTAssertFalse(failedWithLegacyZero.isFailed)
        XCTAssertFalse(failedWithLegacyZero.isSucceeded)

        let cancelledWithLegacyZero = RuntimeEvent(
            kind: "client",
            statusCode: 0,
            durationMS: 1_000,
            phase: .completed,
            upstreamStatusCode: 499
        )
        XCTAssertFalse(cancelledWithLegacyZero.isCancelled)

        let inFlightWithHeaders = RuntimeEvent(
            kind: "client",
            statusCode: 200,
            durationMS: 1_000,
            phase: .inFlight
        )
        XCTAssertTrue(inFlightWithHeaders.isInFlight)
        XCTAssertFalse(inFlightWithHeaders.isSucceeded)
        XCTAssertFalse(inFlightWithHeaders.isFailed)
    }

    func testRequestChainGroupsClientAndUpstreamAttemptsByRequestID() {
        let client = RuntimeEvent(
            id: "CLIENT",
            timestamp: Date(timeIntervalSinceReferenceDate: 30),
            kind: "client",
            statusCode: 200,
            durationMS: 3_000,
            requestID: "REQUEST"
        )
        let firstAttempt = RuntimeEvent(
            id: "UPSTREAM-1",
            timestamp: Date(timeIntervalSinceReferenceDate: 10),
            kind: "upstream",
            statusCode: 429,
            durationMS: 100,
            requestID: "REQUEST"
        )
        let secondAttempt = RuntimeEvent(
            id: "UPSTREAM-2",
            timestamp: Date(timeIntervalSinceReferenceDate: 20),
            kind: "upstream",
            statusCode: 200,
            durationMS: 200,
            requestID: "REQUEST"
        )
        let unrelated = RuntimeEvent(
            id: "OTHER",
            timestamp: Date(timeIntervalSinceReferenceDate: 40),
            kind: "client",
            statusCode: 200,
            durationMS: 1,
            requestID: "OTHER-REQUEST"
        )

        let chain = RuntimeEvent.requestChain(
            [firstAttempt, unrelated, client, secondAttempt],
            selectedID: firstAttempt.id
        )
        XCTAssertEqual(chain.map(\.id), ["CLIENT", "UPSTREAM-2", "UPSTREAM-1"])
        XCTAssertTrue(chain.allSatisfy { $0.requestGroupID == "REQUEST" })

        let legacy = RuntimeEvent(
            id: "LEGACY",
            timestamp: .distantPast,
            kind: "client",
            statusCode: 200,
            durationMS: 1
        )
        XCTAssertEqual(RuntimeEvent.requestChain([legacy], selectedID: legacy.id), [legacy])
        XCTAssertTrue(RuntimeEvent.requestChain([legacy], selectedID: "missing").isEmpty)
    }

    /// clientKind 的 wire 值必须与 sumpterd 的 serde 输出逐字一致 ——
    /// 这两边是独立实现,值一漂 UI 就退回「-」而且没人会注意到。
    func testClientKindWireValuesMatchEngine() throws {
        for (wire, expected) in [
            ("claude_code", ClientKind.claudeCode),
            ("codex", .codex),
            ("grok_build", .grokBuild),
            ("openai_compat", .openaiCompat),
            ("unknown", .unknown)
        ] {
            let json = #"{"id":"E","timestamp":1,"kind":"client","statusCode":200,"durationMS":1,"failover":false,"clientKind":"\#(wire)"}"#
            let event = try JSONDecoder().decode(RuntimeEvent.self, from: Data(json.utf8))
            XCTAssertEqual(event.clientKind, expected, "wire 值 \(wire) 必须解成 \(expected)")
            XCTAssertEqual(expected.rawValue, wire)
        }

        // 旧 stats.json 没这个键:解成 nil,且不能凭空编码出来。
        let legacy = #"{"id":"OLD","timestamp":1,"kind":"client","statusCode":200,"durationMS":1,"failover":false}"#
        let old = try JSONDecoder().decode(RuntimeEvent.self, from: Data(legacy.utf8))
        XCTAssertNil(old.clientKind)
        let reencoded = String(decoding: try JSONEncoder().encode(old), as: UTF8.self)
        XCTAssertFalse(reencoded.contains("clientKind"), "旧事件不得凭空长出 clientKind 键")
    }

    /// 中文展示名:「未知客户端」是「UA 认不出」,与旧事件的「没记过」是两回事。
    func testClientKindDisplayNames() {
        XCTAssertEqual(ClientKind.claudeCode.displayName, "Claude Code")
        XCTAssertEqual(ClientKind.codex.displayName, "Codex")
        XCTAssertEqual(ClientKind.grokBuild.displayName, "Grok Build")
        XCTAssertEqual(ClientKind.openaiCompat.displayName, "OpenAI 兼容客户端")
        XCTAssertEqual(ClientKind.unknown.displayName, "未知客户端")
        XCTAssertEqual(Set(ClientKind.allCases.map(\.displayName)).count, ClientKind.allCases.count)
    }

    func testTokenCountDisplayUsesGroupingAndPreservesMissingValues() {
        XCTAssertEqual(RuntimeEventPresentation.tokenCountDisplay(nil), "—")
        XCTAssertEqual(RuntimeEventPresentation.tokenCountDisplay(0), "0")
        XCTAssertEqual(RuntimeEventPresentation.tokenCountDisplay(1_000), "1,000")
        XCTAssertEqual(RuntimeEventPresentation.tokenCountDisplay(418_003_364), "418,003,364")
    }

    func testFriendlyMessageStatusCodeSynthesis() {
        // 无消息的失败行按状态码合成——「消息列全空」的治本兜底。
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "upstream", statusCode: 429, failover: false, message: nil),
            "上游返回 429(限流)"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 404, failover: false, message: nil),
            "上游返回 404(不存在)"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 402, failover: false, message: nil),
            "上游返回 402(余额不足)"
        )
        // 未知状态码不加括注。
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 418, failover: false, message: nil),
            "上游返回 418"
        )
        // 成功行不合成;failover 空消息标记保留。
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 200, failover: false, message: nil),
            ""
        )
        XCTAssertEqual(
            RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 200, failover: true, message: nil),
            "故障转移"
        )
    }

    func testStatusAndDurationDisplay() {
        XCTAssertEqual(RuntimeEventPresentation.statusDisplay(200), "200")
        XCTAssertEqual(RuntimeEventPresentation.statusDisplay(499), "499")
        XCTAssertEqual(
            RuntimeEventPresentation.statusDisplay(0, upstreamStatusCode: 200),
            "未收到响应头"
        )

        XCTAssertEqual(RuntimeEventPresentation.durationDisplay(850), "850ms")
        XCTAssertEqual(RuntimeEventPresentation.durationDisplay(12_500), "12.5s")
    }

    func testInFlightDisplay() {
        // 进行中统一标成 streaming；有开始时间时显示实时累计秒数。
        XCTAssertEqual(RuntimeEventPresentation.statusDisplay(0, inFlight: true), "streaming")
        XCTAssertEqual(RuntimeEventPresentation.statusDisplay(200, inFlight: true), "streaming")
        XCTAssertEqual(RuntimeEventPresentation.durationDisplay(0, inFlight: true), "streaming…")
        XCTAssertEqual(RuntimeEventPresentation.durationDisplay(82_100, inFlight: true), "streaming…")
        let started = Date(timeIntervalSince1970: 100)
        let now = Date(timeIntervalSince1970: 112.34)
        XCTAssertEqual(
            RuntimeEventPresentation.durationDisplay(0, inFlight: true, startedAt: started, now: now),
            "12.3s"
        )
        // 已完成路径不受默认参数影响。
        XCTAssertEqual(RuntimeEventPresentation.statusDisplay(0, inFlight: false), "未收到响应头")
        XCTAssertEqual(RuntimeEventPresentation.durationDisplay(850, inFlight: false), "850ms")
    }

    func testCacheHitRateUsesProtocolNormalizedProcessedInput() {
        XCTAssertEqual(
            RuntimeEventPresentation.cacheHitRateDisplay(
                cacheReadInputTokens: 384_166_656,
                processedInputTokens: 431_805_163,
                tokenAccountingSemantics: "subset",
                cacheReadPresence: 1
            ),
            "89.0%"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.cacheHitRateDisplay(
                cacheReadInputTokens: 4,
                processedInputTokens: 18,
                tokenAccountingSemantics: "independent",
                cacheReadPresence: 1
            ),
            "22.2%"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.cacheHitRateDisplay(
                cacheReadInputTokens: 64,
                processedInputTokens: 118,
                tokenAccountingSemantics: "subset,independent",
                cacheReadPresence: 1
            ),
            "54.2%"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.cacheHitRateDisplay(
                cacheReadInputTokens: 4,
                processedInputTokens: 18,
                tokenAccountingSemantics: "unknown",
                cacheReadPresence: 1
            ),
            "—"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.cacheHitRateDisplay(
                cacheReadInputTokens: 0,
                processedInputTokens: 10,
                tokenAccountingSemantics: "subset",
                inputPresence: 1,
                cacheReadPresence: 0
            ),
            "—"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.cacheHitRateDisplay(
                cacheReadInputTokens: 4,
                processedInputTokens: 18,
                tokenAccountingSemantics: "subset",
                inputPresence: 0,
                cacheReadPresence: 1
            ),
            "—"
        )
        XCTAssertEqual(RuntimeEventPresentation.tokenCountDisplay(431_805_163), "431,805,163")
    }

    /// 耗时列的核心价值:同样是 91s 总时长,「首字节 2.1s」和「首字节 85s」是两回事。
    func testDurationWithTTFB() {
        // 快首字节 + 长输出:正常。
        XCTAssertEqual(
            RuntimeEventPresentation.durationWithTTFB(ttfbMS: 2_100, durationMS: 91_300),
            "2.1s → 91.3s"
        )
        // 慢首字节:上游在排队,总时长几乎全花在等。
        XCTAssertEqual(
            RuntimeEventPresentation.durationWithTTFB(ttfbMS: 85_400, durationMS: 91_300),
            "85.4s → 91.3s"
        )
        // 亚秒首字节按毫秒显示。
        XCTAssertEqual(
            RuntimeEventPresentation.durationWithTTFB(ttfbMS: 320, durationMS: 4_500),
            "320ms → 4.5s"
        )
        // 旧事件(无 TTFB)回退到只显示总时长,不出现空箭头。
        XCTAssertEqual(
            RuntimeEventPresentation.durationWithTTFB(ttfbMS: nil, durationMS: 91_300),
            "91.3s"
        )
        // 进行中:首字节已定格,总时长实时走。
        let started = Date(timeIntervalSince1970: 100)
        let now = Date(timeIntervalSince1970: 145.6)
        XCTAssertEqual(
            RuntimeEventPresentation.durationWithTTFB(
                ttfbMS: 2_100, durationMS: 0, inFlight: true, startedAt: started, now: now
            ),
            "2.1s → 45.6s"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.durationWithTTFB(ttfbMS: nil, durationMS: 0, inFlight: true),
            "streaming…"
        )
        XCTAssertEqual(
            RuntimeEventPresentation.durationWithTTFB(
                ttfbMS: 8_000,
                durationMS: 0,
                inFlight: true,
                startedAt: started,
                now: Date(timeIntervalSince1970: 101)
            ),
            "8.0s → streaming…"
        )
    }

    func testSlowTTFBThreshold() {
        XCTAssertFalse(RuntimeEventPresentation.isSlowTTFB(nil))
        XCTAssertFalse(RuntimeEventPresentation.isSlowTTFB(2_100))
        XCTAssertFalse(RuntimeEventPresentation.isSlowTTFB(14_999))
        XCTAssertTrue(RuntimeEventPresentation.isSlowTTFB(15_000))
        XCTAssertTrue(RuntimeEventPresentation.isSlowTTFB(85_400))
    }

    /// 消息列分级:成功但有代价的行要能一眼看见,一次打通的行保持安静。
    func testMessageSeverity() {
        func severity(
            status: Int = 200,
            failover: Bool = false,
            message: String? = nil,
            ttfbMS: Int? = nil,
            inFlight: Bool = false
        ) -> RuntimeEventPresentation.MessageSeverity {
            RuntimeEventPresentation.messageSeverity(
                statusCode: status,
                failover: failover,
                message: message,
                ttfbMS: ttfbMS,
                inFlight: inFlight
            )
        }
        // 一次打通:无消息,不制造噪音。
        XCTAssertEqual(severity(), .none)
        XCTAssertEqual(severity(ttfbMS: 2_100), .none)
        // 中性补充信息。
        XCTAssertEqual(severity(message: "bridge openai"), .info)
        XCTAssertEqual(severity(message: "deferred_rounds 2"), .info)
        // 成功但有代价。
        XCTAssertEqual(severity(failover: true), .warning)
        XCTAssertEqual(severity(message: "deferred_rounds 3"), .warning)
        XCTAssertEqual(severity(message: "deferred_rounds 7"), .warning)
        XCTAssertEqual(severity(ttfbMS: 85_400), .warning)
        XCTAssertEqual(severity(message: "unmatched_no_tools"), .warning)
        XCTAssertEqual(severity(message: "deferred_rounds 6"), .warning)
        // 失败行。
        XCTAssertEqual(severity(status: 502, message: "upstream_retryable_status"), .warning)
        // 499 是用户主动取消,不是异常。
        XCTAssertEqual(severity(status: 499, message: "client_disconnected"), .info)
        // 进行中不预判失败(status 还是 0/200)。
        XCTAssertEqual(severity(status: 0, inFlight: true), .none)
    }
}
