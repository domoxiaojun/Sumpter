import Foundation

public struct OpenAIMessage: Codable, Equatable, Sendable {
    public var role: String
    public var content: String

    public init(role: String, content: String) {
        self.role = role
        self.content = content
    }
}

public struct OpenAIChatRequest: Codable, Equatable, Sendable {
    public struct StreamOptions: Codable, Equatable, Sendable {
        public var includeUsage: Bool

        public init(includeUsage: Bool) {
            self.includeUsage = includeUsage
        }
    }

    public var model: String
    public var messages: [OpenAIMessage]
    public var stream: Bool
    public var streamOptions: StreamOptions
    public var maxTokens: Int?
    public var temperature: Double?
    public var topP: Double?
    /// CPA 风格 `model(high)` 解析出的 effort;编码为 `reasoning_effort`。
    public var reasoningEffort: String?

    enum CodingKeys: String, CodingKey {
        case model
        case messages
        case stream
        case streamOptions = "stream_options"
        case maxTokens = "max_tokens"
        case temperature
        case topP = "top_p"
        case reasoningEffort = "reasoning_effort"
    }

    public init(
        model: String,
        messages: [OpenAIMessage],
        stream: Bool = true,
        streamOptions: StreamOptions = StreamOptions(includeUsage: true),
        maxTokens: Int? = nil,
        temperature: Double? = nil,
        topP: Double? = nil,
        reasoningEffort: String? = nil
    ) {
        self.model = model
        self.messages = messages
        self.stream = stream
        self.streamOptions = streamOptions
        self.maxTokens = maxTokens
        self.temperature = temperature
        self.topP = topP
        self.reasoningEffort = reasoningEffort
    }
}

public struct OpenAIStreamChunk: Codable, Equatable, Sendable {
    public struct Choice: Codable, Equatable, Sendable {
        public struct Delta: Codable, Equatable, Sendable {
            public var content: String?
        }

        public var delta: Delta?
        public var finishReason: String?

        enum CodingKeys: String, CodingKey {
            case delta
            case finishReason = "finish_reason"
        }
    }

    public struct Usage: Codable, Equatable, Sendable {
        public var completionTokens: Int?

        enum CodingKeys: String, CodingKey {
            case completionTokens = "completion_tokens"
        }
    }

    public var model: String?
    public var choices: [Choice]
    public var usage: Usage?

    public init(model: String? = nil, choices: [Choice] = [], usage: Usage? = nil) {
        self.model = model
        self.choices = choices
        self.usage = usage
    }
}

public struct SSEEvent: Equatable, Sendable {
    public var event: String
    public var data: JSONValue

    public init(event: String, data: JSONValue) {
        self.event = event
        self.data = data
    }

    public func bytes() -> Data {
        Data("event: \(event)\ndata: \(data.canonicalJSONString())\n\n".utf8)
    }
}

public enum OpenAIBridge {
    public static func makeRequest(
        from request: RoutingRequest,
        upstreamModel: String,
        reasoningEffort: ReasoningEffort? = nil
    ) -> OpenAIChatRequest {
        var messages: [OpenAIMessage] = []
        let system = RequestInspector.systemText(request)
        if !system.isEmpty {
            messages.append(OpenAIMessage(role: "system", content: system))
        }
        for message in request.messages where message.role == "user" || message.role == "assistant" {
            messages.append(OpenAIMessage(role: message.role, content: flattenText(message.content)))
        }
        // 后缀优先于 body;有合法后缀才写 reasoning_effort(auto 透传为 "auto")。
        let effortWire = reasoningEffort.map(\.rawValue)
            ?? ModelName.reasoningEffort(from: request.model).map(\.rawValue)
        return OpenAIChatRequest(
            model: upstreamModel,
            messages: messages,
            maxTokens: request.raw["max_tokens"]?.intValue,
            temperature: request.raw["temperature"]?.doubleValue,
            topP: request.raw["top_p"]?.doubleValue,
            reasoningEffort: effortWire
        )
    }

