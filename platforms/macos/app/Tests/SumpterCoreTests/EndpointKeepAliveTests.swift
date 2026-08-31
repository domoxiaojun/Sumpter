import XCTest
@testable import SumpterCore

/// keepAlive 字段的磁盘往返；sumpterd 侧生效，壳侧入口编辑器也可直接设置。
final class EndpointKeepAliveTests: XCTestCase {
    func testKeepAliveRoundTripAndNewEntryDefault() throws {
        // 手改配置启用后,UI 保存必须保留。
        let json = #"{"id":"e1","baseURL":"https://x.example.com","keepAlive":true}"#
        let endpoint = try JSONDecoder().decode(Endpoint.self, from: Data(json.utf8))
        XCTAssertTrue(endpoint.keepAlive)
        let reencoded = try JSONSerialization.jsonObject(
            with: JSONEncoder().encode(endpoint)
        ) as? [String: Any]
        XCTAssertEqual(reencoded?["keepAlive"] as? Bool, true)

        // 新建入口默认开启并写入 true,让 sumpterd 使用连接池。
        let fresh = Endpoint(id: "e2", name: "e2", baseURL: URL(string: "https://y.example.com")!)
        XCTAssertTrue(fresh.keepAlive)
        let freshObject = try JSONSerialization.jsonObject(
            with: JSONEncoder().encode(fresh)
        ) as? [String: Any]
        XCTAssertEqual(freshObject?["keepAlive"] as? Bool, true)

        // 缺省解码仍按旧配置处理为关闭,保存时不凭空增加字段。
        let legacy = try JSONDecoder().decode(
            Endpoint.self,
            from: Data(#"{"id":"legacy","baseURL":"https://legacy.example.com"}"#.utf8)
        )
        XCTAssertFalse(legacy.keepAlive)
        let legacyObject = try JSONSerialization.jsonObject(
            with: JSONEncoder().encode(legacy)
        ) as? [String: Any]
        XCTAssertNil(legacyObject?["keepAlive"])
    }

    func testPriorityDefaultsNormalizesAndRoundTrips() throws {
        let missing = try JSONDecoder().decode(
            Endpoint.self,
            from: Data(#"{"id":"missing","baseURL":"https://x.example.com"}"#.utf8)
        )
        XCTAssertEqual(missing.priority, 0)
        let missingObject = try JSONSerialization.jsonObject(
            with: JSONEncoder().encode(missing)
        ) as? [String: Any]
        XCTAssertNil(missingObject?["priority"])

        let explicit = try JSONDecoder().decode(
            Endpoint.self,
            from: Data(#"{"id":"explicit","baseURL":"https://x.example.com","priority":7}"#.utf8)
        )
        XCTAssertEqual(explicit.priority, 7)
        let explicitObject = try JSONSerialization.jsonObject(
            with: JSONEncoder().encode(explicit)
        ) as? [String: Any]
        XCTAssertEqual(explicitObject?["priority"] as? Int, 7)

        let negative = try JSONDecoder().decode(
            Endpoint.self,
            from: Data(#"{"id":"negative","baseURL":"https://x.example.com","priority":-2}"#.utf8)
        )
        XCTAssertEqual(negative.priority, 0)
    }
}
