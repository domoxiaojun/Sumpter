import XCTest
@testable import SumpterCore

/// 覆盖从设置界面抽出的纯逻辑：输入校验、候选模型派生、配置告警、表格选择、提交状态机。
final class UILogicTests: XCTestCase {
    // MARK: - InputValidation

    func testInputValidationURLTrimsAndRejectsInvalid() {
        XCTAssertEqual(try InputValidation.url("  https://api.example.com  ", field: "API 地址"), "https://api.example.com")

        for bad in ["", "   ", "not a url", "http://", "example.com",
                    "ftp://api.example.com", "https://user:pass@api.example.com",
                    "https://api.example.com?token=secret", "https://api.example.com#frag"] {
            XCTAssertThrowsError(try InputValidation.url(bad, field: "API 地址")) { error in
                XCTAssertEqual((error as? InputValidationError)?.message, "API 地址无效")
            }
        }
    }

    func testInputValidationProviderID() {
        XCTAssertEqual(try InputValidation.providerID("  backup-third.v2_1  "), "backup-third.v2_1")

        XCTAssertThrowsError(try InputValidation.providerID("   ")) { error in
            XCTAssertEqual((error as? InputValidationError)?.message, "入口 ID 不能为空")
        }
        for bad in ["has space", "bad/slash", "bad:colon"] {
            XCTAssertThrowsError(try InputValidation.providerID(bad)) { error in
                XCTAssertEqual((error as? InputValidationError)?.message, "入口 ID 只能包含字母、数字、点、下划线和横线")
            }
        }
    }

    func testInputValidationTimeoutNormalization() {
        XCTAssertEqual(InputValidation.timeout(30), 30)
        XCTAssertEqual(InputValidation.timeout(0.5), 1)
        XCTAssertEqual(InputValidation.timeout(-5), 1)
        XCTAssertEqual(InputValidation.timeout(.infinity), 15)
        XCTAssertEqual(InputValidation.timeout(.nan), 15)
    }

    func testInputValidationSanitizeProviderIDBase() {
        XCTAssertEqual(InputValidation.sanitizeProviderIDBase("Backup Third"), "backup-third")
        XCTAssertEqual(InputValidation.sanitizeProviderIDBase("A  B"), "a-b")
        XCTAssertEqual(InputValidation.sanitizeProviderIDBase("!!!"), "provider")
        XCTAssertEqual(InputValidation.sanitizeProviderIDBase("   "), "provider")
    }

    func testInputValidationUniqueProviderID() {
        XCTAssertEqual(InputValidation.uniqueProviderID(name: "Backup", existingIDs: []), "backup")
        XCTAssertEqual(InputValidation.uniqueProviderID(name: "Backup", existingIDs: ["backup"]), "backup-2")
        XCTAssertEqual(InputValidation.uniqueProviderID(name: "Backup", existingIDs: ["backup", "backup-2"]), "backup-3")
    }

    func testInputValidationRetryPolicyParsing() throws {
        let parsed = try InputValidation.retryPolicy(
            responseTimeoutText: " 9 ",
            streamIdleTimeoutText: "30",
            max500RetriesText: "3",
            failoverOn500: false,
            retryDelaySecondsText: "4.5",
            maxDeferredRoundsText: "2",
            maxRetryDurationSecondsText: "5",
            sessionStickyRetriesText: "2",
            pinnedIPConcurrencyText: "4"
        )
        XCTAssertEqual(parsed.responseTimeoutSeconds, 9)
        XCTAssertEqual(parsed.streamIdleTimeoutSeconds, 30)
        XCTAssertEqual(parsed.max500Retries, 3)
        XCTAssertFalse(parsed.failoverOn500)
        XCTAssertEqual(parsed.retryDelaySeconds, 4.5)
        XCTAssertEqual(parsed.maxDeferredRounds, 2)
        XCTAssertEqual(parsed.maxRetryDurationSeconds, 5)
        XCTAssertEqual(parsed.sessionStickyRetries, 2)
        XCTAssertEqual(parsed.pinnedIPConcurrency, 4)

        let clientControlled = try InputValidation.retryPolicy(
            responseTimeoutText: " ",
            streamIdleTimeoutText: "",
            maxDeferredRoundsText: "0",
            maxRetryDurationSecondsText: "0",
            sessionStickyRetriesText: "0",
            pinnedIPConcurrencyText: "3"
        )
        XCTAssertNil(clientControlled.responseTimeoutSeconds)
        XCTAssertNil(clientControlled.streamIdleTimeoutSeconds)
        XCTAssertEqual(clientControlled.maxRetryDurationSeconds, 0)
    }