    public static func anthropicEvents(from chunks: [OpenAIStreamChunk], messageID: String) -> [SSEEvent] {
        var events: [SSEEvent] = []
        var started = false
        var model = "unknown"
        var outputTokens = 0
        var stopReason = "end_turn"

        func startIfNeeded(_ chunkModel: String?) {
            guard !started else {
                return
            }
            model = chunkModel ?? model
            events.append(contentsOf: messageStartEvents(messageID: messageID, model: model))
            started = true
        }

        for chunk in chunks {
            startIfNeeded(chunk.model)
            let choice = chunk.choices.first
            if let text = choice?.delta?.content, !text.isEmpty {
                outputTokens += 1
                events.append(SSEEvent(
                    event: "content_block_delta",
                    data: .object([
                        "type": .string("content_block_delta"),
                        "index": .number(0),
                        "delta": .object([
                            "type": .string("text_delta"),
                            "text": .string(text)
                        ])
                    ])
                ))
            }
            if let finishReason = choice?.finishReason {
                stopReason = mapStopReason(finishReason)
            }
            if let completionTokens = chunk.usage?.completionTokens {
                outputTokens = completionTokens
            }
        }

        if !started {
            events.append(contentsOf: messageStartEvents(messageID: messageID, model: model))
        }
        events.append(SSEEvent(event: "content_block_stop", data: .object([
            "type": .string("content_block_stop"),
            "index": .number(0)
        ])))
        events.append(SSEEvent(event: "message_delta", data: .object([
            "type": .string("message_delta"),
            "delta": .object([
                "stop_reason": .string(stopReason),
                "stop_sequence": .null
            ]),
            "usage": .object([
                "output_tokens": .number(Double(outputTokens))
            ])
        ])))
        events.append(SSEEvent(event: "message_stop", data: .object([
            "type": .string("message_stop")
        ])))
        return events
    }

    public static func anthropicSSEBody(fromOpenAISSEBody body: Data, messageID: String) -> Data {
        let chunks = openAIStreamChunks(fromSSEBody: body)
        return anthropicEvents(from: chunks, messageID: messageID)
            .reduce(into: Data()) { partial, event in
                partial.append(event.bytes())
            }
    }

    public static func openAIStreamChunks(fromSSEBody body: Data) -> [OpenAIStreamChunk] {
        guard let text = String(data: body, encoding: .utf8) else {
            return []
        }

        let decoder = JSONDecoder()
        var chunks: [OpenAIStreamChunk] = []
        for eventText in text.components(separatedBy: "\n\n") {
            let dataLines = eventText
                .split(whereSeparator: \.isNewline)
                .compactMap { line -> String? in
                    let trimmed = line.trimmingCharacters(in: .whitespaces)
                    guard trimmed.hasPrefix("data:") else {
                        return nil
                    }
                    return String(trimmed.dropFirst(5)).trimmingCharacters(in: .whitespaces)
                }
            let payload = dataLines.joined(separator: "\n")
            guard !payload.isEmpty, payload != "[DONE]" else {
                continue
            }
            if let data = payload.data(using: .utf8),
               let chunk = try? decoder.decode(OpenAIStreamChunk.self, from: data) {
                chunks.append(chunk)
            }
        }
        return chunks
    }

    private static func flattenText(_ value: JSONValue) -> String {
        switch value {
        case .string(let text):
            return text
        case .array(let blocks):
            return blocks.map { block in
                guard case .object(let object) = block else {
                    return ""
                }
                if object["type"]?.stringValue == "text" {
                    return object["text"]?.stringValue ?? ""
                }
                if object["type"]?.stringValue == "tool_result", let content = object["content"] {
                    return flattenText(content)
                }
                return ""
            }.filter { !$0.isEmpty }.joined(separator: "\n")
        default:
            return ""
        }
    }

    private static func messageStartEvents(messageID: String, model: String) -> [SSEEvent] {
        [
            SSEEvent(event: "message_start", data: .object([
                "type": .string("message_start"),
                "message": .object([
                    "id": .string(messageID),
                    "type": .string("message"),
                    "role": .string("assistant"),
                    "model": .string(model),
                    "content": .array([]),
                    "stop_reason": .null,
                    "stop_sequence": .null,
                    "usage": .object([
                        "input_tokens": .number(0),
                        "output_tokens": .number(0)
                    ])
                ])
            ])),
            SSEEvent(event: "content_block_start", data: .object([
                "type": .string("content_block_start"),
                "index": .number(0),
                "content_block": .object([
                    "type": .string("text"),
                    "text": .string("")
                ])
            ]))
        ]
    }

    private static func mapStopReason(_ finishReason: String) -> String {
        switch finishReason {
        case "stop":
            return "end_turn"
        case "length":
            return "max_tokens"
        case "tool_calls":
            return "tool_use"
        case "content_filter":
            return "end_turn"
        default:
            return "end_turn"
        }
    }
}

