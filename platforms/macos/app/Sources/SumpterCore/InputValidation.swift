import Foundation

/// 设置界面输入校验的可恢复错误。`errorDescription` 直接就是可展示给用户的中文提示，
/// UI 的 `error.localizedDescription` 能原样拿到。
public struct InputValidationError: Error, LocalizedError, Equatable {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }

    public var errorDescription: String? { message }
}

/// 设置界面各类输入的纯校验逻辑。全部无副作用、可单测，AppModel / Sheet 只负责调用与展示。
public enum InputValidation {
    /// 校验 Provider URL：只允许 HTTP(S)，禁止把凭据、query 或 fragment
    /// 藏在地址里。这样与 Linux daemon 的配置契约一致，也避免目录路径拼接
    /// 出无效或带临时参数的探测 URL。返回去空白后的字符串。
    public static func url(_ text: String, field: String) throws -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed),
              let scheme = url.scheme?.lowercased(),
              (scheme == "http" || scheme == "https"),
              url.host != nil,
              url.user == nil,
              url.password == nil,
              url.query == nil,
              url.fragment == nil else {
            throw InputValidationError("\(field)无效")
        }
        return trimmed
    }

    /// 校验入口 ID：非空，且只含字母、数字、点、下划线、横线。返回去空白后的字符串。
    public static func providerID(_ text: String) throws -> String {
        let id = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else {
            throw InputValidationError("入口 ID 不能为空")
        }
        guard id.allSatisfy({ char in
            char.isLetter || char.isNumber || char == "-" || char == "_" || char == "."
        }) else {
            throw InputValidationError("入口 ID 只能包含字母、数字、点、下划线和横线")
        }
        return id
    }

    /// 归一化首个超时：非有限值回退 15，否则至少 1 秒。
    public static func timeout(_ value: Double) -> Double {
        value.isFinite ? max(1, value) : 15
    }

    /// 把任意名称清洗成合法的入口 ID 基串；清洗后为空则回退 "provider"。
    public static func sanitizeProviderIDBase(_ raw: String) -> String {
        let cleaned = raw
            .lowercased()
            .map { char in
                char.isLetter || char.isNumber ? char : "-"
            }
            .reduce(into: "") { $0.append($1) }
            .split(separator: "-")
            .joined(separator: "-")
        return cleaned.isEmpty ? "provider" : cleaned
    }

    /// 基于名称生成不与 `existingIDs` 冲突的入口 ID；冲突时追加 `-2`、`-3`…
    public static func uniqueProviderID(name: String, existingIDs: Set<String>) -> String {
        let base = sanitizeProviderIDBase(name)
        if !existingIDs.contains(base) {
            return base
        }
        var index = 2
        while existingIDs.contains("\(base)-\(index)") {
            index += 1
        }
        return "\(base)-\(index)"
    }

    /// 已解析并校验通过的转发参数。
    public struct RetryPolicyInput: Equatable, Sendable {
        public var responseTimeoutSeconds: Double?
        public var streamIdleTimeoutSeconds: Double?
        public var max500Retries: Int
        public var failoverOn500: Bool
        public var retryDelaySeconds: Double?
        public var passThroughRetryDelay: Bool
        public var maxDeferredRounds: Int
        public var maxRetryDurationSeconds: Double
        public var sessionStickyRetries: Int

        public init(
            responseTimeoutSeconds: Double?,
            streamIdleTimeoutSeconds: Double?,
            max500Retries: Int = 0,
            failoverOn500: Bool = true,
            retryDelaySeconds: Double? = nil,
            passThroughRetryDelay: Bool = true,
            maxDeferredRounds: Int,
            maxRetryDurationSeconds: Double,
            sessionStickyRetries: Int
        ) {
            self.responseTimeoutSeconds = responseTimeoutSeconds
            self.streamIdleTimeoutSeconds = streamIdleTimeoutSeconds
            self.max500Retries = max500Retries
            self.failoverOn500 = failoverOn500
            self.retryDelaySeconds = retryDelaySeconds
            self.passThroughRetryDelay = passThroughRetryDelay
            self.maxDeferredRounds = maxDeferredRounds
            self.maxRetryDurationSeconds = maxRetryDurationSeconds
            self.sessionStickyRetries = sessionStickyRetries
        }
    }

    /// 超时文本框允许留空；非空值必须大于 0。
    public static func retryPolicy(
        responseTimeoutText: String,
        streamIdleTimeoutText: String,
        max500RetriesText: String = "0",
        failoverOn500: Bool = true,
        retryDelaySecondsText: String = "",
        passThroughRetryDelay: Bool = true,
        maxDeferredRoundsText: String,
        maxRetryDurationSecondsText: String,
        sessionStickyRetriesText: String
    ) throws -> RetryPolicyInput {
        let responseTimeout = try optionalPositiveDouble(responseTimeoutText, field: "首响应截止")
        let streamIdleTimeout = try optionalPositiveDouble(streamIdleTimeoutText, field: "流式空闲截止")
        guard let max500Retries = Int(max500RetriesText.trimmingCharacters(in: .whitespacesAndNewlines)),
              max500Retries >= 0 else {
            throw InputValidationError("入口内 500 重试次数必须为 0 或正整数")
        }
        let retryDelaySeconds = try optionalPositiveDouble(retryDelaySecondsText, field: "retry_delay 秒数")
        guard let deferredRounds = Int(maxDeferredRoundsText.trimmingCharacters(in: .whitespacesAndNewlines)),
              deferredRounds >= 0 else {
            throw InputValidationError("故障重试最大轮数必须为 0 或正整数")
        }
        guard let retrySeconds = Double(maxRetryDurationSecondsText.trimmingCharacters(in: .whitespacesAndNewlines)),
              retrySeconds.isFinite,
              retrySeconds >= 0 else {
            throw InputValidationError("跨轮最长时长必须为 0 或正数")
        }
        guard let stickyRetries = Int(sessionStickyRetriesText.trimmingCharacters(in: .whitespacesAndNewlines)),
              stickyRetries >= 0 else {
            throw InputValidationError("粘性入口额外重试次数必须为 0 或正整数")
        }
        return RetryPolicyInput(
            responseTimeoutSeconds: responseTimeout,
            streamIdleTimeoutSeconds: streamIdleTimeout,
            max500Retries: max500Retries,
            failoverOn500: failoverOn500,
            retryDelaySeconds: retryDelaySeconds,
            passThroughRetryDelay: passThroughRetryDelay,
            maxDeferredRounds: deferredRounds,
            maxRetryDurationSeconds: retrySeconds,
            sessionStickyRetries: stickyRetries
        )
    }

    public static func optionalPositiveDouble(_ text: String, field: String) throws -> Double? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        guard let value = Double(trimmed), value.isFinite, value > 0 else {
            throw InputValidationError("\(field)必须留空或大于 0")
        }
        return value
    }
}