    func testInputValidationRetryPolicyRejectsBadValues() {
        let cases: [(String, String, String, String, String, String, String)] = [
            ("0", "45", "0", "2", "5", "3", "首响应截止必须留空或大于 0"),
            ("8", "0", "0", "2", "5", "3", "流式空闲截止必须留空或大于 0"),
            ("8", "45", "-1", "2", "5", "3", "故障重试最大轮数必须为 0 或正整数"),
            ("8", "45", "0", "2", "-1", "3", "跨轮最长时长必须为 0 或正数"),
            ("8", "45", "0", "-1", "5", "3", "粘性入口额外重试次数必须为 0 或正整数"),
            ("8", "45", "0", "2", "5", "0", "固定 IP 并发数必须大于 0")
        ]
        for (timeout, streamIdle, rounds, sticky, retrySeconds, ip, expected) in cases {
            XCTAssertThrowsError(try InputValidation.retryPolicy(
                responseTimeoutText: timeout,
                streamIdleTimeoutText: streamIdle,
                maxDeferredRoundsText: rounds,
                maxRetryDurationSecondsText: retrySeconds,
                sessionStickyRetriesText: sticky,
                pinnedIPConcurrencyText: ip
            )) { error in
                XCTAssertEqual((error as? InputValidationError)?.message, expected)
            }
        }
    }

    // MARK: - 构造辅助

    private func makeEndpoint(
        _ id: String,
        key: String = "k-\(UUID().uuidString.prefix(4))",
        enabled: Bool = true,
        host: String? = nil,
        pinnedIPs: [String] = [],
        pinnedIPExclusive: Bool = false,
        stickyGroup: String? = nil,
        catalog: ModelCatalog = ModelCatalog(),
        mappings: [ModelMapping] = []
    ) -> Endpoint {
        Endpoint(
            id: id,
            name: id,
            baseURL: URL(string: "https://\(host ?? id).example.com")!,
            enabled: enabled,
            apiKey: key,
            pinnedIPs: pinnedIPs,
            pinnedIPExclusive: pinnedIPExclusive,
            stickyGroup: stickyGroup,
            catalog: catalog,
            mappings: mappings
        )
    }

    private func makeConfig(
        defaults: [ModelMapping] = [ModelMapping(clientPattern: "claude-opus-*")],
        primary: [Endpoint] = [],
        fallback: [Endpoint] = [],
        featureRules: [FeatureRule] = [],
        listener: ListenerConfig = ListenerConfig()
    ) -> AppConfig {
        var endpoints = primary + fallback
        for index in endpoints.indices where endpoints[index].mappings.isEmpty {
            endpoints[index].mappings = defaults
        }
        return AppConfig(
            listener: listener,
            pools: [
                Pool(
                    id: "primary",
                    name: "Provider",
                    role: .primary,
                    endpoints: endpoints
                )
            ],
            featureRules: featureRules
        ).normalizedBuiltInFeatureRules()
    }

    // MARK: - FeatureRouteCandidates

    func testPoolCandidatesUseExplicitEntryMappingsAfterNormalization() {
        let config = makeConfig(
            defaults: [],
            primary: [
                makeEndpoint("p1", mappings: [
                    ModelMapping(clientPattern: "gpt-5.4"),
                    ModelMapping(clientPattern: "claude-sonnet-4-6")
                ]),
                makeEndpoint("p2", mappings: [
                    ModelMapping(clientPattern: "qwen3.7-plus"),
                    ModelMapping(clientPattern: "gpt-5.4")
                ])
            ]
        )
        let pool = try! XCTUnwrap(config.primaryPool)
        XCTAssertEqual(
            FeatureRouteCandidates.models(for: pool),
            ["gpt-5.4", "claude-sonnet-4-6", "qwen3.7-plus"],
            "归一化后只展示入口显式声明的模型，并保持入口顺序"
        )
    }

