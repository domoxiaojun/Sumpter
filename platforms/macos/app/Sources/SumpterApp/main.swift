import AppKit
import SumpterCore
import ServiceManagement
import SwiftUI
import UserNotifications

struct SumpterNativeApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        // 状态项与主窗口都交给 AppDelegate 经 AppKit 管理(见 StatusItemController):
        // SwiftUI 的 MenuBarExtra 只能整体选 .window 或 .menu,左右键行为必然一致,
        // 做不到「左键直接开主窗口、右键弹操作菜单」。
        //
        // SwiftUI App 要求至少声明一个 Scene;本 app 是 LSUIElement(无 Dock 图标、
        // 无应用菜单栏),Settings 场景不会被显示出来,只用于满足这个要求。
        Settings { EmptyView() }
    }
}

/// macOS 要求 UNUserNotificationCenter 的 delegate 在 app 启动完成前挂上,
/// 否则 app 在前台时到达的通知不会回调 willPresent,系统默认不弹 banner。
/// 纯 SwiftUI App 无 AppDelegate,这里用 NSApplicationDelegateAdaptor 补上。
///
/// 同时在这里建状态项:AppModel 由 StatusItemController 持有(它和设置窗口共用同一份)。
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItemController: StatusItemController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        MainActor.assumeIsolated {
            NativeNotifier.shared.configure()
            statusItemController = StatusItemController(model: AppModel())
        }
    }
}

enum NotificationAuthorizationState: String, Sendable {
    case notDetermined
    case denied
    case authorized
    case provisional
    case unknown

    init(_ status: UNAuthorizationStatus) {
        switch status {
        case .notDetermined:
            self = .notDetermined
        case .denied:
            self = .denied
        case .authorized:
            self = .authorized
        case .provisional:
            self = .provisional
        @unknown default:
            self = .unknown
        }
    }

    var allowsDelivery: Bool {
        self == .authorized || self == .provisional
    }
}

struct SystemSoundOption: Hashable, Identifiable, Sendable {
    let fileName: String
    let url: URL

    var id: String { fileName }

    var title: String {
        URL(fileURLWithPath: fileName).deletingPathExtension().lastPathComponent
    }
}

enum SystemSoundCatalog {
    static let directoryURL = URL(fileURLWithPath: "/System/Library/Sounds", isDirectory: true)
    private static let supportedExtensions: Set<String> = ["aiff", "wav", "caf", "m4a", "mp3"]

    static func available() -> [SystemSoundOption] {
        guard let urls = try? FileManager.default.contentsOfDirectory(
            at: directoryURL,
            includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles]
        ) else {
            return []
        }
        return urls
            .filter { url in
                guard supportedExtensions.contains(url.pathExtension.lowercased()) else { return false }
                return (try? url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true
            }
            .map { SystemSoundOption(fileName: $0.lastPathComponent, url: $0) }
            .sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
    }

    static func option(named fileName: String) -> SystemSoundOption? {
        available().first { $0.fileName == fileName }
    }
}

enum NotificationSoundPreference: Hashable, Identifiable, Sendable {
    case systemDefault
    case silent
    case system(fileName: String)

    static let defaultsKey = "notificationSoundPreference"

    var id: String {
        switch self {
        case .systemDefault: "systemDefault"
        case .silent: "silent"
        case .system(let fileName): "system:\(fileName)"
        }
    }

    var title: String {
        switch self {
        case .systemDefault: "系统默认"
        case .silent: "静音"
        case .system(let fileName):
            SystemSoundOption(fileName: fileName, url: SystemSoundCatalog.directoryURL.appendingPathComponent(fileName)).title
        }
    }

    var systemSoundName: String? {
        switch self {
        case .system(let fileName): fileName
        case .systemDefault, .silent: nil
        }
    }

    static var allCases: [NotificationSoundPreference] {
        [.systemDefault, .silent] + SystemSoundCatalog.available().map { .system(fileName: $0.fileName) }
    }

    static func load() -> NotificationSoundPreference {
        guard let value = UserDefaults.standard.string(forKey: defaultsKey) else {
            return .systemDefault
        }
        switch value {
        case "systemDefault":
            return .systemDefault
        case "silent":
            return .silent
        default:
            let prefix = "system:"
            if value.hasPrefix(prefix) {
                let fileName = String(value.dropFirst(prefix.count))
                return SystemSoundCatalog.option(named: fileName).map { .system(fileName: $0.fileName) } ?? .systemDefault
            }
            // 兼容旧版本保存的裸名称（例如 `pop`），按当前系统目录中的
            // 文件名迁移；不再维护一份硬编码的系统音效清单。
            return SystemSoundCatalog.available()
                .first { $0.title.caseInsensitiveCompare(value) == .orderedSame }
                .map { .system(fileName: $0.fileName) } ?? .systemDefault
        }
    }

