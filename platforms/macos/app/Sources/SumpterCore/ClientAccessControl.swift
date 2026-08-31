import Darwin
import Foundation

public enum ClientAccessControl {
    public static func isAllowed(clientHost: String?, allowedCIDRs: [String]) -> Bool {
        guard let host = clientHost?.trimmingCharacters(in: .whitespacesAndNewlines),
              let address = IPAddress.parse(host) else {
            return allowedCIDRs.isEmpty
        }
        if address.isLoopback {
            return true
        }
        guard !allowedCIDRs.isEmpty else {
            return true
        }
        return allowedCIDRs.contains { cidr in
            guard let network = CIDRNetwork.parse(cidr) else {
                return false
            }
            return network.contains(address)
        }
    }

    /// 客户端是否来自环回地址(127.0.0.0/8 或 ::1)。nil/无法解析视为非环回。
    public static func isLoopback(clientHost: String?) -> Bool {
        guard let host = clientHost?.trimmingCharacters(in: .whitespacesAndNewlines),
              let address = IPAddress.parse(host) else {
            return false
        }
        return address.isLoopback
    }

    /// CIDR 字面量是否合法(供设置界面保存前校验;裸地址视为全前缀)。
    public static func isValidCIDR(_ text: String) -> Bool {
        CIDRNetwork.parse(text) != nil
    }
}

private enum IPAddress: Equatable {
    case v4([UInt8])
    case v6([UInt8])

    static func parse(_ raw: String) -> IPAddress? {
        let trimmed = raw
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        if let v4 = parseIPv4(trimmed) {
            return .v4(v4)
        }
        if let v6 = parseIPv6(trimmed) {
            if isIPv4Mapped(v6) {
                return .v4(Array(v6.suffix(4)))
            }
            return .v6(v6)
        }
        return nil
    }

    var bits: Int {
        switch self {
        case .v4:
            return 32
        case .v6:
            return 128
        }
    }

    var bytes: [UInt8] {
        switch self {
        case .v4(let bytes), .v6(let bytes):
            return bytes
        }
    }

    var isLoopback: Bool {
        switch self {
        case .v4(let bytes):
            return bytes.first == 127
        case .v6(let bytes):
            return bytes.prefix(15).allSatisfy { $0 == 0 } && bytes.last == 1
        }
    }

    private static func parseIPv4(_ raw: String) -> [UInt8]? {
        var address = in_addr()
        let result = raw.withCString { inet_pton(AF_INET, $0, &address) }
        guard result == 1 else {
            return nil
        }
        return withUnsafeBytes(of: address.s_addr) { Array($0) }
    }

    private static func parseIPv6(_ raw: String) -> [UInt8]? {
        var address = in6_addr()
        let result = withUnsafeMutablePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: UInt8.self, capacity: 16) { bytePointer in
                raw.withCString { cString in
                    inet_pton(AF_INET6, cString, bytePointer)
                }
            }
        }
        guard result == 1 else {
            return nil
        }
        return withUnsafeBytes(of: address) { Array($0) }
    }

    private static func isIPv4Mapped(_ bytes: [UInt8]) -> Bool {
        bytes.count == 16 &&
            bytes.prefix(10).allSatisfy { $0 == 0 } &&
            bytes[10] == 0xff &&
            bytes[11] == 0xff
    }
}

private struct CIDRNetwork {
    var address: IPAddress
    var prefixLength: Int

    static func parse(_ raw: String) -> CIDRNetwork? {
        let parts = raw.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: "/", maxSplits: 1)
        guard let address = IPAddress.parse(String(parts.first ?? "")) else {
            return nil
        }
        let prefix: Int
        if parts.count == 2 {
            guard let value = Int(parts[1]), value >= 0, value <= address.bits else {
                return nil
            }
            prefix = value
        } else {
            prefix = address.bits
        }
        return CIDRNetwork(address: address, prefixLength: prefix)
    }

    func contains(_ candidate: IPAddress) -> Bool {
        guard address.bits == candidate.bits else {
            return false
        }
        let networkBytes = address.bytes
        let candidateBytes = candidate.bytes
        let fullBytes = prefixLength / 8
        let remainingBits = prefixLength % 8

        if fullBytes > 0 && Array(networkBytes.prefix(fullBytes)) != Array(candidateBytes.prefix(fullBytes)) {
            return false
        }
        guard remainingBits > 0 else {
            return true
        }
        let mask = UInt8(0xff << UInt8(8 - remainingBits))
        return (networkBytes[fullBytes] & mask) == (candidateBytes[fullBytes] & mask)
    }
}
