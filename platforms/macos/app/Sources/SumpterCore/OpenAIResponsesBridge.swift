import Foundation

/// OpenAI Responses API(`/v1/responses`)与 Anthropic Messages 的桥接。
///
/// Responses 是 OpenAI 2025 起的主力端点(codex 系列模型仅在此端点提供),与老的
/// chat/completions(`OpenAIBridge`)并存:后者仍是 vLLM/Ollama 等兼容生态的事实标准。
/// 第一版仅桥接文本对话(instructions、多轮文本、max_output_tokens、usage、stop_reason);
/// 工具调用不透传——引擎对带工具的请求会跳过非 anthropic 协议入口。
public enum OpenAIResponsesBridge {
    /// Anthropic 请求 → Responses 请求体。system → instructions;
    /// user 文本 → input_text,assistant 历史 → output_text;总是流式(与 chat 桥一致)。
    public static func makeRequestBody(
        from request: RoutingRequest,
        upstreamModel: String,
        reasoningEffort: ReasoningEffort? = nil
    ) -> JSONValue {
        var input: [JSONValue] = []
        for message in request.messages where message.role == "user" || message.role == "assistant" {
            let text = flattenText(message.content)
            guard !text.isEmpty else {
                continue
            }
            let contentType = message.role == "assistant" ? "output_text" : "input_text"
            input.append(.object([
                "role": .string(message.role),
                "content": .array([
                    .object([
                        "type": .string(contentType),
                        "text": .string(text)
                    ])
                ])
            ]))
        }
        var body: [String: JSONValue] = [
            "model": .string(upstreamModel),
            "input": .array(input),
            "stream": .bool(true)
        ]
        let system = RequestInspector.systemText(request)
        if !system.isEmpty {
            body["instructions"] = .string(system)
        }
        if let maxTokens = request.raw["max_tokens"]?.intValue {
            body["max_output_tokens"] = .number(Double(maxTokens))
        }
        if let temperature = request.raw["temperature"]?.doubleValue {
            body["temperature"] = .number(temperature)
        }
        if let topP = request.raw["top_p"]?.doubleValue {
            body["top_p"] = .number(topP)
        }
        // CPA 兼容:model(high) → reasoning.effort;后缀优先于 body。
        let effort = reasoningEffort ?? ModelName.reasoningEffort(from: request.model)
        if let effort {
            body["reasoning"] = .object(["effort": .string(effort.rawValue)])
        }
        return .object(body)
    }

    /// 整段 Responses SSE → 整段 Anthropic SSE(非流式回退路径用)。
    public static func anthropicSSEBody(fromResponsesSSEBody body: Data, messageID: String) -> Data {
        var machine = ResponsesEventMachine(messageID: messageID)
        var output = machine.feed(String(decoding: body, as: UTF8.self))
        output.append(machine.finish())
        return output
    }

    static func flattenText(_ value: JSONValue) -> String {
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
}

/// Responses SSE → Anthropic SSE 的同步状态机;actor 流式桥与整体转换共用。
/// 事件识别以 data JSON 里的 `type` 字段为准(比依赖 `event:` 行更稳)。
struct ResponsesEventMachine {
    private let messageID: String
    private var buffer = ""
    private var started = false
    private var finished = false
    private var model = "unknown"
    private var outputTokens = 0
    private var stopReason = "end_turn"

    init(messageID: String) {
        self.messageID = messageID
    }

    mutating func feed(_ text: String) -> Data {
        buffer += text.replacingOccurrences(of: "\r\n", with: "\n")
        var output = Data()
        while let range = buffer.range(of: "\n\n") {
            let block = String(buffer[..<range.lowerBound])
            buffer.removeSubrange(buffer.startIndex..<range.upperBound)
            output.append(handle(block: block))
        }
        return output
    }

