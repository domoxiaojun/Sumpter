import XCTest
@testable import SumpterCore

final class OpenAIResponsesBridgeTests: XCTestCase {
    func testMakeRequestBodyMapsSystemMessagesAndLimits() throws {
        let request = RoutingRequest(
            model: "claude-fable-5",
            system: .string("be terse"),
            messages: [
                AnthropicMessage(role: "user", content: .string("hi")),
                AnthropicMessage(role: "assistant", content: .string("hello")),
                AnthropicMessage(role: "user", content: .array([
                    .object(["type": .string("text"), "text": .string("again")])
                ]))
            ],
            raw: ["max_tokens": .number(64), "temperature": .number(0.5)]
        )

        let body = OpenAIResponsesBridge.makeRequestBody(from: request, upstreamModel: "gpt-5.4")

        XCTAssertEqual(body["model"]?.stringValue, "gpt-5.4")
        XCTAssertEqual(body["instructions"]?.stringValue, "be terse")
        XCTAssertEqual(body["stream"]?.boolValue, true)
        XCTAssertEqual(body["max_output_tokens"]?.intValue, 64)
        XCTAssertEqual(body["temperature"]?.doubleValue, 0.5)

        guard case .array(let input)? = body["input"] else {
            return XCTFail("input 应为数组")
        }
        func item(_ value: JSONValue?, _ index: Int) -> JSONValue? {
            guard case .array(let items)? = value, items.indices.contains(index) else {
                return nil
            }
            return items[index]
        }
        XCTAssertEqual(input.count, 3)
        XCTAssertEqual(input[0]["role"]?.stringValue, "user")
        XCTAssertEqual(item(input[0]["content"], 0)?["type"]?.stringValue, "input_text")
        XCTAssertEqual(item(input[0]["content"], 0)?["text"]?.stringValue, "hi")
        XCTAssertEqual(input[1]["role"]?.stringValue, "assistant")
        XCTAssertEqual(item(input[1]["content"], 0)?["type"]?.stringValue, "output_text")
        XCTAssertEqual(item(input[2]["content"], 0)?["text"]?.stringValue, "again")
    }

    func testStreamEventsBridgeToAnthropicSSE() throws {
        let sse = """
        event: response.created
        data: {"type":"response.created","response":{"id":"resp_1","model":"gpt-5.4"}}

        event: response.output_text.delta
        data: {"type":"response.output_text.delta","delta":"Hel"}

        event: response.output_text.delta
        data: {"type":"response.output_text.delta","delta":"lo"}

        event: response.completed
        data: {"type":"response.completed","response":{"status":"completed","model":"gpt-5.4","usage":{"input_tokens":9,"output_tokens":2}}}

        """.replacingOccurrences(of: "\n", with: "\n").data(using: .utf8)!

        let output = String(
            decoding: OpenAIResponsesBridge.anthropicSSEBody(fromResponsesSSEBody: sse, messageID: "msg_test"),
            as: UTF8.self
        )

        XCTAssertTrue(output.contains("event: message_start"))
        XCTAssertTrue(output.contains("\"model\":\"gpt-5.4\""))
        XCTAssertTrue(output.contains("event: content_block_start"))
        XCTAssertTrue(output.contains("\"text\":\"Hel\""))
        XCTAssertTrue(output.contains("\"text\":\"lo\""))
        XCTAssertTrue(output.contains("event: content_block_stop"))
        XCTAssertTrue(output.contains("\"stop_reason\":\"end_turn\""))
        XCTAssertTrue(output.contains("\"output_tokens\":2"))
        XCTAssertTrue(output.contains("event: message_stop"))
        // 事件顺序:message_start 必须在首个 delta 之前
        let startIndex = try XCTUnwrap(output.range(of: "message_start")).lowerBound
        let deltaIndex = try XCTUnwrap(output.range(of: "text_delta")).lowerBound
        XCTAssertLessThan(startIndex, deltaIndex)
    }

    func testIncompleteMapsToMaxTokensStopReason() {
        let sse = """
        data: {"type":"response.output_text.delta","delta":"partial"}

        data: {"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"output_tokens":7}}}

        """.data(using: .utf8)!

        let output = String(
            decoding: OpenAIResponsesBridge.anthropicSSEBody(fromResponsesSSEBody: sse, messageID: "msg_test"),
            as: UTF8.self
        )

        XCTAssertTrue(output.contains("\"stop_reason\":\"max_tokens\""))
        XCTAssertTrue(output.contains("\"output_tokens\":7"))
    }

    func testChunkedFeedAcrossEventBoundary() async {
        // 事件被任意切块(跨 \n\n 边界)时仍能正确桥接。
        let bridge = ResponsesStreamEventBridge(messageID: "msg_chunk")
        let full = """
        data: {"type":"response.created","response":{"model":"gpt-5.4"}}

        data: {"type":"response.output_text.delta","delta":"AB"}

        data: {"type":"response.completed","response":{"status":"completed","usage":{"output_tokens":1}}}

        """
        var output = Data()
        let mid = full.index(full.startIndex, offsetBy: full.count / 2)
        output.append(await bridge.feed(Data(full[..<mid].utf8)))
        output.append(await bridge.feed(Data(full[mid...].utf8)))
        output.append(await bridge.finish())

        let text = String(decoding: output, as: UTF8.self)
        XCTAssertTrue(text.contains("message_start"))
        XCTAssertTrue(text.contains("\"text\":\"AB\""))
        XCTAssertTrue(text.contains("message_stop"))
        // completed 已发结束事件,finish 不应重复
        XCTAssertEqual(text.components(separatedBy: "event: message_stop").count - 1, 1)
    }

    func testFinishSynthesizesStopWhenUpstreamDiesMidStream() async {
        // 上游断流没发 completed:finish 兜底补齐结束事件,客户端不挂流。
        let bridge = ResponsesStreamEventBridge(messageID: "msg_dead")
        _ = await bridge.feed(Data("data: {\"type\":\"response.output_text.delta\",\"delta\":\"X\"}\n\n".utf8))
        let tail = String(decoding: await bridge.finish(), as: UTF8.self)
        XCTAssertTrue(tail.contains("content_block_stop"))
        XCTAssertTrue(tail.contains("message_stop"))
    }
}
