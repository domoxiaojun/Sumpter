import Foundation
import XCTest
@testable import SumpterCore

final class ModelGroupsTests: XCTestCase {
    private func config() -> AppConfig {
        AppConfig(endpoints: ["a", "b"].map { id in Endpoint(id: id, name: id,
            baseURL: URL(string: "https://\(id).invalid")!,
            mappings: [ModelMapping(clientPattern: "gpt-*", thinking: .adaptive, effort: .high)]) })
    }

    func testMigrationPreservesRetryMappingAndWildcard() throws {
        var c = config(); let retry = c.retry
        c.migrateModelGroups(); try c.validateModelGroups()
        XCTAssertEqual(c.retry, retry)
        XCTAssertEqual(c.modelGroups?.first?.models, ["gpt-*"])
        let projected = c.routingEndpoints(for: "gpt-not-in-catalog")
        XCTAssertEqual(projected.map(\.id), ["a", "b"])
        XCTAssertEqual(projected[0].mappings.first?.effort, .high)
        XCTAssertEqual(projected[0].stickyGroup, c.endpoints[0].stickyGroup)
    }

    func testAllAndSelectedModelScopeOverridesAndEmptyGroups() throws {
        var c = config()
        c.modelGroups = [ModelGroup(id: "main", models: ["gpt-x", "claude-x"], bindings: [
            ModelGroupBinding(endpointID: "a", priority: 2, models: nil),
            ModelGroupBinding(endpointID: "b", models: ["gpt-x"], overrides: [ModelGroupModelOverride(model: "gpt-x", upstreamModel: "private-gpt", priority: 1)])])]
        try c.validateModelGroups()
        let projected = c.routingEndpoints(for: "gpt-x")
        XCTAssertEqual(projected[1].preferredMapping(for: "gpt-x")?.upstreamModel, "private-gpt")
        XCTAssertEqual(projected[1].preferredMapping(for: "gpt-x")?.effort, .high)
        XCTAssertNil(projected[1].preferredMapping(for: "claude-x"))
        c.modelGroups = []; XCTAssertTrue(c.routingEndpoints(for: "gpt-x").isEmpty)
    }

    func testWireRoundtripAndReferencePruning() throws {
        var c = config(); c.migrateModelGroups()
        c.modelGroups?[0].bindings[0].models = nil
        let data = try JSONEncoder().encode(c)
        let restored = try JSONDecoder().decode(AppConfig.self, from: data)
        XCTAssertEqual(restored.modelGroups, c.modelGroups)
        XCTAssertEqual(restored.endpoints[0].mappings[0].effort, .high)
        c.endpoints.removeLast(); c.pruneDanglingEndpointReferences()
        try c.validateModelGroups()
        XCTAssertEqual(c.modelGroups?[0].bindings.map(\.endpointID), ["a"])
    }