    func save() {
        UserDefaults.standard.set(id, forKey: Self.defaultsKey)
    }
}

/// Claude Code、Codex CLI 与 Grok Build 在协议层事件名不同，但在用户侧只有同一组
/// 通知意图。这个分类集中维护，避免设置页出现两套重复开关。
enum UnifiedNotificationCategory: String, CaseIterable, Identifiable, Sendable {
    case actionRequired = "action_required"
    case status
    case turnCompleted = "turn_completed"
    case subtaskCompleted = "subtask_completed"
    case turnFailed = "turn_failed"

    var id: String { rawValue }
    var category: String { rawValue }

    var title: String {
        switch self {
        case .actionRequired: "需要我处理"
        case .status: "普通状态提示"
        case .turnCompleted: "回合完成"
        case .subtaskCompleted: "子任务完成"
        case .turnFailed: "回合异常 / 中断"
        }
    }

    var hint: String {
        switch self {
        case .actionRequired: "权限、输入、选择、确认"
        case .status: "低频状态变化（默认关闭）"
        case .turnCompleted: "主会话正常结束"
        case .subtaskCompleted: "Agent / 子代理任务结束"
        case .turnFailed: "最终失败或客户端主动中断"
        }
    }

    var systemImage: String {
        switch self {
        case .actionRequired: "hand.raised"
        case .status: "info.circle"
        case .turnCompleted: "checkmark.circle"
        case .subtaskCompleted: "square.stack.3d.up"
        case .turnFailed: "exclamationmark.triangle"
        }
    }

}

private func loadNotificationPreference(
    key: String,
    legacyKey: String,
    defaultValue: Bool
) -> Bool {
    let defaults = UserDefaults.standard
    if let value = defaults.object(forKey: key) as? Bool {
        return value
    }
    if let value = defaults.object(forKey: legacyKey) as? Bool {
        defaults.set(value, forKey: key)
        return value
    }
    return defaultValue
}

/// 运行页/统计页自动刷新设置的 UserDefaults 键与可选间隔。
private let autoRefreshEnabledDefaultsKey = "runtimeAutoRefreshEnabled"
private let runtimeAutoRefreshIntervalDefaultsKey = "runtimeAutoRefreshIntervalSeconds"
private let statisticsAutoRefreshIntervalDefaultsKey = "statisticsAutoRefreshIntervalSeconds"
private let runHistoryPageSizeDefaultsKey = "runtimeRunHistoryPageSize"
private let runtimeHistoryPageSizeDefaultsKey = "runtimeStatisticsHistoryPageSize"
private let runtimeErrorPageSizeDefaultsKey = "runtimeStatisticsErrorPageSize"
private let runtimeDimensionPageSizeDefaultsKey = "runtimeStatisticsDimensionPageSize"
private let runtimeEndpointPageSizeDefaultsKey = "runtimeStatisticsEndpointPageSize"
private let runtimeProjectPageSizeDefaultsKey = "runtimeStatisticsProjectPageSize"
private let runtimeSessionPageSizeDefaultsKey = "runtimeStatisticsSessionPageSize"
private let runtimeModelPageSizeDefaultsKey = "runtimeStatisticsModelPageSize"
let autoRefreshIntervalChoices: [Double] = [1, 2, 5, 10, 15, 30]

private func storedRuntimePageSize(forKey key: String, default fallback: Int = AdminWire.RuntimeHistoryPage.defaultPageSize) -> Int {
    let stored = UserDefaults.standard.integer(forKey: key)
    return AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(stored) ? stored : fallback
}

