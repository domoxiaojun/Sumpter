import Foundation

public enum ProviderModelCatalogError: Error, LocalizedError, Sendable {
    case invalidBaseURL
    /// 保留给旧调用方；当前模型目录探测允许无鉴权上游，不再主动抛出此错误。
    case missingAPIKey
    case noModels(String)

    public var errorDescription: String? {
        switch self {
        case .invalidBaseURL:
            "API 地址无效"
        case .missingAPIKey:
            "API Key 不能为空"
        case .noModels(let summary):
            summary.isEmpty ? "无可用 /models 端点" : summary
        }
    }
}

public enum ProviderModelCatalog {
    /// 与本机 Claude Code 对齐的出站 User-Agent。
    /// 中转/DashScope Coding Plan 常按 claude-cli UA 放行;代理所有上游请求统一强制此值。
    /// 版本随本机 `claude --version` 更新(当前 2.1.220)。
    public static let userAgent = "claude-cli/2.1.220 (external, cli)"
    public static let candidatePaths = ["/v1/models", "/models", "/v1/model/list", "/api/v1/models"]
    public static let maxResponseBytes = 2 * 1024 * 1024
    public static let maxModelCount = 5_000

    /// 模型目录探测专用 session。出站行为面必须与 Rust 转发路径(`outbound.rs`)一致:
    /// 不读系统/环境代理、不跟随重定向、不用缓存。`URLSession.shared` 会走系统代理,
    /// 于是设了代理的机器上「获取模型」与真实转发走两条不同链路,探测结论不代表转发结果。
    ///
    /// 已知残余差异:URLSession 无法强制 HTTP/1.1(平台限制,h2 由 ALPN 自动协商),
    /// 而转发路径锁了 `http1_only()`。
    public static let directSession: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.connectionProxyDictionary = [:]
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.httpShouldUsePipelining = false
        return URLSession(configuration: configuration)
    }()

    /// 对齐转发路径的 `redirect::none()`:探测不跟随重定向。
    private static let redirectBlocker = CatalogRedirectBlocker()

    /// 系统配了代理时,在探测失败信息后追加的说明。
    ///
    /// 探测和转发一样禁用代理(见 `directSession`),所以在「只有走代理才能出网」的机器上
    /// 这里必然失败。不说明的话很容易被误读成上游挂了,而实际上真正的转发也走不通。
    /// 只报告检测到哪一类代理,不回显主机和端口(可能带内网地址或凭据)。
    static func systemProxyHint(
        settings: [String: Any]? = CFNetworkCopySystemProxySettings()?
            .takeRetainedValue() as? [String: Any]
    ) -> String? {
        guard let settings else { return nil }
        var kinds: [String] = []
        if settings[kCFNetworkProxiesHTTPEnable as String] as? Int == 1 {
            kinds.append("HTTP 代理")
        }
        if settings[kCFNetworkProxiesHTTPSEnable as String] as? Int == 1 {
            kinds.append("HTTPS 代理")
        }
        if settings[kCFNetworkProxiesProxyAutoConfigEnable as String] as? Int == 1 {
            kinds.append("自动代理配置(PAC)")
        }
        guard !kinds.isEmpty else { return nil }
        return "(系统启用了\(kinds.joined(separator: "、")):模型探测与实际转发一样不走代理,"
            + "所以这里失败并不代表上游可用——需要代理才能出网的话,转发同样不通)"
    }

    /// 汇总失败原因,并在系统配了代理时补一句口径说明。
    /// `hint` 可注入,便于在不依赖本机真实网络偏好设置的情况下测试拼接。
    static func summarize(_ errors: [String], hint: String? = systemProxyHint()) -> String {
        let summary = errors.prefix(4).joined(separator: "；")
        guard let hint else { return summary }
        return summary + hint
    }

    public static func modelIDs(from data: Data) throws -> [String] {
        let object = try JSONSerialization.jsonObject(with: data)
        return modelIDs(fromJSONObject: object)
    }

    public static func modelIDs(fromJSONObject object: Any) -> [String] {
        func visit(_ value: Any, into ids: inout Set<String>, depth: Int) {
            guard depth <= 8, ids.count < maxModelCount else { return }
            if let string = value as? String {
                let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty { ids.insert(trimmed) }
                return
            }
            if let array = value as? [Any] {
                array.forEach { visit($0, into: &ids, depth: depth + 1) }
                return
            }
            guard let dictionary = value as? [String: Any] else { return }
            let before = ids.count
            for key in ["data", "models", "result", "items", "results"] {
                if let child = dictionary[key] { visit(child, into: &ids, depth: depth + 1) }
            }
            if ids.count == before {
                for key in ["id", "name", "model", "model_id"] {
                    if let model = dictionary[key] as? String {
                        visit(model, into: &ids, depth: depth + 1)
                        break
                    }
                }
            }
            if ids.count == before, let entries = dictionary["models"] as? [String: Any] {
                entries.keys.forEach { ids.insert($0) }
            }
            if ids.count == before,
               !dictionary.isEmpty,
               ["data", "models", "result", "items", "results"].allSatisfy({ dictionary[$0] == nil }),
               dictionary.values.allSatisfy({ $0 is [String: Any] || $0 is NSNull }) {
                dictionary.keys.forEach { ids.insert($0) }
            }
        }

        var ids = Set<String>()
        visit(object, into: &ids, depth: 0)
        return ids.sorted().prefix(maxModelCount).map { $0 }
    }

    /// Resolve candidate paths against the configured base path. A base URL
    /// ending in `/v1` must not produce `/v1/v1/models`; custom prefixes such
    /// as `/apps/anthropic` are retained for every candidate.
    static func candidateURLs(baseURL: URL) -> [URL] {
        let prefix = baseURL.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let prefixPath = prefix.isEmpty ? "" : "/\(prefix)"
        var paths: [String] = []
        func append(_ candidate: String) {
            let path: String
            if prefixPath.hasSuffix("/v1") && candidate.hasPrefix("/v1/") {
                path = prefixPath + String(candidate.dropFirst(3))
            } else {
                path = prefixPath + candidate
            }
            if !paths.contains(path) { paths.append(path) }
        }
        append("/v1/models")
        append("/models")
        append("/v1/model/list")
        if prefixPath.isEmpty { append("/api/v1/models") }
        return paths.compactMap { path in
            var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
            components?.path = path
            components?.query = nil
            components?.fragment = nil
            return components?.url
        }
    }

    public static func fetch(
        baseURL: URL,
        apiKey: String,
        timeout: TimeInterval = 3,
        overallDeadline: TimeInterval = 12,
        session: URLSession = directSession
    ) async throws -> [String] {
        let key = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let scheme = baseURL.scheme?.lowercased(),
              (scheme == "http" || scheme == "https"),
              baseURL.host != nil,
              baseURL.user == nil,
              baseURL.password == nil,
              baseURL.query == nil,
              baseURL.fragment == nil else {
            throw ProviderModelCatalogError.invalidBaseURL
        }

        // 无 API Key 也要尝试无鉴权目录端点：本地/内网兼容服务常故意不要求
        // 鉴权。带 Key 时先复用 Rust 数据面同时发送的两种鉴权头，再尝试
        // 单头兼容组合；无 Key 时每个候选路径只发一次。
        let authHeaderSets = authenticationHeaderSets(for: key)
        var errors: [String] = []
        let deadline = Date().addingTimeInterval(overallDeadline)

        for url in candidateURLs(baseURL: baseURL) {
            let path = url.path
            for headers in authHeaderSets {
                // 死入口会让每个组合都超时;超过总时限就尽早失败,不再逐个耗满 8s。
                if Date() >= deadline {
                    errors.append("整体超时,已停止尝试")
                    let unique = dedupedErrors(errors)
                    throw ProviderModelCatalogError.noModels(
                        summarize(unique)
                    )
                }
                let remaining = max(1, deadline.timeIntervalSinceNow)
                var request = URLRequest(url: url, timeoutInterval: min(timeout, remaining))
                for (header, value) in headers where !header.isEmpty {
                    request.setValue(value, forHTTPHeaderField: header)
                }
                request.setValue("application/json", forHTTPHeaderField: "Accept")
                request.setValue("identity", forHTTPHeaderField: "Accept-Encoding")
                request.setValue("close", forHTTPHeaderField: "Connection")
                request.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
                request.setValue(userAgent, forHTTPHeaderField: "User-Agent")

                do {
                    let (data, response) = try await session.data(
                        for: request,
                        delegate: redirectBlocker
                    )
                    let status = (response as? HTTPURLResponse)?.statusCode ?? 200
                    guard status == 200 else {
                        errors.append("\(path) HTTP \(status)")
                        continue
                    }
                    guard data.count <= maxResponseBytes else {
                        errors.append("\(path) 响应过大(>\(maxResponseBytes / 1024) KiB)")
                        continue
                    }
                    let ids = try modelIDs(from: data)
                    if !ids.isEmpty {
                        return ids
                    }
                    errors.append("\(path) 200 但无模型列表")
                } catch {
                    if let urlError = error as? URLError,
                       case .badServerResponse = urlError.code {
                        errors.append("\(path) HTTP 错误")
                    } else {
                        let message = error.localizedDescription
                        errors.append("\(path) \(String(message.prefix(50)))")
                    }
                }
            }
        }

        var unique: [String] = []
        for error in errors where !unique.contains(error) {
            unique.append(error)
        }
        throw ProviderModelCatalogError.noModels(summarize(unique))
    }

    private static func dedupedErrors(_ errors: [String]) -> [String] {
        var unique: [String] = []
        for error in errors where !unique.contains(error) {
            unique.append(error)
        }
        return unique
    }

    /// 返回目录探测使用的鉴权组合；空 Key 用一个空头标记表示无鉴权请求。
    /// 设为 internal 便于纯单元测试锁定“空 Key 不再短路、且不发空头”的契约。
    static func authenticationHeaders(for key: String) -> [(String, String)] {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return [("", "")]
        }
        return [
            ("x-api-key", trimmed),
            ("Authorization", "Bearer \(trimmed)"),
            ("Authorization", "x-api-key \(trimmed)")
        ]
    }

    /// 探测优先复用数据面同时发送的两种鉴权头；随后保留单头兼容重试。
    static func authenticationHeaderSets(for key: String) -> [[(String, String)]] {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return [[]] }
        return [
            [("Authorization", "Bearer \(trimmed)"), ("x-api-key", trimmed)],
            [("x-api-key", trimmed)],
            [("Authorization", "Bearer \(trimmed)")],
            [("Authorization", "x-api-key \(trimmed)")],
        ]
    }
}

/// 探测请求不跟随重定向,与转发路径 `reqwest::redirect::Policy::none()` 对齐。
/// 上游把 `/v1/models` 重定向到别处时,探测若跟随会给出转发拿不到的「可用」假象。
final class CatalogRedirectBlocker: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest
    ) async -> URLRequest? {
        nil
    }
}