    func testV6StoreMigrationCreatesGroupsWithoutChangingRetry() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let url = dir.appendingPathComponent("config.json")
        var wire = try JSONSerialization.jsonObject(with: JSONEncoder().encode(config())) as! [String: Any]
        wire["schemaVersion"] = 6
        try JSONSerialization.data(withJSONObject: wire).write(to: url)
        let result = try ConfigStore(url: url).loadWithMigration()
        XCTAssertEqual(result.config.schemaVersion, 7)
        XCTAssertNotNil(result.migrationNotice)
        XCTAssertEqual(result.config.retry, config().retry)
        XCTAssertEqual(result.config.modelGroups?.first?.bindings.count, 2)
    }

    func testCurrentStoreRejectsExplicitNullModelGroups() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("config.json")
        let data = Data(#"{"schemaVersion":7,"endpoints":[],"modelGroups":null}"#.utf8)
        try data.write(to: url)
        XCTAssertThrowsError(try ConfigStore(url: url).loadWithMigration())
    }

    func testProjectedExactMappingKeepsPrecedenceOverSourceWildcard() throws {
        var c = config()
        c.endpoints[0].mappings = [
            ModelMapping(clientPattern: "*", upstreamModel: "broad-model",
                         failoverTimeoutSeconds: 3, effort: .low),
            ModelMapping(clientPattern: "gpt-x", upstreamModel: "exact-model", thinking: .passthrough,
                         failoverTimeoutSeconds: 19, capabilities: ["text"], effort: .high)
        ]
        c.modelGroups = [ModelGroup(id: "main", models: ["gpt-x"],
                                    bindings: [ModelGroupBinding(endpointID: "a", models: nil)])]
        let projected = c.routingEndpoints(for: "gpt-x").first!
        let mapping = try XCTUnwrap(projected.preferredMapping(for: "gpt-x"))
        XCTAssertEqual(mapping.upstreamModel, "exact-model")
        XCTAssertEqual(mapping.failoverTimeoutSeconds, 19)
        XCTAssertEqual(mapping.effort, .high)
        XCTAssertEqual(mapping.capabilities, ["text"])
    }

    func testWildcardGroupInheritsLongestPrefixAndValidatesRawScopes() throws {
        var c = config()
        c.endpoints[0].mappings = [
            ModelMapping(clientPattern: "*", upstreamModel: "broad"),
            ModelMapping(clientPattern: "gpt-*", upstreamModel: "specific")
        ]
        c.modelGroups = [ModelGroup(id: "main", models: ["*"], bindings: [ModelGroupBinding(endpointID: "a", models: nil)])]
        XCTAssertEqual(c.routingEndpoints(for: "gpt-x")[0].preferredMapping(for: "gpt-x")?.upstreamModel, "specific")
        for invalid in [" ", "(high)", "gpt*x", "gpt**"] {
            c.modelGroups?[0].bindings[0].models = [invalid]
            XCTAssertThrowsError(try c.validateModelGroups())
        }
    }

    func testModelCategoriesAreDerivedOnlyFromEndpointMappings() {
        var endpoints = config().endpoints
        endpoints[0].catalog = ModelCatalog(models: [" GPT-5.6-sol ", "claude_opus_5", "plain"])
        endpoints[1].mappings = [
            ModelMapping(clientPattern: "vendor:model"),
            ModelMapping(clientPattern: "gpt-5.6-sol")
        ]
        let categories = ModelGroupCatalog.categories(endpoints: endpoints)
        XCTAssertEqual(categories.map(\.id), ["gpt", "vendor"])
        XCTAssertEqual(categories.first(where: { $0.id == "gpt" })?.models, ["gpt-*", "gpt-5.6-sol"])
        XCTAssertEqual(ModelGroupCatalog.prefix(for: "vendor:model"), "vendor")
        XCTAssertEqual(ModelGroupCatalog.prefix(for: "plain"), ModelGroupCatalog.otherID)
    }

    func testVersionedFamiliesKeepOriginalModelIDs() {
        let categories = ModelGroupCatalog.categories(models: ["qwen-turbo", "qwen3.6-plus", "Qwen3.7-plus", "qwen2.5", "wan2.7-video"])
        XCTAssertEqual(categories.map(\.id), ["qwen", "wan"])
        XCTAssertEqual(categories[0].models.count, 4)
        XCTAssertTrue(categories[0].models.contains("Qwen3.7-plus"))
        XCTAssertEqual(ModelGroupCatalog.prefix(for: "plain"), ModelGroupCatalog.otherID)
    }

    func testBindingCandidatesIntersectOnlyThisEndpointSupport() {
        var endpoint = config().endpoints[0]
        endpoint.catalog = ModelCatalog(models: ["qwen3.6-plus", "qwen3.7-plus", "private-alias", "out-of-group"])
        endpoint.mappings = [ModelMapping(clientPattern: "gpt-*"), ModelMapping(clientPattern: "custom")]
        let group = ["qwen*", "gpt-x", "custom", "alias", "claude-x"]
        let original = endpoint
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: endpoint, groupModels: group), ["custom", "gpt-x"])
        XCTAssertEqual(endpoint, original)
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: nil, groupModels: group), [])
        endpoint.catalog = ModelCatalog(); endpoint.mappings = []
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: endpoint, groupModels: group), [])
        endpoint.mappings = [ModelMapping(clientPattern: "*")]
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: endpoint, groupModels: ["qwen*", "claude-x"]), ["claude-x", "qwen*"])
    }

    func testFetchedOnlyCatalogsNeverCreateCandidates() {
        var endpoint = config().endpoints[0]
        endpoint.catalog = ModelCatalog(models: ["qwen3.6-plus", "qwen3.7-plus"])
        endpoint.mappings = []
        XCTAssertEqual(ModelGroupCatalog.categories(endpoints: [endpoint]), [])
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: endpoint, groupModels: ["*"]), [])
        endpoint.mappings = [ModelMapping(clientPattern: "qwen3.6-plus", upstreamModel: "private-qwen")]
        XCTAssertEqual(ModelGroupCatalog.categories(endpoints: [endpoint]).flatMap(\.models), ["qwen3.6-plus"])
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: endpoint, groupModels: ["qwen*"]), ["qwen3.6-plus"])
        endpoint.mappings = [ModelMapping(clientPattern: "qwen*")]
        XCTAssertEqual(ModelGroupCatalog.categories(endpoints: [endpoint]).flatMap(\.models), ["qwen*"])
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: endpoint, groupModels: ["*"]), ["qwen*"])
    }

    func testAddedClientNamesAndWildcardsDeduplicateAcrossEndpoints() {
        var endpoints = config().endpoints
        endpoints[0].mappings = [ModelMapping(clientPattern: "alias", upstreamModel: "private-model"), ModelMapping(clientPattern: "gpt-*")]
        endpoints[1].mappings = [ModelMapping(clientPattern: "alias", upstreamModel: "another-upstream"), ModelMapping(clientPattern: "qwen3.7-plus")]
        XCTAssertEqual(ModelGroupCatalog.categories(endpoints: endpoints).flatMap(\.models).sorted(), ["alias", "gpt-*", "qwen3.7-plus"])
        XCTAssertEqual(ModelGroupCatalog.availableModels(endpoint: endpoints[0], groupModels: ["alias", "qwen3.7-plus"]), ["alias"])
    }

    /// 默认组绑定跟随入口库:入口库把 b 移到顺序 1(优先级 1)后,默认组
    /// 的绑定顺序与优先级同步为 b(1) → a(5) → c(9);悬空引用剔除、
    /// 刻意排除的入口不会被自动补回。与 Rust 同名行为对齐。
    func testSyncDefaultGroupBindingsFollowsEndpointLibraryOrder() throws {
        var c = AppConfig(endpoints: [
            Endpoint(id: "a", name: "A", baseURL: URL(string: "https://a.invalid")!,
                     priority: 5, mappings: [ModelMapping(clientPattern: "gpt-*")]),
            Endpoint(id: "b", name: "B", baseURL: URL(string: "https://b.invalid")!,
                     priority: 1, mappings: [ModelMapping(clientPattern: "gpt-*")]),
            Endpoint(id: "c", name: "C", baseURL: URL(string: "https://c.invalid")!,
                     priority: 9, mappings: [ModelMapping(clientPattern: "gpt-*")]),
        ])
        c.modelGroups = [ModelGroup(id: "default", name: "默认模型组", models: ["gpt-*"], bindings: [
            ModelGroupBinding(endpointID: "a", priority: 5, models: nil),
            ModelGroupBinding(endpointID: "b", priority: 3, models: nil),
            ModelGroupBinding(endpointID: "deleted", priority: 2, models: nil),
            ModelGroupBinding(endpointID: "c", priority: 9, models: ["gpt-x"]),
        ])]
        // 用户在入口库把 b 移到顺序 1,并把 b 的优先级调成 1。
        let b = c.endpoints.remove(at: 1)
        c.endpoints.insert(b, at: 0)
        c.endpoints[0].priority = 1

        c.syncDefaultGroupBindings()
        try c.validateModelGroups()

        let bindings = c.modelGroups?.first?.bindings ?? []
        XCTAssertEqual(bindings.map(\.endpointID), ["b", "a", "c"], "顺序跟随入口库数组")
        XCTAssertEqual(bindings.map(\.priority), [1, 5, 9], "优先级跟随入口库 Priority")
        XCTAssertFalse(bindings.contains { $0.endpointID == "deleted" }, "悬空引用被剔除")
        XCTAssertEqual(bindings.first { $0.endpointID == "c" }?.models, ["gpt-x"], "承接范围等组内设置保留")

        // 非默认组不被同步。
        c.modelGroups?.append(ModelGroup(id: "custom", models: ["gpt-*"], bindings: [
            ModelGroupBinding(endpointID: "c", priority: 0, models: nil),
        ]))
        c.syncDefaultGroupBindings()
        XCTAssertEqual(c.modelGroups?.last?.bindings.map(\.endpointID), ["c"])
        XCTAssertEqual(c.modelGroups?.last?.bindings.first?.priority, 0)
    }
}