@MainActor
final class AppModel: ObservableObject {
    // AppModel 行为按职责拆到同模块的 extension 文件；状态保持模块内可见，
    // 让各 extension 共享同一套 @Published 与 latest-wins 生命周期。
    static let runtimeAPIVersion = 1
    @Published var config: AppConfig = .bootstrap
    @Published var isProxyRunning = false
    @Published var statusText = "正在初始化"
    @Published var configPath = ""
    @Published var lastError: String?
    @Published var endpointCount = 0
    @Published var runtime = RuntimeSnapshot()
    @Published var runtimeSummary: AdminWire.RuntimeSummary?
    @Published var runtimeAnalytics: AdminWire.RuntimeAnalytics?
    /// Lightweight picker snapshot; unlike `runtimeAnalytics` it contains
    /// only the lightweight filter facets and is safe to refresh on the statistics
    /// cadence.
    @Published var runtimeFacets: AdminWire.RuntimeFacetSnapshot?
    /// These errors are independent from admin liveness. An analytics decode/query
    /// failure must not make a healthy daemon appear unreachable.
    @Published var runtimeSummaryError: String?
    @Published var runtimeAnalyticsError: String?
    @Published var runtimeAnalyticsLoading = false
    @Published var runtimeEventsError: String?
    @Published var runtimePage: AdminWire.RuntimeEventPage?
    @Published var runtimeEventDetail: AdminWire.RuntimeEventDetail?
    /// 运行页自己的稳定历史快照。统计页使用 runtimeHistoryPage；两者不能
    /// 共用页码、筛选或选中项，否则切换页面会悄悄改变另一页的结果。
    @Published var runHistoryPage: AdminWire.RuntimeHistoryPage?
    @Published var runHistoryPageSize: Int = {
        let stored = UserDefaults.standard.integer(forKey: runHistoryPageSizeDefaultsKey)
        return AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(stored) ? stored : AdminWire.RuntimeHistoryPage.defaultPageSize
    }() {
        didSet { UserDefaults.standard.set(runHistoryPageSize, forKey: runHistoryPageSizeDefaultsKey) }
    }
    @Published var runHistoryKindFilter = "client"
    @Published var runHistoryLoading = false
    @Published var runHistoryError: String?
    @Published var runtimeAnalyticsRange = "today"
    @Published var runtimeAnalyticsClientKind = ""
    @Published var runtimeAnalyticsEndpointID = ""
    @Published var runtimeAnalyticsProject = ""
    @Published var runtimeAnalyticsSessionID = ""
    @Published var runtimeAnalyticsModel = ""
    @Published var runtimeAnalyticsRequestPurpose = ""
    @Published var runtimeAnalyticsOutcome = ""
    @Published var runtimeAnalyticsFailureKind = ""
    @Published var runtimeAnalyticsFailurePhase = ""
    // SSE cursor is an internal reconciliation detail. Publishing every
    // cursor advance invalidates every AppModel-observing pane although no
    // visible value changed.
    var runtimeChangeSeq = 0
    // v2 history/analytics state. The v1 cursor page above remains in place
    // for the run view and old daemons; the statistics view uses this stable
    // snapshot so paging never re-parses the whole runtime store.
    @Published var runtimeHistoryPage: AdminWire.RuntimeHistoryPage?
    @Published var runtimeHistoryPageSize: Int = storedRuntimePageSize(forKey: runtimeHistoryPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeHistoryPageSize, forKey: runtimeHistoryPageSizeDefaultsKey) }
    }
    @Published var runtimeHistoryLoading = false
    @Published var runtimeHistoryError: String?
    @Published var runtimeHistorySnapshotSeq: Int?
    @Published var runtimeHistoryGeneration: Int?
    @Published var runtimeRequestChain: AdminWire.RuntimeRequestChain?
    @Published var runtimeRequestChainLoading = false
    @Published var runtimeRequestChainError: String?
    @Published var runtimeTrendSeries: AdminWire.RuntimeTrendSeries?
    @Published var runtimeErrorPage: AdminWire.RuntimeErrorPage?
    @Published var runtimeErrorPageSize: Int = storedRuntimePageSize(forKey: runtimeErrorPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeErrorPageSize, forKey: runtimeErrorPageSizeDefaultsKey) }
    }
    @Published var runtimeErrorPageLoading = false
    /// 旧的单维度页状态保留给兼容视图；当前概览和成本看板使用三张
    /// 独立分页表，同时保留各自搜索、排序和页大小。
    @Published var runtimeDimensionPage: AdminWire.RuntimeDimensionPage?
    @Published var runtimeDimensionPageLoading = false
    @Published var runtimeDimensionKind = "project"
    @Published var runtimeDimensionSearch = ""
    @Published var runtimeDimensionSort = "last_seen"
    @Published var runtimeDimensionOrder = "desc"
    @Published var runtimeDimensionPageSize: Int = storedRuntimePageSize(forKey: runtimeDimensionPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeDimensionPageSize, forKey: runtimeDimensionPageSizeDefaultsKey) }
    }
    @Published var runtimeProjectsPage: AdminWire.RuntimeDimensionPage?
    @Published var runtimeSessionsPage: AdminWire.RuntimeDimensionPage?
    @Published var runtimeEndpointsPage: AdminWire.RuntimeDimensionPage?
    @Published var runtimeModelsPage: AdminWire.RuntimeDimensionPage?
    @Published var runtimeDimensionsLoading = false
    @Published var runtimeV2Error: String?
    @Published var runtimeV2Loading = false
    @Published var runtimeProjectSearch = ""
    @Published var runtimeSessionSearch = ""
    @Published var runtimeProjectSort = "last_seen"
    @Published var runtimeSessionSort = "last_seen"
    @Published var runtimeProjectOrder = "desc"
    @Published var runtimeSessionOrder = "desc"
    @Published var runtimeEndpointSearch = ""
    @Published var runtimeEndpointSort = "last_seen"
    @Published var runtimeEndpointOrder = "desc"
    @Published var runtimeModelSearch = ""
    @Published var runtimeModelSort = "last_seen"
    @Published var runtimeModelOrder = "desc"
    @Published var runtimeEndpointPageSize: Int = storedRuntimePageSize(forKey: runtimeEndpointPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeEndpointPageSize, forKey: runtimeEndpointPageSizeDefaultsKey) }
    }
    @Published var runtimeProjectPageSize: Int = storedRuntimePageSize(forKey: runtimeProjectPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeProjectPageSize, forKey: runtimeProjectPageSizeDefaultsKey) }
    }
    @Published var runtimeSessionPageSize: Int = storedRuntimePageSize(forKey: runtimeSessionPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeSessionPageSize, forKey: runtimeSessionPageSizeDefaultsKey) }
    }
    @Published var runtimeModelPageSize: Int = storedRuntimePageSize(forKey: runtimeModelPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeModelPageSize, forKey: runtimeModelPageSizeDefaultsKey) }
    }
    @Published var runtimeV2ProjectID = ""
    /// Optional display-name alias paired with the stable project ID. The
    /// daemon treats both fields as independent AND predicates; keeping the
    /// name here prevents a UI-selected renamed project from being sent as an
    /// ID in the wrong column.
    @Published var runtimeV2ProjectName = ""
    @Published var runtimeV2SessionID = ""
    /// UI-only project drill-down. Unlike `runtimeV2ProjectID`, this never
    /// changes the global snapshot filters; it is merged into session
    /// projection requests only.
    @Published var runtimeLocalProjectID = ""
    @Published var runtimeLocalProjectName = ""
    /// UI-only session drill-down used by the model table. It does not alter
    /// the global analytics filters or the project/session picker state.
    @Published var runtimeLocalSessionID = ""
    @Published var runtimeLocalSessionName = ""
    @Published var runtimeStorageProbe: AdminWire.RuntimeStorageProbe?
    @Published var runtimeRetention: AdminWire.RuntimeRetention?
    @Published var runtimePricing: AdminWire.RuntimePricing?
    @Published var runtimeExportEstimate: AdminWire.RuntimeExportEstimate?
    @Published var runtimeExportEstimateError: String?
    @Published var runtimeExportBusy = false
    /// 统计页可见性与当前看板由 UsagePane 明确告知模型。后台轮询只在
    /// 统计页可见时读取轻量 facets 与当前 v3 看板，避免运行页/设置页
    /// 承担高基数聚合。
    @Published var statisticsVisible = false
    @Published var health = ProxyHealthSummary(state: .stopped, headline: "已停止")
    @Published var transientMessage: String?
    @Published var claudeNotificationsEnabled = false
    @Published var claudeNotificationArguments: Set<String> = []
    @Published var codexNotificationsEnabled = false
    @Published var codexNotificationArguments: Set<String> = []
    @Published var codexNotificationHookStatus: CodexNotificationHookStatus = .notConfigured
    @Published var codexNotificationHookPath = "~/.codex/hooks.json"
    @Published var grokNotificationsEnabled = false
    @Published var grokNotificationHookPath = "~/.grok/hooks/sumpter-notify.json"
    /// 统一客户端通知类别开关。行动/完成/失败默认开启；普通状态默认关闭，
    /// 避免 auth_success、computer_use_exit 等高频状态把真正需要处理的
    /// 权限/选择和最终失败淹没。类别过滤对 Claude 与 Codex 共用。
    @Published var actionNotificationsEnabled = loadNotificationPreference(
        key: "notificationActionRequiredEnabled", legacyKey: "claudeActionNotificationsEnabled", defaultValue: true
    )
    @Published var statusNotificationsEnabled = loadNotificationPreference(
        key: "notificationStatusEnabled", legacyKey: "claudeStatusNotificationsEnabled", defaultValue: false
    )
    @Published var turnCompletionNotificationsEnabled = loadNotificationPreference(
        key: "notificationTurnCompletedEnabled", legacyKey: "claudeTurnCompletionNotificationsEnabled", defaultValue: true
    )
    @Published var subtaskNotificationsEnabled = loadNotificationPreference(
        key: "notificationSubtaskCompletedEnabled", legacyKey: "claudeSubtaskNotificationsEnabled", defaultValue: true
    )
    @Published var failureNotificationsEnabled = loadNotificationPreference(
        key: "notificationTurnFailedEnabled", legacyKey: "claudeFailureNotificationsEnabled", defaultValue: true
    )
    @Published var notificationAuthorizationStatus: NotificationAuthorizationState = .unknown
    @Published var notificationSoundPreference = NotificationSoundPreference.load()
    @Published var notificationError: String?
    @Published var loginItemEnabled = false
    @Published var fetchingModelEndpointIDs: Set<String> = []
    /// sidecar 进程生命周期;与上游健康正交,菜单栏优先展示它。
    @Published var sidecarState: SidecarState = .stopped
    /// sumpterd 上次 reload 回执里的配置代号;与本地落盘内容对不上即「引擎未跟上配置」。
    @Published var engineGeneration: String?
    @Published var configMigrationNotice: ConfigMigrationNotice?
    /// 捕获面板只持有轻量索引；明文详情按当前选中请求懒加载。
    @Published var diagnosticCapture: AdminWire.DiagnosticCaptureIndex?
    @Published var diagnosticCaptureDetail: AdminWire.DiagnosticRequestCapture?
    @Published var diagnosticCaptureError: String?
    @Published var diagnosticCaptureDetailError: String?
    @Published var diagnosticCaptureBusy = false
    @Published var diagnosticCaptureDetailBusy = false
    @Published var diagnosticCaptureExportBusy = false
    /// 菜单栏/运行页的总状态:进程态优先,其次上游健康。
    var indicator: StatusIndicator {
        switch sidecarState {
        case .stopped: .stopped
        case .starting: .starting
        case .crashed: .crashed
        case .unreachable: .unreachable
        case .running: .health(health.state)
        }
    }

    /// 菜单标题行:进程异常时盖过健康摘要,避免「已停止」误导。
    var statusHeadline: String {
        switch sidecarState {
        case .starting: "启动中 · 正在拉起引擎"
        case .crashed: "引擎异常退出 · 点「启动运行」重试"
        case .unreachable: "引擎无响应 · 进程在但控制通道不通"
        case .stopped: "已停止"
        case .running: health.headline
        }
    }

    /// 运行页/统计页事件区是否自动跟随最新数据;关掉后页面冻结在当下,手动刷新才更新。
    @Published var autoRefreshEnabled: Bool = (UserDefaults.standard.object(forKey: autoRefreshEnabledDefaultsKey) as? Bool) ?? true {
        didSet {
            UserDefaults.standard.set(autoRefreshEnabled, forKey: autoRefreshEnabledDefaultsKey)
            startStatusPolling()
        }
    }

    /// 运行页自动刷新间隔(秒);默认 5 秒。读取时归一到可选集合,避免旧值让 Picker 空选。
    @Published var runtimeAutoRefreshIntervalSeconds: Double = {
        let stored = UserDefaults.standard.double(forKey: runtimeAutoRefreshIntervalDefaultsKey)
        return autoRefreshIntervalChoices.contains(stored) ? stored : 5
    }() {
        didSet {
            UserDefaults.standard.set(runtimeAutoRefreshIntervalSeconds, forKey: runtimeAutoRefreshIntervalDefaultsKey)
            startStatusPolling()
        }
    }

    /// 统计页排行刷新间隔(秒);默认 15 秒。运行状态和最近事件仍按运行页间隔更新。
    @Published var statisticsAutoRefreshIntervalSeconds: Double = {
        let stored = UserDefaults.standard.double(forKey: statisticsAutoRefreshIntervalDefaultsKey)
        return autoRefreshIntervalChoices.contains(stored) ? stored : 15
    }() {
        didSet {
            UserDefaults.standard.set(statisticsAutoRefreshIntervalSeconds, forKey: statisticsAutoRefreshIntervalDefaultsKey)
            startStatusPolling()
        }
    }

    let sidecar = SidecarController()
    var admin: AdminClient?
    var controlToken = ""
    var eventsTask: Task<Void, Never>?
    /// sumpterd 当前生效的监听配置;保存时对比,变了就重启进程(热 reload 不重绑端口)。
    var lastAppliedListener: ListenerConfig?
    var store: ConfigStore?
    var transientToken = 0
    var pollingTask: Task<Void, Never>?
    /// Monotonic generations implement latest-wins for overlapping UI requests.
    var refreshRequestGeneration = 0
    var runHistoryRequestGeneration = 0
    var analyticsRequestGeneration = 0
    var runtimeFacetsRequestGeneration = 0
    var runtimeV2RequestGeneration = 0
    var runtimeErrorPageRequestGeneration = 0
    var runtimeDimensionRequestGeneration = 0
    /// Child requests have their own latest-wins generations.  The snapshot
    /// generation invalidates the whole v2 view, while these counters prevent
    /// an error/dimension/request-chain response from writing after only that
    /// child was replaced.
    var runtimeRequestChainRequestGeneration = 0
    var runtimeMaintenanceRequestGeneration = 0
    var runtimeExportRequestGeneration = 0
    var runtimeExportEstimateRequestGeneration = 0
    var statisticsBoard = "overview"
    var runtimeRequestChainTask: Task<Void, Never>?
    var lastRuntimeAnalyticsRefreshAt: Date?
    var lastRuntimeV2RefreshAt: Date?
    var lastRuntimeFacetsRefreshAt: Date?
    var detailRequestGeneration = 0
    var diagnosticCaptureRequestGeneration = 0
    var diagnosticCaptureDetailRequestGeneration = 0
    var diagnosticCaptureExportRequestGeneration = 0
    var diagnosticCaptureTask: Task<Void, Never>?
    var diagnosticCaptureDetailTask: Task<Void, Never>?
    var diagnosticCaptureExportTask: Task<Void, Never>?
    /// Config writes can overlap while model catalogs finish on different
    /// network tasks.  Serialize the persistence/reload pipeline so a slower
    /// write cannot overwrite a newer endpoint/catalog edit.
    var configPersistenceTail: Task<Void, Never>?
    /// 每次 sidecar 重启都换一个连接代次，旧 admin 请求即使晚返回也不能回写新 UI。
    var adminConnectionGeneration = 0
    /// SSE 事件到达后的计数刷新防抖(事件本身即时上屏,计数合并成 1 秒 1 次)。
    var countersRefreshTask: Task<Void, Never>?
    /// 运行页第一页的稳定快照自动跟随已完成事件；短暂防抖可把一批
    /// 同时完成的请求合并成一次分页读取，避免流式事件逐条重载。
    var runHistoryAutoRefreshTask: Task<Void, Never>?
    /// 启停按钮的在途标记,防连点。
    @Published var toggleInFlight = false

    init() {
        Task { await bootstrap() }
        startStatusPolling()
    }

    /// 与设置窗口无关的兜底轮询。自动刷新关闭时不再周期性请求 admin；
    /// SSE 仍可接收即时事件，统计快照按自己的间隔更新。
    func startStatusPolling() {
        pollingTask?.cancel()
        pollingTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                let enabled = self.autoRefreshEnabled
                let interval = max(1, self.runtimeAutoRefreshIntervalSeconds)
                do {
                    try await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
                } catch {
                    return
                }
                guard !Task.isCancelled, enabled else { continue }
                // 事件/summary 走当前间隔；完整 legacy analytics 不再进入
                // 常规轮询。统计页的 facets 与 v3 快照按较慢的统计间隔
                // 刷新，避免每 1~2 秒重算整张 SQLite 看板。
                if self.statisticsVisible {
                    let statisticsInterval = max(1, self.statisticsAutoRefreshIntervalSeconds)
                    let facetsDue = self.lastRuntimeFacetsRefreshAt.map {
                        Date().timeIntervalSince($0) >= statisticsInterval
                    } ?? true
                    if facetsDue { self.reloadRuntimeFacets() }
                    let snapshotDue = self.lastRuntimeV2RefreshAt.map {
                        Date().timeIntervalSince($0) >= statisticsInterval
                    } ?? true
                    if snapshotDue, !self.runtimeV2Loading {
                        // Refresh the snapshot in place.  Keeping the last
                        // accepted rows visible prevents the statistics page
                        // from blanking/rebuilding every cadence tick.
                        self.refreshRuntimeV2(resetSnapshot: true, preserveVisibleContent: true)
                    }
                }
                await self.refreshStatus(reconcileEvents: true)
            }
        }
    }

}
enum AppModelError: Error, LocalizedError {
    case invalidInput(String)

