import Foundation
import XCTest
@testable import SumpterCore

final class ProviderModelCatalogTests: XCTestCase {
    func testOpenAIProbeDoesNotRequestClaudeAliases() {
        for mode in [EndpointProtocolMode.openai, .openaiResponses, .gemini] {
            let identities = ProviderModelCatalog.probeIdentities(mode: mode, settings: UserAgentSettings())
            XCTAssertEqual(identities.count, 1)
            XCTAssertNotEqual(identities[0].protocolMode, .anthropic)
            XCTAssertFalse(identities[0].userAgent.hasPrefix("claude-cli"))
        }
    }

    func testCatalogParsesCodexSlugAndGeminiResourceName() throws {
        XCTAssertEqual(try ProviderModelCatalog.modelIDs(from: Data(#"{"models":[{"slug":"gemini-dynamic-review"}]}"#.utf8)), ["gemini-dynamic-review"])
        XCTAssertEqual(try ProviderModelCatalog.modelIDs(from: Data(#"{"models":[{"name":"models/gemini-dynamic-review"}]}"#.utf8)), ["models/gemini-dynamic-review"])
    }

    func testFetchMergesClaudeAliasesWithRawCPAIdentifiers() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [DialectCatalogURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let models = try await ProviderModelCatalog.fetch(
            baseURL: XCTUnwrap(URL(string: "https://cpa.invalid")),
            apiKey: "synthetic-catalog-key", timeout: 1, overallDeadline: 3,
            session: session, protocolMode: .anthropic)
        XCTAssertEqual(models, ["claude-synthetic-gemini-alias", "gemini-dynamic-review", "gpt-synthetic"])
    }
}

private final class DialectCatalogURLProtocol: URLProtocol, @unchecked Sendable {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let isClaude = request.value(forHTTPHeaderField: "anthropic-version") != nil
            || (request.value(forHTTPHeaderField: "User-Agent")?.hasPrefix("claude-cli") ?? false)
        let body = isClaude
            ? #"{"data":[{"id":"claude-synthetic-gemini-alias"}]}"#
            : #"{"data":[{"id":"gemini-dynamic-review"},{"id":"gpt-synthetic"}]}"#
        let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: ["Content-Type":"application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}
