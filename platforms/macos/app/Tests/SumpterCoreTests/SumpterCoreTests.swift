import Foundation
import XCTest
@testable import SumpterCore

final class SumpterCoreTests: XCTestCase {
    func testWindowLifecycleKeepsRegularPolicyWhileSwitchingApps() {
        var state = AppWindowLifecycleState.accessory
        state = state.applying(.opened)
        XCTAssertEqual(state, .regular)
        // App deactivation has no lifecycle event: the state remains regular,
        // so Cmd+Tab can return to the visible settings window.
        XCTAssertEqual(state, .regular)
        XCTAssertEqual(state.applying(.hidden), .accessory)
        XCTAssertEqual(state.applying(.closed), .accessory)
    }

    func testRuntimeTimestampUsesAppleReferenceSeconds() {
        let date = Date(timeIntervalSince1970: 1_756_000_000)
        let apple = RuntimeEvent.Timestamp.appleReferenceSeconds(date)
        XCTAssertEqual(RuntimeEvent.Timestamp.date(from: apple).timeIntervalSince1970, date.timeIntervalSince1970, accuracy: 0.001)
        XCTAssertEqual(RuntimeEvent.Timestamp.date(from: 1_000_000_000).timeIntervalSinceReferenceDate, 1_000_000_000, accuracy: 0.001)
    }

    private func webFetchPrompt(content: String = "example page") -> String {
        """

        Web page content:
        ---
        \(content)
        ---

        summarize it

        Provide a concise response based only on the content above. In your response:
         - Enforce a strict 125-character maximum for quotes from any source document.
         - Never produce or reproduce exact song lyrics.
        """
    }

    private func webSearchRequest(query: String = "swift concurrency") -> RoutingRequest {
        RoutingRequest(
            model: "claude-opus-5",
            system: .array([
                .object(["type": .string("text"), "text": .string("You are Claude Code, Anthropic's official CLI for Claude.")]),
                .object(["type": .string("text"), "text": .string("You are an assistant for performing a web search tool use")])
            ]),
            messages: [AnthropicMessage(role: "user", content: .string("Perform a web search for the query: \(query)"))],
            tools: [["type": .string("web_search_20250305"), "name": .string("web_search")]],
            raw: ["tool_choice": .object(["type": .string("tool"), "name": .string("web_search")])]
        )
    }

    private func webFetchRequest(content: String = "example page") -> RoutingRequest {
        RoutingRequest(
            model: "claude-opus-5",
            system: .string("You are Claude Code, Anthropic's official CLI for Claude."),
            messages: [AnthropicMessage(role: "user", content: .string(webFetchPrompt(content: content)))]
        )
    }

    private func classifierRequest(transcript: String = "Bash {\"command\":\"git status\"}") -> RoutingRequest {
        RoutingRequest(
            model: "claude-opus-5",
            system: .array([
                .object(["type": .string("text"), "text": .string("x-anthropic-billing-header: cc_version=2.1.220")]),
                .object(["type": .string("text"), "text": .string("You are a security monitor for autonomous AI coding agents.\n\n## Context")])
            ]),
            messages: [
                AnthropicMessage(role: "user", content: .array([
                    .object(["type": .string("text"), "text": .string("<transcript>\n\(transcript)\n</transcript>")]),
                    .object(["type": .string("text"), "text": .string("Err on the side of blocking.")])
                ]))
            ],
            raw: ["stop_sequences": .array([.string("</block>")])]
        )
    }

    private func sessionTitleRequest() -> RoutingRequest {
        RoutingRequest(
            model: "gpt-5.6-luna(high)",
            system: .array([
                .object(["type": .string("text"), "text": .string("x-anthropic-billing-header: cc_version=2.1.220")]),
                .object([
                    "type": .string("text"),
                    "text": .string("Write the title in Chinese. Keep technical terms and code identifiers in their original form.")
                ])
            ]),
            messages: [
                AnthropicMessage(
                    role: "user",
                    content: .array([
                        .object([
                            "type": .string("text"),
                            "text": .string("<session>\nUser: Swift 后端要换什么？\n</session>")
                        ])
                    ])
                )
            ]
        )
    }

    func testModelPatternAndCleaning() {
        XCTAssertEqual(ModelName.clean("claude-opus-4-8[1m]"), "claude-opus-4-8")
        XCTAssertTrue(ModelPattern("claude-opus-*").matches("claude-opus-4-8[1m]"))
        XCTAssertFalse(ModelPattern("claude-opus-*").matches("claude-haiku-4-5"))
    }

    func testModelNameParsesCPAStyleEffortSuffix() {
        // CPA 兼容:model(high) 剥离后缀用于路由,并解析 effort。
        let parsed = ModelName.parse("gpt-5.6-luna(high)")
        XCTAssertEqual(parsed.baseName, "gpt-5.6-luna")
        XCTAssertEqual(parsed.effort, .high)
        XCTAssertEqual(ModelName.clean("gpt-5.6-luna(high)"), "gpt-5.6-luna")
        XCTAssertEqual(ModelName.reasoningEffort(from: "gpt-5.6-luna(xhigh)"), .xhigh)
        XCTAssertEqual(ModelName.reasoningEffort(from: "claude-opus-4-8(max)"), .max)
        XCTAssertEqual(ModelName.reasoningEffort(from: "gpt-5.6-luna(none)"), ReasoningEffort.none)
        XCTAssertEqual(ModelName.reasoningEffort(from: "gpt-5.6-luna(auto)"), .auto)
        // 大小写 / 括号内空格
        XCTAssertEqual(ModelName.reasoningEffort(from: "gpt-5.6-luna(HIGH)"), .high)
        XCTAssertEqual(ModelName.parse("gpt-5.6-luna (medium)").baseName, "gpt-5.6-luna")
        XCTAssertEqual(ModelName.parse("gpt-5.6-luna (medium)").effort, .medium)
        // 可与 [1m] 叠用(先去 bracket 再去 effort)
        XCTAssertEqual(ModelName.clean("claude-opus-4-8(high)[1m]"), "claude-opus-4-8")
        // 未知括号不剥离;ultra 已是合法档位(对齐 Rust ReasoningEffort)。
        XCTAssertEqual(ModelName.clean("weird-model(custom)"), "weird-model(custom)")
        XCTAssertEqual(ModelName.reasoningEffort(from: "gpt-5.6-luna(ultra)"), .ultra)
        XCTAssertNil(ModelName.reasoningEffort(from: "gpt-5.6-luna"))
        // 路由匹配:后缀不影响 pattern
        XCTAssertTrue(ModelPattern("gpt-5.6-luna").matches("gpt-5.6-luna(high)"))
        XCTAssertTrue(ModelPattern("gpt-5.6-*").matches("gpt-5.6-luna(high)"))
    }

    func testOpenAIBridgesInjectReasoningEffortFromModelSuffix() throws {
        let request = RoutingRequest(
            model: "gpt-5.6-luna(high)",
            messages: [AnthropicMessage(role: "user", content: .string("hi"))]
        )
        let chat = OpenAIBridge.makeRequest(from: request, upstreamModel: "gpt-5.6-luna")
        XCTAssertEqual(chat.model, "gpt-5.6-luna")
        XCTAssertEqual(chat.reasoningEffort, "high")

        let responses = OpenAIResponsesBridge.makeRequestBody(from: request, upstreamModel: "gpt-5.6-luna")
        guard case .object(let body) = responses else {
            return XCTFail("expected object body")
        }
        XCTAssertEqual(body["model"]?.stringValue, "gpt-5.6-luna")
        guard case .object(let reasoning) = body["reasoning"] else {
            return XCTFail("expected reasoning object")
        }
        XCTAssertEqual(reasoning["effort"]?.stringValue, "high")

        let plain = RoutingRequest(
            model: "gpt-5.6-luna",
            messages: [AnthropicMessage(role: "user", content: .string("hi"))]
        )
        XCTAssertNil(OpenAIBridge.makeRequest(from: plain, upstreamModel: "gpt-5.6-luna").reasoningEffort)
        if case .object(let plainBody) = OpenAIResponsesBridge.makeRequestBody(from: plain, upstreamModel: "gpt-5.6-luna") {
            XCTAssertNil(plainBody["reasoning"])
        } else {
            XCTFail("expected object body")
        }
    }

    func testFeatureRouteRejectsMixedClientTools() {
        let rule = FeatureRule(
            id: "websearch",
            name: "WebSearch",
            enabled: true,
            match: FeatureMatch(toolTypePrefix: "web_search"),
            target: RouteTarget(poolID: "primary", model: "claude-haiku-4-5-20251001")
        )

        let pureServerTool = RoutingRequest(
            model: "claude-opus-4-8",
            tools: [["type": .string("web_search_20250305")]]
        )
        XCTAssertTrue(RequestInspector.featureRule(rule, matches: pureServerTool))

        let mixedClientTools = RoutingRequest(
            model: "claude-opus-4-8",
            tools: [["type": .string("web_search_20250305")], ["name": .string("Bash")]]
        )
        XCTAssertFalse(RequestInspector.featureRule(rule, matches: mixedClientTools))
    }