    var errorDescription: String? {
        switch self {
        case .invalidInput(let message):
            return message
        }
    }
}

enum NativeNotificationError: Error, LocalizedError, Sendable {
    case authorizationDenied

    var errorDescription: String? {
        switch self {
        case .authorizationDenied:
            "系统通知权限未开启，请在系统设置中允许Sumpter发送通知。"
        }
    }
}

/// `UNUserNotificationCenter` completion handlers are delivered by the
/// notification service connection, not by the main actor. Keep the
/// continuation bridge outside `NativeNotifier`'s `@MainActor` isolation so
/// Swift 6 does not insert a main-actor precondition into those callbacks.
private enum NotificationAuthorizationBridge {
    static func authorizationStatus() async -> NotificationAuthorizationState {
        await withCheckedContinuation { continuation in
            UNUserNotificationCenter.current().getNotificationSettings { settings in
                continuation.resume(
                    returning: NotificationAuthorizationState(settings.authorizationStatus)
                )
            }
        }
    }

    static func requestAuthorization() async throws -> NotificationAuthorizationState {
        try await withCheckedThrowingContinuation { continuation in
            UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                UNUserNotificationCenter.current().getNotificationSettings { settings in
                    continuation.resume(
                        returning: NotificationAuthorizationState(settings.authorizationStatus)
                    )
                }
            }
        }
    }
}