    func testProviderCandidatesComeOnlyFromEntryMappings() {
        let config = makeConfig(
            defaults: [],
            fallback: [
                makeEndpoint("b1", mappings: [
                    ModelMapping(clientPattern: "claude-haiku-4-5-20251001"),
                    ModelMapping(clientPattern: "claude-sonnet-4-6")
                ]),
                makeEndpoint("b2", mappings: [
                    ModelMapping(clientPattern: "claude-haiku-4-5-20251001"),
                    ModelMapping(clientPattern: "deepseek-v4-flash")
                ])
            ]
        )
        let pool = try! XCTUnwrap(config.primaryPool)
        XCTAssertEqual(
            FeatureRouteCandidates.models(for: pool),
            ["claude-haiku-4-5-20251001", "claude-sonnet-4-6", "deepseek-v4-flash"]
        )
    }

    func testEndpointCandidatesUseMigratedMappingAndCatalog() {
        let mapped = makeEndpoint("mapped", mappings: [ModelMapping(clientPattern: "gpt-5.4")])
        let bare = makeEndpoint("bare", catalog: ModelCatalog(models: ["extra-model"]))
        let config = makeConfig(defaults: [], primary: [mapped, bare])
        let pool = try! XCTUnwrap(config.primaryPool)

        XCTAssertEqual(
            FeatureRouteCandidates.models(for: mapped, in: pool),
            ["gpt-5.4"],
            "有自带映射的入口只给映射里声明的"
        )
        let normalizedBare = try! XCTUnwrap(pool.endpoint(id: "bare"))
        XCTAssertEqual(
            FeatureRouteCandidates.models(for: normalizedBare, in: pool),
            ["extra-model"],
            "没有显式映射时只展示入口目录"
        )
        XCTAssertEqual(
            FeatureRouteCandidates.models(config: config, poolID: "primary", endpointID: "mapped"),
            ["gpt-5.4"]
        )
        XCTAssertEqual(
            FeatureRouteCandidates.models(config: config, poolID: "primary", endpointID: nil),
            ["gpt-5.4"]
        )
    }

    func testEndpointCandidatesDoNotUsePoolFallback() {
        let bare = makeEndpoint("bare", catalog: ModelCatalog(models: ["extra-model"]))
        let raw = AppConfig(
            pools: [Pool(
                id: "primary",
                name: "Provider",
                role: .primary,
                endpoints: [bare]
            )]
        )
        let pool = try! XCTUnwrap(raw.primaryPool)

        XCTAssertEqual(
            FeatureRouteCandidates.models(for: bare, in: pool),
            ["extra-model"],
            "池级模型规则不再参与运行时候选"
        )
    }

    // MARK: - TableSelection

    private struct SelectionRow: Identifiable, Equatable {
        let id: String
    }

    func testTableSelectionSingleSelected() {
        let rows = [SelectionRow(id: "a"), SelectionRow(id: "b"), SelectionRow(id: "c")]
        XCTAssertEqual(TableSelection.singleSelected(rows, selection: ["b"]), SelectionRow(id: "b"))
        XCTAssertNil(TableSelection.singleSelected(rows, selection: []))
        XCTAssertNil(TableSelection.singleSelected(rows, selection: ["a", "b"]))
        XCTAssertNil(TableSelection.singleSelected(rows, selection: ["missing"]))
    }

    func testTableSelectionSanitize() {
        XCTAssertEqual(TableSelection.sanitize(["a", "z"], validIDs: ["a", "b"], selectFirst: false), ["a"])
        XCTAssertEqual(TableSelection.sanitize([], validIDs: ["b", "a"], selectFirst: true), ["a"])
        XCTAssertEqual(TableSelection.sanitize(["z"], validIDs: ["a", "b"], selectFirst: true), ["a"])
        XCTAssertEqual(TableSelection.sanitize(["z"], validIDs: [], selectFirst: true), [])
        XCTAssertEqual(TableSelection.sanitize([], validIDs: ["a"], selectFirst: false), [])
    }