public actor OpenAIStreamEventBridge {
    private let messageID: String
    private var buffer = ""
    private var started = false
    private var model = "unknown"
    private var outputTokens = 0
    private var stopReason = "end_turn"

    public init(messageID: String) {
        self.messageID = messageID
    }

    public func feed(_ data: Data) -> Data {
        buffer += String(decoding: data, as: UTF8.self)
            .replacingOccurrences(of: "\r\n", with: "\n")
        var output = Data()

        while let range = buffer.range(of: "\n\n") {
            let eventText = String(buffer[..<range.lowerBound])
            buffer.removeSubrange(buffer.startIndex..<range.upperBound)
            guard let chunk = decodeChunk(from: eventText) else {
                continue
            }
            for event in events(for: chunk) {
                output.append(event.bytes())
            }
        }

        return output
    }

    public func finish() -> Data {
        var output = Data()
        if let chunk = decodeChunk(from: buffer) {
            for event in events(for: chunk) {
                output.append(event.bytes())
            }
        }
        buffer = ""
        if !started {
            for event in startEvents(model: model) {
                output.append(event.bytes())
            }
            started = true
        }
        for event in stopEvents() {
            output.append(event.bytes())
        }
        return output
    }

    private func decodeChunk(from eventText: String) -> OpenAIStreamChunk? {
        let payload = eventText
            .split(whereSeparator: \.isNewline)
            .compactMap { line -> String? in
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                guard trimmed.hasPrefix("data:") else {
                    return nil
                }
                return String(trimmed.dropFirst(5)).trimmingCharacters(in: .whitespaces)
            }
            .joined(separator: "\n")
        guard !payload.isEmpty, payload != "[DONE]" else {
            return nil
        }
        guard let data = payload.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(OpenAIStreamChunk.self, from: data)
    }

    private func events(for chunk: OpenAIStreamChunk) -> [SSEEvent] {
        var events: [SSEEvent] = []
        if !started {
            model = chunk.model ?? model
            events.append(contentsOf: startEvents(model: model))
            started = true
        }

        let choice = chunk.choices.first
        if let text = choice?.delta?.content, !text.isEmpty {
            outputTokens += 1
            events.append(SSEEvent(
                event: "content_block_delta",
                data: .object([
                    "type": .string("content_block_delta"),
                    "index": .number(0),
                    "delta": .object([
                        "type": .string("text_delta"),
                        "text": .string(text)
                    ])
                ])
            ))
        }
        if let finishReason = choice?.finishReason {
            stopReason = mapStopReason(finishReason)
        }
        if let completionTokens = chunk.usage?.completionTokens {
            outputTokens = completionTokens
        }
        return events
    }

    private func startEvents(model: String) -> [SSEEvent] {
        [
            SSEEvent(event: "message_start", data: .object([
                "type": .string("message_start"),
                "message": .object([
                    "id": .string(messageID),
                    "type": .string("message"),
                    "role": .string("assistant"),
                    "model": .string(model),
                    "content": .array([]),
                    "stop_reason": .null,
                    "stop_sequence": .null,
                    "usage": .object([
                        "input_tokens": .number(0),
                        "output_tokens": .number(0)
                    ])
                ])
            ])),
            SSEEvent(event: "content_block_start", data: .object([
                "type": .string("content_block_start"),
                "index": .number(0),
                "content_block": .object([
                    "type": .string("text"),
                    "text": .string("")
                ])
            ]))
        ]
    }

    private func stopEvents() -> [SSEEvent] {
        [
            SSEEvent(event: "content_block_stop", data: .object([
                "type": .string("content_block_stop"),
                "index": .number(0)
            ])),
            SSEEvent(event: "message_delta", data: .object([
                "type": .string("message_delta"),
                "delta": .object([
                    "stop_reason": .string(stopReason),
                    "stop_sequence": .null
                ]),
                "usage": .object([
                    "output_tokens": .number(Double(outputTokens))
                ])
            ])),
            SSEEvent(event: "message_stop", data: .object([
                "type": .string("message_stop")
            ]))
        ]
    }

    private func mapStopReason(_ finishReason: String) -> String {
        switch finishReason {
        case "stop":
            return "end_turn"
        case "length":
            return "max_tokens"
        case "tool_calls":
            return "tool_use"
        case "content_filter":
            return "end_turn"
        default:
            return "end_turn"
        }
    }
}