@MainActor
final class NativeNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NativeNotifier()
    private var soundCache: [String: NSSound] = [:]

    private override init() {
        super.init()
    }

    func configure() {
        UNUserNotificationCenter.current().delegate = self
        preloadSounds()
    }

    /// 把通知相关的诊断信息追加到 proxy.log(app 目前唯一的文件日志出口)。
    /// nonisolated + static:投递失败可能发生在任意隔离域,随处可调。
    nonisolated static func appendLog(_ message: String) {
        guard let url = try? SumpterPaths.logURL() else { return }
        let stamp = ISO8601DateFormatter().string(from: Date())
        let line = "[\(stamp)] [notify] \(message)\n"
        guard let data = line.data(using: .utf8) else { return }
        if let handle = try? FileHandle(forWritingTo: url) {
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: data)
        } else {
            try? data.write(to: url, options: .atomic)
        }
    }

    func authorizationStatus() async -> NotificationAuthorizationState {
        await NotificationAuthorizationBridge.authorizationStatus()
    }

    func requestAuthorization() async throws -> NotificationAuthorizationState {
        try await NotificationAuthorizationBridge.requestAuthorization()
    }

    func deliver(
        title: String,
        message: String,
        subtitle: String = "",
        threadIdentifier: String = "sumpter",
        soundPreference: NotificationSoundPreference
    ) async throws -> NotificationAuthorizationState {
        var status = await authorizationStatus()
        if status == .notDetermined {
            status = try await requestAuthorization()
        }
        guard status.allowsDelivery else {
            throw NativeNotificationError.authorizationDenied
        }

        let content = UNMutableNotificationContent()
        content.title = title
        content.body = message
        if !subtitle.isEmpty {
            content.subtitle = subtitle
        }
        // 同会话(session_id)通知在通知中心堆叠成一组,不同会话互不淹没。
        content.threadIdentifier = threadIdentifier
        switch soundPreference {
        case .systemDefault:
            content.sound = .default
        case .silent:
            content.sound = nil
        default:
            // 自定义音效交给通知系统播放,与 banner 绑定;不再用 NSSound 旁路,
            // 避免 banner 投递失败时仍有声音、反而掩盖问题。文件名来自系统音效目录。
            if let name = soundPreference.systemSoundName {
                content.sound = UNNotificationSound(named: UNNotificationSoundName(name))
            } else {
                content.sound = .default
            }
        }

        let request = UNNotificationRequest(
            identifier: "sumpter-\(UUID().uuidString)",
            content: content,
            trigger: nil
        )
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            UNUserNotificationCenter.current().add(request) { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            }
        }
        return status
    }

    func playSound(_ preference: NotificationSoundPreference) {
        switch preference {
        case .silent:
            return
        case .systemDefault:
            NSSound.beep()
        default:
            guard let name = preference.systemSoundName,
                  let sound = cachedSound(named: name) else {
                NSSound.beep()
                return
            }
            if sound.isPlaying {
                sound.stop()
            }
            sound.currentTime = 0
            sound.play()
        }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        var options: UNNotificationPresentationOptions = [.banner, .list]
        if notification.request.content.sound != nil {
            options.insert(.sound)
        }
        completionHandler(options)
    }

    private func preloadSounds() {
        for preference in NotificationSoundPreference.allCases {
            guard let name = preference.systemSoundName else {
                continue
            }
            _ = cachedSound(named: name)
        }
    }

    private func cachedSound(named fileName: String) -> NSSound? {
        if let cached = soundCache[fileName] {
            return cached
        }
        let sound = SystemSoundCatalog.option(named: fileName)
            .flatMap { NSSound(contentsOf: $0.url, byReference: true) }
        soundCache[fileName] = sound
        return sound
    }
}

