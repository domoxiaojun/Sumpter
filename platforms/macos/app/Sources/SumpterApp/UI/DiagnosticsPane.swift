import SumpterCore
import AppKit
import SwiftUI

private extension JSONEncoder {
    static var diagnosticPretty: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return encoder
    }
}

/// 诊断正文可能接近捕获上限，折叠状态下不要提前构造 Headers/Chunks 的长字符串或
/// 子视图树。与通用 `FullRowDisclosure` 的区别是这里保存 builder，只有展开后才调用。
private struct LazyRowDisclosure<Label: View, Content: View>: View {
    @State private var isExpanded = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private let label: () -> Label
    private let content: () -> Content

    init(
        @ViewBuilder label: @escaping () -> Label,
        @ViewBuilder content: @escaping () -> Content
    ) {
        self.label = label
        self.content = content
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                if reduceMotion {
                    isExpanded.toggle()
                } else {
                    withAnimation(.easeOut(duration: 0.18)) { isExpanded.toggle() }
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.semibold))
                        .frame(width: 18, height: 18)
                    label()
                    Spacer(minLength: 0)
                }
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(isExpanded ? "已展开" : "已收起")
            .accessibilityHint("双击展开或收起")

            if isExpanded {
                content()
                    .padding(.leading, 26)
                    .padding(.bottom, 4)
            }
        }
    }
}

/// Headers/Chunks 的拼接结果可能是数百 MiB。将字符串缓存到 disclosure 自身的状态中，
/// 避免索引轮询或其它 `@Published` 更新导致已展开区反复复制原始数组；收起时释放副本。
private struct LazyTextDisclosure<Label: View>: View {
    @State private var isExpanded = false
    @State private var resolvedText: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private let label: () -> Label
    private let text: () -> String

    init(
        @ViewBuilder label: @escaping () -> Label,
        text: @escaping () -> String
    ) {
        self.label = label
        self.text = text
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                let update = {
                    isExpanded.toggle()
                    if isExpanded, resolvedText == nil {
                        resolvedText = text()
                    } else if !isExpanded {
                        resolvedText = nil
                    }
                }
                if reduceMotion {
                    update()
                } else {
                    withAnimation(.easeOut(duration: 0.18)) { update() }
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.semibold))
                        .frame(width: 18, height: 18)
                    label()
                    Spacer(minLength: 0)
                }
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(isExpanded ? "已展开" : "已收起")
            .accessibilityHint("双击展开或收起")

            if isExpanded, let resolvedText {
                ScrollView([.horizontal, .vertical]) {
                    Text(resolvedText.isEmpty ? "(空)" : resolvedText)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                }
                .frame(maxHeight: 260)
                .padding(.leading, 26)
                .padding(.bottom, 4)
                .background(.black.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
            }
        }
    }
}

struct SecurityPane: View {
    @ObservedObject var model: AppModel
    @State private var inboundAuthToken = ""
    @State private var confirmClearToken = false
    /// 监听配置的本地草稿。
    ///
    /// 旧版把三个输入框直接绑到 `model.config.listener`:字没保存就已改内存配置,
    /// 菜单栏与运行页立刻显示新地址;更糟的是此后**任何**无关保存(改一条映射)
    /// 都会把这份半编辑值一并落盘。草稿隔离后,只有点「保存监听配置」才生效。
    @State private var draftHost = ""
    @State private var draftPort = ""
    @State private var draftCIDRs = ""
    @State private var draftTrustedProxies = ""
    @State private var draftLoaded = false
    @State private var listenerError: String?
    @State private var confirmRestartListener = false


    var body: some View {
        SettingsPage(title: SettingsSection.security.title, subtitle: SettingsSection.security.subtitle) {
            listenerPanel
            authPanel
            InboundTokensPanel(model: model)
            launchPanel
            UnifiedAttributionPanel(model: model)
        }
        .onAppear { loadDraftIfNeeded() }
        // 外部变更(/__reload 或手改文件)后同步草稿,避免拿旧值覆盖。
        .onChange(of: model.config.listener) { _, _ in
            if !listenerDirty { loadDraft() }
        }
        .confirmationDialog("清除入站 Token？", isPresented: $confirmClearToken) {
            Button("清除 Token", role: .destructive) {
                model.clearInboundAuthToken()
                inboundAuthToken = ""
            }
            Button("取消", role: .cancel) {}
        }
        .confirmationDialog(
            "代理正在运行,改监听地址会重启引擎并断开进行中的请求。继续？",
            isPresented: $confirmRestartListener
        ) {
            Button("保存并重启引擎", role: .destructive) { commitListener() }
            Button("取消", role: .cancel) {}
        }

    }