    func testStrictBuiltInRequestKindsMatchRealShapes() {
        XCTAssertEqual(RequestInspector.detectedRequestKind(webSearchRequest()), .websearch)
        XCTAssertEqual(RequestInspector.detectedRequestKind(webFetchRequest()), .webfetch)
        XCTAssertEqual(RequestInspector.detectedRequestKind(classifierRequest()), .classifier)

        var classifierStage2 = classifierRequest()
        classifierStage2.raw.removeValue(forKey: "stop_sequences")
        XCTAssertEqual(RequestInspector.detectedRequestKind(classifierStage2), .classifier)

        var severityStage1 = classifierRequest()
        severityStage1.raw["stop_sequences"] = .array([.string("</severity>")])
        XCTAssertEqual(RequestInspector.detectedRequestKind(severityStage1), .classifier)

        var legacyClassifier = classifierRequest()
        legacyClassifier.tools = [["name": .string("classify_result")]]
        legacyClassifier.raw.removeValue(forKey: "stop_sequences")
        legacyClassifier.raw["tool_choice"] = .object(["type": .string("tool"), "name": .string("classify_result")])
        XCTAssertEqual(RequestInspector.detectedRequestKind(legacyClassifier), .classifier)
    }

    func testRequestPurposeRecognizesSessionTitleWithoutUsingModelAlias() {
        let title = sessionTitleRequest()
        XCTAssertNil(RequestInspector.detectedRequestKind(title))
        XCTAssertEqual(RequestInspector.requestPurpose(title), .sessionTitle)
        XCTAssertEqual(RequestInspector.requestPurpose(webSearchRequest()), .webSearch)
        XCTAssertEqual(RequestInspector.requestPurpose(webFetchRequest()), .webFetch)
        XCTAssertEqual(RequestInspector.requestPurpose(classifierRequest()), .classifier)

        let ordinaryLuna = RoutingRequest(
            model: "gpt-5.6-luna(high)",
            system: .string("You are Claude Code, Anthropic's official CLI for Claude."),
            messages: [AnthropicMessage(role: "user", content: .string("implement this feature"))],
            tools: [["name": .string("Bash")]]
        )
        XCTAssertEqual(RequestInspector.requestPurpose(ordinaryLuna), .standard)

        var extraMessage = title
        extraMessage.messages.append(AnthropicMessage(role: "user", content: .string("more")))
        XCTAssertEqual(RequestInspector.requestPurpose(extraMessage), .standard)

        var withTool = title
        withTool.tools = [["name": .string("Read")]]
        XCTAssertEqual(RequestInspector.requestPurpose(withTool), .standard)
    }

    func testWebFetchHistoryDoesNotPoisonMainOrClassifierRequests() {
        let fetched = webFetchPrompt(content: "old fetched page")
        let main = RoutingRequest(
            model: "claude-fable-5",
            system: .string("You are Claude Code, Anthropic's official CLI for Claude."),
            messages: [
                AnthropicMessage(role: "user", content: .string("research this")),
                AnthropicMessage(role: "assistant", content: .array([
                    .object(["type": .string("tool_use"), "name": .string("WebFetch")])
                ])),
                AnthropicMessage(role: "user", content: .array([
                    .object(["type": .string("tool_result"), "content": .string(fetched)])
                ])),
                AnthropicMessage(role: "user", content: .string("now edit the code"))
            ],
            tools: [["name": .string("Bash")], ["name": .string("Read")]]
        )
        XCTAssertNil(RequestInspector.detectedRequestKind(main))

        let classifier = classifierRequest(transcript: "User: research\nTool result: \(fetched)\nBash {\"command\":\"swift test\"}")
        XCTAssertEqual(RequestInspector.detectedRequestKind(classifier), .classifier)
    }

    func testStrictKindsRejectNearMissesAndDoNotCrossMatch() {
        XCTAssertEqual(
            RequestInspector.detectedRequestKind(webSearchRequest(query: "quote Web page content: exactly")),
            .websearch
        )

        var wrongChoice = webSearchRequest()
        wrongChoice.raw["tool_choice"] = .object(["type": .string("auto")])
        XCTAssertNil(RequestInspector.detectedRequestKind(wrongChoice))

        var mixedTools = webSearchRequest()
        mixedTools.tools.append(["name": .string("Bash")])
        XCTAssertNil(RequestInspector.detectedRequestKind(mixedTools))

        var extraHistory = webFetchRequest()
        extraHistory.messages.insert(AnthropicMessage(role: "user", content: .string("older turn")), at: 0)
        XCTAssertNil(RequestInspector.detectedRequestKind(extraHistory))

        let quotedClassifier = RoutingRequest(
            model: "claude-opus-5",
            system: .string("You are Claude Code. Documentation says: You are a security monitor for autonomous AI coding agents."),
            messages: [AnthropicMessage(role: "user", content: .string("explain that sentence"))],
            tools: [["name": .string("Read")]]
        )
        XCTAssertNil(RequestInspector.detectedRequestKind(quotedClassifier))
    }

    func testStickyHomeIndexIsStable() {
        let request = RoutingRequest(
            model: "claude-opus-4-8",
            system: .string("security monitor"),
            messages: [
                AnthropicMessage(role: "user", content: .string("hello"))
            ]
        )

        XCTAssertEqual(StickyHasher.sessionKey(for: request), StickyHasher.sessionKey(for: request))
        XCTAssertEqual(
            StickyHasher.homeIndex(for: request, candidateCount: 3),
            StickyHasher.homeIndex(for: request, candidateCount: 3)
        )
    }

    func testStickyHasherMatchesPythonGoldenValues() {
        let plain = RoutingRequest(
            model: "claude-opus-4-8",
            system: .string("security monitor"),
            messages: [AnthropicMessage(role: "user", content: .string("hello"))]
        )
        XCTAssertEqual(StickyHasher.sessionKey(for: plain), "c088a79e54c3ae6180d208dd14bdb123")
        XCTAssertEqual(StickyHasher.homeIndex(for: plain, candidateCount: 3), 2)

        let block = RoutingRequest(
            model: "claude-opus-4-8",
            system: .array([
                .object(["type": .string("text"), "text": .string("系统A")]),
                .object(["type": .string("text"), "text": .string("系统B")])
            ]),
            messages: [
                AnthropicMessage(role: "assistant", content: .string("skip")),
                AnthropicMessage(role: "user", content: .array([
                    .object(["type": .string("text"), "text": .string("你好")])
                ]))
            ]
        )
        XCTAssertEqual(StickyHasher.sessionKey(for: block), "7e076b0f92ecfc4fae7e78413e76b88a")
        XCTAssertEqual(StickyHasher.homeIndex(for: block, candidateCount: 3), 0)
    }

    func testEndpointModelMappingDuplicateCheckUsesCleanClientModel() throws {
        let endpoint = Endpoint(
            id: "b",
            name: "Backup",
            baseURL: try XCTUnwrap(URL(string: "https://backup.example.com")),
            mappings: [
                ModelMapping(
                    clientPattern: "claude-haiku-4-5-20251001",
                    upstreamModel: "provider-haiku"
                )
            ]
        )

        XCTAssertTrue(endpoint.hasMapping(clientPattern: "claude-haiku-4-5-20251001[1m]"))
        // 映射 id 现在派生自 clientPattern(稳定标识),排除自身即用该 id。
        XCTAssertEqual(endpoint.mappings[0].id, "claude-haiku-4-5-20251001")
        XCTAssertFalse(endpoint.hasMapping(
            clientPattern: "claude-haiku-4-5-20251001",
            excluding: "claude-haiku-4-5-20251001"
        ))
        XCTAssertFalse(endpoint.hasMapping(clientPattern: "claude-sonnet-4-6"))
    }

    func testEndpointModelMappingPrefersExactOverWildcard() throws {
        let endpoint = Endpoint(
            id: "mixed",
            name: "Mixed",
            baseURL: try XCTUnwrap(URL(string: "https://mixed.example.com")),
            mappings: [
                ModelMapping(clientPattern: "claude-opus-*", upstreamModel: "wildcard"),
                ModelMapping(clientPattern: "claude-opus-5", upstreamModel: "exact")
            ]
        )

        XCTAssertEqual(endpoint.preferredMapping(for: "claude-opus-5[1m]")?.upstreamModel, "exact")
        XCTAssertEqual(endpoint.preferredMapping(for: "claude-opus-4-6")?.upstreamModel, "wildcard")
    }