    mutating func finish() -> Data {
        var output = Data()
        if !buffer.isEmpty {
            output.append(handle(block: buffer))
            buffer = ""
        }
        if !started {
            output.append(bytes(of: startEvents()))
            started = true
        }
        // 上游没发 completed(连接中断等)时兜底补齐结束事件,客户端不至于挂流。
        if !finished {
            output.append(bytes(of: stopEvents()))
            finished = true
        }
        return output
    }

    private mutating func handle(block: String) -> Data {
        guard let payload = dataPayload(of: block), payload != "[DONE]",
              let data = payload.data(using: .utf8),
              let json = try? JSONDecoder().decode(JSONValue.self, from: data),
              case .object(let object) = json else {
            return Data()
        }
        switch object["type"]?.stringValue ?? "" {
        case "response.created", "response.in_progress":
            if let responseModel = object["response"]?["model"]?.stringValue {
                model = responseModel
            }
            var events: [SSEEvent] = []
            if !started {
                events = startEvents()
                started = true
            }
            return bytes(of: events)
        case "response.output_text.delta":
            guard let delta = object["delta"]?.stringValue, !delta.isEmpty else {
                return Data()
            }
            var events: [SSEEvent] = []
            if !started {
                events.append(contentsOf: startEvents())
                started = true
            }
            outputTokens += 1
            events.append(SSEEvent(
                event: "content_block_delta",
                data: .object([
                    "type": .string("content_block_delta"),
                    "index": .number(0),
                    "delta": .object([
                        "type": .string("text_delta"),
                        "text": .string(delta)
                    ])
                ])
            ))
            return bytes(of: events)
        case "response.completed", "response.incomplete", "response.failed":
            let response = object["response"]
            if let responseModel = response?["model"]?.stringValue {
                model = responseModel
            }
            if let tokens = response?["usage"]?["output_tokens"]?.intValue {
                outputTokens = tokens
            }
            stopReason = Self.mapStopReason(
                status: response?["status"]?.stringValue ?? "completed",
                incompleteReason: response?["incomplete_details"]?["reason"]?.stringValue
            )
            var events: [SSEEvent] = []
            if !started {
                events.append(contentsOf: startEvents())
                started = true
            }
            if !finished {
                events.append(contentsOf: stopEvents())
                finished = true
            }
            return bytes(of: events)
        default:
            return Data()
        }
    }

    private func dataPayload(of block: String) -> String? {
        let lines = block
            .split(whereSeparator: \.isNewline)
            .compactMap { line -> String? in
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                guard trimmed.hasPrefix("data:") else {
                    return nil
                }
                return String(trimmed.dropFirst(5)).trimmingCharacters(in: .whitespaces)
            }
        let payload = lines.joined(separator: "\n")
        return payload.isEmpty ? nil : payload
    }

    private func startEvents() -> [SSEEvent] {
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

    private func bytes(of events: [SSEEvent]) -> Data {
        events.reduce(into: Data()) { partial, event in
            partial.append(event.bytes())
        }
    }

    private static func mapStopReason(status: String, incompleteReason: String?) -> String {
        switch status {
        case "incomplete":
            return incompleteReason == "max_output_tokens" ? "max_tokens" : "end_turn"
        default:
            return "end_turn"
        }
    }
}

/// 流式桥:引擎按 chunk 喂入 Responses SSE,吐出 Anthropic SSE。与 OpenAIStreamEventBridge 同接口。
public actor ResponsesStreamEventBridge {
    private var machine: ResponsesEventMachine

    public init(messageID: String) {
        machine = ResponsesEventMachine(messageID: messageID)
    }

    public func feed(_ data: Data) -> Data {
        machine.feed(String(decoding: data, as: UTF8.self))
    }

    public func finish() -> Data {
        machine.finish()
    }
}

/// 引擎侧统一的上游 SSE 桥接口——chat/completions 与 responses 两种桥按协议二选一。
public protocol UpstreamSSEBridging: Actor {
    func feed(_ data: Data) -> Data
    func finish() -> Data
}

extension OpenAIStreamEventBridge: UpstreamSSEBridging {}
extension ResponsesStreamEventBridge: UpstreamSSEBridging {}
