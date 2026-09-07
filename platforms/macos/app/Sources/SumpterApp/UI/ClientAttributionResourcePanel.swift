import AppKit
import SwiftUI

/// 非 shell 配置器（Gemini wrapper / pi 扩展）的资源与安装说明卡片。
/// 这两类脚本没有统一的 status/install 子命令，因此只提供资源存在性和可复制命令。
struct ClientAttributionResourcePanel: View {
    let title: String
    let subtitle: String
    let resourceName: String
    let resourceExtension: String
    let clientName: String
    let command: (String) -> String
    let detail: String

    private var resourceURL: URL? {
        Bundle.main.url(forResource: resourceName, withExtension: resourceExtension)
            ?? Bundle.module.url(forResource: resourceName, withExtension: resourceExtension)
    }

    var body: some View {
        SectionPanel(title: title, hint: subtitle) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Image(systemName: resourceURL == nil ? "xmark.octagon.fill" : "checkmark.circle.fill")
                        .foregroundStyle(resourceURL == nil ? .red : .green)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(resourceURL == nil ? "App 资源缺失" : "App 资源已内置")
                            .font(.callout.weight(.semibold))
                        Text(resourceURL == nil
                             ? "请重新安装完整 App，或从仓库对应脚本路径获取文件。"
                             : "\(clientName) 的归因资源可以从当前 App 复制到客户端主机。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let resourceURL {
                    let installCommand = command(resourceURL.path)
                    HStack(alignment: .top, spacing: 8) {
                        Text(installCommand)
                            .font(.caption2.monospaced())
                            .textSelection(.enabled)
                        Button("复制命令") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(installCommand, forType: .string)
                        }
                        .controlSize(.small)
                    }
                }
            }
        }
    }
}