enum ClaudeNotificationHooks {
    private static let events: [(name: String, argument: String)] = [
        ("Notification", "notification"),
        ("Stop", "stop"),
        ("SubagentStop", "subagent_stop"),
        ("StopFailure", "stop_failure")
    ]

    static func isEnabled() -> Bool {
        guard let settings = try? loadSettings(),
              let hooks = settings["hooks"] as? [String: Any] else {
            return false
        }
        return events.allSatisfy { event in
            containsSumpterCommand(hooks[event.name])
        }
    }

    static func enabledArguments() -> Set<String> {
        guard let settings = try? loadSettings(),
              let hooks = settings["hooks"] as? [String: Any] else {
            return []
        }
        return Set(events.compactMap { event in
            containsSumpterCommand(hooks[event.name]) ? event.argument : nil
        })
    }

    static func backupExists() -> Bool {
        FileManager.default.fileExists(atPath: backupURL().path)
    }

    static func backupSettings() throws {
        try FileManager.default.createDirectory(at: claudeDirectory(), withIntermediateDirectories: true)
        let source = settingsURL()
        let destination = backupURL()
        if FileManager.default.fileExists(atPath: destination.path) {
            try FileManager.default.removeItem(at: destination)
        }
        if FileManager.default.fileExists(atPath: source.path) {
            try FileManager.default.copyItem(at: source, to: destination)
        } else {
            try Data("{}".utf8).write(to: destination, options: .atomic)
        }
    }

