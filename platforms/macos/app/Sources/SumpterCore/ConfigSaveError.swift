import Foundation

/// User-facing recovery advice; the original failure remains available in the
/// details and no successful disk write is confused with an applied config.
public struct ConfigSaveError: Error, LocalizedError, CustomStringConvertible {
    public let code: String
    public let reason: String

    public init(code: String, reason: String) {
        self.code = code
        self.reason = reason
    }

    public var description: String {
        let guidance: String
        switch code {
        case "generation_conflict": guidance = "配置已在其他位置更新，请重新加载最新配置后再编辑保存。"
        case "invalid_config": guidance = "配置内容无效，请修正后重试。"
        case "config_write_failed": guidance = "配置写入失败，请检查配置文件权限和磁盘空间后重试。"
        case "listener_rebind_failed": guidance = "监听配置未生效，请检查地址和端口占用后重试。"
        case "reload_failed": guidance = "配置未能应用到引擎，请检查服务状态后重新加载配置。"
        case "bad_token": guidance = "管理连接认证已失效，请重新启动引擎后再保存。"
        default: guidance = "配置保存失败，请检查服务状态后重试。"
        }
        return reason.isEmpty ? guidance : "\(guidance) 详情：\(reason)"
    }

    public var errorDescription: String? { description }
}