    private var listenerPanel: some View {
        SectionPanel(title: "监听", hint: "默认只监听 127.0.0.1；如果改成局域网地址，建议同时启用入站认证。") {
            VStack(alignment: .leading, spacing: 12) {
                FormLine(title: "Host") {
                    TextField("127.0.0.1", text: $draftHost)
                        .frame(maxWidth: 260)
                }
                FormLine(title: "Port") {
                    TextField("57878", text: $draftPort)
                        .frame(width: 120)
                }
                FormLine(title: "CIDR") {
                    TextField("192.168.31.0/24, 10.0.0.0/8", text: $draftCIDRs)
                }
                FormLine(title: "可信代理") {
                    VStack(alignment: .leading, spacing: 4) {
                        TextField("172.18.0.5, 10.0.0.0/8", text: $draftTrustedProxies)
                        Text("事件的请求源 IP 按 X-Real-IP、X-Forwarded-For、连接对端的顺序取第一个合法值，不需要先登记代理。这里填多层代理中的已知代理，X-Forwarded-For 链会跳过它们；不影响入站认证与 CIDR 白名单。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                if let listenerError {
                    Label(listenerError, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(.red)
                }
                HStack {
                    if listenerDirty {
                        Label("有未保存的修改", systemImage: "pencil.circle")
                            .font(.caption)
                            .foregroundStyle(.orange)
                    }
                    Spacer()
                    Button("还原") { loadDraft() }
                        .disabled(!listenerDirty)
                    Button {
                        attemptSaveListener()
                    } label: {
                        Label("保存监听配置", systemImage: "square.and.arrow.down")
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!listenerDirty)
                }
            }
        }
    }

    // MARK: - 监听草稿

    private func loadDraftIfNeeded() {
        guard !draftLoaded else { return }
        loadDraft()
        draftLoaded = true
    }

    private func loadDraft() {
        draftHost = model.config.listener.host
        draftPort = String(model.config.listener.port)
        draftCIDRs = model.config.listener.allowedCIDRs.joined(separator: ", ")
        draftTrustedProxies = model.config.listener.trustedProxyCIDRs.joined(separator: ", ")
        listenerError = nil
    }

    private var listenerDirty: Bool {
        parsedCIDRs != model.config.listener.allowedCIDRs
            || parsedTrustedProxies != model.config.listener.trustedProxyCIDRs
            || draftHost != model.config.listener.host
            || draftPort != String(model.config.listener.port)
    }

    /// 逗号/空白分隔;保留用户输入顺序,过滤空片段(输入中途的尾随逗号不再被吞)。
    private var parsedCIDRs: [String] {
        Self.splitCIDRList(draftCIDRs)
    }

    private var parsedTrustedProxies: [String] {
        Self.splitCIDRList(draftTrustedProxies)
    }

    private static func splitCIDRList(_ text: String) -> [String] {
        text
            .split { $0 == "," || $0 == " " || $0 == "\n" || $0 == "\t" }
            .map(String.init)
            .filter { !$0.isEmpty }
    }

    /// 保存前一次性校验:端口范围、CIDR 语法。旧版无校验,错值要等 rebind 失败才知道,
    /// 而且失败后代理停在停止态。
    private func validateListener() -> String? {
        let host = draftHost.trimmingCharacters(in: .whitespaces)
        if host.isEmpty {
            return "Host 不能为空(填 127.0.0.1 或 0.0.0.0)"
        }
        guard let port = Int(draftPort.trimmingCharacters(in: .whitespaces)) else {
            return "Port 必须是数字"
        }
        guard (1...65535).contains(port) else {
            return "Port 必须在 1-65535 之间"
        }
        for cidr in parsedCIDRs where !ClientAccessControl.isValidCIDR(cidr) {
            return "CIDR 格式不正确:\(cidr)"
        }
        for cidr in parsedTrustedProxies where !ClientAccessControl.isValidCIDR(cidr) {
            return "可信代理格式不正确:\(cidr)"
        }
        return nil
    }

    private func attemptSaveListener() {
        if let error = validateListener() {
            listenerError = error
            return
        }
        listenerError = nil
        // 运行中改监听 = 引擎重启 + 断流,先确认。
        if model.sidecarState.isActive {
            confirmRestartListener = true
        } else {
            commitListener()
        }
    }

    private func commitListener() {
        model.updateListener(
            host: draftHost.trimmingCharacters(in: .whitespaces),
            port: Int(draftPort.trimmingCharacters(in: .whitespaces)) ?? model.config.listener.port,
            allowedCIDRs: parsedCIDRs,
            trustedProxyCIDRs: parsedTrustedProxies
        )
    }

    /// 认证徽章颜色:本机监听下「未启用」是常态,用灰色;暴露到局域网才用橙色提醒。
    private var authBadgeColor: Color {
        if !model.config.listener.authToken.isEmpty {
            return .green
        }
        return Self.isLoopback(model.config.listener.host) ? .secondary : .orange
    }

    private static func isLoopback(_ host: String) -> Bool {
        let cleaned = host.trimmingCharacters(in: .whitespaces).lowercased()
        return cleaned == "127.0.0.1" || cleaned == "localhost" || cleaned == "::1"
    }

    private var authPanel: some View {
        SectionPanel(title: "默认入站 Token", hint: "启用后客户端需要携带对应 Token，适合局域网访问或多人共用。") {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    StatusBadge(
                        text: !model.config.listener.authToken.isEmpty ? "已启用" : "未启用",
                        systemImage: !model.config.listener.authToken.isEmpty ? "lock.fill" : "lock.open",
                        color: authBadgeColor
                    )
                    if !model.config.listener.authToken.isEmpty {
                        Text("明文 Token 已保存")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                }
                FormLine(title: "Token") {
                    RevealableSecureField(placeholder: "输入新 Token", text: $inboundAuthToken)
                }
                HStack {
                    Spacer()
                    Button {
                        model.setInboundAuthToken(inboundAuthToken)
                        inboundAuthToken = ""
                    } label: {
                        Label("设置 Token", systemImage: "key")
                    }
                    .disabled(inboundAuthToken.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    Button(role: .destructive) {
                        confirmClearToken = true
                    } label: {
                        Label("清除 Token", systemImage: "trash")
                    }
                    .disabled(model.config.listener.authToken.isEmpty)
                }
            }
        }
    }

    private var launchPanel: some View {
        SectionPanel(title: "登录项") {
            HStack {
                Toggle("登录时自动启动", isOn: Binding(
                    get: { model.loginItemEnabled },
                    set: { model.setLoginItem(enabled: $0) }
                ))
                Spacer()
                Button {
                    model.refreshLoginItemStatus()
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
            }
        }
    }


}

struct StorageLimitEditor: View {
    enum Presentation {
        case inline
        case sheet(onClose: () -> Void)
    }

    @ObservedObject var model: AppModel
    let probe: AdminWire.RuntimeStorageProbe
    var presentation: Presentation = .inline
    @State private var maxAgeDays = ""
    @State private var limitMB = ""
    @State private var dirty = false
    @State private var saving = false
    @State private var validationError: String?

    private var retention: AdminWire.RuntimeRetention { model.runtimeRetention ?? probe.retention }

    private var usedBytes: Int { probe.liveBytes }

    private func formatBytes(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .memory)
    }

    init(
        model: AppModel,
        probe: AdminWire.RuntimeStorageProbe,
        presentation: Presentation = .inline
    ) {
        self.model = model
        self.probe = probe
        self.presentation = presentation
    }

    @ViewBuilder
    private var formContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Label("自动保留策略", systemImage: "arrow.triangle.2.circlepath")
                    .font(.callout.weight(.semibold))
                Spacer(minLength: 0)
                Text(retentionSummary)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) { maxAgeInput; input; buttons }
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 10) { maxAgeInput; input }
                    buttons
                }
            }
            Text("系统按滚动 24 小时的保存天数和 SQLite 有效占用上限自动轮换；任一条件先达到就触发。按请求组删除，进行中的请求组会完整保留。留空可分别关闭对应条件；轮换不会立即缩小数据库文件，需要真正回收空间时使用“重置并新建数据库”。")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let validationError {
                Label(validationError, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if case .sheet = presentation, let error = model.runtimeV2Error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    var body: some View {
        Group {
            if case let .sheet(onClose) = presentation {
                SheetShell(
                    title: "运行统计存储设置",
                    primaryTitle: saving ? "保存中…" : "完成",
                    primaryDisabled: saving,
                    onCancel: onClose,
                    onSubmit: onClose
                ) {
                    formContent
                }
            } else {
                formContent
                    .padding(10)
                    .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
        }
        .task { syncFromModel() }
        .onChange(of: model.runtimeRetention?.maxAgeDays) { _, _ in syncFromModel() }
        .onChange(of: model.runtimeRetention?.storageLimitBytes) { _, _ in syncFromModel() }
        .onChange(of: maxAgeDays) { _, _ in
            dirty = true
            validationError = nil
        }
        .onChange(of: limitMB) { _, _ in
            dirty = true
            validationError = nil
        }
    }

    private var maxAgeInput: some View {
        HStack(spacing: 8) {
            Text("最长保存")
                .foregroundStyle(.secondary)
            TextField("例如 30", text: $maxAgeDays)
                .textFieldStyle(.roundedBorder)
                .frame(minWidth: 86, maxWidth: 120)
            Text("天")
                .foregroundStyle(.secondary)
        }
    }

    private var input: some View {
        HStack(spacing: 8) {
            Text("容量上限")
                .foregroundStyle(.secondary)
            TextField("例如 1024", text: $limitMB)
                .textFieldStyle(.roundedBorder)
                .frame(minWidth: 86, maxWidth: 120)
            Text("MB").foregroundStyle(.secondary)
        }
    }

    private var buttons: some View {
        HStack(spacing: 8) {
            Button(saving ? "保存中…" : "保存保留策略") { save() }
                .controlSize(.small).disabled(saving)
            Button("关闭时间上限") {
                maxAgeDays = ""
                dirty = false
                save(maxAgeDaysValue: nil, storageLimitBytesValue: retention.storageLimitBytes)
            }
            .controlSize(.small)
            .disabled(saving || retention.maxAgeDays == nil)
            Button("关闭容量上限") {
                limitMB = ""
                dirty = false
                save(maxAgeDaysValue: retention.maxAgeDays, storageLimitBytesValue: nil)
            }
            .controlSize(.small)
            .disabled(saving || retention.storageLimitBytes == nil)
        }
    }

    private var retentionSummary: String {
        switch (retention.maxAgeDays, retention.storageLimitBytes) {
        case let (age?, bytes?):
            let percent = min(100, Int((Double(usedBytes) / Double(max(1, bytes)) * 100).rounded()))
            return "最长 \(age) 天 · 容量 \(formatBytes(bytes)) · 已用 \(percent)%"
        case let (age?, nil):
            return "最长 \(age) 天 · 容量不限制"
        case let (nil, bytes?):
            let percent = min(100, Int((Double(usedBytes) / Double(max(1, bytes)) * 100).rounded()))
            return "时间不限制 · 容量 \(formatBytes(bytes)) · 已用 \(percent)%"
        case (nil, nil):
            return "仅手动清理"
        }
    }

    private func syncFromModel() {
        guard !dirty else { return }
        maxAgeDays = retention.maxAgeDays.map(String.init) ?? ""
        limitMB = retention.storageLimitBytes.map { String(max(1, $0 / 1_048_576)) } ?? ""
        dirty = false
    }

    private func save(maxAgeDaysValue: Int? = nil, storageLimitBytesValue: Int? = nil) {
        let ageText = maxAgeDays.trimmingCharacters(in: .whitespacesAndNewlines)
        let age = maxAgeDaysValue ?? (ageText.isEmpty ? nil : Int(ageText))
        guard ageText.isEmpty || (age ?? 0) >= 1 else {
            validationError = "最大保存天数必须是至少 1 天的整数"
            model.flash("最大保存天数必须是至少 1 天的整数")
            return
        }

        let value = limitMB.trimmingCharacters(in: .whitespacesAndNewlines)
        let megabytes = value.isEmpty ? nil : Int(value)
        let storageBytes: Int?
        if let storageLimitBytesValue {
            storageBytes = storageLimitBytesValue
        } else {
            guard value.isEmpty || ((megabytes ?? 0) >= 1 && (megabytes ?? 0) <= Int.max / 1_048_576) else {
                validationError = "存储上限必须是至少 1 MB 的整数"
                model.flash("存储上限必须是至少 1 MB 的整数")
                return
            }
            storageBytes = megabytes.map { $0 * 1_048_576 }
        }
        validationError = nil
        saving = true
        model.updateRuntimeRetention(AdminWire.RuntimeRetentionUpdate(
            expectedRevision: retention.revision,
            maxAgeDays: age,
            storageLimitBytes: storageBytes
        ))
        dirty = false
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { saving = false }
    }
}

struct DiagnosticsPane: View {
    /// 展开详情只用于定位问题，不应把接近 512 MiB/1 GiB 的捕获正文交给
    /// SwiftUI Text。完整原始内容仍可通过导出向导写入文件；面板内只显示有界预览。
    private let diagnosticPreviewLimit = 128 * 1024

    @ObservedObject var model: AppModel
    @State private var confirmRestore = false
    @State private var selectedCaptureID = ""
    /// 容量输入必须是 String 绑定。`TextField(value:format:)` 只在提交(回车/失焦)时才把
    /// 解析结果写回绑定,而「开始捕获」是同屏按钮 —— 用户敲完 1024 直接点按钮,binding 还是
    /// 上一个值,于是按 512 MB 开始。用文本绑定 + 自己解析,每次按键即时生效。
    @State private var captureCapacityMB = "512"
    @State private var captureCapacityDirty = false
    @State private var captureJSONEncodingBusy = false
    @State private var captureJSONEncodingError: String?
    @State private var captureCopyFeedback: String?
    @State private var captureJSONAction: CaptureJSONAction?
    @State private var captureJSONRequestID: UUID?
    @State private var confirmRawDetailJSON = false
    @State private var pendingRawDetailAction: CaptureJSONAction?
    @State private var detailLoadRequestedID: String?
    @State private var diagnosticExportPrivacy = "raw"
    @State private var confirmRawCaptureExport = false

    private enum CaptureJSONAction {
        case copy
        case export
    }

    private func formatBytes(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .memory)
    }

    private var rawCaptureExportMessage: String {
        "raw 快照可能接近 \(captureCapacityMB) MB，包含未脱敏的请求、响应、Headers 和流式 Chunk。仅保存到可信位置；导出通过临时文件流式完成，不加载到 App 内存。"
    }

    var body: some View {
        SettingsPage(title: SettingsSection.diagnostics.title, subtitle: SettingsSection.diagnostics.subtitle) {
            ClientConnectionDiagnostics(model: model)
            diagnosticCapturePanel
            diagnosticsPanel
            claudeSettingsPanel
        }
        .task {
            if !captureCapacityDirty, let maxBytes = model.diagnosticCapture?.maxBytes {
                captureCapacityMB = String(max(1, maxBytes / 1_048_576))
            }
            model.refreshDiagnosticCapture()
        }
        .task {
            // 页面可见时只低频刷新轻量索引；正文、Headers 与 Chunk 仍需显式读取。
            // 索引未变化时 AppModel 不发布新值，避免无意义地重绘整个诊断页。
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard !Task.isCancelled else { break }
                if !model.diagnosticCaptureBusy {
                    model.refreshDiagnosticCapture()
                }
            }
        }
        // 完整 JSON 只在用户明确点击复制/导出后编码。选中一条详情时不再
        // 自动把可能很大的捕获记录复制成 pretty-printed String。
        .task(id: captureJSONRequestID) {
            guard let requestID = captureJSONRequestID,
                  let action = captureJSONAction,
                  let capture = model.diagnosticCaptureDetail else { return }
            captureJSONEncodingBusy = true
            captureJSONEncodingError = nil
            captureCopyFeedback = nil
            defer {
                // 切换请求时 onChange 会先使当前请求失效；不要让旧任务覆盖
                // 新请求的状态。
                if captureJSONRequestID == requestID {
                    captureJSONEncodingBusy = false
                    captureJSONRequestID = nil
                    captureJSONAction = nil
                }
            }
            let encodingTask = Task.detached(priority: .utility) { () -> Data? in
                try? JSONEncoder.diagnosticPretty.encode(capture)
            }
            let encodedData = await withTaskCancellationHandler(operation: {
                await encodingTask.value
            }, onCancel: {
                encodingTask.cancel()
            })
            guard !Task.isCancelled else { return }
            guard let encodedData else {
                captureJSONEncodingError = "诊断 JSON 编码失败"
                return
            }
            switch action {
            case .copy:
                copyJSON(String(decoding: encodedData, as: UTF8.self))
            case .export:
                exportJSON(encodedData, requestID: capture.requestID)
            }
        }
        .onChange(of: model.diagnosticCapture?.records) { _, records in
            let records = records ?? []
            // 索引刷新不能隐式选中第一条:正文可能很大,详情必须由用户明确选择。
            // 已有选择仍在索引中时保持它;已被清空/淘汰时才清除选择。
            let selectedID = selectedCaptureID
            let selectionStillExists = records.contains { record in record.requestID == selectedID }
            if !selectedID.isEmpty && !selectionStillExists {
                selectedCaptureID = ""
            }
        }
        .onChange(of: selectedCaptureID) { _, value in
            // Index selection stays cheap. Full bodies/headers/chunks are
            // fetched only after an explicit user action below.
            detailLoadRequestedID = nil
            pendingRawDetailAction = nil
            confirmRawDetailJSON = false
            model.loadDiagnosticCaptureDetail(id: nil)
        }
        .onChange(of: model.diagnosticCaptureDetail?.requestID) { _, _ in
            // 详情切换时，取消/失效仍在后台编码的上一条记录。
            captureJSONRequestID = nil
            captureJSONAction = nil
            captureJSONEncodingBusy = false
            captureJSONEncodingError = nil
            captureCopyFeedback = nil
            pendingRawDetailAction = nil
            confirmRawDetailJSON = false
        }
        .onChange(of: model.diagnosticCapture?.maxBytes) { _, maxBytes in
            guard !captureCapacityDirty, let maxBytes else { return }
            captureCapacityMB = String(max(1, maxBytes / 1_048_576))
        }
        .onChange(of: model.diagnosticCapture?.enabled) { oldEnabled, enabled in
            // 只有服务端成功返回 stopped 才丢弃本地草稿；请求失败时保留用户输入，
            // 避免下一次索引刷新把尚未生效的容量静默改回旧值。
            if oldEnabled == true, enabled == false, model.diagnosticCaptureError == nil {
                captureCapacityDirty = false
                if let maxBytes = model.diagnosticCapture?.maxBytes {
                captureCapacityMB = String(max(1, maxBytes / 1_048_576))
            }
        }
        }
        .confirmationDialog("还原 Claude Code 配置？", isPresented: $confirmRestore) {
            Button("还原", role: .destructive) {
                model.restoreClaudeSettingsBackup()
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("当前 ~/.claude/settings.json 会被备份文件覆盖。")
        }
        .confirmationDialog(
            "导出未脱敏原始捕获？",
            isPresented: $confirmRawCaptureExport,
            titleVisibility: .visible
        ) {
            Button("确认导出 raw", role: .destructive) {
                model.exportDiagnosticCapture(privacy: "raw", confirmRaw: true)
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text(verbatim: rawCaptureExportMessage)
        }
        .confirmationDialog(
            "复制或导出原始诊断 JSON？",
            isPresented: $confirmRawDetailJSON,
            titleVisibility: .visible
        ) {
            Button("确认 raw JSON", role: .destructive) {
                guard let action = pendingRawDetailAction else { return }
                pendingRawDetailAction = nil
                beginCaptureJSON(action)
            }
            Button("取消", role: .cancel) { pendingRawDetailAction = nil }
        } message: {
            Text("当前详情包含原始请求/响应/Headers/Chunk，可能含敏感数据；仅用于可信的本地诊断。")
        }
    }

    @ViewBuilder
    private var captureCapacityControls: some View {
        ViewThatFits(in: .horizontal) {
            HStack { captureCapacityFields; Spacer(minLength: 0); captureStartStopButton }
            VStack(alignment: .leading, spacing: 8) {
                captureCapacityFields
                captureStartStopButton
            }
        }
    }

    @ViewBuilder
    private var captureCapacityFields: some View {
        Text("容量上限").font(.callout.weight(.semibold))
        TextField("容量", text: Binding(
            get: { captureCapacityMB },
            set: {
                captureCapacityMB = $0
                captureCapacityDirty = true
            }
        ))
            .textFieldStyle(.roundedBorder)
            .frame(width: 92)
            .disabled(model.diagnosticCapture?.enabled == true || model.diagnosticCaptureBusy)
            .accessibilityLabel("诊断捕获容量上限，单位 MB")
        Text("MB").font(.caption).foregroundStyle(.secondary)
        if model.diagnosticCapture?.enabled == true {
            Text("采集中不可修改").font(.caption).foregroundStyle(.secondary)
        }
    }

    private var captureStartStopButton: some View {
        Group {
            if model.diagnosticCapture?.enabled == true {
                Button("停止捕获") { model.setDiagnosticCapture(enabled: false) }
                    .disabled(model.diagnosticCaptureBusy)
            } else {
                Button("开始捕获") { model.setDiagnosticCapture(enabled: true, maxBytes: captureMaxBytes) }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.diagnosticCaptureBusy || !model.sidecarState.isActive || captureMaxBytes == nil)
            }
        }
    }

    private var diagnosticCapturePanel: some View {
        SectionPanel(title: "完整诊断捕获", hint: "捕获内容按原样暂存；刷新只读取索引，选中请求后才读取源数据详情。导出默认使用源数据，脱敏仅作为主动选项。") {
            VStack(alignment: .leading, spacing: 12) {
                let shownCount = model.diagnosticCapture?.records.count ?? 0
                let totalCount = model.diagnosticCapture?.recordCount ?? shownCount
                let indexSuffix = model.diagnosticCapture?.indexTruncated == true ? "（仅显示最近 200 条）" : ""
                ViewThatFits(in: .horizontal) {
                    HStack {
                        captureStatusBadge
                        captureIndexSummary(shownCount: shownCount, totalCount: totalCount, suffix: indexSuffix)
                        Spacer(minLength: 0)
                        captureToolbar(totalCount: totalCount)
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            captureStatusBadge
                            captureIndexSummary(shownCount: shownCount, totalCount: totalCount, suffix: indexSuffix)
                        }
                        captureToolbar(totalCount: totalCount)
                    }
                }
                captureCapacityControls
                if captureMaxBytes == nil {
                    Label("容量必须是大于 0 的整数 MB", systemImage: "exclamationmark.triangle.fill")
                        .font(.caption).foregroundStyle(.orange)
                } else if model.diagnosticCapture?.limitReached == true {
                    Label("已达到容量上限并自动停止；已捕获内容仍然保留。", systemImage: "externaldrive.badge.exclamationmark")
                        .foregroundStyle(.orange)
                } else {
                    Text("手动开始后持续捕获，手动停止不会清空；容量按全部保留记录累计，默认 512 MB。")
                        .font(.caption).foregroundStyle(.secondary)
                }
                if let error = model.diagnosticCaptureError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption).foregroundStyle(.orange)
                }
                if model.diagnosticCaptureExportBusy {
                    Label("正在下载全部捕获快照…（不加载到 App 内存）", systemImage: "arrow.down.circle")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if model.diagnosticCaptureBusy && model.diagnosticCapture == nil {
                    Text("正在读取捕获索引…").font(.callout).foregroundStyle(.secondary)
                } else if let capture = model.diagnosticCapture, !capture.records.isEmpty {
                    ViewThatFits(in: .horizontal) {
                        captureSelectionToolbar
                        VStack(alignment: .leading, spacing: 8) { captureSelectionToolbar }
                    }
                    if captureJSONEncodingBusy {
                        Label("正在准备诊断 JSON…", systemImage: "clock")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else if let captureJSONEncodingError {
                        Label(captureJSONEncodingError, systemImage: "exclamationmark.triangle.fill")
                            .font(.caption)
                            .foregroundStyle(.red)
                    } else if let captureCopyFeedback {
                        Label(captureCopyFeedback, systemImage: captureCopyFeedback == "已复制" ? "checkmark.circle" : "exclamationmark.triangle")
                            .font(.caption)
                            .foregroundStyle(captureCopyFeedback == "已复制" ? .green : .red)
                    }
                    if model.diagnosticCaptureDetailBusy {
                        Text("正在读取选中请求详情…").font(.callout).foregroundStyle(.secondary)
                    } else if let error = model.diagnosticCaptureDetailError {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .font(.caption).foregroundStyle(.orange)
                    } else if detailLoadRequestedID != selectedCaptureID,
                              let record = capture.records.first(where: { $0.requestID == selectedCaptureID }) {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("已加载索引：\(record.method) \(record.path)")
                                .font(.caption.monospaced())
                                .textSelection(.enabled)
                            Text("\(record.clientModel) · \(record.statusCode.map(String.init) ?? "进行中") · \(record.attemptCount) 次上游尝试 · \(record.clientChunkCount) 个客户端 Chunk")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text("正文、Headers 和 Chunk 尚未读取；这样切换请求不会卡住界面。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Button {
                                detailLoadRequestedID = selectedCaptureID
                                model.loadDiagnosticCaptureDetail(id: selectedCaptureID)
                            } label: {
                                Label("读取完整正文与流诊断", systemImage: "doc.text.magnifyingglass")
                            }
                            .buttonStyle(.borderedProminent)
                        }
                    } else if let detail = model.diagnosticCaptureDetail {
                        VStack(alignment: .leading, spacing: 10) {
                            Text("\(detail.method) \(detail.path) · \(detail.clientModel) → \(detail.effectiveModel) · \(detail.completedAtMS.map { "\($0)ms" } ?? "进行中")")
                                .font(.caption.monospaced()).textSelection(.enabled)
                            Text(RuntimeEventPresentation.protocolPath(sourceFormat: detail.sourceFormat, targetFormat: detail.targetFormat, routeMode: detail.routeMode))
                                .font(.caption).foregroundStyle(.secondary)
                            if let trace = detail.websocketTrace {
                                Text(RuntimeEventPresentation.websocketSummary(trace))
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Text(captureProjectLine(detail.clientDeclared))
                                .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                            if detail.truncated || detail.inboundBodyTruncated {
                                Label("达到内存上限，后续内容已截断", systemImage: "exclamationmark.triangle.fill")
                                    .foregroundStyle(.orange)
                            }
                            if let failure = detail.failureDetail {
                                diagnosticDisclosure("错误 · \(detail.failureKind ?? "unknown")", text: previewText(failure))
                            }
                            diagnosticDisclosure("入站请求 Headers", text: headersText(detail.inboundHeaders))
                            diagnosticDisclosure("入站原始 Body · \(detail.inboundBodyBytes) bytes", text: previewText(detail.inboundBody))
                            ForEach(Array(detail.attempts.enumerated()), id: \.element.id) { index, attempt in
                                LazyRowDisclosure(label: {
                                    Text("上游尝试 #\(index + 1) · \(attempt.endpointName) · \(attempt.responseStatus.map(String.init) ?? attempt.error ?? "等待响应")")
                                        .font(.callout.weight(.semibold))
                                        .lineLimit(2)
                                        .help("上游尝试 #\(index + 1) · \(attempt.endpointName)")
                                }) {
                                    VStack(alignment: .leading, spacing: 10) {
                                        diagnosticDisclosure("\(attempt.outboundMethod) \(attempt.outboundURL) Headers", text: headersText(attempt.outboundHeaders))
                                        diagnosticDisclosure("出站原始 Body · \(attempt.outboundBodyBytes) bytes", text: previewText(attempt.outboundBody))
                                        diagnosticDisclosure("上游响应 Headers", text: headersText(attempt.responseHeaders))
                                        diagnosticDisclosure("原始上游 Chunks", text: chunksText(attempt.upstreamChunks))
                                        if let error = attempt.error { diagnosticDisclosure("尝试错误", text: previewText(error)) }
                                    }.padding(.top, 8)
                                }
                            }
                            diagnosticDisclosure("桥接后客户端 Chunks", text: chunksText(detail.clientChunks))
                        }
                    }
                } else if model.diagnosticCapture != nil {
                    Text(model.diagnosticCapture?.enabled == true ? "正在等待下一次代理请求…" : "开启捕获后，新请求会显示在这里。")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
        }
    }

    private var captureStatusBadge: some View {
        StatusBadge(text: captureStatus.text, systemImage: captureStatus.icon, color: captureStatus.color)
    }

    private func captureIndexSummary(shownCount: Int, totalCount: Int, suffix: String) -> some View {
        let captured = Double(model.diagnosticCapture?.capturedBytes ?? 0)
        let maximum = Double(model.diagnosticCapture?.maxBytes ?? 512 * 1_048_576)
        let ratio = maximum > 0 ? captured / maximum : 0
        return VStack(alignment: .leading, spacing: 4) {
            Text("\(formatBytes(Int(captured))) / \(formatBytes(Int(maximum))) · 显示最近 \(shownCount) / 共 \(totalCount) 条索引\(suffix)")
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
            RuntimeProgressBar(
                value: ratio,
                color: model.diagnosticCapture?.limitReached == true ? .orange : .accentColor,
                label: "捕获容量 \(Int((min(1, max(0, ratio)) * 100).rounded()))%"
            )
            .frame(maxWidth: 260)
        }
    }

    @ViewBuilder
    private func captureToolbar(totalCount: Int) -> some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) { captureToolbarControls(totalCount: totalCount) }
            VStack(alignment: .leading, spacing: 8) { captureToolbarControls(totalCount: totalCount) }
        }
    }

    @ViewBuilder
    private func captureToolbarControls(totalCount: Int) -> some View {
        Button("清空", role: .destructive) { model.clearDiagnosticCapture() }
            .disabled(model.diagnosticCaptureBusy || model.diagnosticCapture?.records.isEmpty != false)
        Picker("导出隐私", selection: $diagnosticExportPrivacy) {
            Text("源数据（默认）").tag("raw")
            Text("脱敏副本").tag("redacted")
        }
        .labelsHidden()
        Button {
            if diagnosticExportPrivacy == "raw" {
                confirmRawCaptureExport = true
            } else {
                model.exportDiagnosticCapture(privacy: "redacted")
            }
        } label: {
            Label(diagnosticExportPrivacy == "raw" ? "导出源数据…" : "导出脱敏副本", systemImage: "arrow.down.doc")
        }
        .disabled(model.diagnosticCaptureExportBusy || totalCount == 0)
    }

    @ViewBuilder
    private var captureSelectionToolbar: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) { captureSelectionControls }
            VStack(alignment: .leading, spacing: 8) { captureSelectionControls }
        }
    }

    @ViewBuilder
    private var captureSelectionControls: some View {
        Picker("请求", selection: $selectedCaptureID) {
            Text("选择请求以读取源数据详情…").tag("")
            if let records = model.diagnosticCapture?.records {
                ForEach(records) { record in
                    Text("\(record.clientModel) · \(record.statusCode.map(String.init) ?? "进行中") · \(record.requestID)").tag(record.requestID)
                }
            }
        }
        .labelsHidden()
        Text("单条复制/导出仅针对当前选中；上方导出默认源数据，也支持脱敏副本")
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(2)
        Button {
            requestCaptureJSON(.copy)
        } label: {
            Label(captureCopyFeedback == "已复制" ? "已复制" : "复制 raw JSON", systemImage: captureCopyFeedback == "已复制" ? "checkmark" : "doc.on.doc")
        }
                        .disabled(!detailMatchesSelection || captureJSONEncodingBusy)
        Button("导出 raw JSON…") {
            requestCaptureJSON(.export)
        }
                        .disabled(!detailMatchesSelection || captureJSONEncodingBusy)
    }

    private var captureMaxBytes: Int? {
        guard let megabytes = Int(captureCapacityMB.trimmingCharacters(in: .whitespaces)),
              megabytes > 0,
              megabytes <= Int.max / 1_048_576 else { return nil }
        return megabytes * 1_048_576
    }

    private var captureStatus: (text: String, icon: String, color: Color) {
        if model.diagnosticCapture?.enabled == true { return ("采集中", "record.circle.fill", .green) }
        if model.diagnosticCapture?.limitReached == true { return ("已达容量", "externaldrive.badge.exclamationmark", .orange) }
        if model.diagnosticCapture?.stopReason == "manual" { return ("已停止", "stop.circle", .secondary) }
        return ("未开始", "pause.circle", .secondary)
    }

    private func diagnosticDisclosure(
        _ title: String,
        text: @autoclosure @escaping () -> String
    ) -> some View {
        LazyTextDisclosure(label: {
            Text(title)
                .font(.callout.weight(.semibold))
                .lineLimit(2)
                .help(title)
        }, text: text)
    }

    /// 捕获记录只带客户端用 `X-Sumpter-*` 声明的归因;Codex 的结构化 workspace 没有复制
    /// 进捕获(它在入站 Body 的 client_metadata 里),所以没声明时不能笼统写「未识别项目」。
    private func captureProjectLine(_ declared: ClientDeclaredMetadata?) -> String {
        guard let context = RuntimeEventPresentation.projectContext(
            eventKind: "client",
            metadata: nil,
            declared: declared
        ), context.source == .clientDeclared else {
            return "项目：未声明（Codex 工作区见入站 Body 的 client_metadata）"
        }
        let detail = context.detail.map { "（\($0)）" } ?? ""
        return "项目：\(context.name)\(detail) · \(context.source.label)"
    }

    private func headersText(_ headers: [AdminWire.DiagnosticHeader]) -> String {
        var output = ""
        var used = 0
        var truncated = false
        for header in headers {
            // Append pieces independently: interpolating a single enormous header
            // value would first allocate the whole value again before the bound
            // could be applied.
            appendLimited(header.name, to: &output, used: &used, truncated: &truncated)
            appendLimited(": ", to: &output, used: &used, truncated: &truncated)
            appendLimited(header.value, to: &output, used: &used, truncated: &truncated)
            guard !truncated else { break }
            appendLimited("\n", to: &output, used: &used, truncated: &truncated)
        }
        return previewSuffix(output, truncated: truncated)
    }

    private func chunksText(_ chunks: [AdminWire.DiagnosticChunk]) -> String {
        var output = ""
        var used = 0
        var truncated = false
        for (index, chunk) in chunks.enumerated() {
            if index > 0 {
                appendLimited("\n\n", to: &output, used: &used, truncated: &truncated)
            }
            appendLimited("[+\(chunk.atMS)ms · \(chunk.bytes) bytes\(chunk.truncated ? " · 已截断" : "")]\n", to: &output, used: &used, truncated: &truncated)
            appendLimited(chunk.data, to: &output, used: &used, truncated: &truncated)
            guard !truncated else { break }
        }
        return previewSuffix(output, truncated: truncated)
    }

    private func previewText(_ text: String) -> String {
        var output = ""
        var used = 0
        var truncated = false
        appendLimited(text, to: &output, used: &used, truncated: &truncated)
        return previewSuffix(output, truncated: truncated)
    }

    private func appendLimited(
        _ text: String,
        to output: inout String,
        used: inout Int,
        truncated: inout Bool
    ) {
        guard !truncated else { return }
        let remaining = diagnosticPreviewLimit - used
        guard remaining > 0 else {
            truncated = true
            return
        }
        let bytes = text.utf8
        if bytes.count <= remaining {
            output.append(text)
            used += bytes.count
        } else {
            output.append(String(decoding: bytes.prefix(remaining), as: UTF8.self))
            used = diagnosticPreviewLimit
            truncated = true
        }
    }

    private func previewSuffix(_ text: String, truncated: Bool) -> String {
        guard truncated else { return text.isEmpty ? "(空)" : text }
        return text + "\n\n… 已截断，仅显示前 128 KiB；请使用文件导出获取完整内容。"
    }

    private func requestCaptureJSON(_ action: CaptureJSONAction) {
        guard detailMatchesSelection, !captureJSONEncodingBusy else { return }
        pendingRawDetailAction = action
        confirmRawDetailJSON = true
    }

    private var detailMatchesSelection: Bool {
        guard !selectedCaptureID.isEmpty,
              let detail = model.diagnosticCaptureDetail else { return false }
        return detail.requestID == selectedCaptureID
    }

    private func beginCaptureJSON(_ action: CaptureJSONAction) {
        guard detailMatchesSelection, !captureJSONEncodingBusy else { return }
        captureJSONAction = action
        captureJSONRequestID = UUID()
    }

    private func copyJSON(_ text: String) {
        if PasteboardCopy.write(text) {
            captureCopyFeedback = "已复制"
            model.flash("raw 诊断 JSON 已复制")
        } else {
            captureCopyFeedback = "复制失败"
            model.flash("复制失败")
        }
    }

    private func exportJSON(_ data: Data, requestID: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "sumpter-diagnostic-\(requestID).json"
        if panel.runModal() == .OK, let url = panel.url {
            do {
                try data.write(to: url, options: .atomic)
                model.flash("raw 诊断 JSON 已导出")
            } catch {
                model.flash("诊断 JSON 导出失败")
            }
        }
    }

    private var diagnosticsPanel: some View {
        SectionPanel(title: "诊断") {
            VStack(alignment: .leading, spacing: 14) {
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 8) {
                    InfoRow(title: "配置路径", value: model.configPath, copyable: true)
                    InfoRow(title: "当前状态", value: "\(model.statusText) · \(model.config.listener.host):\(model.config.listener.port)")
                    InfoRow(title: "最近请求", value: recentEventLine)
                }
                HStack {
                    Button {
                        model.openConfigDirectory()
                    } label: {
                        Label("打开配置目录", systemImage: "folder")
                    }
                    Spacer()
                }
            }
        }
    }

    private var claudeSettingsPanel: some View {
        SectionPanel(title: "Claude Code 配置备份", hint: "启用通知会改写 ~/.claude/settings.json；备份后可随时还原。") {
            HStack {
                Button {
                    model.backupClaudeSettings()
                } label: {
                    Label("备份配置", systemImage: "archivebox")
                }
                Button(role: .destructive) {
                    confirmRestore = true
                } label: {
                    Label("还原配置", systemImage: "arrow.uturn.backward")
                }
                .disabled(!ClaudeNotificationHooks.backupExists())
                Spacer()
                StatusBadge(
                    text: ClaudeNotificationHooks.backupExists() ? "已有备份" : "无备份",
                    systemImage: ClaudeNotificationHooks.backupExists() ? "checkmark.circle.fill" : "tray",
                    color: ClaudeNotificationHooks.backupExists() ? .green : .secondary
                )
            }
        }
    }

    private var recentEventLine: String {
        guard let event = model.runtime.recentEvents.first else {
            return "暂无最近请求"
        }
        return [
            RuntimeEventDisplay.time(event.timestamp),
            RuntimeEventDisplay.kind(event.kind),
            event.requestPurpose?.displayName,
            event.clientModel ?? event.upstreamModel,
            RuntimeEventDisplay.endpoint(event),
            RuntimeEventDisplay.outcome(event),
            RuntimeEventDisplay.statusDetail(event),
            RuntimeEventPresentation.durationDisplay(
                event.durationMS,
                inFlight: event.isInFlight,
                startedAt: event.timestamp
            )
        ].compactMap { $0 }.joined(separator: " · ")
    }

}