    static func restoreBackup() throws {
        guard backupExists() else {
            throw AppModelError.invalidInput("尚未创建 Claude Code 配置备份")
        }
        try FileManager.default.createDirectory(at: claudeDirectory(), withIntermediateDirectories: true)
        let destination = settingsURL()
        if FileManager.default.fileExists(atPath: destination.path) {
            try? FileManager.default.copyItem(at: destination, to: destination.appendingPathExtension("restore-bak"))
            try FileManager.default.removeItem(at: destination)
        }
        try FileManager.default.copyItem(at: backupURL(), to: destination)
    }

    static func setEnabled(_ enabled: Bool, port: Int) throws {
        try setSelectedArguments(enabled ? Set(events.map(\.argument)) : [], port: port)
    }

    /// 监听端口变更后刷新 hook 脚本(脚本内嵌端口):hooks 已启用才重写。
    /// 旧版只在切开关时写脚本 —— 改端口后 Claude Code 的通知会静默失联,
    /// 直到用户碰一次通知开关才恢复。
    static func rewriteScriptIfEnabled(port: Int) throws {
        guard isEnabled() || !enabledArguments().isEmpty else { return }
        try writeHookScript(port: port)
    }

    static func setSelectedArguments(_ arguments: Set<String>, port: Int) throws {
        try FileManager.default.createDirectory(at: claudeDirectory(), withIntermediateDirectories: true)
        var settings = (try? loadSettings()) ?? [:]
        var hooks = settings["hooks"] as? [String: Any] ?? [:]

        if !arguments.isEmpty {
            try writeHookScript(port: port)
            let needle = hookScriptURL().lastPathComponent
            for event in events {
                guard arguments.contains(event.argument) else {
                    let cleaned = ClaudeHookEditing.removingCommands(matching: needle, from: hooks[event.name])
                    if cleaned.isEmpty {
                        hooks.removeValue(forKey: event.name)
                    } else {
                        hooks[event.name] = cleaned
                    }
                    continue
                }
                // 合并而非覆盖:先删旧Sumpter命令再追加,保留用户在该事件上已有的其它 hooks。
                hooks[event.name] = ClaudeHookEditing.upsertCommand(
                    "/bin/zsh \(hookScriptURL().path) \(event.argument)",
                    matching: needle,
                    into: hooks[event.name]
                )
            }
        } else {
            let needle = hookScriptURL().lastPathComponent
            for event in events {
                let cleaned = ClaudeHookEditing.removingCommands(matching: needle, from: hooks[event.name])
                if cleaned.isEmpty {
                    hooks.removeValue(forKey: event.name)
                } else {
                    hooks[event.name] = cleaned
                }
            }
        }

        if hooks.isEmpty {
            settings.removeValue(forKey: "hooks")
        } else {
            settings["hooks"] = hooks
        }
        try saveSettings(settings)
    }

