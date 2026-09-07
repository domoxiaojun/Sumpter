import Foundation

public enum PiAttributionHint {
    public enum State: String, Sendable {
        case observed, unattributed, unknown

        public var message: String {
            switch self {
            case .observed: "已观察到 pi 项目归因。当前项目与会话信息来自客户端声明。"
            case .unattributed: "存在未归因的 pi 请求。请在 pi 所在主机加载扩展，并为 Sumpter provider 设置 X-Sumpter-Client: pi。"
            case .unknown: "当前视图尚无可判定的 pi 项目数据，不能据此判断扩展是否已安装。"
            }
        }
    }

    public static func state(projects: [ClaudeAttributionHint.ProjectRow]) -> State {
        let rows = projects.filter { $0.clientKinds.contains("pi") && $0.attempts > 0 }
        if rows.contains(where: { $0.name == "unidentified_project" || $0.projectSource == "missing_workspace_metadata" }) {
            return .unattributed
        }
        if rows.contains(where: { ["workspace_local", "client_declared"].contains($0.projectSource ?? "") }) {
            return .observed
        }
        return .unknown
    }
}