    func testProviderModelCatalogParsesSupportedResponseShapes() throws {
        let cases: [(String, [String])] = [
            (#"[{"id":"model-a"},{"name":"model-b"}]"#, ["model-a", "model-b"]),
            (#"{"data":[{"model":"model-c"},{"model_id":"model-d"}]}"#, ["model-c", "model-d"]),
            (#"{"models":["model-e", "model-f", "model-e"]}"#, ["model-e", "model-f"]),
            (#"{"result":[{"id":"model-g"}, "model-h"]}"#, ["model-g", "model-h"]),
            (#"{"result":{"data":{"models":{"model-i":{},"model-j":{}}}}}"#, ["model-i", "model-j"])
        ]

        for (json, expected) in cases {
            let ids = try ProviderModelCatalog.modelIDs(from: Data(json.utf8))
            XCTAssertEqual(ids, expected)
        }
    }

    func testModelCatalogDeduplicatesTrimmedIDs() throws {
        let catalog = ModelCatalog(
            models: [" model-a ", "model-a[1m]", "model-b", "  ", "model-b"]
        )
        XCTAssertEqual(catalog.models, ["model-a", "model-b"])
        XCTAssertEqual(catalog.uniqueModels, ["model-a", "model-b"])

        let decoded = try JSONDecoder().decode(
            ModelCatalog.self,
            from: Data(#"{"models":["model-a"," model-a ","model-c"]}"#.utf8)
        )
        XCTAssertEqual(decoded.models, ["model-a", "model-c"])
    }

    func testProviderModelCatalogCandidatePathsRespectBasePath() throws {
        let root = try XCTUnwrap(URL(string: "https://example.com"))
        XCTAssertEqual(ProviderModelCatalog.candidateURLs(baseURL: root).map(\.path),
                       ["/v1/models", "/models", "/v1/model/list", "/api/v1/models"])
        let v1 = try XCTUnwrap(URL(string: "https://example.com/v1/"))
        XCTAssertEqual(ProviderModelCatalog.candidateURLs(baseURL: v1).map(\.path),
                       ["/v1/models", "/v1/model/list"])
        let custom = try XCTUnwrap(URL(string: "https://example.com/apps/anthropic"))
        XCTAssertEqual(ProviderModelCatalog.candidateURLs(baseURL: custom).map(\.path),
                       ["/apps/anthropic/v1/models", "/apps/anthropic/models", "/apps/anthropic/v1/model/list"])
    }

    func testProviderModelCatalogAuthenticationHeadersAllowAnonymousProviders() {
        let anonymous = ProviderModelCatalog.authenticationHeaders(for: "   ")
        XCTAssertEqual(anonymous.count, 1)
        XCTAssertEqual(anonymous.first?.0, "")
        XCTAssertEqual(anonymous.first?.1, "")

        let authenticated = ProviderModelCatalog.authenticationHeaders(for: "sk-test")
        XCTAssertEqual(authenticated.count, 3)
        XCTAssertEqual(authenticated.map { $0.0 }, ["x-api-key", "Authorization", "Authorization"])
        XCTAssertEqual(authenticated.map { $0.1 }, ["sk-test", "Bearer sk-test", "x-api-key sk-test"])

        let sets = ProviderModelCatalog.authenticationHeaderSets(for: "sk-test")
        XCTAssertEqual(sets.first?.map { $0.0 }, ["Authorization", "x-api-key"])
        XCTAssertEqual(sets.first?.map { $0.1 }, ["Bearer sk-test", "sk-test"])
        let anonymousSets = ProviderModelCatalog.authenticationHeaderSets(for: " ")
        XCTAssertEqual(anonymousSets.count, 1)
        XCTAssertTrue(anonymousSets.first?.isEmpty == true)
    }

    /// 系统代理提示:注入 settings 字典,不依赖本机真实网络偏好设置。
    func testSystemProxyHintNamesProxyKindWithoutLeakingHostOrPort() {
        XCTAssertNil(
            ProviderModelCatalog.systemProxyHint(settings: [:]),
            "没有启用任何代理时不应给提示"
        )
        XCTAssertNil(
            ProviderModelCatalog.systemProxyHint(settings: nil),
            "读不到系统设置时不应给提示"
        )
        XCTAssertNil(
            ProviderModelCatalog.systemProxyHint(settings: [
                kCFNetworkProxiesHTTPEnable as String: 0,
                kCFNetworkProxiesHTTPProxy as String: "10.0.0.1"
            ]),
            "enable=0 时即使填了主机也不算启用"
        )

        let https = try? XCTUnwrap(
            ProviderModelCatalog.systemProxyHint(settings: [
                kCFNetworkProxiesHTTPSEnable as String: 1,
                kCFNetworkProxiesHTTPSProxy as String: "proxy.internal.invalid",
                kCFNetworkProxiesHTTPSPort as String: 8080
            ])
        )
        let hint = https ?? ""
        XCTAssertTrue(hint.contains("HTTPS 代理"), "应点出代理类型:\(hint)")
        XCTAssertFalse(
            hint.contains("proxy.internal.invalid") || hint.contains("8080"),
            "不得回显主机或端口(可能带内网信息或凭据):\(hint)"
        )
        XCTAssertTrue(hint.contains("不走代理"), "要说清探测与转发口径一致:\(hint)")

        let pac = ProviderModelCatalog.systemProxyHint(settings: [
            kCFNetworkProxiesProxyAutoConfigEnable as String: 1
        ])
        XCTAssertTrue(pac?.contains("PAC") == true, "PAC 也要识别:\(pac ?? "nil")")
    }

    /// 提示必须真的被拼进调用方看到的失败摘要 —— 只测 systemProxyHint() 本身不够。
    func testSummarizeAppendsProxyHintToFailureSummary() {
        let errors = ["/v1/models HTTP 407", "/models HTTP 407"]

        XCTAssertEqual(
            ProviderModelCatalog.summarize(errors, hint: nil),
            "/v1/models HTTP 407；/models HTTP 407",
            "没有代理时摘要不应被改动"
        )

        let withHint = ProviderModelCatalog.summarize(errors, hint: "(系统启用了 HTTPS 代理:…不走代理…)")
        XCTAssertTrue(withHint.hasPrefix("/v1/models HTTP 407"), "原因要排在前面:\(withHint)")
        XCTAssertTrue(withHint.contains("不走代理"), "提示要被拼上:\(withHint)")

        // 只保留前 4 条原因的既有行为不能被提示打乱。
        let many = (1...6).map { "/p\($0) HTTP 500" }
        let truncated = ProviderModelCatalog.summarize(many, hint: nil)
        XCTAssertFalse(truncated.contains("/p5"), "仍应只保留前 4 条:\(truncated)")
    }

    /// 实测:探测不跟随重定向。302 的 Location 指向 /redirected,stub 会记下被请求的
    /// 路径 —— 跟随了就会出现 /redirected,并且 fetch 会拿到 followed-redirect 模型。
    func testProviderModelCatalogDoesNotFollowRedirects() async throws {
        RedirectingCatalogURLProtocol.requestedPaths = []
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [RedirectingCatalogURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }

        do {
            let models = try await ProviderModelCatalog.fetch(
                baseURL: try XCTUnwrap(URL(string: "https://redirect.example.invalid")),
                apiKey: "sk-test-redirect",
                timeout: 1,
                overallDeadline: 3,
                session: session
            )
            XCTFail("不应拿到模型:302 不跟随时每个候选路径都只有非 200 响应,实际 \(models)")
        } catch {
            // 预期:所有候选路径都以 302 结束,最终 noModels。
        }

        XCTAssertFalse(
            RedirectingCatalogURLProtocol.requestedPaths.contains { $0.hasSuffix("/redirected") },
            "探测跟随了重定向,与转发路径的 redirect::none() 不一致:\(RedirectingCatalogURLProtocol.requestedPaths)"
        )
        XCTAssertTrue(
            RedirectingCatalogURLProtocol.requestedPaths.contains("/v1/models"),
            "候选路径本身应当被请求过:\(RedirectingCatalogURLProtocol.requestedPaths)"
        )
    }

    /// 实测 `connectionProxyDictionary` 这套机制在本平台确实被 URLSession 尊重,
    /// 并且 directSession 的空字典把代理关掉了。
    ///
    /// 正向:显式把代理指到本地假代理端口 → 假代理必须收到连接。
    /// 反向:同一个 .invalid 域名走 directSession → 假代理不能收到任何连接。
    /// 缺了正向这一半,反向的「没连上」就可能只是因为域名解析失败而非禁了代理。
    func testDirectSessionBypassesConfiguredProxyWhileExplicitProxyIsHonored() async throws {
        let probe = try LocalConnectionProbe()
        defer { probe.stop() }

        // 两种 scheme 都要测:真实上游的 baseURL 全是 https,而 HTTPS 代理走的是
        // CONNECT 隧道,和明文 HTTP 代理不是同一条机制路径。只测 http 证明不了
        // 用户实际会遇到的那一种。
        let cases: [(String, URL, [String: Any])] = [
            (
                "HTTP 代理",
                try XCTUnwrap(URL(string: "http://catalog-proxy-probe.example.invalid/v1/models")),
                [
                    kCFNetworkProxiesHTTPEnable as String: 1,
                    kCFNetworkProxiesHTTPProxy as String: "127.0.0.1",
                    kCFNetworkProxiesHTTPPort as String: Int(probe.port)
                ]
            ),
            (
                "HTTPS 代理(CONNECT)",
                try XCTUnwrap(URL(string: "https://catalog-proxy-probe.example.invalid/v1/models")),
                [
                    kCFNetworkProxiesHTTPSEnable as String: 1,
                    kCFNetworkProxiesHTTPSProxy as String: "127.0.0.1",
                    kCFNetworkProxiesHTTPSPort as String: Int(probe.port)
                ]
            )
        ]

        for (label, target, proxyDictionary) in cases {
            // 正向:代理生效 → 请求打到本地假代理。
            probe.reset()
            let proxied = URLSessionConfiguration.ephemeral
            proxied.connectionProxyDictionary = proxyDictionary
            let proxiedSession = URLSession(configuration: proxied)
            _ = try? await proxiedSession.data(for: URLRequest(url: target, timeoutInterval: 3))
            XCTAssertTrue(
                probe.waitForConnection(timeout: 3),
                "\(label):显式配置的代理没有生效 —— 那反向断言的「没连上」就可能只是域名解析失败,证明不了 directSession 禁了代理"
            )
            proxiedSession.invalidateAndCancel()

            // 反向:directSession 不走任何代理。
            probe.reset()
            _ = try? await ProviderModelCatalog.directSession.data(
                for: URLRequest(url: target, timeoutInterval: 3)
            )
            XCTAssertFalse(
                probe.waitForConnection(timeout: 1),
                "\(label):directSession 仍然走了代理,与转发路径的 no_proxy 不一致"
            )
        }
    }

    func testProviderModelCatalogFetchesAnonymousEndpointWithoutAuthHeaders() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [AnonymousCatalogURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }

        let models = try await ProviderModelCatalog.fetch(
            baseURL: try XCTUnwrap(URL(string: "https://anonymous.example.test")),
            apiKey: "",
            timeout: 1,
            overallDeadline: 2,
            session: session
        )
        XCTAssertEqual(models, ["anonymous-model"])
    }

    func testRoutePlannerUsesFeatureTargetAndEndpointMapping() throws {
        let endpoint = Endpoint(
            id: "third",
            name: "Third API",
            baseURL: try XCTUnwrap(URL(string: "https://example.com")),
            protocolMode: .openai,
            mappings: [
                ModelMapping(
                    clientPattern: "claude-haiku-4-5-20251001",
                    upstreamModel: "provider-haiku",
                    thinking: .disabled,
                    context: .standard
                )
            ]
        )
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [endpoint]
        config.featureRules[2].enabled = true

        let request = classifierRequest()

        let plan = try RoutePlanner().plan(request: request, config: config)
        XCTAssertEqual(plan.endpoints.first?.endpointID, "third")
        XCTAssertEqual(plan.featureRuleID, "classifier")
        XCTAssertEqual(plan.endpoints.first?.providerProtocol, .openai)
        XCTAssertEqual(plan.endpoints.first?.upstreamModel, "provider-haiku")
    }

    func testFeatureRouteProtocolOverrideReplacesEndpointProtocol() throws {
        // endpoint 自身是 anthropic;规则指定 openai → 计划里的协议被覆盖,引擎会走 OpenAI 桥接。
        let endpoint = Endpoint(
            id: "backup",
            name: "Backup",
            baseURL: try XCTUnwrap(URL(string: "https://example.com")),
            protocolMode: .auto,
            mappings: [ModelMapping(clientPattern: "gpt-5.4", upstreamModel: "gpt-5.4")]
        )
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [endpoint]
        config.featureRules[2] = FeatureRule(
            id: "classifier",
            name: "安全分类器",
            enabled: true,
            match: FeatureMatch(systemContains: "security monitor"),
            target: RouteTarget(poolID: "primary", model: "gpt-5.4", protocolOverride: .openai)
        )

        let request = RoutingRequest(
            model: "claude-opus-4-8",
            system: .string("security monitor classification prompt"),
            messages: [AnthropicMessage(role: "user", content: .string("check"))]
        )

        let plan = try RoutePlanner().plan(request: request, config: config)
        XCTAssertEqual(plan.featureRuleID, "classifier")
        XCTAssertEqual(plan.endpoints.first?.providerProtocol, .openai)

        // override 为 nil 时继承 endpoint 自身协议(回归保护)。
        config.featureRules[2].target = RouteTarget(poolID: "primary", model: "gpt-5.4")
        let inherited = try RoutePlanner().plan(request: request, config: config)
        XCTAssertEqual(inherited.endpoints.first?.providerProtocol, .anthropic)

        // 普通路由(不命中规则)不受任何 override 影响。
        config.featureRules[2].target = RouteTarget(poolID: "primary", model: "gpt-5.4", protocolOverride: .openai)
        config.pools[0].endpoints[0].mappings.append(
            ModelMapping(clientPattern: "claude-haiku-4-5-20251001", upstreamModel: "provider-haiku")
        )
        let normal = try RoutePlanner().plan(
            request: RoutingRequest(
                model: "claude-haiku-4-5-20251001",
                messages: [AnthropicMessage(role: "user", content: .string("hi"))]
            ),
            config: config
        )
        XCTAssertNil(normal.featureRuleID)
        XCTAssertEqual(normal.endpoints.first?.providerProtocol, .anthropic)
    }

    func testFeatureRouteWithoutMappingReportsUnifiedProviderError() throws {
        let primary = Endpoint(
            id: "primary",
            name: "Primary",
            baseURL: try XCTUnwrap(URL(string: "https://primary.example.com"))
        )
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [primary]
        config.featureRules[2] = FeatureRule(
            id: "classifier",
            name: "安全分类器",
            enabled: true,
            match: FeatureMatch(systemContains: "security monitor"),
            target: RouteTarget(poolID: "primary", model: "claude-missing")
        )

        XCTAssertThrowsError(try RoutePlanner().plan(
            request: RoutingRequest(
                model: "claude-opus-4-8",
                system: .string("security monitor"),
                messages: [AnthropicMessage(role: "user", content: .string("hi"))]
            ),
            config: config
        )) { error in
            XCTAssertEqual(
                error as? RoutePlanningError,
                .noCompatibleProvider(provider: "candidates", sourceFormat: .anthropic)
            )
        }
    }

    func testRoutePlannerSkipsProviderEndpointsWithoutMatchingMapping() throws {
        let haiku = Endpoint(
            id: "haiku-provider",
            name: "Haiku Provider",
            baseURL: try XCTUnwrap(URL(string: "https://haiku.example.com")),
            mappings: [
                ModelMapping(clientPattern: "claude-haiku-4-5-20251001", upstreamModel: "provider-haiku")
            ]
        )
        let qwen = Endpoint(
            id: "qwen-provider",
            name: "Qwen Provider",
            baseURL: try XCTUnwrap(URL(string: "https://qwen.example.com")),
            mappings: [
                ModelMapping(clientPattern: "qwen3.7-plus", upstreamModel: "qwen3.7-plus")
            ]
        )
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [haiku, qwen]

        let plan = try RoutePlanner().plan(
            request: RoutingRequest(model: "qwen3.7-plus"),
            config: config
        )

        XCTAssertEqual(plan.endpoints.map(\.endpointID), ["qwen-provider"])
    }

    func testEndpointProtocolModeRoundTripsAllFourWireValues() throws {
        for mode in EndpointProtocolMode.allCases {
            let data = try JSONEncoder().encode(mode)
            XCTAssertEqual(try JSONDecoder().decode(EndpointProtocolMode.self, from: data), mode)
        }
        XCTAssertEqual(Endpoint(id: "new", name: "new", baseURL: try XCTUnwrap(URL(string: "https://new.example"))).protocolMode, .auto)
    }

    func testConfigStoreIgnoresLegacyFixedIPsAndRemovesThemOnSave() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-fixed-ip-removal-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")
        let original = Data(#"""
        {"schemaVersion":7,"listener":{"host":"127.0.0.1","port":57878},
         "retry":{"pinnedIPConcurrency":3,"sessionStickyRetries":2,"max500Retries":1},
         "endpoints":[{"id":"legacy","baseURL":"https://example.invalid","protocol":"auto",
          "pinnedIPs":["203.0.113.10","203.0.113.11"],"pinnedIP":"203.0.113.12","pinnedIPExclusive":true,
          "priority":10,"stickyGroup":"account-a","keepAlive":true}],"featureRules":[]}
        """#.utf8)
        try original.write(to: url)
        let store = ConfigStore(url: url)
        let loaded = try store.loadWithMigration()
        XCTAssertNil(loaded.migrationNotice)
        XCTAssertEqual(try Data(contentsOf: url), original)
        XCTAssertEqual(loaded.config.retry.sessionStickyRetries, 2)
        XCTAssertEqual(loaded.config.retry.max500Retries, 1)
        XCTAssertEqual(loaded.config.endpoints.first?.baseURL.absoluteString, "https://example.invalid")
        XCTAssertEqual(loaded.config.endpoints.first?.stickyGroup, "account-a")
        XCTAssertEqual(loaded.config.endpoints.first?.priority, 10)
        XCTAssertEqual(loaded.config.endpoints.first?.keepAlive, true)

        try store.save(loaded.config)
        let saved = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        XCTAssertEqual(saved["schemaVersion"] as? Int, 7)
        let retry = try XCTUnwrap(saved["retry"] as? [String: Any])
        XCTAssertNil(retry["pinnedIPConcurrency"])
        let endpoints = try XCTUnwrap(saved["endpoints"] as? [[String: Any]])
        let endpoint = try XCTUnwrap(endpoints.first)
        for key in ["pinnedIPs", "pinnedIP", "pinnedIPExclusive"] {
            XCTAssertNil(endpoint[key])
        }
        XCTAssertEqual(try store.load(), loaded.config)
    }

    func testConfigStoreMigratesLegacyPassthroughToAutoAndBacksUp() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-config-migration-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")
        let json = #"{"schemaVersion":3,"listener":{"host":"127.0.0.1","port":57878,"inboundDialectPassthrough":true},"pools":[{"id":"primary","role":"primary","endpoints":[{"id":"a","baseURL":"https://a.example","protocol":"anthropic"},{"id":"b","baseURL":"https://b.example","protocol":"openai"}]}],"featureRules":[]}"#
        try Data(json.utf8).write(to: url)

        let result = try ConfigStore(url: url).loadWithMigration()
        XCTAssertEqual(result.config.schemaVersion, 7)
        XCTAssertEqual(result.config.pools[0].endpoints.map(\.protocolMode), [.auto, .auto])
        XCTAssertEqual(result.migrationNotice?.autoEndpointIDs, ["a", "b"])
        XCTAssertEqual(result.migrationNotice?.expandedLegacyPassthroughEndpoints, 2)
        XCTAssertNotNil(result.migrationNotice?.backupFile)
        let backups = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.lastPathComponent.contains("before-schema-v7") }
        XCTAssertEqual(backups.count, 1)
        let permissions = try XCTUnwrap(
            try FileManager.default.attributesOfItem(atPath: backups[0].path)[.posixPermissions]
                as? NSNumber
        ).intValue
        XCTAssertEqual(permissions & 0o777, 0o600)
        let stored = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        XCTAssertEqual(stored?["schemaVersion"] as? Int, 7)
        XCTAssertNil((stored?["listener"] as? [String: Any])?["inboundDialectPassthrough"])

        let secondLoad = try ConfigStore(url: url).loadWithMigration()
        XCTAssertNil(secondLoad.migrationNotice)
        let backupsAfterSecondLoad = try FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil
        ).filter { $0.lastPathComponent.contains("before-schema-v7") }
        XCTAssertEqual(backupsAfterSecondLoad.count, 1)
    }

    func testConfigStoreLegacyFalsePreservesFixedProtocolAndMigratesV4() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-config-migration-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")
        let json = #"{"schemaVersion":3,"listener":{"inboundDialectPassthrough":false},"pools":[{"id":"primary","role":"primary","endpoints":[{"id":"a","baseURL":"https://a.example","protocol":"openai-responses"}]}],"featureRules":[]}"#
        try Data(json.utf8).write(to: url)
        let result = try ConfigStore(url: url).loadWithMigration()
        XCTAssertEqual(result.config.pools[0].endpoints.first?.protocolMode, .openaiResponses)

        let v4: [String: Any] = [
            "schemaVersion": 4,
            "listener": [:],
            "pools": [[
                "id": "primary",
                "role": "primary",
                "endpoints": [[
                    "id": "a",
                    "baseURL": "https://a.example",
                    "protocol": "auto",
                    "obsoleteProviderOption": true,
                ]],
            ]],
            "featureRules": [],
        ]
        try JSONSerialization.data(withJSONObject: v4).write(to: url)
        let migrated = try ConfigStore(url: url).loadWithMigration()
        XCTAssertEqual(migrated.config.schemaVersion, 7)
        XCTAssertEqual(migrated.config.pools[0].endpoints[0].protocolMode, .auto)
        XCTAssertEqual(migrated.migrationNotice?.removedFields, [])
        let stored = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        let storedEndpoint = (stored["endpoints"] as? [[String: Any]])?.first
        XCTAssertNil(storedEndpoint?["obsoleteProviderOption"])
    }

    func testConfigStoreMigratesV5GlobalModelsToEndpointMappingsOnce() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-config-global-models-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")
        let json: [String: Any] = [
            "schemaVersion": 5,
            "listener": [:],
            "pools": [[
                "id": "primary",
                "role": "primary",
                "globalModels": [["pattern": " claude-opus-* ", "thinking": "adaptive"]],
                "endpoints": [[
                    "id": "inherit", "baseURL": "https://inherit.invalid", "protocol": "anthropic", "mappings": [],
                ], [
                    "id": "explicit", "baseURL": "https://explicit.invalid", "protocol": "anthropic",
                    "mappings": [["clientPattern": "gpt-5.4", "upstreamModel": "gpt-5.4", "thinking": "disabled", "context": "standard"]],
                ]],
            ]],
        ]
        try JSONSerialization.data(withJSONObject: json).write(to: url)

        let result = try ConfigStore(url: url).loadWithMigration()
        XCTAssertEqual(result.migrationNotice?.removedFields, ["pools[].globalModels"])
        XCTAssertEqual(result.config.pools[0].endpoints[0].mappings.first?.clientPattern.rawValue, "claude-opus-*")
        XCTAssertEqual(result.config.pools[0].endpoints[1].mappings.count, 1)
        let stored = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        XCTAssertNil(stored["pools"])
        XCTAssertNotNil(((stored["endpoints"] as? [[String: Any]])?.first?["mappings"] as? [[String: Any]])?.first)
        XCTAssertNil(try ConfigStore(url: url).loadWithMigration().migrationNotice)
    }

    func testConfigStoreRejectsV4EndpointWithoutExplicitProtocol() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-config-v4-required-protocol-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")
        let json = #"{"schemaVersion":4,"listener":{},"pools":[{"id":"primary","role":"primary","endpoints":[{"id":"missing","baseURL":"https://missing.example"}]}],"featureRules":[]}"#
        try Data(json.utf8).write(to: url)

        XCTAssertThrowsError(try ConfigStore(url: url).loadWithMigration()) { error in
            XCTAssertEqual(
                error as? ConfigStoreError,
                .missingField("pools[0].endpoints[0].protocol")
            )
        }
        XCTAssertFalse(
            try FileManager.default.contentsOfDirectory(atPath: directory.path)
                .contains { $0.contains("before-schema-v7") }
        )
    }

    func testConfigStoreRejectsLegacyProtocolsArrayAndInvalidProtocolInV4() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-config-v4-protocol-validation-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")

        let legacyArray = #"{"schemaVersion":4,"listener":{},"pools":[{"id":"primary","role":"primary","endpoints":[{"id":"a","baseURL":"https://a.example","protocol":"auto","protocols":["anthropic"]}]}],"featureRules":[]}"#
        try Data(legacyArray.utf8).write(to: url)
        XCTAssertThrowsError(try ConfigStore(url: url).loadWithMigration()) { error in
            XCTAssertEqual(
                error as? ConfigStoreError,
                .legacyField("pools[0].endpoints[0].protocols")
            )
        }
        XCTAssertFalse(
            try FileManager.default.contentsOfDirectory(atPath: directory.path)
                .contains { $0.contains("before-schema-v7") }
        )

        let invalidProtocol = #"{"schemaVersion":4,"listener":{},"pools":[{"id":"primary","role":"primary","endpoints":[{"id":"a","baseURL":"https://a.example","protocol":"automatic"}]}],"featureRules":[]}"#
        try Data(invalidProtocol.utf8).write(to: url)
        XCTAssertThrowsError(try ConfigStore(url: url).loadWithMigration()) { error in
            XCTAssertEqual(
                error as? ConfigStoreError,
                .invalidField("pools[0].endpoints[0].protocol")
            )
        }
        XCTAssertFalse(
            try FileManager.default.contentsOfDirectory(atPath: directory.path)
                .contains { $0.contains("before-schema-v7") }
        )
    }

    func testConfigStoreRejectsNonBooleanLegacyPassthroughWithoutWriting() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-config-v3-invalid-passthrough-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")
        let original = Data(#"{"schemaVersion":3,"listener":{"inboundDialectPassthrough":"true"},"pools":[],"featureRules":[]}"#.utf8)
        try original.write(to: url)

        XCTAssertThrowsError(try ConfigStore(url: url).loadWithMigration()) { error in
            XCTAssertEqual(
                error as? ConfigStoreError,
                .invalidField("listener.inboundDialectPassthrough")
            )
        }
        XCTAssertEqual(try Data(contentsOf: url), original)
        XCTAssertFalse(
            try FileManager.default.contentsOfDirectory(atPath: directory.path)
                .contains { $0.contains("before-schema-v7") }
        )
    }

    func testMigrationNoticeDecodesRustSSEWire() throws {
        let json = #"{"id":"migration-1","fromSchema":3,"toSchema":7,"backupFile":"config.before-schema-v7-1.json","endpointCount":2,"expandedLegacyPassthroughEndpoints":2,"convertedToAutoEndpointIds":["a","b"],"removedFields":["listener.inboundDialectPassthrough","pools[].endpoints[].searchDialect"]}"#

        let notice = try JSONDecoder().decode(ConfigMigrationNotice.self, from: Data(json.utf8))

        XCTAssertEqual(notice.id, "migration-1")
        XCTAssertEqual(notice.autoEndpointIDs, ["a", "b"])
        XCTAssertEqual(notice.expandedLegacyPassthroughEndpoints, 2)
        XCTAssertEqual(notice.toSchema, 7)
        XCTAssertEqual(notice.removedFields, ["listener.inboundDialectPassthrough", "pools[].endpoints[].searchDialect"])
    }

    func testAutoEndpointResolvesToEverySourceFormatNatively() throws {
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [Endpoint(
            id: "auto",
            name: "Auto",
            baseURL: try XCTUnwrap(URL(string: "https://auto.example")),
            mappings: [ModelMapping(clientPattern: "claude-opus-4-8")]
        )]
        let request = RoutingRequest(model: "claude-opus-4-8")
        for source in ProviderProtocol.allCases {
            let endpoint = try XCTUnwrap(RoutePlanner().plan(
                request: request,
                config: config,
                sourceFormat: source
            ).endpoints.first)
            XCTAssertEqual(endpoint.configuredProtocol, .auto)
            XCTAssertEqual(endpoint.providerProtocol, source)
            XCTAssertEqual(endpoint.routeMode, .native)
        }
    }

    func testNativeCandidatesExcludeHigherPriorityTranslatedCandidates() throws {
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [
            Endpoint(
                id: "bridge",
                name: "Bridge",
                baseURL: try XCTUnwrap(URL(string: "https://bridge.example")),
                protocolMode: .anthropic,
                priority: 0,
                mappings: [ModelMapping(clientPattern: "claude-opus-4-8")]
            ),
            Endpoint(
                id: "native",
                name: "Native",
                baseURL: try XCTUnwrap(URL(string: "https://native.example")),
                protocolMode: .openaiResponses,
                priority: 100,
                mappings: [ModelMapping(clientPattern: "claude-opus-4-8")]
            )
        ]
        let plan = try RoutePlanner().plan(
            request: RoutingRequest(model: "claude-opus-4-8"),
            config: config,
            sourceFormat: .openaiResponses
        )
        XCTAssertEqual(plan.endpoints.map(\.endpointID), ["native"])
        XCTAssertEqual(plan.endpoints.first?.routeMode, .native)
    }

    func testFixedEndpointCannotBeOverriddenToDifferentTargetProtocol() throws {
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [Endpoint(
            id: "anthropic-only",
            name: "Anthropic",
            baseURL: try XCTUnwrap(URL(string: "https://anthropic.example")),
            protocolMode: .anthropic
        )]
        config.featureRules[2] = FeatureRule(
            id: "classifier",
            name: "分类器",
            enabled: true,
            match: FeatureMatch(systemContains: "security monitor"),
            target: RouteTarget(poolID: "primary", model: "claude-opus-4-8", protocolOverride: .openai)
        )
        XCTAssertThrowsError(try RoutePlanner().plan(
            request: RoutingRequest(model: "claude-opus-4-8", system: .string("security monitor")),
            config: config
        )) { error in
            XCTAssertEqual(
                error as? RoutePlanningError,
                .noCompatibleProvider(provider: "candidates", sourceFormat: .anthropic)
            )
        }
    }

    func testPinnedFixedProtocolMismatchDoesNotFallBackToCompatiblePoolEndpoint() throws {
        var config = AppConfig.bootstrap
        config.pools[0].endpoints = [
            Endpoint(
                id: "pinned-anthropic",
                name: "Pinned Anthropic",
                baseURL: try XCTUnwrap(URL(string: "https://anthropic.example")),
                protocolMode: .anthropic
            ),
            Endpoint(
                id: "pool-auto",
                name: "Pool Auto",
                baseURL: try XCTUnwrap(URL(string: "https://auto.example")),
                protocolMode: .auto
            )
        ]
        config.featureRules[2] = FeatureRule(
            id: "classifier",
            name: "分类器",
            enabled: true,
            match: FeatureMatch(systemContains: "security monitor"),
            target: RouteTarget(
                poolID: "primary",
                model: "claude-opus-4-8",
                protocolOverride: .openai,
                endpointID: "pinned-anthropic"
            )
        )

        XCTAssertThrowsError(try RoutePlanner().plan(
            request: RoutingRequest(model: "claude-opus-4-8", system: .string("security monitor")),
            config: config
        )) { error in
            XCTAssertEqual(
                error as? RoutePlanningError,
                .noCompatibleProvider(provider: "pinned-anthropic", sourceFormat: .anthropic)
            )
        }
    }

    // MARK: - config.json (schema v5)

    func testRetryPolicyDefaultStatusCodesMatchRustSidecar() {
        let retry = RetryPolicy()
        XCTAssertEqual(
            retry.retryableStatusCodes,
            Set([401, 402, 403, 429, 502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 529, 530])
        )
        XCTAssertEqual(retry.deferredStatusCodes, retry.retryableStatusCodes)
        XCTAssertFalse(retry.retryableStatusCodes.contains(400))
        XCTAssertEqual(retry.max500Retries, 0)
        XCTAssertTrue(retry.failoverOn500)
        XCTAssertNil(retry.retryDelaySeconds)

        let configured = RetryPolicy(max500Retries: 3, failoverOn500: false, retryDelaySeconds: 2.5)
        let wire = try! JSONSerialization.jsonObject(with: JSONEncoder().encode(configured)) as! [String: Any]
        XCTAssertEqual(wire["max500Retries"] as? Int, 3)
        XCTAssertEqual(wire["failoverOn500"] as? Bool, false)
        XCTAssertEqual(wire["retryDelaySeconds"] as? Double, 2.5)
    }

    func testConfigRoundTripsWithoutLoss() throws {
        let config = AppConfig(
            listener: ListenerConfig(host: "0.0.0.0", port: 12_345, allowedCIDRs: ["10.0.0.0/8"], authToken: "tok"),
            retry: RetryPolicy(
                responseTimeoutSeconds: 8,
                streamIdleTimeoutSeconds: nil,
                maxDeferredRounds: 2,
                maxRetryDurationSeconds: 900,
                sessionStickyRetries: 1
            ),
            pools: [
                Pool(
                    id: "primary",
                    name: "Provider",
                    role: .primary,
                    endpoints: [
                        Endpoint(
                            id: "acc1",
                            name: "acc1",
                            baseURL: try XCTUnwrap(URL(string: "https://anyrouter.top")),
                            enabled: true,
                            apiKey: "sk-1",
                            stickyGroup: "anyrouter",
                            catalog: ModelCatalog(models: ["claude-opus-5"], source: "api", status: "已获取", updatedAt: "2026-08-01 10:00:00"),
                            mappings: [ModelMapping(clientPattern: "claude-opus-*", thinking: .adaptive, context: .oneMillion)]
                        ),
                        Endpoint(
                            id: "b1",
                            name: "b1",
                            baseURL: try XCTUnwrap(URL(string: "https://b1.example.com")),
                            protocolMode: .openaiResponses,
                            apiKey: "sk-2",
                            priority: 10,
                            mappings: [
                                ModelMapping(
                                    clientPattern: "gpt-5.4",
                                    upstreamModel: "gpt-5.4-upstream",
                                    thinking: .passthrough,
                                    context: .standard,
                                    failoverTimeoutSeconds: 12
                                )
                            ]
                        )
                    ]
                )
            ],
            featureRules: [
                FeatureRule(
                    id: "classifier",
                    name: "安全分类器",
                    enabled: true,
                    match: FeatureMatch(systemContains: "security monitor"),
                    target: RouteTarget(poolID: "primary", model: "gpt-5.4", protocolOverride: .openai, endpointID: "b1")
                )
            ]
        )

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(config)
        let decoded = try JSONDecoder().decode(AppConfig.self, from: data)

        XCTAssertEqual(decoded.listener, config.listener)
        XCTAssertEqual(decoded.retry, config.retry)
        XCTAssertEqual(decoded.pools.count, 1)
        let primaryEndpoint = try XCTUnwrap(decoded.primaryPool?.endpoints.first)
        XCTAssertEqual(primaryEndpoint.apiKey, "sk-1")
        XCTAssertEqual(primaryEndpoint.stickyGroup, "anyrouter")
        XCTAssertEqual(primaryEndpoint.catalog.models, ["claude-opus-5"])
        XCTAssertEqual(decoded.primaryPool?.endpoints.first?.mappings.map(\.id), ["claude-opus-*"])

        let fallbackMapping = try XCTUnwrap(decoded.primaryPool?.endpoints.first(where: { $0.id == "b1" })?.mappings.first)
        XCTAssertEqual(fallbackMapping.clientPattern.rawValue, "gpt-5.4")
        XCTAssertEqual(fallbackMapping.upstreamModel, "gpt-5.4-upstream")
        XCTAssertEqual(fallbackMapping.thinking, .passthrough)
        XCTAssertEqual(fallbackMapping.context, .standard)
        XCTAssertEqual(fallbackMapping.failoverTimeoutSeconds, 12)
        XCTAssertEqual(decoded.primaryPool?.endpoints.first(where: { $0.id == "b1" })?.protocolMode, .openaiResponses)

        let rule = try XCTUnwrap(decoded.featureRules.first { $0.id == "classifier" })
        XCTAssertTrue(rule.enabled)
        XCTAssertEqual(rule.match, FeatureMatch(requestKind: .classifier))
        XCTAssertEqual(rule.target.endpointID, "b1")
        XCTAssertEqual(rule.target.protocolOverride, .openai)

        // 配置文件里不该出现运行时 UUID、也不该有 Python 兼容残留字段。
        let text = String(decoding: data, as: UTF8.self)
        for noise in ["\"id\":\"" + fallbackMapping.id, "secretRef", "pythonKind", "main_accounts", "backup_accounts", "modelPatterns"] {
            XCTAssertFalse(text.contains(noise), "配置里不该出现 \(noise)")
        }
        XCTAssertTrue(text.contains("\"schemaVersion\":7"))
    }

    func testConfigDecodesWithMissingOptionalKeys() throws {
        // 手写的精简配置:少写的键都应落到合理默认值,而不是解码失败。
        let json = """
        {
          "endpoints": [
            {"id": "e1", "baseURL": "https://e1.example.com", "apiKey": "k"}
          ]
        }
        """
        let config = try JSONDecoder().decode(AppConfig.self, from: Data(json.utf8))
        XCTAssertEqual(config.schemaVersion, AppConfig.currentSchemaVersion)
        XCTAssertEqual(config.listener.port, 57_878)
        XCTAssertEqual(config.listener.authToken, "")
        XCTAssertNil(config.retry.responseTimeoutSeconds)
        XCTAssertEqual(config.retry.maxRetryDurationSeconds, 0)
        XCTAssertEqual(config.retry.sessionStickyRetries, 2)

        let endpoint = try XCTUnwrap(config.endpoints.first)
        XCTAssertEqual(endpoint.name, "e1", "缺 name 时回退成 id")
        XCTAssertEqual(endpoint.protocolMode, .auto)
        XCTAssertTrue(endpoint.enabled)
        XCTAssertNil(endpoint.stickyGroup)
        XCTAssertTrue(endpoint.mappings.isEmpty)

        XCTAssertTrue(config.endpoints.first?.mappings.isEmpty == true)
        // 内建分流规则即使没写也会补齐(默认停用)。
        XCTAssertEqual(Set(config.featureRules.map(\.id)), BuiltInFeatureRules.ids)
        XCTAssertTrue(config.featureRules.allSatisfy { !$0.enabled })
    }

    func testNormalizedDoesNotCreatePoolLevelModelRules() throws {
        let normalized = AppConfig.bootstrap.normalizedBuiltInFeatureRules()
        XCTAssertTrue(normalized.primaryPool?.endpoints.isEmpty == true)
        XCTAssertTrue(normalized.primaryPool?.acceptedModels.isEmpty == true)
    }

    func testPoolAcceptedModelsIsUnionOfEntryMappings() throws {
        let pool = Pool(
            id: "primary",
            name: "主池",
            role: .primary,
            endpoints: [
                Endpoint(
                    id: "a",
                    name: "a",
                    baseURL: try XCTUnwrap(URL(string: "https://a.example.com")),
                    mappings: [ModelMapping(clientPattern: "gpt-5.4"), ModelMapping(clientPattern: "claude-opus-*")]
                ),
                Endpoint(
                    id: "b",
                    name: "b",
                    baseURL: try XCTUnwrap(URL(string: "https://b.example.com")),
                    mappings: [ModelMapping(clientPattern: "qwen3.7-plus")]
                )
            ]
        )
        XCTAssertEqual(pool.acceptedModels, ["gpt-5.4", "claude-opus-*", "qwen3.7-plus"])
        XCTAssertTrue(pool.matches(model: "claude-opus-4-8"))
        XCTAssertTrue(pool.matches(model: "qwen3.7-plus"))
        XCTAssertFalse(pool.matches(model: "deepseek-v4"))

        let bare = Pool(id: "primary", name: "Provider", role: .primary)
        XCTAssertTrue(bare.acceptedModels.isEmpty, "没有规则或映射时 Provider 不承接模型")
        XCTAssertFalse(bare.matches(model: "claude-opus-4-8"))
    }

    func testBuiltInFeatureRulesNormalizeMatchAndPreserveUserTarget() {
        var config = AppConfig.bootstrap
        config.featureRules[0] = FeatureRule(
            id: "websearch",
            name: "User Edited",
            enabled: true,
            match: FeatureMatch(systemContains: "wrong matcher"),
            target: RouteTarget(poolID: "primary", model: "custom-target", effortOverride: .high)
        )

        let normalized = config.normalizedBuiltInFeatureRules()
        let rule = normalized.featureRules[0]

        XCTAssertEqual(rule.id, "websearch")
        XCTAssertEqual(rule.name, "WebSearch")
        XCTAssertTrue(rule.enabled)
        XCTAssertEqual(
            rule.target,
            RouteTarget(poolID: "primary", model: "custom-target", effortOverride: .high)
        )
        XCTAssertEqual(rule.match, FeatureMatch(requestKind: .websearch))
    }

    func testFeatureRouteEffortOverrideUsesStableOptionalWireValue() throws {
        let target = RouteTarget(poolID: "primary", model: "gpt-5.6-luna", effortOverride: .xhigh)
        let encoded = try JSONEncoder().encode(target)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertEqual(object["effort"] as? String, "xhigh")
        XCTAssertEqual(try JSONDecoder().decode(RouteTarget.self, from: encoded), target)

        let inherited = RouteTarget(poolID: "primary", model: "gpt-5.6-luna")
        let inheritedObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(inherited)) as? [String: Any]
        )
        XCTAssertNil(inheritedObject["effort"])
    }

    func testOpenAIBridgeCreatesAnthropicEvents() {
        let chunks = [
            OpenAIStreamChunk(
                model: "gpt-test",
                choices: [
                    OpenAIStreamChunk.Choice(
                        delta: .init(content: "hi"),
                        finishReason: nil
                    )
                ],
                usage: nil
            ),
            OpenAIStreamChunk(
                model: "gpt-test",
                choices: [
                    OpenAIStreamChunk.Choice(
                        delta: .init(content: nil),
                        finishReason: "stop"
                    )
                ],
                usage: .init(completionTokens: 7)
            )
        ]

        let events = OpenAIBridge.anthropicEvents(from: chunks, messageID: "msg_test")
        XCTAssertEqual(events.first?.event, "message_start")
        XCTAssertTrue(events.contains { $0.event == "content_block_delta" })
        XCTAssertEqual(events.last?.event, "message_stop")
        XCTAssertTrue(String(data: events[2].bytes(), encoding: .utf8)?.contains("hi") == true)
    }

    func testOpenAIBridgeParsesSSEBody() {
        let body = Data("""
        event: ignored
        data: {"model":"gpt-test","choices":[{"delta":{"content":"hello"},"finish_reason":null}]}

        data: {"model":"gpt-test","choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"completion_tokens":3}}

        data: [DONE]

        """.utf8)

        let chunks = OpenAIBridge.openAIStreamChunks(fromSSEBody: body)
        XCTAssertEqual(chunks.count, 2)
        XCTAssertEqual(chunks.first?.choices.first?.delta?.content, "hello")

        let bridged = String(
            data: OpenAIBridge.anthropicSSEBody(fromOpenAISSEBody: body, messageID: "msg_test"),
            encoding: .utf8
        )
        XCTAssertTrue(bridged?.contains("content_block_delta") == true)
        XCTAssertTrue(bridged?.contains("message_stop") == true)
    }

    func testClientAccessControlAllowsLoopbackAndCIDR() {
        XCTAssertTrue(ClientAccessControl.isAllowed(clientHost: "127.0.0.1", allowedCIDRs: ["10.0.0.0/8"]))
        XCTAssertTrue(ClientAccessControl.isAllowed(clientHost: "::1", allowedCIDRs: ["10.0.0.0/8"]))
        XCTAssertTrue(ClientAccessControl.isAllowed(clientHost: "10.2.3.4", allowedCIDRs: ["10.0.0.0/8"]))
        XCTAssertFalse(ClientAccessControl.isAllowed(clientHost: "192.168.1.8", allowedCIDRs: ["10.0.0.0/8"]))
        XCTAssertTrue(ClientAccessControl.isAllowed(clientHost: "::ffff:10.2.3.4", allowedCIDRs: ["10.0.0.0/8"]))
    }

    func testCIDRValidationForSettingsForm() {
        XCTAssertTrue(ClientAccessControl.isValidCIDR("192.168.1.0/24"))
        XCTAssertTrue(ClientAccessControl.isValidCIDR("10.0.0.0/8"))
        XCTAssertTrue(ClientAccessControl.isValidCIDR("1.2.3.4"))          // 裸地址 = 全前缀
        XCTAssertTrue(ClientAccessControl.isValidCIDR("2001:db8::/32"))
        XCTAssertFalse(ClientAccessControl.isValidCIDR("192.168.1.0/33"))  // 前缀越界
        XCTAssertFalse(ClientAccessControl.isValidCIDR("999.1.1.1/24"))
        XCTAssertFalse(ClientAccessControl.isValidCIDR("not-an-ip"))
        XCTAssertFalse(ClientAccessControl.isValidCIDR(""))
    }

    func testContextModeStripUsesStableWireValue() throws {
        let data = try JSONEncoder().encode(ContextMode.strip)
        XCTAssertEqual(String(decoding: data, as: UTF8.self), #""strip""#)
        XCTAssertEqual(try JSONDecoder().decode(ContextMode.self, from: data), .strip)
        XCTAssertEqual(ContextMode.strip.displayName, "剥离 1M")
    }

    func testListenerNeverEncodesRemovedInboundDialectPassthrough() throws {
        let off = ListenerConfig(
            host: "0.0.0.0",
            port: 12345,
            allowedCIDRs: ["10.0.0.0/8"],
            authToken: "sk-inbound"
        )
        let offTree = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: try JSONEncoder().encode(off)) as? [String: Any]
        )
        XCTAssertNil(offTree["inboundDialectPassthrough"])
        // 删除旧字段不能影响其余 Listener 字段。
        XCTAssertEqual(offTree["host"] as? String, "0.0.0.0")
        XCTAssertEqual(offTree["port"] as? Int, 12345)
        XCTAssertEqual(offTree["allowedCIDRs"] as? [String], ["10.0.0.0/8"])
        XCTAssertEqual(offTree["authToken"] as? String, "sk-inbound")

        let roundTripped = try JSONDecoder().decode(
            ListenerConfig.self,
            from: try JSONEncoder().encode(off)
        )
        XCTAssertEqual(roundTripped, off)
    }

    /// 列表选中的 id 必须**跨 decode 稳定**:旧版每次 decode 现生成 UUID,
    /// 「重新加载配置」或外部 /__reload 后选中丢失、开着的编辑 sheet 保存报「不存在」。
    func testListSelectionIDsSurviveReload() throws {
        let json = """
        {
          "schemaVersion": 3,
          "listener": {"host": "127.0.0.1", "port": 57878, "allowedCIDRs": [], "authToken": ""},
          "retry": {"responseTimeoutSeconds": null, "streamIdleTimeoutSeconds": null,
                    "max500Retries": 0, "retryDelaySeconds": null,
                    "maxDeferredRounds": 0, "maxRetryDurationSeconds": 1800, "sessionStickyRetries": 2},
          "endpoints": [{
            "id": "e1", "name": "e1", "baseURL": "https://a.example.com",
            "protocol": "anthropic", "enabled": true, "apiKey": "k",
            "mappings": [
              {"clientPattern": "gpt-5.4", "upstreamModel": "gpt-5.4",
               "thinking": "disabled", "context": "standard"},
              {"clientPattern": "claude-opus-*", "upstreamModel": "",
               "thinking": "adaptive", "context": "oneMillion"},
              {"clientPattern": "claude-fable-5", "upstreamModel": "",
               "thinking": "adaptive", "context": "oneMillion"}
            ]
          }],
          "featureRules": []
        }
        """
        let data = Data(json.utf8)
        let first = try JSONDecoder().decode(AppConfig.self, from: data)
        let second = try JSONDecoder().decode(AppConfig.self, from: data)

        // 显式写入的旧值保持不变；只有缺省/缺键才使用新的 0 无限默认。
        XCTAssertEqual(first.retry.maxRetryDurationSeconds, 1800)

        // 两次独立 decode 的 id 必须一致(模拟 reload 前后)。
        XCTAssertEqual(
            first.endpoints[0].mappings.map(\.id),
            second.endpoints[0].mappings.map(\.id)
        )
        // 且是内容派生值,不是 UUID 噪声。
        XCTAssertEqual(first.endpoints[0].mappings.map(\.id), ["gpt-5.4", "claude-opus-*", "claude-fable-5"])

        // 删掉第一条后,后续映射的 id 不变。
        var mutated = first
        mutated.endpoints[0].mappings.removeFirst()
        XCTAssertEqual(mutated.endpoints[0].mappings.map(\.id), ["claude-opus-*", "claude-fable-5"])
    }
}

/// 本地 TCP 监听器,只用来观察「有没有连接进来」。
/// 用真实 socket 而不是 URLProtocol:代理是 URLSession 之下的连接层行为,
/// URLProtocol 会把请求整个拦掉,根本走不到代理判定。
private final class LocalConnectionProbe: @unchecked Sendable {
    private let listenFD: Int32
    private let lock = NSLock()
    private var sawConnection = false
    private var stopped = false
    let port: UInt16

    init() throws {
        // 全程用局部 fd:成员在 port 赋值前不能被闭包捕获。
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw NSError(domain: "LocalConnectionProbe", code: 1)
        }
        var reuse: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = 0  // 让内核挑端口
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        let bound = withUnsafePointer(to: &addr) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                bind(fd, sockaddrPointer, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0, listen(fd, 8) == 0 else {
            close(fd)
            throw NSError(domain: "LocalConnectionProbe", code: 2)
        }

        var actual = sockaddr_in()
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let named = withUnsafeMutablePointer(to: &actual) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                getsockname(fd, sockaddrPointer, &length)
            }
        }
        guard named == 0 else {
            close(fd)
            throw NSError(domain: "LocalConnectionProbe", code: 3)
        }
        port = UInt16(bigEndian: actual.sin_port)
        listenFD = fd

        Thread.detachNewThread { [weak self] in
            while true {
                let accepted = accept(fd, nil, nil)
                if accepted < 0 { return }
                close(accepted)
                guard let self else { return }
                self.lock.lock()
                self.sawConnection = true
                self.lock.unlock()
            }
        }
    }

    func waitForConnection(timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            lock.lock()
            let seen = sawConnection
            lock.unlock()
            if seen { return true }
            usleep(20_000)
        }
        return false
    }

    func reset() {
        lock.lock()
        sawConnection = false
        lock.unlock()
    }

    func stop() {
        lock.lock()
        defer { lock.unlock() }
        guard !stopped else { return }
        stopped = true
        close(listenFD)
    }
}

/// 返回 302 的 stub:证明模型目录探测不跟随重定向(对齐 outbound.rs 的
/// `reqwest::redirect::Policy::none()`)。跟随了就会二次进入 startLoading。
private final class RedirectingCatalogURLProtocol: URLProtocol {
    /// 被请求过的路径,用来判断有没有跟着 Location 走。
    nonisolated(unsafe) static var requestedPaths: [String] = []

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let url = request.url!
        Self.requestedPaths.append(url.path)
        if url.path.hasSuffix("/redirected") {
            // 只有跟随了重定向才会走到这里,给个「成功」响应,好让断言能区分。
            let response = HTTPURLResponse(
                url: url,
                statusCode: 200,
                httpVersion: nil,
                headerFields: ["Content-Type": "application/json"]
            )!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: Data(#"{"data":[{"id":"followed-redirect"}]}"#.utf8))
            client?.urlProtocolDidFinishLoading(self)
            return
        }
        let target = url.deletingLastPathComponent().appendingPathComponent("redirected")
        let response = HTTPURLResponse(
            url: url,
            statusCode: 302,
            httpVersion: nil,
            headerFields: ["Location": target.absoluteString]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// URLSession stub used to prove the anonymous catalog path does not emit
/// empty or synthetic authentication headers. It never opens a real socket.
private final class AnonymousCatalogURLProtocol: URLProtocol {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let headers = request.allHTTPHeaderFields ?? [:]
        let hasAuth = headers.keys.contains { key in
            let normalized = key.lowercased()
            return normalized == "authorization" || normalized == "x-api-key"
        }
        let status = hasAuth ? 401 : 200
        let body = hasAuth
            ? Data(#"{"error":"unexpected auth header"}"#.utf8)
            : Data(#"{"data":[{"id":"anonymous-model"}]}"#.utf8)
        let response = HTTPURLResponse(
            url: request.url!,
            statusCode: status,
            httpVersion: nil,
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
