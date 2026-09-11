import AppKit
import SwiftUI
import XCTest
@testable import SumpterCore
@testable import SumpterApp

final class EventLayoutPreviewTests: XCTestCase {
    @MainActor func testRenderEventLayouts() throws {
        let outputDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-event-layout-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: outputDirectory) }

        func event(_ id: String, _ changes: [String: Any]) throws -> RuntimeEvent {
            let base: [String: Any] = [
                "id": id, "kind": "client", "timestamp": Date().timeIntervalSinceReferenceDate - 60,
                "durationMS": 38900, "ttfbMS": 6700, "statusCode": 200, "upstreamStatusCode": 200,
                "phase": "completed", "outcome": "succeeded", "failover": false,
                "clientKind": "claude_code", "effectiveModel": "claude-fable-5-1", "endpointName": "主入口",
                "projectName": "demo-project", "projectSource": "workspace_local", "localUser": "demo",
                "streamTrace": ["chunkCount": 128, "terminalEvent": "message_stop"],
                "cacheRead": ["state": "hit", "readTokens": 206242, "finality": "confirmed"],
                "usageSummary": ["inputTokens": 78, "outputTokens": 2754]
            ]
            return try JSONDecoder().decode(RuntimeEvent.self, from: JSONSerialization.data(withJSONObject: base.merging(changes) { _, new in new }))
        }
        let live = try [
            event("live-stream", ["phase": "inFlight", "outcome": NSNull(), "durationMS": 35000,
                "usageSummary": ["inputTokens": 33, "outputTokens": 3],
                "cacheRead": ["state": "hit", "readTokens": 222950, "finality": "provisional"]]),
            event("live-wait", ["phase": "inFlight", "outcome": NSNull(), "durationMS": 178000,
                "statusCode": 0, "upstreamStatusCode": NSNull(), "ttfbMS": NSNull(), "streamTrace": NSNull(),
                "clientKind": "codex", "effectiveModel": "gpt-6-astra", "endpointName": "备用入口",
                "usageSummary": NSNull(), "cacheRead": ["state": "pending", "finality": "unknown"]])
        ]
        let history = try [
            event("success", ["targetFormat": "openai-responses", "clientKind": "codex", "effectiveModel": "gpt-6-astra",
                "cacheRead": ["state": "hit", "readTokens": 203776, "finality": "confirmed"],
                "usageSummary": ["inputTokens": 204082, "outputTokens": 53]]),
            event("rejected", ["statusCode": 400, "outcome": "failed", "effectiveModel": NSNull(), "endpointName": NSNull(),
                "failureKind": "client_request_rejected", "durationMS": 0, "ttfbMS": NSNull(), "streamTrace": NSNull(),
                "usageSummary": NSNull(), "cacheRead": ["state": "unknown", "finality": "unknown"]]),
            event("zero-cache", ["targetFormat": "openai-responses", "clientKind": "codex", "effectiveModel": "gpt-6-astra", "ttfbMS": 2100,
                "usageSummary": ["inputTokens": 1520, "outputTokens": 682],
                "cacheRead": ["state": "miss", "readTokens": 0, "finality": "confirmed"]]),
            event("cancelled", ["statusCode": 499, "outcome": "cancelled", "effectiveModel": "gpt-5.6-luna", "clientKind": "codex",
                "durationMS": 29100, "ttfbMS": NSNull(), "streamTrace": NSNull(), "usageSummary": NSNull(),
                "cacheRead": ["state": "unknown", "finality": "unknown"]]),
            event("stream-failed", ["outcome": "failed", "failureKind": "stream_interrupted",
                "failureDetail": "上游在流式响应完成前关闭连接，请检查入口状态后重试。",
                "effectiveModel": "claude-sonnet-4-6-long-model-name", "usageSummary": ["inputTokens": 98765, "outputTokens": 43210]])
        ]
        for (name, width, scheme) in [("macos-wide", 1320.0, ColorScheme.light), ("macos-standard", 1040.0, .light),
                                       ("macos-compact", 720.0, .light), ("macos-dark", 1320.0, .dark)] {
            let content = VStack(alignment: .leading, spacing: 12) {
                Text("运行事件 · 界面预览（模拟数据）").font(.title3.weight(.semibold))
                RecentEventsPanel(events: history, liveEvents: live,
                    hint: "点击事件查看完整请求链；进行中的用量为上游已报告的暂计值。",
                    currentPage: 1, totalPages: 3, totalCount: 25, pageSize: 10)
                Spacer(minLength: 0)
            }
            .padding(20)
            .frame(width: width, height: 1040, alignment: .topLeading)
            .background(SumpterTheme.palette(for: scheme).canvas)
            .environment(\.colorScheme, scheme)
            .environment(\.sumpterPalette, SumpterTheme.palette(for: scheme))
            let host = NSHostingView(rootView: content)
            host.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: 1040), styleMask: [.borderless], backing: .buffered, defer: false)
            window.contentView = host
            host.layoutSubtreeIfNeeded()
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
            let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: bitmap)
            let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try png.write(to: outputDirectory.appendingPathComponent("\(name).png"))
        }
    }
}
