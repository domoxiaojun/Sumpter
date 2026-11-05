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

    func testGeminiPathsAndAuthenticationUseNativeContract() throws {
        for (base, path) in [("https://gemini.invalid", "/v1beta/models"), ("https://gemini.invalid/v1beta/", "/v1beta/models"), ("https://gemini.invalid/proxy/v1", "/proxy/v1/models"), ("https://gemini.invalid/v1alpha", "/v1alpha/models")] {
            let urls = ProviderModelCatalog.candidateURLs(baseURL: try XCTUnwrap(URL(string: base)), protocolMode: .gemini)
            XCTAssertEqual(urls.count, 1)
            XCTAssertEqual(urls.first?.path, path)
        }
        let key = ProviderModelCatalog.authenticationHeaderSets(for: " synthetic ", protocolMode: .gemini)
        XCTAssertEqual(key.count, 1)
        XCTAssertEqual(key[0].map { $0.0 }, ["x-goog-api-key"])
        XCTAssertEqual(key[0].map { $0.1 }, ["synthetic"])
        let bearer = ProviderModelCatalog.authenticationHeaderSets(for: "Bearer synthetic-oauth", protocolMode: .gemini)
        XCTAssertEqual(bearer[0].map { $0.0 }, ["Authorization"])
        XCTAssertEqual(bearer[0].map { $0.1 }, ["Bearer synthetic-oauth"])
    }

    func testGeminiFetchFollowsOpaquePageTokensAndNormalizesModelNames() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [GeminiCatalogURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let models = try await ProviderModelCatalog.fetch(
            baseURL: XCTUnwrap(URL(string: "https://gemini.invalid/v1beta")),
            apiKey: "synthetic", session: session, protocolMode: .gemini
        )
        XCTAssertEqual(models, ["gemini-embedding", "gemini-test"])
    }

    func testGeminiFetchRejectsIncompleteAndCyclicPagination() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [GeminiCatalogURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        for host in ["cycle.invalid", "failure.invalid"] {
            do {
                _ = try await ProviderModelCatalog.fetch(
                    baseURL: XCTUnwrap(URL(string: "https://\(host)")),
                    apiKey: "synthetic", session: session, protocolMode: .gemini
                )
                XCTFail("分页失败不能返回部分模型")
            } catch {
                XCTAssertTrue(error.localizedDescription.contains(host == "cycle.invalid" ? "分页重复" : "403"))
            }
        }
    }
}

private final class GeminiCatalogURLProtocol: URLProtocol, @unchecked Sendable {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let url = request.url!
        XCTAssertEqual(request.value(forHTTPHeaderField: "x-goog-api-key"), "synthetic")
        XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
        XCTAssertNil(request.value(forHTTPHeaderField: "x-api-key"))
        XCTAssertEqual(url.path, "/v1beta/models")
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        let token = components.queryItems?.first { $0.name == "pageToken" }?.value
        if token != nil {
            XCTAssertEqual(token, "opaque+/=& token")
            XCTAssertTrue(components.percentEncodedQuery?.contains("%2B") == true)
        }
        let status = url.host == "failure.invalid" && token != nil ? 403 : 200
        let body = token == nil || url.host == "cycle.invalid"
            ? #"{"models":[{"name":"models/gemini-test"}],"nextPageToken":"opaque+/=& token"}"#
            : #"{"models":[{"name":"models/gemini-embedding"}]}"#
        let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
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