    func testTableSelectionCanMove() {
        let ids = ["a", "b", "c"]
        XCTAssertTrue(TableSelection.canMove(id: "b", orderedIDs: ids, direction: -1))
        XCTAssertTrue(TableSelection.canMove(id: "b", orderedIDs: ids, direction: 1))
        XCTAssertFalse(TableSelection.canMove(id: "a", orderedIDs: ids, direction: -1))
        XCTAssertFalse(TableSelection.canMove(id: "c", orderedIDs: ids, direction: 1))
        XCTAssertFalse(TableSelection.canMove(id: nil, orderedIDs: ids, direction: -1))
        XCTAssertFalse(TableSelection.canMove(id: "missing", orderedIDs: ids, direction: 1))
    }

    func testTableSelectionBlockMove() {
        let ids = ["a", "b", "c", "d", "e"]
        // 连续块整体上移/下移。
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: ["b", "c"], direction: -1), ["b", "c", "a", "d", "e"])
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: ["b", "d"], direction: 1), ["a", "c", "b", "e", "d"])
        // 抵边的行不动;紧随其后的选中行顶在它后面。
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: ["a", "b"], direction: -1), ids)
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: ["a", "c"], direction: -1), ["a", "c", "b", "d", "e"])
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: ["d", "e"], direction: 1), ids)
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: ["c", "e"], direction: 1), ["a", "b", "d", "c", "e"])
        // 空选/非法方向原样返回。
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: [], direction: -1), ids)
        XCTAssertEqual(TableSelection.moved(ids: ids, selection: ["b"], direction: 0), ids)
        // canMove(set) = 移动会产生变化。
        XCTAssertTrue(TableSelection.canMove(selection: ["b", "c"], orderedIDs: ids, direction: -1))
        XCTAssertFalse(TableSelection.canMove(selection: ["a", "b"], orderedIDs: ids, direction: -1))
        XCTAssertFalse(TableSelection.canMove(selection: [], orderedIDs: ids, direction: 1))
    }

    func testTableSelectionSingleMoveToIndex() {
        let ids = ["a", "b", "c", "d"]
        // 目标下标按“移除源元素之后”的数组计算，适合拖拽前/后插入。
        XCTAssertEqual(TableSelection.moved(id: "a", orderedIDs: ids, toIndex: 2), ["b", "c", "a", "d"])
        XCTAssertEqual(TableSelection.moved(id: "d", orderedIDs: ids, toIndex: 0), ["d", "a", "b", "c"])
        XCTAssertEqual(TableSelection.moved(id: "b", orderedIDs: ids, toIndex: 99), ["a", "c", "d", "b"])
        XCTAssertEqual(TableSelection.moved(id: "missing", orderedIDs: ids, toIndex: 1), ids)
    }

    // MARK: - SubmissionState

    func testSubmissionStateHappyPath() {
        var state = SubmissionState.idle
        XCTAssertFalse(state.isSubmitting)
        XCTAssertEqual(state.errorText, "")

        XCTAssertTrue(state.begin())
        XCTAssertEqual(state, .submitting)
        XCTAssertTrue(state.isSubmitting)
        XCTAssertEqual(state.errorText, "")

        state.succeed()
        XCTAssertEqual(state, .idle)
        XCTAssertFalse(state.isSubmitting)
    }

    func testSubmissionStateRejectsDoubleSubmit() {
        var state = SubmissionState.idle
        XCTAssertTrue(state.begin())
        XCTAssertFalse(state.begin(), "已在提交中应拒绝再次进入")
        XCTAssertEqual(state, .submitting)
    }

    func testSubmissionStateFailureRetainsMessageThenClearsOnRetry() {
        var state = SubmissionState.idle
        _ = state.begin()
        state.fail("网络错误")
        XCTAssertEqual(state, .failed("网络错误"))
        XCTAssertFalse(state.isSubmitting)
        XCTAssertEqual(state.errorText, "网络错误")

        // 失败后可再次提交，并在进入提交态时清空上一次错误。
        XCTAssertTrue(state.begin())
        XCTAssertTrue(state.isSubmitting)
        XCTAssertEqual(state.errorText, "")
    }
}
