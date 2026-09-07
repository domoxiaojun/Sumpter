import Foundation
import SumpterCore

/// sumpterd admin API 客户端(127.0.0.1 + X-Control-Token)。
///
/// wire 模型独立定义(`AdminWire`),不依赖 SumpterProxy(该 target 将被删除);
/// `Date` 用 JSONDecoder 默认策略(secondsSinceReferenceDate)—— 与 sumpterd 的
/// Apple 纪元时间戳天然兼容。
public struct AdminClient: Sendable {
    public let port: Int
    public let token: String

    private static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 30
        config.waitsForConnectivity = false
        return URLSession(configuration: config)
    }()

    /// SSE 长连接专用(不设请求超时)。
    private static let streamSession: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 3600 * 24 * 365
        config.timeoutIntervalForResource = 3600 * 24 * 365
        return URLSession(configuration: config)
    }()

    public init(port: Int, token: String) {
        self.port = port
        self.token = token
    }

    public enum AdminError: Error, LocalizedError {
        case badStatus(Int, String)
        case invalidResponse
        case fileIO(String)

        public var errorDescription: String? {
            switch self {
            case .badStatus(let code, let body):
                return "admin API \(code):\(body)"
            case .invalidResponse:
                return "admin API 响应无法解析"
            case .fileIO(let message):
                return "保存诊断捕获失败：\(message)"
            }
        }

        /// Machine-readable error code returned by the local admin service.
        /// UI callers use this to distinguish an expired history snapshot from
        /// an ordinary query failure without treating it as a liveness error.
        public var serverCode: String? {
            guard case .badStatus(_, let body) = self,
                  let data = body.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                return nil
            }
            return object["error"] as? String
        }

        public var statusCode: Int? {
            guard case .badStatus(let code, _) = self else { return nil }
            return code
        }
    }

    private func request(_ path: String, method: String = "GET", query: [String: String] = [:]) -> URLRequest {
        var components = URLComponents()
        components.scheme = "http"
        components.host = "127.0.0.1"
        components.port = port
        components.path = path
        if !query.isEmpty {
            components.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) }
        }
        var request = URLRequest(url: components.url!)
        request.httpMethod = method
        request.setValue(token, forHTTPHeaderField: "X-Control-Token")
        return request
    }

    private func send<T: Decodable>(_ request: URLRequest, as type: T.Type) async throws -> T {
        let data = try await sendData(request)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func sendData(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await Self.session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw AdminError.invalidResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            throw AdminError.badStatus(http.statusCode, String(decoding: data, as: UTF8.self))
        }
        return data
    }

    private func jsonRequest(_ path: String, method: String, body: [String: Any]? = nil) throws -> URLRequest {
        var value = request(path, method: method)
        if let body {
            value.httpBody = try JSONSerialization.data(withJSONObject: body)
            value.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return value
    }

    // MARK: - 端点

    public func status() async throws -> AdminWire.Status {
        var value = request("/admin/status")
        // status 同时用于启动握手探针和周期 liveness；旧 admin 端口不可达时
        // 不应让 UI 等完整的 30 秒请求超时。
        value.timeoutInterval = 5
        return try await send(value, as: AdminWire.Status.self)
    }

    public func runtimeSummary() async throws -> AdminWire.RuntimeSummary {
        try await send(request("/admin/runtime/summary"), as: AdminWire.RuntimeSummary.self)
    }

    public func runtimeEvents(
        beforeSeq: Int? = nil,
        afterChangeSeq: Int? = nil,
        limit: Int = 10,
        kind: String? = nil,
        requestID: String? = nil,
        outcome: String? = nil,
        from: Double? = nil,
        to: Double? = nil
    ) async throws -> AdminWire.RuntimeEventPage {
        var query = ["limit": String(min(200, max(1, limit)))]
        if let beforeSeq { query["beforeSeq"] = String(beforeSeq) }
        if let afterChangeSeq { query["afterChangeSeq"] = String(afterChangeSeq) }
        if let kind { query["kind"] = kind }
        if let requestID { query["requestID"] = requestID }
        if let outcome { query["outcome"] = outcome }
        if let from { query["from"] = String(from) }
        if let to { query["to"] = String(to) }
        return try await send(request("/admin/runtime/events", query: query), as: AdminWire.RuntimeEventPage.self)
    }

    public func runtimeEvent(id: String) async throws -> AdminWire.RuntimeEventDetail {
        try await send(request("/admin/runtime/events/\(id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id)"), as: AdminWire.RuntimeEventDetail.self)
    }

    public func runtimeAnalytics(
        range: String = "24h",
        clientKind: String = "",
        endpointID: String = "",
        project: String = "",
        sessionID: String = "",
        from: Double? = nil,
        to: Double? = nil
    ) async throws -> AdminWire.RuntimeAnalytics {
        var query = ["range": range]
        if !clientKind.isEmpty { query["clientKind"] = clientKind }
        if !endpointID.isEmpty { query["endpointID"] = endpointID }
        if !project.isEmpty { query["project"] = project }
        if !sessionID.isEmpty { query["sessionID"] = sessionID }
        if let from { query["from"] = String(from) }
        if let to { query["to"] = String(to) }
        return try await send(request("/admin/runtime/analytics", query: query), as: AdminWire.RuntimeAnalytics.self)
    }

    public func runtimeAnalytics(range: String = "today", filter: AdminWire.RuntimeFilter) async throws -> AdminWire.RuntimeAnalytics {
        var query: [String: String] = ["range": range]
        filter.add(to: &query)
        return try await send(request("/admin/runtime/analytics", query: query), as: AdminWire.RuntimeAnalytics.self)
    }

    public func reload() async throws -> AdminWire.ReloadAck {
        try await send(request("/admin/reload", method: "POST"), as: AdminWire.ReloadAck.self)
    }

    /// 由 sumpterd 统一探测 Provider 模型目录，确保超时和出站
    /// 代理策略与真实转发一致；UI 进程不再直接连接上游。
    public func providerModels(endpointID: String) async throws -> AdminWire.ProviderModels {
        let body: [String: Any] = ["endpointID": endpointID]
        return try await send(
            jsonRequest("/admin/provider-models", method: "POST", body: body),
            as: AdminWire.ProviderModels.self
        )
    }

    public func resetRuntime() async throws {
        _ = try await send(request("/admin/runtime/reset", method: "POST"), as: AdminWire.ResetAck.self)
    }

    public func recreateRuntime() async throws {
        _ = try await send(request("/admin/runtime/recreate", method: "POST"), as: AdminWire.ResetAck.self)
    }

    public func previewRuntimeCleanup(olderThan: Double) async throws -> AdminWire.RuntimeCleanupPreview {
        try await send(
            jsonRequest("/admin/runtime/cleanup/preview", method: "POST", body: ["olderThan": olderThan]),
            as: AdminWire.RuntimeCleanupPreview.self
        )
    }

    public func cleanupRuntime(olderThan: Double) async throws -> AdminWire.RuntimeCleanupMutation {
        try await send(
            jsonRequest("/admin/runtime/cleanup", method: "POST", body: ["olderThan": olderThan]),
            as: AdminWire.RuntimeCleanupMutation.self
        )
    }

    public func deleteRuntimeSession(
        sessionID: String,
        confirmUnidentified: Bool = false
    ) async throws -> AdminWire.SessionMutation {
        var query = ["sessionID": sessionID]
        if confirmUnidentified {
            query["confirmUnidentified"] = "true"
        }
        return try await send(
            request("/admin/runtime/session", method: "DELETE", query: query),
            as: AdminWire.SessionMutation.self
        )
    }

    public func exportRuntimeSession(sessionID: String) async throws -> Data {
        try await sendData(request("/admin/runtime/session/export", query: ["sessionID": sessionID]))
    }

    /// 清除某项目的会话粘性归属(affinity 键来自该项目的历史事件)。
    /// 返回清除的归属条数;0 = 该项目当前没有粘性归属。
    @discardableResult
    public func clearProjectSticky(projectID: String) async throws -> Int {
        struct StickyClearAck: Decodable {
            let cleared: Int
            let matched: Int
        }
        let body: [String: Any] = ["projectID": projectID]
        let ack = try await send(
            jsonRequest("/admin/runtime/projects/sticky-clear", method: "POST", body: body),
            as: StickyClearAck.self
        )
        return ack.cleared
    }

    public func diagnostics() async throws -> AdminWire.Diagnostics {
        try await send(request("/admin/diagnostics"), as: AdminWire.Diagnostics.self)
    }

    /// 仅读取诊断捕获的轻量索引；明文请求、响应和 Chunk 通过详情接口按需读取。
    public func diagnosticCaptureIndex() async throws -> AdminWire.DiagnosticCaptureIndex {
        var value = request("/admin/diagnostic-capture")
        value.timeoutInterval = 10
        return try await send(value, as: AdminWire.DiagnosticCaptureIndex.self)
    }

    // MARK: - Runtime analytics v3

    /// Stable, SQLite-backed event history.  The v1 cursor endpoint remains
    /// available through `runtimeEvents`; callers that need an exact count and
    /// a repeatable page should use this method.
    public func runtimeEventPage(
        page: Int = 1,
        pageSize: Int = AdminWire.RuntimeHistoryPage.defaultPageSize,
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeHistoryPage {
        var query: [String: String] = [
            "view": "page",
            "page": String(max(1, page)),
            "pageSize": String(AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) ? pageSize : AdminWire.RuntimeHistoryPage.defaultPageSize)
        ]
        if let snapshotSeq { query["snapshotSeq"] = String(snapshotSeq) }
        if let historyGeneration { query["historyGeneration"] = String(historyGeneration) }
        filter.add(to: &query)
        var value = request("/admin/runtime/events", query: query)
        value.timeoutInterval = 30
        return try await send(value, as: AdminWire.RuntimeHistoryPage.self)
    }

    public func runtimeRequestChain(requestID: String) async throws -> AdminWire.RuntimeRequestChain {
        try await send(
            request("/admin/runtime/request-chain", query: ["requestID": requestID]),
            as: AdminWire.RuntimeRequestChain.self
        )
    }

    public func runtimeTrends(
        range: String = "24h",
        granularity: String = "auto",
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeTrendSeries {
        var query = ["range": range, "granularity": granularity]
        if let snapshotSeq { query["snapshotSeq"] = String(snapshotSeq) }
        if let historyGeneration { query["historyGeneration"] = String(historyGeneration) }
        filter.add(to: &query)
        return try await send(request("/admin/runtime/trends", query: query), as: AdminWire.RuntimeTrendSeries.self)
    }

    /// Lightweight filter facets. The daemon computes all picker
    /// dimensions in one read transaction instead of four independent
    /// dimension queries (or the much heavier legacy analytics aggregate).
    public func runtimeFacets(
        range: String = "24h",
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeFacetSnapshot {
        var query = ["range": range]
        filter.add(to: &query)
        return try await send(request("/admin/runtime/facets", query: query), as: AdminWire.RuntimeFacetSnapshot.self)
    }

    public func runtimeErrors(
        page: Int = 1,
        pageSize: Int = AdminWire.RuntimeHistoryPage.defaultPageSize,
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeErrorPage {
        var query = ["page": String(max(1, page)), "pageSize": String(AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) ? pageSize : AdminWire.RuntimeHistoryPage.defaultPageSize)]
        if let snapshotSeq { query["snapshotSeq"] = String(snapshotSeq) }
        if let historyGeneration { query["historyGeneration"] = String(historyGeneration) }
        filter.add(to: &query)
        return try await send(request("/admin/runtime/errors", query: query), as: AdminWire.RuntimeErrorPage.self)
    }

    public func runtimeProjects(
        page: Int = 1,
        pageSize: Int = AdminWire.RuntimeHistoryPage.defaultPageSize,
        search: String? = nil,
        sort: String = "last_seen",
        order: String = "desc",
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeDimensionPage {
        try await runtimeDimensionPage(
            path: "/admin/runtime/projects", kind: "project", page: page, pageSize: pageSize,
            search: search, sort: sort, order: order, snapshotSeq: snapshotSeq,
            historyGeneration: historyGeneration, filter: filter
        )
    }

    public func runtimeSessions(
        page: Int = 1,
        pageSize: Int = AdminWire.RuntimeHistoryPage.defaultPageSize,
        search: String? = nil,
        sort: String = "last_seen",
        order: String = "desc",
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeDimensionPage {
        try await runtimeDimensionPage(
            path: "/admin/runtime/sessions", kind: "session", page: page, pageSize: pageSize,
            search: search, sort: sort, order: order, snapshotSeq: snapshotSeq,
            historyGeneration: historyGeneration, filter: filter
        )
    }

    /// Unified high-cardinality dimension endpoint. `kind` accepts endpoint,
    /// model, clientKind, purpose, failureKind, failurePhase, protocol,
    /// streamTerminal, project and session.
    public func runtimeDimensions(
        kind: String,
        page: Int = 1,
        pageSize: Int = AdminWire.RuntimeHistoryPage.defaultPageSize,
        search: String? = nil,
        sort: String = "last_seen",
        order: String = "desc",
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeDimensionPage {
        var query = [
            "kind": kind,
            "page": String(max(1, page)),
            "pageSize": String(AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) ? pageSize : AdminWire.RuntimeHistoryPage.defaultPageSize),
            "sort": sort,
            "order": order,
        ]
        if let search, !search.isEmpty { query["search"] = search }
        if let snapshotSeq { query["snapshotSeq"] = String(snapshotSeq) }
        if let historyGeneration { query["historyGeneration"] = String(historyGeneration) }
        filter.add(to: &query)
        return try await send(
            request("/admin/runtime/dimensions", query: query),
            as: AdminWire.RuntimeDimensionPage.self
        )
    }

    private func runtimeDimensionPage(
        path: String,
        kind: String,
        page: Int,
        pageSize: Int,
        search: String?,
        sort: String,
        order: String,
        snapshotSeq: Int?,
        historyGeneration: Int?,
        filter: AdminWire.RuntimeFilter
    ) async throws -> AdminWire.RuntimeDimensionPage {
        var query = [
            "page": String(max(1, page)),
            "pageSize": String(AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) ? pageSize : AdminWire.RuntimeHistoryPage.defaultPageSize),
            "sort": sort,
            "order": order,
        ]
        _ = kind // endpoint path carries the dimension; kept for call-site clarity.
        if let search, !search.isEmpty { query["search"] = search }
        if let snapshotSeq { query["snapshotSeq"] = String(snapshotSeq) }
        if let historyGeneration { query["historyGeneration"] = String(historyGeneration) }
        filter.add(to: &query)
        return try await send(request(path, query: query), as: AdminWire.RuntimeDimensionPage.self)
    }

    public func runtimeStorage() async throws -> AdminWire.RuntimeStorageProbe {
        try await send(request("/admin/runtime/storage"), as: AdminWire.RuntimeStorageProbe.self)
    }

    public func runtimeRetention() async throws -> AdminWire.RuntimeRetention {
        try await send(request("/admin/runtime/retention"), as: AdminWire.RuntimeRetention.self)
    }

    public func updateRuntimeRetention(_ update: AdminWire.RuntimeRetentionUpdate) async throws -> AdminWire.RuntimeRetention {
        var value = try jsonRequest("/admin/runtime/retention", method: "PUT", body: update.dictionary)
        value.timeoutInterval = 15
        return try await send(value, as: AdminWire.RuntimeRetention.self)
    }

    public func runtimePricing() async throws -> AdminWire.RuntimePricing {
        try await send(request("/admin/runtime/pricing"), as: AdminWire.RuntimePricing.self)
    }

    public func updateRuntimePricing(_ update: AdminWire.RuntimePricingUpdate) async throws -> AdminWire.RuntimePricingMutation {
        var value = try jsonRequest("/admin/runtime/pricing", method: "PUT", body: update.dictionary)
        value.timeoutInterval = 15
        return try await send(value, as: AdminWire.RuntimePricingMutation.self)
    }

    public func runtimeExportEstimate(
        scope: String = "events",
        format: String = "jsonl",
        privacy: String = "stored",
        confirmStored: Bool = false,
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws -> AdminWire.RuntimeExportEstimate {
        var query = ["scope": scope, "format": format, "privacy": privacy]
        if confirmStored { query["confirmStored"] = "true" }
        if let snapshotSeq { query["snapshotSeq"] = String(snapshotSeq) }
        if let historyGeneration { query["historyGeneration"] = String(historyGeneration) }
        filter.add(to: &query)
        return try await send(request("/admin/runtime/export/estimate", query: query), as: AdminWire.RuntimeExportEstimate.self)
    }

    /// Stream an analytics export to disk.  `stored` is intentionally explicit
    /// at the API boundary; the server rejects it unless `confirmStored=true`.
    public func downloadRuntimeExport(
        to destination: URL,
        scope: String = "events",
        format: String = "jsonl",
        privacy: String = "stored",
        confirmStored: Bool = false,
        snapshotSeq: Int? = nil,
        historyGeneration: Int? = nil,
        filter: AdminWire.RuntimeFilter = .init()
    ) async throws {
        var query = ["scope": scope, "format": format, "privacy": privacy]
        if confirmStored { query["confirmStored"] = "true" }
        if let snapshotSeq { query["snapshotSeq"] = String(snapshotSeq) }
        if let historyGeneration { query["historyGeneration"] = String(historyGeneration) }
        filter.add(to: &query)
        var value = request("/admin/runtime/export", query: query)
        value.timeoutInterval = 3600 * 24
        let (temporaryURL, response) = try await Self.streamSession.download(for: value)
        guard let http = response as? HTTPURLResponse else {
            try? FileManager.default.removeItem(at: temporaryURL)
            throw AdminError.invalidResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            let body: Data
            if let handle = try? FileHandle(forReadingFrom: temporaryURL),
               let prefix = try? handle.read(upToCount: 8 * 1024) {
                body = prefix
            } else {
                body = Data()
            }
            try? FileManager.default.removeItem(at: temporaryURL)
            throw AdminError.badStatus(http.statusCode, String(decoding: body, as: UTF8.self))
        }
        do {
            let fileManager = FileManager.default
            if fileManager.fileExists(atPath: destination.path) {
                _ = try fileManager.replaceItemAt(destination, withItemAt: temporaryURL)
            } else {
                try fileManager.moveItem(at: temporaryURL, to: destination)
            }
        } catch {
            try? FileManager.default.removeItem(at: temporaryURL)
            throw AdminError.fileIO(error.localizedDescription)
        }
    }

    public func diagnosticCaptureDetail(id: String) async throws -> AdminWire.DiagnosticRequestCapture {
        // Request IDs are opaque; slash must not become another URL path segment.
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/")
        let encoded = id.addingPercentEncoding(withAllowedCharacters: allowed) ?? id
        var value = request("/admin/diagnostic-capture/\(encoded)")
        // A selected capture can be close to 512 MiB (or a user-configured 1024 MiB).
        // Allow local JSON transfer and decoding enough time instead of reporting a
        // large record as a network failure.
        value.timeoutInterval = 120
        return try await send(value, as: AdminWire.DiagnosticRequestCapture.self)
    }

    /// 下载最近一次原子落盘的完整捕获快照。默认使用源数据；`raw`
    /// 必须同时传 `confirmRaw=true`，保留显式风险确认。
    /// URLSession download task 直接写临时文件，避免大快照进入 App 内存。
    public func downloadDiagnosticCapture(
        to destination: URL,
        privacy: String = "raw",
        confirmRaw: Bool = false,
        scope: String = "all",
        format: String = "jsonl",
        requestID: String? = nil
    ) async throws {
        var query = ["privacy": privacy, "scope": scope, "format": format]
        if confirmRaw { query["confirmRaw"] = "true" }
        if let requestID, !requestID.isEmpty { query["requestID"] = requestID }
        var value = request("/admin/diagnostic-capture/export", query: query)
        value.timeoutInterval = 3600 * 24
        let (temporaryURL, response) = try await Self.streamSession.download(for: value)
        guard let http = response as? HTTPURLResponse else {
            try? FileManager.default.removeItem(at: temporaryURL)
            throw AdminError.invalidResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            let message: String
            if let handle = try? FileHandle(forReadingFrom: temporaryURL),
               let data = try? handle.read(upToCount: 8 * 1024) {
                message = String(decoding: data, as: UTF8.self)
            } else {
                message = "诊断捕获导出失败"
            }
            try? FileManager.default.removeItem(at: temporaryURL)
            throw AdminError.badStatus(http.statusCode, message)
        }

        do {
            let fileManager = FileManager.default
            if fileManager.fileExists(atPath: destination.path) {
                _ = try fileManager.replaceItemAt(destination, withItemAt: temporaryURL)
            } else {
                try fileManager.moveItem(at: temporaryURL, to: destination)
            }
        } catch {
            try? FileManager.default.removeItem(at: temporaryURL)
            throw AdminError.fileIO(error.localizedDescription)
        }
    }

    public func setDiagnosticCapture(enabled: Bool, maxBytes: Int? = nil) async throws -> AdminWire.DiagnosticCaptureIndex {
        var body: [String: Any] = ["enabled": enabled]
        if let maxBytes { body["maxBytes"] = maxBytes }
        var value = try jsonRequest("/admin/diagnostic-capture", method: "PUT", body: body)
        value.timeoutInterval = 10
        return try await send(value, as: AdminWire.DiagnosticCaptureIndex.self)
    }

    public func clearDiagnosticCapture() async throws {
        var value = request("/admin/diagnostic-capture", method: "DELETE")
        value.timeoutInterval = 10
        _ = try await send(value, as: AdminWire.ClearedAck.self)
    }

    /// 订阅 SSE 事件流;流断开即结束(调用方负责重连策略)。
    public func events() -> AsyncThrowingStream<AdminWire.Event, Error> {
        let request = request("/admin/events")
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let (bytes, response) = try await Self.streamSession.bytes(for: request)
                    guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                        throw AdminError.invalidResponse
                    }
                    var eventName = ""
                    var dataLines: [String] = []
                    for try await line in bytes.lines {
                        if line.isEmpty {
                            if !eventName.isEmpty || !dataLines.isEmpty {
                                let payload = dataLines.joined(separator: "\n")
                                if let event = AdminWire.Event.parse(name: eventName, data: payload) {
                                    continuation.yield(event)
                                }
                            }
                            eventName = ""
                            dataLines = []
                        } else if line.hasPrefix(":") {
                            continue // keep-alive 注释
                        } else if line.hasPrefix("event:") {
                            eventName = String(line.dropFirst(6)).trimmingCharacters(in: .whitespaces)
                        } else if line.hasPrefix("data:") {
                            dataLines.append(String(line.dropFirst(5)).trimmingCharacters(in: .whitespaces))
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in
                task.cancel()
            }
        }
    }
}

/// admin API 的 wire 模型(与 sumpterd 的 serde 输出同构)。
public enum AdminWire {
    public struct ProviderModels: Decodable, Equatable, Sendable {
        public let endpointID: String
        public let models: [String]
        public let source: String
        public let updatedAt: String
    }

    public struct RuntimeCounters: Decodable, Equatable, Sendable {
        public let clientRequests: Int
        public let clientSuccesses: Int
        public let clientFailures: Int
        public let upstreamAttempts: Int
        public let upstreamSuccesses: Int
        public let upstreamFailures: Int
        public let failovers: Int
    }

    public struct RuntimeStorage: Decodable, Equatable, Sendable {
        public let backend: String
        public let state: String
        public let pendingEvents: Int
        public let pendingBytes: Int?
        public let eventCount: Int
        public let dbBytes: Int
        public let walBytes: Int
        public let backfillComplete: Bool?
        public let backfillFailed: Int?
        public let indexesReady: Bool?
        public let rollupComplete: Bool?
        public let rollupFailed: Int?
        public let rollupDirtyBuckets: Int?
        public let lastCommitAt: Double?
        public let lastError: String?
    }

    public struct RuntimeSummary: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let storage: RuntimeStorage
        public let resetGeneration: Int
        public let counters: RuntimeCounters
        public let latestEvent: RuntimeEvent?
    }

    public struct RuntimeChange: Decodable, Equatable, Sendable {
        public let seq: Int
        public let changeSeq: Int
        public let event: RuntimeEvent
    }

    public struct RuntimeEventListItem: Decodable, Equatable, Identifiable, Sendable {
        public let seq: Int
        public let changeSeq: Int
        public let id: String
        public let timestamp: Double
        public let kind: String
        public let phase: RuntimeEventPhase?
        public let outcome: RuntimeEventOutcome?
        public let statusCode: Int
        public let requestID: String?
        public let requestMethod: String?
        public let requestPath: String?
        public let routeIntent: String?
        public let sessionID: String?
        public let clientKind: ClientKind?
        /// The compact projection mirrors the high-signal routing fields so
        /// list/SSE updates remain useful before the full detail request
        /// completes.  Optional values are intentionally nullable for old
        /// daemons and legacy events.
        public let clientModel: String?
        public let requestPurpose: RequestPurpose?
        public let featureRuleID: String?
        public let endpointID: String?
        public let endpointName: String?
        public let modelGroupID: String?
        public let modelGroupName: String?
        public let upstreamHost: String?
        public let sourceFormat: ProviderProtocol?
        public let targetFormat: ProviderProtocol?
        public let routeMode: RouteMode?
        public let effectiveModel: String?
        public let upstreamModel: String?
        public let failureKind: RuntimeFailureKind?
        public let failurePhase: RuntimeFailurePhase?
        public let failureDetail: String?
        public let message: String?
        public let toolCalls: [String]?
        public let streamTrace: StreamTrace?
        public let durationMS: Int
        public let ttfbMS: Int?
        public let timeoutMS: Int?
        public let upstreamStatusCode: Int?
        public let upstreamRequestID: String?
        public let codexMetadata: CodexMetadata?
        /// Client-declared project attribution. Kept in the projection because the
        /// event list is this app's primary load path; without it a Claude Code
        /// request would render as an unidentified project until its detail is fetched.
        public let clientDeclared: ClientDeclaredMetadata?
        public let grokMetadata: GrokMetadata?
        public let projectName: String?
        public let projectSource: String?
        public let localUser: String?
        public let codexThreadClass: String?
        public let attributionScope: String?
        public let failover: Bool

        public init(change: RuntimeChange) {
            let event = change.event
            seq = change.seq
            changeSeq = change.changeSeq
            id = event.id
            timestamp = event.timestamp.timeIntervalSinceReferenceDate
            kind = event.kind
            phase = event.phase
            outcome = event.outcome
            statusCode = event.statusCode
            requestID = event.requestID
            requestMethod = event.requestMethod
            requestPath = event.requestPath
            routeIntent = event.routeIntent
            sessionID = event.sessionID
            clientKind = event.clientKind
            clientModel = event.clientModel
            requestPurpose = event.requestPurpose
            featureRuleID = event.featureRuleID
            endpointID = event.endpointID
            endpointName = event.endpointName
            modelGroupID = event.modelGroupID
            modelGroupName = event.modelGroupName
            upstreamHost = event.upstreamHost
            sourceFormat = event.sourceFormat
            targetFormat = event.targetFormat
            routeMode = event.routeMode
            effectiveModel = event.effectiveModel
            upstreamModel = event.upstreamModel
            failureKind = event.failureKind
            failurePhase = event.failurePhase
            failureDetail = event.failureDetail
            message = event.message
            toolCalls = event.toolCalls
            streamTrace = event.streamTrace
            durationMS = event.durationMS
            ttfbMS = event.ttfbMS
            timeoutMS = event.timeoutMS
            upstreamStatusCode = event.upstreamStatusCode
            upstreamRequestID = event.upstreamRequestID
            codexMetadata = event.codexMetadata
            clientDeclared = event.clientDeclared
            grokMetadata = event.grokMetadata
            projectName = event.projectName
            projectSource = event.projectSource
            localUser = event.localUser
            codexThreadClass = event.codexThreadClass
            attributionScope = event.attributionScope
            failover = event.failover
        }

        public var runtimeEvent: RuntimeEvent {
            RuntimeEvent(
                id: id,
                timestamp: RuntimeEvent.Timestamp.date(from: timestamp),
                kind: kind,
                endpointID: endpointID,
                endpointName: endpointName,
                upstreamHost: upstreamHost,
                clientModel: clientModel,
                clientKind: clientKind,
                sourceFormat: sourceFormat,
                targetFormat: targetFormat,
                routeMode: routeMode,
                upstreamModel: upstreamModel,
                effectiveModel: effectiveModel,
                statusCode: statusCode,
                durationMS: durationMS,
                failover: failover,
                message: message,
                toolCalls: toolCalls,
                streamTrace: streamTrace,
                outcome: outcome,
                phase: phase,
                featureRuleID: featureRuleID,
                failureDetail: failureDetail,
                failureKind: failureKind,
                failurePhase: failurePhase,
                requestPurpose: requestPurpose,
                requestID: requestID,
                requestMethod: requestMethod,
                requestPath: requestPath,
                routeIntent: routeIntent,
                sessionID: sessionID,
                ttfbMS: ttfbMS,
                timeoutMS: timeoutMS,
                upstreamStatusCode: upstreamStatusCode,
                upstreamRequestID: upstreamRequestID,
                codexMetadata: codexMetadata,
                clientDeclared: clientDeclared,
                grokMetadata: grokMetadata,
                projectName: projectName,
                projectSource: projectSource,
                localUser: localUser,
                codexThreadClass: codexThreadClass,
                attributionScope: attributionScope
            )
        }

        /// Merge a list projection into an already-loaded full event.
        ///
        /// `/admin/runtime/events` intentionally returns a compact projection;
        /// optional `nil` fields therefore mean "not present in this projection",
        /// not "clear the value from the full detail". Keeping the merge here
        /// makes SSE updates and pagination use the same lossless rule.
        public func mergedRuntimeEvent(with existing: RuntimeEvent?) -> RuntimeEvent {
            guard var event = existing else { return runtimeEvent }
            event.timestamp = RuntimeEvent.Timestamp.date(from: timestamp)
            event.kind = kind
            event.statusCode = statusCode
            event.durationMS = durationMS
            event.failover = failover
            if let phase { event.phase = phase }
            if let outcome { event.outcome = outcome }
            if let requestID { event.requestID = requestID }
            if let requestMethod { event.requestMethod = requestMethod }
            if let requestPath { event.requestPath = requestPath }
            if let routeIntent { event.routeIntent = routeIntent }
            if let sessionID { event.sessionID = sessionID }
            if let clientKind { event.clientKind = clientKind }
            if let clientModel { event.clientModel = clientModel }
            if let requestPurpose { event.requestPurpose = requestPurpose }
            if let featureRuleID { event.featureRuleID = featureRuleID }
            if let endpointID { event.endpointID = endpointID }
            if let endpointName { event.endpointName = endpointName }
            if let modelGroupID { event.modelGroupID = modelGroupID }
            if let modelGroupName { event.modelGroupName = modelGroupName }
            if let upstreamHost { event.upstreamHost = upstreamHost }
            if let sourceFormat { event.sourceFormat = sourceFormat }
            if let targetFormat { event.targetFormat = targetFormat }
            if let routeMode { event.routeMode = routeMode }
            if let effectiveModel { event.effectiveModel = effectiveModel }
            if let upstreamModel { event.upstreamModel = upstreamModel }
            if let failureKind { event.failureKind = failureKind }
            if let failurePhase { event.failurePhase = failurePhase }
            if let failureDetail { event.failureDetail = failureDetail }
            if let message { event.message = message }
            if let toolCalls { event.toolCalls = toolCalls }
            if let streamTrace { event.streamTrace = streamTrace }
            if let ttfbMS { event.ttfbMS = ttfbMS }
            if let timeoutMS { event.timeoutMS = timeoutMS }
            if let upstreamStatusCode { event.upstreamStatusCode = upstreamStatusCode }
            if let upstreamRequestID { event.upstreamRequestID = upstreamRequestID }
            if let codexMetadata { event.codexMetadata = codexMetadata }
            if let clientDeclared { event.clientDeclared = clientDeclared }
            if let grokMetadata { event.grokMetadata = grokMetadata }
            if let projectName { event.projectName = projectName }
            if let projectSource { event.projectSource = projectSource }
            if let localUser { event.localUser = localUser }
            if let codexThreadClass { event.codexThreadClass = codexThreadClass }
            if let attributionScope { event.attributionScope = attributionScope }
            return event
        }
    }

    public struct RuntimeEventPage: Decodable, Equatable, Sendable {
        public let events: [RuntimeEventListItem]
        public let hasMore: Bool
        public let resetGeneration: Int?
        public let cursorValid: Bool?
    }

    /// Optional filters shared by all v3 history/analytics endpoints. Empty
    /// strings are omitted so an unselected SwiftUI Picker cannot accidentally
    /// become an exact-match filter.
    public struct RuntimeFilter: Codable, Equatable, Sendable {
        public var kind: String?
        public var outcome: String?
        public var clientKind: String?
        public var requestPurpose: String?
        public var requestID: String?
        public var endpointID: String?
        public var model: String?
        public var projectID: String?
        /// Human-readable project display-name compatibility alias. Keep it
        /// independent from `projectID`; when both are supplied the daemon
        /// applies them as an AND filter so a renamed/colliding project cannot
        /// silently broaden a query.
        public var project: String?
        public var sessionID: String?
        public var failureKind: String?
        public var failurePhase: String?
        public var from: Double?
        public var to: Double?

        public init(
            kind: String? = nil,
            outcome: String? = nil,
            clientKind: String? = nil,
            requestPurpose: String? = nil,
            requestID: String? = nil,
            endpointID: String? = nil,
            model: String? = nil,
            projectID: String? = nil,
            project: String? = nil,
            sessionID: String? = nil,
            failureKind: String? = nil,
            failurePhase: String? = nil,
            from: Double? = nil,
            to: Double? = nil
        ) {
            self.kind = kind
            self.outcome = outcome
            self.clientKind = clientKind
            self.requestPurpose = requestPurpose
            self.requestID = requestID
            self.endpointID = endpointID
            self.model = model
            self.projectID = projectID
            self.project = project
            self.sessionID = sessionID
            self.failureKind = failureKind
            self.failurePhase = failurePhase
            self.from = from
            self.to = to
        }

        fileprivate func add(to query: inout [String: String]) {
            let strings: [(String, String?)] = [
                ("kind", kind), ("outcome", outcome), ("clientKind", clientKind),
                ("requestPurpose", requestPurpose), ("requestID", requestID),
                ("endpointID", endpointID), ("model", model), ("projectID", projectID),
                ("project", project),
                ("sessionID", sessionID), ("failureKind", failureKind),
                ("failurePhase", failurePhase),
            ]
            for (key, value) in strings {
                if let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty {
                    query[key] = value
                }
            }
            if let from { query["from"] = Self.timestampQueryValue(from) }
            if let to { query["to"] = Self.timestampQueryValue(to) }
        }

        private static func timestampQueryValue(_ value: Double) -> String {
            guard value.isFinite else { return "0" }
            if value.rounded() == value { return String(Int64(value)) }
            return String(value)
        }
    }

    public struct RuntimeHistoryPage: Decodable, Equatable, Sendable {
        public static let defaultPageSize = 10
        public static let allowedPageSizes = [defaultPageSize, 25, 50, 100, 200]
        public let apiVersion: Int
        public let events: [RuntimeEventListItem]
        public let page: Int
        public let pageSize: Int
        public let totalCount: Int
        public let totalPages: Int
        public let snapshotSeq: Int
        public let historyGeneration: Int
        public let resetGeneration: Int
        public let retainedFromSeq: Int
        public let hasNext: Bool
        public let hasPrevious: Bool
        public let nextCursor: Int?
        public let previousCursor: Int?
        public let filters: RuntimeFilter
    }

    public struct RuntimeRequestChain: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let requestID: String
        public let events: [RuntimeEventListItem]
        public let truncated: Bool
    }

    public struct RuntimeUsageFieldPresence: Decodable, Equatable, Sendable {
        public let inputTokens: Int
        public let outputTokens: Int
        public let cacheReadInputTokens: Int
        public let cacheCreationInputTokens: Int
        public let reasoningTokens: Int
    }

    public struct RuntimeTokenMetrics: Decodable, Equatable, Sendable {
        public let inputTokens: Int
        public let outputTokens: Int
        public let cacheReadInputTokens: Int
        public let cacheCreationInputTokens: Int
        public let reasoningTokens: Int
        public let uncachedInputTokens: Int
        public let processedInputTokens: Int
        public let processedTotalTokens: Int
        public let observedRequests: Int
        public let accountingKnownRequests: Int
        public let accountingUnknownRequests: Int
        public let cacheReadReportedRequests: Int
        public let cacheReadHitRequests: Int
        public let cacheReadTokenEligibleRequests: Int
        public let cacheReadTokenUnknownRequests: Int
        public let cacheReadTokenRate: Double?
        public let cacheReadRequestRate: Double?
        public let cacheCreationTokenEligibleRequests: Int?
        public let cacheCreationTokenUnknownRequests: Int?
        public let cacheCreationTokenRate: Double?
        public let usageFieldPresence: RuntimeUsageFieldPresence
    }

    public struct RuntimeLatencyThresholdBucket: Decodable, Equatable, Identifiable, Sendable {
        public var id: Int { thresholdMS }
        public let thresholdMS: Int
        public let exceededRequests: Int
    }

    /// Merge-safe latency metrics used by Analytics v3. Sums and threshold
    /// counters can be combined across hourly rollups without inventing a
    /// statistic that the stored snapshot cannot prove.
    public struct RuntimeLatencyMetrics: Decodable, Equatable, Sendable {
        public let observedRequests: Int
        public let sumMS: Int
        public let averageMS: Double?
        public let thresholdBuckets: [RuntimeLatencyThresholdBucket]
    }

    public struct RuntimeLatencyThresholds: Decodable, Equatable, Sendable {
        public let ttfbMS: [Int]
        public let durationMS: [Int]
    }

    public struct RuntimeCostCoverage: Decodable, Equatable, Sendable {
        public let estimatedCostMicros: Int
        public let pricedRequests: Int
        public let unpricedRequests: Int
        public let unknownAccountingRequests: Int
        public let complete: Bool
        public let currency: String?
        public let priceVersion: Int?
    }

    public struct RuntimeTrendPoint: Decodable, Equatable, Identifiable, Sendable {
        public var id: Double { bucketStart }
        public let bucketStart: Double
        public let bucketEnd: Double
        public let clientRequests: Int
        public let clientSuccesses: Int
        public let clientFailures: Int
        public let clientCancelled: Int
        public let clientTerminalRequests: Int
        public let clientUnknownResults: Int
        public let failovers: Int
        public let failoverTerminalRequests: Int
        public let failoverRecoveredRequests: Int
        public let failoverRecoveryRate: Double?
        public let upstreamAttempts: Int
        public let upstreamSuccesses: Int
        public let upstreamFailures: Int
        public let tokens: RuntimeTokenMetrics
        public let ttfbMS: RuntimeLatencyMetrics
        public let durationMS: RuntimeLatencyMetrics
        public let cost: RuntimeCostCoverage
    }

    public struct RuntimeTrendSeries: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let rollupUsed: Bool
        public let granularity: String
        /// Server-selected bucket width; long retained histories may use a
        /// multi-day bucket while keeping the response bounded.
        public let bucketSeconds: Int?
        public let from: Double
        public let to: Double
        public let snapshotSeq: Int
        public let historyGeneration: Int
        public let retainedFromSeq: Int
        public let thresholds: RuntimeLatencyThresholds
        public let points: [RuntimeTrendPoint]
        public let totals: RuntimeTrendPoint
        public let filters: RuntimeFilter
    }

    public struct RuntimeFacetSnapshot: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let snapshotSeq: Int
        public let historyGeneration: Int
        public let retainedFromSeq: Int
        public let facets: RuntimeAnalytics.Facets
    }

    public struct RuntimeErrorGroup: Decodable, Equatable, Identifiable, Sendable {
        public var id: String {
            "kind=\(failureKind ?? "-")|phase=\(failurePhase ?? "-")|endpoint=\(endpointID ?? "-")|model=\(model ?? "-")|status=\(upstreamStatusCode.map(String.init) ?? "-")"
        }
        public let failureKind: String?
        public let failurePhase: String?
        public let endpointID: String?
        public let endpointName: String?
        public let model: String?
        public let upstreamStatusCode: Int?
        public let occurrences: Int
        public let affectedRequests: Int
        public let affectedSessions: Int
        public let recoveredAfterFailover: Int
        public let firstSeen: Double
        public let lastSeen: Double
        public let sampleEventIDs: [String]
    }

    public struct RuntimeErrorPage: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let groups: [RuntimeErrorGroup]
        public let page: Int
        public let pageSize: Int
        public let totalCount: Int
        public let totalPages: Int
        public let snapshotSeq: Int
        public let historyGeneration: Int
        public let retainedFromSeq: Int
        public let hasNext: Bool
        public let hasPrevious: Bool
        public let filters: RuntimeFilter
    }

    public struct RuntimeDimensionRow: Decodable, Equatable, Identifiable, Sendable {
        public var id: String { "\(key)|\(name)|\(source)" }
        public let key: String
        public let name: String
        public let source: String
        public let requests: Int
        public let successes: Int
        public let failures: Int
        public let cancelled: Int
        public let failovers: Int
        public let slowDurationRequests: Int?
        public let criticalDurationRequests: Int?
        public let slowTTFBRequests: Int?
        public let criticalTTFBRequests: Int?
        public let inputTokens: Int
        public let outputTokens: Int
        public let cacheReadInputTokens: Int
        public let cacheCreationInputTokens: Int
        public let processedInputTokens: Int?
        public let processedTotalTokens: Int
        public let cacheReadReportedRequests: Int?
        public let cacheReadHitRequests: Int?
        public let cacheReadTokenRate: Double?
        public let cacheReadRequestRate: Double?
        public let firstSeen: Double
        public let lastSeen: Double
        public let averageDurationMS: Double?
        public let averageTTFBMS: Double?
        public let relatedCount: Int
        public let workspacePaths: [String]
        /// Distinct inbound clients represented by this grouped row. The
        /// project identity and source remain unchanged; this is display-only
        /// context for the project list.
        public let clientKinds: [String]?
        /// Optional for compatibility with an older daemon. Current daemons
        /// calculate this per request using the active endpoint/model price.
        public let cost: RuntimeCostCoverage?
    }

    public struct RuntimeDimensionPage: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let kind: String
        public let rows: [RuntimeDimensionRow]
        public let page: Int
        public let pageSize: Int
        public let totalCount: Int
        public let totalPages: Int
        public let snapshotSeq: Int
        public let historyGeneration: Int
        public let retainedFromSeq: Int
        public let hasNext: Bool
        public let hasPrevious: Bool
        public let search: String?
        public let sort: String
        public let order: String
        public let filters: RuntimeFilter
    }

    public struct RuntimeRetention: Decodable, Equatable, Sendable {
        public let revision: Int
        /// Rolling whole-day retention window. `nil` means the time
        /// dimension is disabled; the daemon evaluates it in 24-hour UTC-like
        /// seconds rather than local calendar days.
        public let maxAgeDays: Int?
        public let storageLimitBytes: Int?

        private enum CodingKeys: String, CodingKey {
            case revision, maxAgeDays, storageLimitBytes
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            revision = try container.decode(Int.self, forKey: .revision)
            maxAgeDays = try container.decodeIfPresent(Int.self, forKey: .maxAgeDays)
            storageLimitBytes = try container.decodeIfPresent(Int.self, forKey: .storageLimitBytes)
        }
    }

    public struct RuntimeCleanupPreview: Decodable, Equatable, Sendable {
        public let olderThan: Double
        public let deletableEvents: Int
        public let deletableRequests: Int
        public let remainingEvents: Int
    }

    public struct RuntimeCleanupMutation: Decodable, Equatable, Sendable {
        public let olderThan: Double
        public let deletedEvents: Int
        public let deletedRequests: Int
        public let remainingEvents: Int
        public let historyGeneration: Int
    }

    public struct RuntimeStorageProbe: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let backend: String
        public let schemaVersion: Int
        public let projectionVersion: Int
        public let projectionBackfillCursor: Int
        public let projectionBackfillComplete: Bool
        public let projectionIndexesReady: Bool
        public let missingIndexes: [String]
        public let hourlyRollupComplete: Bool
        public let hourlyRollupMaxSeq: Int
        public let hourlyRollupHistoryGeneration: Int
        public let hourlyRollupFailed: Bool
        public let hourlyRollupDirtyBuckets: Int
        public let retainedEvents: Int
        public let completedEvents: Int
        public let inFlightEvents: Int
        public let minSeq: Int?
        public let maxSeq: Int?
        public let earliestTimestamp: Double?
        public let latestTimestamp: Double?
        public let retainedFromSeq: Int
        public let historyGeneration: Int
        public let resetGeneration: Int
        public let userDeletedEvents: Int
        public let userDeletedRequests: Int
        public let payloadBytes: Int
        public let databaseBytes: Int
        public let liveBytes: Int
        public let allocatedBytes: Int
        public let freelistBytes: Int
        public let walBytes: Int
        public let pendingEvents: Int?
        public let pendingBytes: Int?
        public let retention: RuntimeRetention
        /// Older SQLite files may still have removed auto-pruning columns.
        /// The daemon never reads them; this optional flag keeps the client
        /// tolerant of daemons predating the warning field.
        public let legacyRetentionDetected: Bool?
    }

    public struct RuntimeModelPrice: Codable, Equatable, Identifiable, Sendable {
        public var id: Int
        public var endpointID: String?
        public var modelKey: String
        public var effectiveFrom: Double
        public var effectiveTo: Double?
        public var inputPerMillionMicros: Int?
        public var outputPerMillionMicros: Int?
        public var cacheReadPerMillionMicros: Int?
        public var cacheCreationPerMillionMicros: Int?
    }

    public struct RuntimePricing: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let revision: Int
        public let currency: String
        public let prices: [RuntimeModelPrice]
    }

    /// PUT `/admin/runtime/pricing` acknowledgement. The full price list is
    /// intentionally fetched with a subsequent GET so revision conflicts never
    /// overwrite a newer catalog with the caller's stale draft.
    public struct RuntimePricingMutation: Decodable, Equatable, Sendable {
        public let revision: Int
        public let currency: String
        public let priceCount: Int
    }

    public struct RuntimeRetentionUpdate: Equatable, Sendable {
        public let expectedRevision: Int
        public let maxAgeDays: Int?
        public let storageLimitBytes: Int?

        fileprivate var dictionary: [String: Any] {
            var value: [String: Any] = ["expectedRevision": expectedRevision]
            value["maxAgeDays"] = maxAgeDays ?? NSNull()
            value["storageLimitBytes"] = storageLimitBytes ?? NSNull()
            return value
        }
    }

    public struct RuntimePricingUpdate: Equatable, Sendable {
        public let expectedRevision: Int
        public let currency: String
        public let prices: [RuntimeModelPrice]

        fileprivate var dictionary: [String: Any] {
            [
                "expectedRevision": expectedRevision,
                "currency": currency,
                "prices": prices.map { price in
                    var value: [String: Any] = [
                        "endpointID": price.endpointID ?? NSNull(),
                        "modelKey": price.modelKey,
                        "effectiveFrom": price.effectiveFrom,
                    ]
                    value["effectiveTo"] = price.effectiveTo ?? NSNull()
                    value["inputPerMillionMicros"] = price.inputPerMillionMicros ?? NSNull()
                    value["outputPerMillionMicros"] = price.outputPerMillionMicros ?? NSNull()
                    value["cacheReadPerMillionMicros"] = price.cacheReadPerMillionMicros ?? NSNull()
                    value["cacheCreationPerMillionMicros"] = price.cacheCreationPerMillionMicros ?? NSNull()
                    return value
                },
            ]
        }
    }

    public struct RuntimeExportEstimate: Decodable, Equatable, Sendable {
        public let apiVersion: Int
        public let scope: String
        public let format: String
        public let privacy: String
        public let privacyScope: String
        public let rowCount: Int
        public let estimatedBytes: Int
        public let snapshotSeq: Int
        public let historyGeneration: Int
        public let retainedFromSeq: Int
    }

    public typealias RuntimeEventDetail = RuntimeChange

    public struct RuntimeAnalytics: Decodable, Equatable, Sendable {
        /// Count of requests for which each usage field was explicitly present.
        /// A present field with value `0` is still counted; absent fields remain
        /// distinguishable from an explicit zero in the token columns.
        public struct UsageFieldPresence: Decodable, Equatable, Sendable {
            public let inputTokens: Int
            public let outputTokens: Int
            public let cacheReadInputTokens: Int
            public let cacheCreationInputTokens: Int
            public let reasoningTokens: Int

            private enum CodingKeys: String, CodingKey {
                case inputTokens, outputTokens, cacheReadInputTokens, cacheCreationInputTokens, reasoningTokens
            }

            public init(from decoder: Decoder) throws {
                let container = try decoder.container(keyedBy: CodingKeys.self)
                inputTokens = try container.decodeIfPresent(Int.self, forKey: .inputTokens) ?? 0
                outputTokens = try container.decodeIfPresent(Int.self, forKey: .outputTokens) ?? 0
                cacheReadInputTokens = try container.decodeIfPresent(Int.self, forKey: .cacheReadInputTokens) ?? 0
                cacheCreationInputTokens = try container.decodeIfPresent(Int.self, forKey: .cacheCreationInputTokens) ?? 0
                reasoningTokens = try container.decodeIfPresent(Int.self, forKey: .reasoningTokens) ?? 0
            }
        }

        public struct AppliedFilters: Decodable, Equatable, Sendable {
            public let clientKind: String?
            public let endpointID: String?
            public let projectID: String?
            public let project: String?
            public let sessionID: String?
        }

        public struct TokenUsage: Decodable, Equatable, Sendable {
            public let inputTokens: Int?
            public let outputTokens: Int?
            public let cacheReadInputTokens: Int?
            public let cacheCreationInputTokens: Int?
            public let reasoningTokens: Int?
            public let uncachedInputTokens: Int?
            public let processedInputTokens: Int?
            public let processedTotalTokens: Int?
            public let totalTokens: Int?
            public let observedRequests: Int?
            public let tokenAccountingSemantics: String?
            public let tokenAccountingQuality: String?
            public let cacheReadTokenEligibleRequests: Int?
            public let cacheReadTokenUnknownRequests: Int?
            public let cacheReadTokenRate: Double?
            public let cacheReadRequestRate: Double?
            public let cacheCreationTokenEligibleRequests: Int?
            public let cacheCreationTokenUnknownRequests: Int?
            public let cacheCreationTokenRate: Double?
            public let usageFieldPresence: UsageFieldPresence?
        }

        public struct LatencyBuckets: Decodable, Equatable, Sendable {
            public let under1s: Int
            public let from1sTo3s: Int
            public let from3sTo6s: Int
            public let over6s: Int
        }

        public struct DimensionRow: Decodable, Equatable, Identifiable, Sendable {
            public var id: String { name }
            public let name: String
            public let attempts: Int
            public let successes: Int
            public let failures: Int
            public let cancelled: Int
            public let pending: Int?
            public let successRate: Double?
            public let failovers: Int
            public let averageDurationMS: Double?
            public let averageTTFBMS: Double?
            /// Legacy analytics responses sometimes included the matching
            /// event IDs, while the SQLite projection used by current
            /// daemons omits this high-cardinality field.  It is not used by
            /// the dashboard (errors expose their own bounded sample IDs),
            /// so keep it optional instead of making a valid dimension row
            /// fail the entire analytics response.
            public let eventIDs: [String]?
            public let inputTokens: Int?
            public let outputTokens: Int?
            public let cacheReadInputTokens: Int?
            public let cacheCreationInputTokens: Int?
            public let totalTokens: Int?
            public let observedRequests: Int?
            public let reasoningTokens: Int?
            public let uncachedInputTokens: Int?
            public let processedInputTokens: Int?
            public let processedTotalTokens: Int?
            public let tokenAccountingSemantics: String?
            public let tokenAccountingQuality: String?
            public let cacheReadTokenEligibleRequests: Int?
            public let cacheReadTokenUnknownRequests: Int?
            public let cacheReadTokenRate: Double?
            public let cacheReadRequestRate: Double?
            public let cacheCreationTokenEligibleRequests: Int?
            public let cacheCreationTokenUnknownRequests: Int?
            public let cacheCreationTokenRate: Double?
            public let usageFieldPresence: UsageFieldPresence?
            public let projectSource: String?
            /// Sanitized workspace suffixes supplied by project analytics;
            /// never a full absolute path.
            public let workspacePaths: [String]?
            /// Session-only context; other analytics dimensions leave these nil.
            public let projects: [String]?
            public let clientKinds: [String]?
        }

        public struct FacetRow: Decodable, Equatable, Identifiable, Sendable {
            public var id: String { value }
            public let value: String
            public let count: Int
        }

        public struct CountRow: Decodable, Equatable, Identifiable, Sendable {
            public var id: String { name }
            public let name: String
            public let count: Int
        }

        public let range: String
        public let clientRequests: Int
        public let clientSuccesses: Int
        public let clientFailures: Int
        public let clientCancelled: Int
        public let clientPending: Int?
        public let clientSuccessRate: Double?
        public let upstreamAttempts: Int
        public let upstreamSuccesses: Int
        public let upstreamFailures: Int
        public let failovers: Int
        public let averageDurationMS: Double?
        public let averageTTFBMS: Double?
        public let tokenUsage: TokenUsage?
        public let latencyBuckets: LatencyBuckets?
        public let endpoints: [DimensionRow]?
        public let models: [DimensionRow]?
        public let clientKinds: [DimensionRow]?
        public let requestPurposes: [DimensionRow]?
        public let featureRules: [DimensionRow]?
        public let protocolRoutes: [DimensionRow]?
        public let failureKinds: [DimensionRow]?
        public let failurePhases: [DimensionRow]?
        public let upstreamStatuses: [DimensionRow]?
        public let streamTerminals: [DimensionRow]?
        public let projects: [DimensionRow]?
        public let sessions: [DimensionRow]?
        public struct Facets: Decodable, Equatable, Sendable {
            public let clientKinds: [FacetRow]?
            public let endpoints: [FacetRow]?
            public let projects: [FacetRow]?
            public let sessions: [FacetRow]?
            public let models: [FacetRow]?
            public let requestPurposes: [FacetRow]?
            public let failureKinds: [FacetRow]?
            public let failurePhases: [FacetRow]?
        }
        public let facets: Facets?
        public let toolCalls: [CountRow]?
        public let codexMetadataPresent: Int?
        public let internalFeatureRequests: Int?
        public let internalFeatures: [DimensionRow]?
        public let filtersApplied: Bool?
        public let filterWarning: String?
        public let appliedFilters: AppliedFilters?
        public let skippedEvents: Int?
        public let truncated: Bool?
    }
    public struct Status: Decodable, Equatable, Sendable {
        public struct Listener: Decodable, Equatable, Sendable {
            public let host: String
            public let port: Int
            public let allowedCIDRs: [String]
            public let hasAuthToken: Bool
        }
        public struct Counters: Decodable, Equatable, Sendable {
            public let clientRequests: Int
            public let clientSuccesses: Int
            public let clientFailures: Int
            public let upstreamAttempts: Int
            public let failovers: Int
        }
        public struct Health: Decodable, Equatable, Sendable {
            public let state: String
            public let successRate: Double?
            public let sampleCount: Int
            public let lastSuccess: Date?
        }
        public let running: Bool
        public let runtimeApiVersion: Int?
        public let generation: String
        public let uptimeSeconds: Int
        public let listener: Listener
        public let providers: Int
        public let endpoints: Int
        public let counters: Counters
        public let health: Health
        public let lastError: String?
    }

    public struct ReloadAck: Decodable, Equatable, Sendable {
        public let generation: String
        public let warnings: [String]
    }

    public struct ResetAck: Decodable, Sendable {
        public let reset: Bool
        public let recreated: Bool?
        public let resetGeneration: Int?
    }

    public struct SessionMutation: Decodable, Sendable {
        public let resetGeneration: Int
        public let deletedEvents: Int
        public let deletedRequests: Int
    }

    public struct ClearedAck: Decodable, Sendable {
        public let cleared: Bool
    }

    public struct DiagnosticHeader: Codable, Equatable, Sendable {
        public let name: String
        public let value: String
    }

    public struct DiagnosticChunk: Codable, Equatable, Sendable {
        public let atMS: Int
        public let bytes: Int
        public let data: String
        public let truncated: Bool
    }

    public struct DiagnosticAttemptCapture: Codable, Equatable, Identifiable, Sendable {
        public let id: String
        public let endpointID: String
        public let endpointName: String
        public let `protocol`: String
        public let sourceFormat: ProviderProtocol?
        public let targetFormat: ProviderProtocol?
        public let routeMode: RouteMode?
        public let pinnedIP: String?
        public let startedAtMS: Int
        public let outboundMethod: String
        public let outboundURL: String
        public let outboundHeaders: [DiagnosticHeader]
        public let outboundBody: String
        public let outboundBodyBytes: Int
        public let outboundBodyTruncated: Bool
        public let responseStatus: Int?
        public let responseHeaders: [DiagnosticHeader]
        public let upstreamChunks: [DiagnosticChunk]
        public let error: String?
        public let completedAtMS: Int?
    }

    public struct DiagnosticCaptureIndexRecord: Decodable, Equatable, Identifiable, Sendable {
        public var id: String { requestID }
        public let requestID: String
        public let timestamp: Double
        public let method: String
        public let path: String
        public let clientKind: String
        public let requestPurpose: String
        public let clientModel: String
        public let effectiveModel: String
        public let featureRuleID: String?
        public let sourceFormat: ProviderProtocol?
        public let targetFormat: ProviderProtocol?
        public let routeMode: RouteMode?
        public let completedAtMS: Int?
        public let statusCode: Int?
        public let outcome: String?
        public let failureKind: String?
        public let truncated: Bool
        public let attemptCount: Int
        public let clientChunkCount: Int
    }

    public struct DiagnosticCaptureIndex: Decodable, Equatable, Sendable {
        public let enabled: Bool
        public let startedAt: Double?
        public let maxBytes: Int
        public let capturedBytes: Int
        public let limitReached: Bool
        public let stopReason: String?
        /// Number of retained records on the daemon; `records` may be capped
        /// to the newest 200 entries for a lightweight index response.
        public let recordCount: Int?
        public let indexTruncated: Bool?
        public let records: [DiagnosticCaptureIndexRecord]
    }

    public struct DiagnosticRequestCapture: Codable, Equatable, Identifiable, Sendable {
        public var id: String { requestID }
        public let requestID: String
        public let timestamp: Double
        public let method: String
        public let path: String
        public let inboundHeaders: [DiagnosticHeader]
        public let inboundBody: String
        public let inboundBodyBytes: Int
        public let inboundBodyTruncated: Bool
        public let clientKind: String
        public let requestPurpose: String
        public let clientModel: String
        public let effectiveModel: String
        public let featureRuleID: String?
        /// 客户端 `X-Sumpter-*` 声明的项目归因。可选:旧 daemon 的捕获记录没有这一段。
        public let clientDeclared: ClientDeclaredMetadata?
        public let sourceFormat: ProviderProtocol?
        public let targetFormat: ProviderProtocol?
        public let routeMode: RouteMode?
        public let attempts: [DiagnosticAttemptCapture]
        public let clientChunks: [DiagnosticChunk]
        public let completedAtMS: Int?
        public let statusCode: Int?
        public let outcome: String?
        public let failureKind: String?
        public let failureDetail: String?
        public let truncated: Bool
    }

    public struct Diagnostics: Decodable, Equatable, Sendable {
        public let version: String
        public let uptimeSeconds: Int
        public let configPath: String?
        public let generation: String
        public let warnings: [String]
    }

    public enum Event: Sendable {
        case runtimeChange(RuntimeChange)
        case notify(
            clientKind: String?,
            title: String,
            message: String,
            sound: String?,
            type: String?,
            category: String?,
            priority: String?,
            actionID: String?,
            sessionID: String?,
            cwd: String?
        )
        case configReloaded(generation: String)
        case migrationNotice(ConfigMigrationNotice)
        case statsReset

        static func parse(name: String, data: String) -> Event? {
            let payload = Data(data.utf8)
            switch name {
            case "runtime-change":
                guard let change = try? JSONDecoder().decode(RuntimeChange.self, from: payload) else {
                    return nil
                }
                return .runtimeChange(change)
            case "notify":
                struct Wire: Decodable {
                    // 老版 sumpterd 没有来源字段；缺省按 Claude Code 兼容处理。
                    let clientKind: String?
                    let title: String
                    let message: String
                    let sound: String?
                    let type: String?
                    let category: String?
                    let priority: String?
                    let actionID: String?
                    // 2026-08 增补(hook 富化);老 sumpterd 不发,保持可缺省。
                    let sessionId: String?
                    let cwd: String?
                }
                guard let wire = try? JSONDecoder().decode(Wire.self, from: payload) else {
                    return nil
                }
                return .notify(
                    clientKind: wire.clientKind,
                    title: wire.title,
                    message: wire.message,
                    sound: wire.sound,
                    type: wire.type,
                    category: wire.category,
                    priority: wire.priority,
                    actionID: wire.actionID,
                    sessionID: wire.sessionId,
                    cwd: wire.cwd
                )
            case "config_reloaded":
                struct Wire: Decodable { let generation: String }
                guard let wire = try? JSONDecoder().decode(Wire.self, from: payload) else {
                    return nil
                }
                return .configReloaded(generation: wire.generation)
            case "migration_notice":
                guard let notice = try? JSONDecoder().decode(ConfigMigrationNotice.self, from: payload) else {
                    return nil
                }
                return .migrationNotice(notice)
            case "stats-reset":
                return .statsReset
            default:
                return nil
            }
        }
    }
}