    private static func writeHookScript(port: Int) throws {
        let token = try ControlTokenStore.ensureToken(at: try SumpterPaths.controlTokenURL())
        let script = ClaudeNotifyScript.content(port: port, token: token)
        try script.write(to: hookScriptURL(), atomically: true, encoding: .utf8)
        // 脚本内嵌了控制 token,收紧到仅属主可读写执行,避免本机其它用户读走 token。
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: hookScriptURL().path)
    }

    private static func containsSumpterCommand(_ value: Any?) -> Bool {
        ClaudeHookEditing.containsCommand(matching: hookScriptURL().lastPathComponent, in: value)
    }

    private static func loadSettings() throws -> [String: Any] {
        let url = settingsURL()
        guard FileManager.default.fileExists(atPath: url.path) else {
            return [:]
        }
        let data = try Data(contentsOf: url)
        return (try JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    private static func saveSettings(_ settings: [String: Any]) throws {
        let url = settingsURL()
        if FileManager.default.fileExists(atPath: url.path) {
            try? FileManager.default.copyItem(at: url, to: url.appendingPathExtension("bak"))
        }
        let data = try JSONSerialization.data(withJSONObject: settings, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: url, options: .atomic)
    }

    private static func claudeDirectory() -> URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".claude", isDirectory: true)
    }

    private static func settingsURL() -> URL {
        claudeDirectory().appendingPathComponent("settings.json")
    }

    private static func hookScriptURL() -> URL {
        claudeDirectory().appendingPathComponent("sumpter-notify.sh")
    }

    private static func backupURL() -> URL {
        claudeDirectory().appendingPathComponent("settings.json.sumpter-backup")
    }
}

// 入口必须放在文件末尾:main.swift 顶层代码按声明顺序执行,若在此之前调用
// main() 会进入 run loop 永不返回,导致后面声明的全局(如 autoRefreshIntervalChoices)
// 未初始化,启动时读到非法内存崩溃。
SumpterNativeApp.main()
