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
    private static let runtimeAPIVersion = 1
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
    @Published private(set) var runtimeAnalyticsLoading = false
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
    @Published private(set) var runHistoryLoading = false
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
    @Published private(set) var runtimeHistoryLoading = false
    @Published var runtimeHistoryError: String?
    @Published var runtimeHistorySnapshotSeq: Int?
    @Published var runtimeHistoryGeneration: Int?
    @Published var runtimeRequestChain: AdminWire.RuntimeRequestChain?
    @Published private(set) var runtimeRequestChainLoading = false
    @Published var runtimeRequestChainError: String?
    @Published var runtimeTrendSeries: AdminWire.RuntimeTrendSeries?
    @Published var runtimeErrorPage: AdminWire.RuntimeErrorPage?
    @Published var runtimeErrorPageSize: Int = storedRuntimePageSize(forKey: runtimeErrorPageSizeDefaultsKey) {
        didSet { UserDefaults.standard.set(runtimeErrorPageSize, forKey: runtimeErrorPageSizeDefaultsKey) }
    }
    @Published private(set) var runtimeErrorPageLoading = false
    /// 旧的单维度页状态保留给兼容视图；当前概览和成本看板使用三张
    /// 独立分页表，同时保留各自搜索、排序和页大小。
    @Published var runtimeDimensionPage: AdminWire.RuntimeDimensionPage?
    @Published private(set) var runtimeDimensionPageLoading = false
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
    @Published private(set) var runtimeDimensionsLoading = false
    @Published var runtimeV2Error: String?
    @Published private(set) var runtimeV2Loading = false
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
    @Published private(set) var runtimeExportBusy = false
    /// 统计页可见性与当前看板由 UsagePane 明确告知模型。后台轮询只在
    /// 统计页可见时读取轻量 facets 与当前 v3 看板，避免运行页/设置页
    /// 承担高基数聚合。
    @Published private(set) var statisticsVisible = false
    @Published var health = ProxyHealthSummary(state: .stopped, headline: "已停止")
    @Published var transientMessage: String?
    @Published var claudeNotificationsEnabled = false
    @Published var claudeNotificationArguments: Set<String> = []
    @Published var codexNotificationsEnabled = false
    @Published var codexNotificationArguments: Set<String> = []
    @Published private(set) var codexNotificationHookStatus: CodexNotificationHookStatus = .notConfigured
    @Published private(set) var codexNotificationHookPath = "~/.codex/hooks.json"
    @Published var grokNotificationsEnabled = false
    @Published private(set) var grokNotificationHookPath = "~/.grok/hooks/sumpter-notify.json"
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
    @Published private(set) var diagnosticCaptureBusy = false
    @Published private(set) var diagnosticCaptureDetailBusy = false
    @Published private(set) var diagnosticCaptureExportBusy = false
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

    private let sidecar = SidecarController()
    private var admin: AdminClient?
    private var controlToken = ""
    private var eventsTask: Task<Void, Never>?
    /// sumpterd 当前生效的监听配置;保存时对比,变了就重启进程(热 reload 不重绑端口)。
    private var lastAppliedListener: ListenerConfig?
    private var store: ConfigStore?
    private var transientToken = 0
    private var pollingTask: Task<Void, Never>?
    /// Monotonic generations implement latest-wins for overlapping UI requests.
    private var refreshRequestGeneration = 0
    private var runHistoryRequestGeneration = 0
    private var analyticsRequestGeneration = 0
    private var runtimeFacetsRequestGeneration = 0
    private var runtimeV2RequestGeneration = 0
    private var runtimeErrorPageRequestGeneration = 0
    private var runtimeDimensionRequestGeneration = 0
    /// Child requests have their own latest-wins generations.  The snapshot
    /// generation invalidates the whole v2 view, while these counters prevent
    /// an error/dimension/request-chain response from writing after only that
    /// child was replaced.
    private var runtimeRequestChainRequestGeneration = 0
    private var runtimeMaintenanceRequestGeneration = 0
    private var runtimeExportRequestGeneration = 0
    private var runtimeExportEstimateRequestGeneration = 0
    private var statisticsBoard = "overview"
    private var runtimeRequestChainTask: Task<Void, Never>?
    private var lastRuntimeAnalyticsRefreshAt: Date?
    private var lastRuntimeV2RefreshAt: Date?
    private var lastRuntimeFacetsRefreshAt: Date?
    private var detailRequestGeneration = 0
    private var diagnosticCaptureRequestGeneration = 0
    private var diagnosticCaptureDetailRequestGeneration = 0
    private var diagnosticCaptureExportRequestGeneration = 0
    private var diagnosticCaptureTask: Task<Void, Never>?
    private var diagnosticCaptureDetailTask: Task<Void, Never>?
    private var diagnosticCaptureExportTask: Task<Void, Never>?
    /// Config writes can overlap while model catalogs finish on different
    /// network tasks.  Serialize the persistence/reload pipeline so a slower
    /// write cannot overwrite a newer endpoint/catalog edit.
    private var configPersistenceTail: Task<Void, Never>?
    /// 每次 sidecar 重启都换一个连接代次，旧 admin 请求即使晚返回也不能回写新 UI。
    private var adminConnectionGeneration = 0
    /// SSE 事件到达后的计数刷新防抖(事件本身即时上屏,计数合并成 1 秒 1 次)。
    private var countersRefreshTask: Task<Void, Never>?
    /// 运行页第一页的稳定快照自动跟随已完成事件；短暂防抖可把一批
    /// 同时完成的请求合并成一次分页读取，避免流式事件逐条重载。
    private var runHistoryAutoRefreshTask: Task<Void, Never>?
    /// 启停按钮的在途标记,防连点。
    @Published private(set) var toggleInFlight = false

    init() {
        Task { await bootstrap() }
        startStatusPolling()
    }

    /// 与设置窗口无关的兜底轮询。自动刷新关闭时不再周期性请求 admin；
    /// SSE 仍可接收即时事件，统计快照按自己的间隔更新。
    private func startStatusPolling() {
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

    /// 展示一条瞬时操作结果(成功/失败),几秒后自动消失,不占用持久健康状态。
    func flash(_ message: String) {
        transientMessage = message
        transientToken += 1
        let token = transientToken
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: 4_000_000_000)
            guard let self, self.transientToken == token else { return }
            self.transientMessage = nil
        }
    }

    func toggleProxy() {
        guard !toggleInFlight else { return }   // 防连点:spawn+握手比 actor 启动慢得多
        toggleInFlight = true
        Task {
            defer { toggleInFlight = false }
            if sidecarState.isActive {
                await stopSidecar()
                try? removeAutostartMarker()
            } else {
                await startSidecar()
                if sidecarState == .running {
                    try? writeAutostartMarker()
                }
            }
            await refreshStatus()
        }
    }

    /// spawn sumpterd 并等握手;成功后建立 admin 通道与 SSE 订阅。
    private func startSidecar() async {
        guard !sidecar.isRunning else { return }
        sidecarState = .starting
        statusText = "启动中"
        // A restarted daemon has its own authoritative runtime snapshot.  Do
        // not keep rendering the previous connection's in-flight overlay while
        // the new admin channel is being established: if the old process died
        // mid-stream, that row can otherwise keep counting forever even though
        // the new daemon has already normalized it on startup.
        runtimePage = nil
        runtimeChangeSeq = 0
        runHistoryRequestGeneration &+= 1
        runHistoryPage = nil
        runHistoryLoading = false
        runHistoryError = nil
        runtimeEventDetail = nil
        runtime.recentEvents.removeAll(where: \.isInFlight)
        do {
            let dir = try SumpterPaths.appSupportDirectory()
            let handshake = try await sidecar.start(configDir: dir)
            // token 由 sumpterd 生成/复用,读同一文件。
            controlToken = try ControlTokenStore.ensureToken(at: SumpterPaths.controlTokenURL())
            let newAdmin = AdminClient(port: handshake.adminPort, token: controlToken)
            // 握手成功不等于旧 URLSession 请求已经切换完成。先用新端口做一次
            // 轻量探针，确认 admin 真正可达后再暴露给所有诊断/统计请求。
            _ = try await newAdmin.status()
            adminConnectionGeneration &+= 1
            admin = newAdmin
            engineGeneration = handshake.generation
            sidecarState = .running
            isProxyRunning = true
            statusText = "运行中"
            lastAppliedListener = config.listener
            subscribeAdminEvents(connectionGeneration: adminConnectionGeneration)
            // 新 admin 端口确认可达后立即刷新轻量捕获索引，避免监听重启后诊断页
            // 还停留在旧连接的错误或空状态，正文仍按需加载。
            refreshDiagnosticCapture()
        } catch {
            // 握手后的 admin 探针失败时 sidecar 可能已经启动；必须回收它，
            // 否则下一次启动会被 isRunning 拦截而继续复用失效端口。
            await stopSidecar()
            sidecarState = .stopped
            lastError = "\(error)"
            statusText = "启动失败"
            flash("引擎启动失败")
        }
    }

    private func stopSidecar() async {
        // 先失效连接和所有 latest-wins 代次，再等待进程退出。MainActor 在 await
        // 期间可重入；如果晚清空 admin，用户此时点击诊断会继续打到旧随机端口。
        adminConnectionGeneration &+= 1
        refreshRequestGeneration &+= 1
        analyticsRequestGeneration &+= 1
        detailRequestGeneration &+= 1
        lastRuntimeAnalyticsRefreshAt = nil
        admin = nil
        eventsTask?.cancel()
        eventsTask = nil
        countersRefreshTask?.cancel()
        countersRefreshTask = nil
        runHistoryAutoRefreshTask?.cancel()
        runHistoryAutoRefreshTask = nil
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureTask = nil
        diagnosticCaptureDetailTask?.cancel()
        diagnosticCaptureDetailTask = nil
        diagnosticCaptureExportTask?.cancel()
        diagnosticCaptureExportRequestGeneration &+= 1
        diagnosticCaptureExportTask = nil
        runtimeRequestChainTask?.cancel()
        runtimeRequestChainTask = nil
        diagnosticCaptureRequestGeneration &+= 1
        diagnosticCaptureDetailRequestGeneration &+= 1
        diagnosticCapture = nil
        diagnosticCaptureDetail = nil
        diagnosticCaptureError = nil
        diagnosticCaptureDetailError = nil
        diagnosticCaptureBusy = false
        diagnosticCaptureDetailBusy = false
        diagnosticCaptureExportBusy = false
        runtimeV2RequestGeneration &+= 1
        runHistoryRequestGeneration &+= 1
        runtimeErrorPageRequestGeneration &+= 1
        runtimeDimensionRequestGeneration &+= 1
        runtimeRequestChainRequestGeneration &+= 1
        runtimeMaintenanceRequestGeneration &+= 1
        runtimeExportRequestGeneration &+= 1
        runtimeExportEstimateRequestGeneration &+= 1
        runtimeV2Loading = false
        runHistoryLoading = false
        runHistoryPage = nil
        runHistoryError = nil
        runtimeHistoryLoading = false
        runtimeRequestChainLoading = false
        runtimeErrorPageLoading = false
        runtimeDimensionsLoading = false
        runtimeExportBusy = false
        clearRuntimeV2Snapshot()
        runtimeHistoryError = nil
        runtimeV2Error = nil
        runtimeExportEstimateError = nil
        isProxyRunning = false
        engineGeneration = nil
        await sidecar.stop()
        sidecarState = .stopped
        statusText = "已停止"
    }

    /// 订阅 sumpterd 的 SSE:运行事件增量上屏、通知投递、外部配置变更对账。
    private func subscribeAdminEvents(connectionGeneration: Int) {
        eventsTask?.cancel()
        guard let admin else { return }
        eventsTask = Task { [weak self] in
            var retryNanoseconds: UInt64 = 1_000_000_000
            while !Task.isCancelled {
                do {
                    guard let model = self else { return }
                    guard model.adminConnectionGeneration == connectionGeneration,
                          model.admin != nil else { return }
                    await model.reconcileRuntimeChanges(using: admin)
                    for try await event in admin.events() {
                        guard let model = self else { return }
                        guard model.adminConnectionGeneration == connectionGeneration,
                              !Task.isCancelled else { return }
                        await model.handleAdminEvent(event)
                        retryNanoseconds = 1_000_000_000
                    }
                } catch {
                    if Task.isCancelled { return }
                }
                try? await Task.sleep(nanoseconds: retryNanoseconds)
                retryNanoseconds = min(retryNanoseconds * 2, 15_000_000_000)
            }
        }
    }

    private func reconcileRuntimeChanges(using admin: AdminClient) async {
        guard autoRefreshEnabled else { return }
        guard runtimeChangeSeq > 0 else {
            // A zero cursor is explicitly non-authoritative (old daemon,
            // reset, or a page without change metadata). Re-read the latest
            // page instead of pretending SSE is a complete source of truth.
            if runtimePage != nil {
                await refreshStatus(loadLatestEvents: true)
            }
            return
        }
        do {
            var cursor = runtimeChangeSeq
            while true {
                let page = try await admin.runtimeEvents(afterChangeSeq: cursor, limit: 200)
                guard page.cursorValid != false,
                      page.resetGeneration == nil || page.resetGeneration == runtimeSummary?.resetGeneration else {
                    await refreshStatus(loadLatestEvents: true)
                    return
                }
                for item in page.events {
                    applyRuntimeListItem(item, resetGeneration: page.resetGeneration)
                }
                let nextCursor = page.events.map(\.changeSeq).max() ?? cursor
                guard page.hasMore else { return }
                guard !page.events.isEmpty, nextCursor > cursor else {
                    await refreshStatus(loadLatestEvents: true)
                    return
                }
                cursor = nextCursor
            }
        } catch {
            await refreshStatus(loadLatestEvents: true)
        }
    }

    /// 事件即时并入本地快照(与引擎同口径:同 id 原地更新、按类各留 200 条),
    /// 计数则防抖合并拉取——一次长流会推很多 delta 事件,不该每条都打一次 admin。
    private func applyRuntimeEvent(_ event: RuntimeEvent) {
        var events = runtime.recentEvents
        if let index = events.firstIndex(where: { $0.id == event.id }) {
            events[index] = event
        } else {
            events.insert(event, at: 0)
            events = RuntimeEvent.trimmed(events, perKindLimit: 200)
        }
        runtime.recentEvents = events
        let evaluatedHealth = ProxyHealthEvaluator.evaluate(
            events: events,
            isRunning: sidecarState == .running
        )
        if evaluatedHealth != health {
            health = evaluatedHealth
        }
        scheduleCountersRefresh()
    }

    private func applyRuntimeListItem(
        _ item: AdminWire.RuntimeEventListItem,
        resetGeneration: Int? = nil
    ) {
        runtimeChangeSeq = max(runtimeChangeSeq, item.changeSeq)
        // Statistics are served from a stable SQLite snapshot and do not need
        // the run page's per-event overlay. Avoid publishing the high-rate
        // runtime list while this pane is visible; the next page transition
        // (or window show) performs one authoritative refresh for the run UI.
        guard !statisticsVisible else { return }
        var items = runtimePage?.events ?? []
        if let index = items.firstIndex(where: { $0.id == item.id }) {
            // SSE reconnects and a paged response can deliver an older change.
            // Ignore it everywhere, not only in the compact page, otherwise it
            // could still roll back the full in-memory RuntimeEvent below.
            guard item.changeSeq >= items[index].changeSeq else { return }
            items[index] = item
        } else {
            items.append(item)
        }
        items.sort { $0.seq > $1.seq }
        let retainedLimit = max(runtimePage?.events.count ?? 50, 50)
        if items.count > retainedLimit {
            items = Array(items.prefix(retainedLimit))
        }
        runtimePage = AdminWire.RuntimeEventPage(
            events: items,
            hasMore: runtimePage?.hasMore ?? false,
            resetGeneration: resetGeneration ?? runtimePage?.resetGeneration,
            cursorValid: true
        )
        // The list endpoint intentionally returns a compact projection. Merge it
        // into the existing full event so an SSE/list refresh cannot erase usage,
        // tool calls, streamTrace or Codex metadata already loaded for this ID.
        let existing = runtime.recentEvents.first(where: { $0.id == item.id })
        applyRuntimeEvent(item.mergedRuntimeEvent(with: existing))
        // 进行中的 delta 只更新实时 overlay；进入终态后自动重建第一页。
        // 旧 daemon 若没有 phase 也必须走这条兼容路径。
        if item.phase != .inFlight {
            scheduleRunHistoryAutoRefresh()
        }
    }

    /// 自动把运行页第一页推进到最新稳定快照。用户正在查看更早页时不
    /// 强行跳页；回到第一页的分页动作会主动建立新快照。
    func scheduleRunHistoryAutoRefresh() {
        guard autoRefreshEnabled, runHistoryPage?.page == 1,
              runHistoryAutoRefreshTask == nil else { return }
        runHistoryAutoRefreshTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 220_000_000)
            guard let self else { return }
            self.runHistoryAutoRefreshTask = nil
            guard !Task.isCancelled, self.autoRefreshEnabled,
                  self.runHistoryPage?.page == 1 else { return }
            self.refreshRunHistory(resetSnapshot: true)
        }
    }

    private func scheduleCountersRefresh() {
        guard countersRefreshTask == nil else { return }
        countersRefreshTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            guard let self else { return }
            self.countersRefreshTask = nil
            await self.refreshStatus()
        }
    }

    private func handleAdminEvent(_ event: AdminWire.Event) async {
        switch event {
        case .notify(let clientKind, let title, let message, _, let kind, let category, _, _, let sessionID, let cwd):
            guard notificationsEnabled else { return }
            if clientKind == "codex" {
                guard codexNotificationsEnabled else { return }
            } else if clientKind == "grok_build" {
                guard grokNotificationsEnabled else { return }
            } else {
                guard claudeNotificationsEnabled else { return }
            }
            if clientKind == "codex" {
                // A real SSE delivery is the only reliable evidence that the
                // Codex `/hooks` trust step has completed.
                CodexNotificationHooks.markVerified()
                codexNotificationHookStatus = .verified
            }
            guard shouldDeliverNotification(category: category) else { return }
            if clientKind == "codex" {
                await deliverNotification(
                    title: title,
                    message: message,
                    kind: kind,
                    category: category,
                    sessionID: sessionID,
                    // Codex notifications stay fully fixed and do not expose
                    // even the workspace basename as a subtitle.
                    cwd: nil,
                    clientKind: "codex"
                )
            } else if clientKind == "grok_build" {
                await deliverNotification(
                    title: title,
                    message: message,
                    kind: kind,
                    category: category,
                    sessionID: sessionID,
                    cwd: cwd,
                    clientKind: "grok_build"
                )
            } else {
                await deliverNotification(
                    title: title,
                    message: message,
                    kind: kind,
                    category: category,
                    sessionID: sessionID,
                    cwd: cwd,
                    clientKind: "claude_code"
                )
            }
        case .configReloaded(let generation):
            // 外部 /__reload 或 CLI 改动:sumpterd 已自主读盘,UI 重读磁盘对账内存副本。
            engineGeneration = generation
            let previousListener = config.listener
            var listenerChanged = false
            if let store, let result = try? store.loadWithMigration() {
                presentMigrationNoticeIfNeeded(result.migrationNotice)
                let normalized = result.config.normalizedBuiltInFeatureRules()
                if normalized != config {
                    listenerChanged = normalized.listener != previousListener
                    config = normalized
                    flash("配置已在外部变更,已重新载入")
                }
            }
            if listenerChanged {
                // 外部改 host/port 时 daemon 的热 reload 不会重绑 proxy listener，
                // 必须走与安全页相同的完整重启和新 admin 握手。
                do {
                    try await pushConfigToSidecar()
                } catch {
                    lastError = "监听配置重启失败：\(error)"
                    flash("监听配置重启失败")
                }
            } else {
                await refreshStatus()
            }
        case .migrationNotice(let notice):
            presentMigrationNoticeIfNeeded(notice)
        case .statsReset:
            guard autoRefreshEnabled else { return }
            runtimeChangeSeq = 0
            runtimeEventDetail = nil
            runtimePage = nil
            runHistoryRequestGeneration &+= 1
            runHistoryPage = nil
            runHistoryError = nil
            runtime.recentEvents = []
            await refreshStatus(loadLatestEvents: true)
        case .runtimeChange(let change):
            guard autoRefreshEnabled else { return }
            guard change.seq > 0, change.changeSeq > 0 else {
                await refreshStatus(loadLatestEvents: true)
                return
            }
            applyRuntimeListItem(AdminWire.RuntimeEventListItem(change: change))
            if !statisticsVisible, runtimeEventDetail?.event.id == change.event.id {
                loadRuntimeEvent(id: change.event.id)
            }
        }
    }

    /// 用户侧只有一个通知总开关；Claude/Codex/Grok 的配置文件仍由各自安全编辑器
    /// 维护，但一次操作会同步安装或移除三边的通知 Hook。
    var notificationsEnabled: Bool {
        !claudeNotificationArguments.isEmpty
            || !codexNotificationArguments.isEmpty
            || grokNotificationsEnabled
    }

    func refresh() {
        Task { await refreshStatus(loadLatestEvents: true) }
    }

    /// UsagePane 在进入/离开导航详情时调用。统计聚合不是 liveness 数据，
    /// 因此不让后台 status 轮询在其它页面预取它。
    func setStatisticsVisible(_ visible: Bool) {
        guard statisticsVisible != visible else { return }
        statisticsVisible = visible
        // UsagePane immediately calls refreshStatisticsIfNeeded after marking
        // the page visible. Keep this setter about lifecycle/generation only;
        // loading facets here would duplicate the first snapshot request and
        // make entering Statistics pay for two identical SQLite scans.
        if !visible {
            // 让离开页面后返回的旧响应失效，但不清空已显示快照，回到页面
            // 时可以先绘制旧数据再按需更新。
            analyticsRequestGeneration &+= 1
            runtimeAnalyticsLoading = false
            runtimeV2RequestGeneration &+= 1
            runtimeV2Loading = false
        }
    }

    /// 设置当前统计看板。首屏只加载基础页/趋势/存储/价格；错误与项目/会话
    /// 维度在对应看板首次进入时读取。
    func setStatisticsBoard(_ board: String) {
        let allowed = ["overview", "trends", "tokens", "errors"]
        guard allowed.contains(board) else { return }
        statisticsBoard = board
        guard statisticsVisible else { return }
        loadRuntimeV2Board(board, force: false)
    }

    /// 统计页的显式刷新入口。只刷新轻量状态与当前 v3 稳定快照；旧完整
    /// analytics 不再进入首屏或自动刷新 critical path。
    func refreshStatisticsNow() {
        guard statisticsVisible else { return }
        Task { await refreshStatus(loadLatestEvents: true) }
        reloadRuntimeFacets()
        refreshRuntimeV2(resetSnapshot: true)
    }

    /// 进入统计页或自动刷新设置变化时调用；短时间内复用刚取得的快照。
    func refreshStatisticsIfNeeded(force: Bool = false) {
        guard statisticsVisible else { return }
        reloadRuntimeFacets()
        if force || runtimeHistoryPage == nil {
            Task { await refreshStatus(loadLatestEvents: true) }
            refreshRuntimeV2(resetSnapshot: true)
        } else {
            loadRuntimeV2Board(statisticsBoard, force: false)
        }
    }

    func setRuntimeAnalyticsRange(_ range: String) {
        guard ["today", "7d", "30d", "all"].contains(range) else { return }
        runtimeAnalyticsRange = range
        clearRuntimeLocalProject()
        clearRuntimeLocalSession()
        reloadRuntimeFacets()
        refreshRuntimeV2(resetSnapshot: true)
    }

    func setRuntimeAnalyticsFilters(clientKind: String? = nil, endpointID: String? = nil, project: String? = nil, sessionID: String? = nil, model: String? = nil, requestPurpose: String? = nil, outcome: String? = nil, failureKind: String? = nil, failurePhase: String? = nil) {
        if let clientKind { runtimeAnalyticsClientKind = clientKind }
        if let endpointID { runtimeAnalyticsEndpointID = endpointID }
        if let project {
            runtimeAnalyticsProject = project
            runtimeV2ProjectName = project
        }
        if let sessionID {
            runtimeAnalyticsSessionID = sessionID
            runtimeV2SessionID = sessionID
        }
        if let model { runtimeAnalyticsModel = model }
        if let requestPurpose { runtimeAnalyticsRequestPurpose = requestPurpose }
        if let outcome { runtimeAnalyticsOutcome = outcome }
        if let failureKind { runtimeAnalyticsFailureKind = failureKind }
        if let failurePhase { runtimeAnalyticsFailurePhase = failurePhase }
        clearRuntimeLocalProject()
        clearRuntimeLocalSession()
        reloadRuntimeFacets()
        refreshRuntimeV2(resetSnapshot: true)
    }

    func setRuntimeLocalProject(_ projectID: String, projectName: String? = nil) {
        runtimeLocalProjectID = projectID
        runtimeLocalProjectName = projectName ?? runtimeProjectsPage?.rows.first(where: { $0.key == projectID })?.name ?? projectID
        runtimeLocalSessionID = ""
        runtimeLocalSessionName = ""
        loadRuntimeDimensionsForBoard()
    }

    func clearRuntimeLocalProject() {
        guard !runtimeLocalProjectID.isEmpty || !runtimeLocalProjectName.isEmpty else { return }
        resetRuntimeLocalProjectState()
        resetRuntimeLocalSessionState()
        if statisticsVisible, ["overview", "tokens"].contains(statisticsBoard) {
            loadRuntimeDimensionsForBoard()
        }
    }

    func setRuntimeLocalSession(_ sessionID: String, sessionName: String? = nil) {
        runtimeLocalSessionID = sessionID
        runtimeLocalSessionName = sessionName ?? runtimeSessionsPage?.rows.first(where: { $0.key == sessionID })?.name ?? sessionID
        loadRuntimeModels(page: 1)
    }

    func clearRuntimeLocalSession() {
        guard !runtimeLocalSessionID.isEmpty || !runtimeLocalSessionName.isEmpty else { return }
        resetRuntimeLocalSessionState()
        if statisticsVisible, ["overview", "tokens"].contains(statisticsBoard) {
            loadRuntimeModels(page: 1)
        }
    }

    private func resetRuntimeLocalProjectState() {
        runtimeLocalProjectID = ""
        runtimeLocalProjectName = ""
    }

    private func resetRuntimeLocalSessionState() {
        runtimeLocalSessionID = ""
        runtimeLocalSessionName = ""
    }

    private func resetRuntimeLocalDrillDownState() {
        resetRuntimeLocalProjectState()
        resetRuntimeLocalSessionState()
    }

    private func reloadRuntimeAnalytics() {
        // Compatibility callers may still use this method name, but the old
        // full-table analytics request is intentionally retired from the macOS
        // UI. Keep the call latest-wins and use the v3 projections instead.
        reloadRuntimeFacets()
        refreshRuntimeV2(resetSnapshot: true)
    }

    /// Refresh only the picker dimensions.  This request is deliberately
    /// independent from the compatibility aggregate so a slow high-cardinality
    /// table cannot block selectors or the automatic status loop.
    private func reloadRuntimeFacets() {
        guard statisticsVisible else { return }
        runtimeFacetsRequestGeneration &+= 1
        let generation = runtimeFacetsRequestGeneration
        let range = runtimeAnalyticsRange
        let filter = runtimeV2Filter()
        Task { [weak self] in
            guard let self, let admin = self.admin else { return }
            do {
                let value = try await admin.runtimeFacets(range: range, filter: filter)
                guard generation == self.runtimeFacetsRequestGeneration else { return }
                if self.runtimeFacets != value {
                    self.runtimeFacets = value
                }
                self.lastRuntimeFacetsRefreshAt = Date()
                self.clearInvalidAnalyticsFacetFilters(value)
            } catch {
                // Facets are an enhancement; preserve the last good selectors
                // and let the v3 board report its own query error.
            }
        }
    }

    private func clearInvalidAnalyticsFacetFilters(_ snapshot: AdminWire.RuntimeFacetSnapshot) {
        let facets = snapshot.facets
        var needsReload = false
        if !runtimeAnalyticsClientKind.isEmpty,
           let rows = facets.clientKinds,
           !rows.contains(where: { $0.value == runtimeAnalyticsClientKind }) {
            runtimeAnalyticsClientKind = ""
            needsReload = true
        }
        if !runtimeAnalyticsEndpointID.isEmpty,
           let rows = facets.endpoints,
           !rows.contains(where: { $0.value == runtimeAnalyticsEndpointID }) {
            runtimeAnalyticsEndpointID = ""
            needsReload = true
        }
        if !runtimeAnalyticsProject.isEmpty,
           let rows = facets.projects,
           !rows.contains(where: { $0.value == runtimeAnalyticsProject }) {
            runtimeAnalyticsProject = ""
            runtimeV2ProjectName = ""
            needsReload = true
        }
        if !runtimeAnalyticsSessionID.isEmpty,
           let rows = facets.sessions,
           !rows.contains(where: { $0.value == runtimeAnalyticsSessionID }) {
            runtimeAnalyticsSessionID = ""
            runtimeV2SessionID = ""
            needsReload = true
        }
        if !runtimeAnalyticsModel.isEmpty,
           let rows = facets.models,
           !rows.contains(where: { $0.value == runtimeAnalyticsModel }) {
            runtimeAnalyticsModel = ""
            needsReload = true
        }
        if !runtimeAnalyticsRequestPurpose.isEmpty,
           let rows = facets.requestPurposes,
           !rows.contains(where: { $0.value == runtimeAnalyticsRequestPurpose }) {
            runtimeAnalyticsRequestPurpose = ""
            needsReload = true
        }
        if !runtimeAnalyticsFailureKind.isEmpty,
           let rows = facets.failureKinds,
           !rows.contains(where: { $0.value == runtimeAnalyticsFailureKind }) {
            runtimeAnalyticsFailureKind = ""
            needsReload = true
        }
        if !runtimeAnalyticsFailurePhase.isEmpty,
           let rows = facets.failurePhases,
           !rows.contains(where: { $0.value == runtimeAnalyticsFailurePhase }) {
            runtimeAnalyticsFailurePhase = ""
            needsReload = true
        }
        if needsReload { reloadRuntimeFacets() }
    }

    /// A filter can outlive a session/project after a reset or retention cleanup.
    /// Clear only values the server can prove are absent; old daemons omitting
    /// facets remain untouched for compatibility.
    private func clearInvalidAnalyticsFilters(_ analytics: AdminWire.RuntimeAnalytics) {
        var needsReload = false
        if !runtimeAnalyticsEndpointID.isEmpty,
           let endpoints = analytics.facets?.endpoints,
           !endpoints.contains(where: { $0.value == runtimeAnalyticsEndpointID }) {
            runtimeAnalyticsEndpointID = ""
            needsReload = true
        }
        if !runtimeAnalyticsProject.isEmpty,
           let projects = analytics.facets?.projects,
           !projects.contains(where: { $0.value == runtimeAnalyticsProject }) {
            runtimeAnalyticsProject = ""
            needsReload = true
        }
        if !runtimeAnalyticsSessionID.isEmpty,
           let sessions = analytics.sessions,
           !sessions.contains(where: { $0.name == runtimeAnalyticsSessionID }) {
            runtimeAnalyticsSessionID = ""
            needsReload = true
        }
        if needsReload {
            reloadRuntimeAnalytics()
        }
    }

    // MARK: - Run history (独立于统计快照)

    private var runHistoryFilter: AdminWire.RuntimeFilter {
        return AdminWire.RuntimeFilter(
            kind: runHistoryKindFilter == "all" ? nil : runHistoryKindFilter
        )
    }

    /// 运行页只读取自己的稳定历史快照。统计页的筛选、页码和 loading
    /// 状态不会被这个请求改写；实时 SSE 事件由 OverviewPane 作为独立
    /// overlay 展示，不占持久历史页的名额。
    func refreshRunHistory(resetSnapshot: Bool = false) {
        runHistoryRequestGeneration &+= 1
        let generation = runHistoryRequestGeneration
        runHistoryLoading = true
        runHistoryError = nil
        let page = resetSnapshot ? 1 : (runHistoryPage?.page ?? 1)
        let anchor = resetSnapshot ? nil : runHistoryPage
        let pageSize = runHistoryPageSize
        let filter = runHistoryFilter
        Task { [weak self] in
            guard let self else { return }
            guard let admin = self.admin else {
                guard generation == self.runHistoryRequestGeneration else { return }
                self.runHistoryLoading = false
                self.runHistoryError = "请先启动代理"
                return
            }
            do {
                var value: AdminWire.RuntimeHistoryPage
                do {
                    value = try await admin.runtimeEventPage(
                        page: max(1, page), pageSize: pageSize,
                        snapshotSeq: anchor?.snapshotSeq,
                        historyGeneration: anchor?.historyGeneration,
                        filter: filter
                    )
                } catch {
                    guard generation == self.runHistoryRequestGeneration else { return }
                    guard self.isRuntimeSnapshotError(error), !resetSnapshot else { throw error }
                    // 事件保留/重置导致旧锚点失效时只恢复一次到第一页。
                    value = try await admin.runtimeEventPage(
                        page: 1, pageSize: pageSize, filter: filter
                    )
                }
                guard generation == self.runHistoryRequestGeneration else { return }
                self.runHistoryPage = value
                self.runHistoryError = nil
                self.runHistoryLoading = false
            } catch {
                guard generation == self.runHistoryRequestGeneration else { return }
                self.runHistoryError = "读取运行事件分页失败：\(error)"
                self.runHistoryLoading = false
            }
        }
    }

    func loadRunHistoryPage(_ page: Int) {
        guard page >= 1, runHistoryPage != nil else { return }
        if page == 1, runHistoryPage?.page != 1 {
            // Returning from an older page should show the current head, not
            // the stale page-1 slice of an earlier snapshot.
            refreshRunHistory(resetSnapshot: true)
            return
        }
        runHistoryRequestGeneration &+= 1
        let generation = runHistoryRequestGeneration
        runHistoryLoading = true
        let pageSize = runHistoryPageSize
        let filter = runHistoryFilter
        let anchor = runHistoryPage
        Task { [weak self] in
            guard let self else { return }
            guard let admin = self.admin else {
                guard generation == self.runHistoryRequestGeneration else { return }
                self.runHistoryLoading = false
                self.runHistoryError = "请先启动代理"
                return
            }
            do {
                let value: AdminWire.RuntimeHistoryPage
                do {
                    value = try await admin.runtimeEventPage(
                        page: page, pageSize: pageSize,
                        snapshotSeq: anchor?.snapshotSeq,
                        historyGeneration: anchor?.historyGeneration,
                        filter: filter
                    )
                } catch {
                    guard generation == self.runHistoryRequestGeneration,
                          self.isRuntimeSnapshotError(error) else { throw error }
                    self.runHistoryPage = nil
                    return self.refreshRunHistory(resetSnapshot: true)
                }
                guard generation == self.runHistoryRequestGeneration else { return }
                self.runHistoryPage = value
                self.runHistoryError = nil
                self.runHistoryLoading = false
            } catch {
                guard generation == self.runHistoryRequestGeneration else { return }
                if self.isRuntimeSnapshotError(error) {
                    self.runHistoryPage = nil
                    self.runHistoryError = "事件快照已变化，请重新加载最新页"
                } else {
                    self.runHistoryError = "读取运行事件分页失败：\(error)"
                }
                self.runHistoryLoading = false
            }
        }
    }

    func setRunHistoryPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runHistoryPageSize = pageSize
        runHistoryPage = nil
        refreshRunHistory(resetSnapshot: true)
    }

    func setRunHistoryKindFilter(_ kind: String) {
        guard ["all", "client", "upstream"].contains(kind) else { return }
        guard runHistoryKindFilter != kind else { return }
        runHistoryKindFilter = kind
        runHistoryPage = nil
        refreshRunHistory(resetSnapshot: true)
    }

    // MARK: - Runtime analytics v2

    private func runtimeV2Filter(includeLocalProject: Bool = false) -> AdminWire.RuntimeFilter {
        let localTodayStart = Calendar.autoupdatingCurrent
            .startOfDay(for: Date())
            .timeIntervalSinceReferenceDate
        let base = AdminWire.RuntimeFilter(
            outcome: runtimeAnalyticsOutcome.isEmpty ? nil : runtimeAnalyticsOutcome,
            clientKind: runtimeAnalyticsClientKind.isEmpty ? nil : runtimeAnalyticsClientKind,
            requestPurpose: runtimeAnalyticsRequestPurpose.isEmpty ? nil : runtimeAnalyticsRequestPurpose,
            endpointID: runtimeAnalyticsEndpointID.isEmpty ? nil : runtimeAnalyticsEndpointID,
            model: runtimeAnalyticsModel.isEmpty ? nil : runtimeAnalyticsModel,
            projectID: runtimeV2ProjectID.isEmpty ? nil : runtimeV2ProjectID,
            project: (runtimeV2ProjectName.isEmpty ? runtimeAnalyticsProject : runtimeV2ProjectName).isEmpty ? nil : (runtimeV2ProjectName.isEmpty ? runtimeAnalyticsProject : runtimeV2ProjectName),
            sessionID: (runtimeV2SessionID.isEmpty ? runtimeAnalyticsSessionID : runtimeV2SessionID).isEmpty ? nil : (runtimeV2SessionID.isEmpty ? runtimeAnalyticsSessionID : runtimeV2SessionID),
            failureKind: runtimeAnalyticsFailureKind.isEmpty ? nil : runtimeAnalyticsFailureKind,
            failurePhase: runtimeAnalyticsFailurePhase.isEmpty ? nil : runtimeAnalyticsFailurePhase,
            // “今天” means the local calendar day in the app's timezone. The
            // daemon receives the lower bound explicitly so its own server
            // timezone cannot turn this into a rolling 24-hour window.
            from: runtimeAnalyticsRange == "today" ? localTodayStart : nil
        )
        guard includeLocalProject,
              !runtimeLocalProjectID.isEmpty || !runtimeLocalProjectName.isEmpty || !runtimeLocalSessionID.isEmpty else {
            return base
        }
        return AdminWire.RuntimeFilter(
            outcome: base.outcome,
            clientKind: base.clientKind,
            requestPurpose: base.requestPurpose,
            endpointID: base.endpointID,
            model: base.model,
            // The synthetic unidentified row has no project_id column; use
            // its display-name predicate instead of asking SQLite for the
            // literal key "unidentified_project".
            projectID: runtimeLocalProjectID == "unidentified_project" ? nil : (runtimeLocalProjectID.isEmpty ? base.projectID : runtimeLocalProjectID),
            project: runtimeLocalProjectID == "unidentified_project" ? runtimeLocalProjectName : (runtimeLocalProjectName.isEmpty ? base.project : runtimeLocalProjectName),
            sessionID: runtimeLocalSessionID.isEmpty ? base.sessionID : runtimeLocalSessionID,
            failureKind: base.failureKind,
            failurePhase: base.failurePhase,
            from: base.from,
            to: base.to
        )
    }

    /// Starts a stable v2 snapshot load. The first page establishes the
    /// `(snapshotSeq, historyGeneration)` anchor; the independent aggregates
    /// then run concurrently against that same anchor.
    func refreshRuntimeV2(
        resetSnapshot: Bool = false,
        preserveVisibleContent: Bool = false
    ) {
        // A new snapshot refresh supersedes any in-flight page/aggregate/child
        // request.  Clear their busy flags here, because the superseded task
        // is deliberately not allowed to mutate state when it returns.
        if runtimeHistoryLoading { runtimeHistoryLoading = false }
        if runtimeErrorPageLoading { runtimeErrorPageLoading = false }
        if runtimeDimensionPageLoading { runtimeDimensionPageLoading = false }
        if runtimeDimensionsLoading { runtimeDimensionsLoading = false }
        if runtimeRequestChainLoading { runtimeRequestChainLoading = false }
        runtimeRequestChainTask?.cancel()
        runtimeRequestChainTask = nil
        runtimeErrorPageRequestGeneration &+= 1
        runtimeDimensionRequestGeneration &+= 1
        runtimeRequestChainRequestGeneration &+= 1
        runtimeV2RequestGeneration &+= 1
        let generation = runtimeV2RequestGeneration
        if resetSnapshot {
            runtimeExportEstimateRequestGeneration &+= 1
            if runtimeExportEstimate != nil { runtimeExportEstimate = nil }
            if runtimeExportEstimateError != nil { runtimeExportEstimateError = nil }
        }
        // Background polling must not toggle the loading state.  The tables
        // remain visible while their replacement snapshot is fetched; setting
        // this flag on every cadence invalidates the whole UsagePane and makes
        // SwiftUI rebuild all native tables, which presents as a page flash.
        if !preserveVisibleContent {
            runtimeV2Loading = true
            if runtimeV2Error != nil { runtimeV2Error = nil }
            if runtimeHistoryError != nil { runtimeHistoryError = nil }
        }
        if resetSnapshot {
            // Child pages belong to the old snapshot. Keep the previous rows
            // only until the base page is accepted, then reload the selected
            // board lazily against the new anchor.
            // Interactive filter/board changes can clear stale child pages.
            // Background polling keeps the previous table rendered until its
            // replacement has arrived, so the page never flashes empty.
            if !preserveVisibleContent {
                runtimeErrorPage = nil
                runtimeDimensionPage = nil
                runtimeProjectsPage = nil
                runtimeSessionsPage = nil
                runtimeEndpointsPage = nil
                runtimeModelsPage = nil
            }
        }
        Task { [weak self] in
            guard let self else { return }
            await self.refreshRuntimeV2Async(
                resetSnapshot: resetSnapshot,
                preserveVisibleContent: preserveVisibleContent,
                generation: generation
            )
        }
    }

    private func refreshRuntimeV2Async(
        resetSnapshot: Bool,
        preserveVisibleContent: Bool,
        generation: Int
    ) async {
        guard let admin else {
            if generation == runtimeV2RequestGeneration {
                runtimeV2Loading = false
                runtimeHistoryLoading = false
                runtimeV2Error = "请先启动代理"
            }
            return
        }
        do {
            let page = try await fetchRuntimeHistoryPage(
                using: admin,
                page: resetSnapshot ? 1 : (runtimeHistoryPage?.page ?? 1),
                resetSnapshot: resetSnapshot,
                retrySnapshot: true,
                requestGeneration: generation
            )
            guard generation == runtimeV2RequestGeneration else { return }
            let pageChanged = runtimeHistoryPage != page
            applyRuntimeHistoryPage(page)
            if runtimeHistoryError != nil { runtimeHistoryError = nil }
            if runtimeHistoryLoading { runtimeHistoryLoading = false }
            let snapshot = page.snapshotSeq
            let historyGeneration = page.historyGeneration
            let filter = runtimeV2Filter()
            let board = statisticsBoard

            // These reads use separate read-only SQLite connections in the
            // daemon. Only request data owned by the visible board: an error
            // or dimension board must not wait for trends, storage, retention
            // and pricing queries that it cannot render. Retention is loaded
            // by the Diagnostics/maintenance surface, not by Statistics.
            async let trendValue = runtimeTrendIfNeeded(
                for: board,
                using: admin,
                snapshotSeq: snapshot,
                historyGeneration: historyGeneration,
                filter: filter
            )
            async let storageValue = runtimeStorageIfNeeded(for: board, using: admin)
            async let pricingValue = runtimePricingIfNeeded(for: board, using: admin)
            async let diagnosticValue = runtimeDiagnosticAnalyticsIfNeeded(for: board, using: admin)
            let (trend, storage, pricing, diagnostic) = try await (trendValue, storageValue, pricingValue, diagnosticValue)
            guard generation == runtimeV2RequestGeneration else { return }
            if let trend, runtimeTrendSeries != trend { runtimeTrendSeries = trend }
            if let storage, runtimeStorageProbe != storage { runtimeStorageProbe = storage }
            if let pricing, runtimePricing != pricing { runtimePricing = pricing }
            if let diagnostic, runtimeAnalytics != diagnostic { runtimeAnalytics = diagnostic }
            lastRuntimeV2RefreshAt = Date()
            if runtimeV2Error != nil { runtimeV2Error = nil }
            if runtimeV2Loading { runtimeV2Loading = false }
            if runtimeHistoryLoading { runtimeHistoryLoading = false }
            // The first paint is now complete. High-cardinality/error data is
            // fetched only for the selected board and never delays the base
            // statistics snapshot.
            // A polling pass with an identical page does not need to reload
            // the selected child table.  Avoiding that request also avoids a
            // second loading-state publication and keeps scrolling stable.
            loadRuntimeV2Board(statisticsBoard, force: !preserveVisibleContent || pageChanged)
        } catch {
            guard generation == runtimeV2RequestGeneration else { return }
            if isRuntimeSnapshotError(error) {
                // A prune/reset can invalidate an anchor between the first
                // request and the aggregate queries. Clear it and retry once
                // at page one; never loop indefinitely on a rapidly changing
                // store.
                clearRuntimeV2Snapshot(expectedGeneration: generation)
                if !resetSnapshot {
                    await refreshRuntimeV2Async(
                        resetSnapshot: true,
                        preserveVisibleContent: preserveVisibleContent,
                        generation: generation
                    )
                    return
                }
            }
            runtimeV2Error = "\(error)"
            runtimeHistoryError = "\(error)"
            runtimeV2Loading = false
            runtimeHistoryLoading = false
        }
    }

    private func runtimeTrendIfNeeded(
        for board: String,
        using admin: AdminClient,
        snapshotSeq: Int,
        historyGeneration: Int,
        filter: AdminWire.RuntimeFilter
    ) async throws -> AdminWire.RuntimeTrendSeries? {
        guard ["overview", "trends", "tokens"].contains(board) else { return nil }
        return try await admin.runtimeTrends(
            range: runtimeAnalyticsRange,
            granularity: "auto",
            snapshotSeq: snapshotSeq,
            historyGeneration: historyGeneration,
            filter: filter
        )
    }

    private func runtimeStorageIfNeeded(
        for board: String,
        using admin: AdminClient
    ) async throws -> AdminWire.RuntimeStorageProbe? {
        guard board == "overview" else { return nil }
        return try await admin.runtimeStorage()
    }

    private func runtimePricingIfNeeded(
        for board: String,
        using admin: AdminClient
    ) async throws -> AdminWire.RuntimePricing? {
        guard board == "tokens" else { return nil }
        return try await admin.runtimePricing()
    }

    private func runtimeDiagnosticAnalyticsIfNeeded(
        for board: String,
        using admin: AdminClient
    ) async throws -> AdminWire.RuntimeAnalytics? {
        guard board == "errors" else { return nil }
        return try await admin.runtimeAnalytics(range: runtimeAnalyticsRange, filter: runtimeV2Filter())
    }

    private func fetchRuntimeHistoryPage(
        using admin: AdminClient,
        page: Int,
        resetSnapshot: Bool,
        retrySnapshot: Bool,
        requestGeneration: Int? = nil
    ) async throws -> AdminWire.RuntimeHistoryPage {
        let anchor = resetSnapshot ? nil : runtimeHistoryPage
        do {
            return try await admin.runtimeEventPage(
                page: max(1, page),
                pageSize: runtimeHistoryPageSize,
                snapshotSeq: anchor?.snapshotSeq,
                historyGeneration: anchor?.historyGeneration,
                filter: runtimeV2Filter()
            )
        } catch {
            if retrySnapshot, isRuntimeSnapshotError(error) {
                // An older request may finish after a newer snapshot has
                // already been established.  It must not clear that newer
                // snapshot or issue a second page-one request.
                if let requestGeneration,
                   requestGeneration != runtimeV2RequestGeneration {
                    throw error
                }
                clearRuntimeV2Snapshot(expectedGeneration: requestGeneration)
                if let requestGeneration,
                   requestGeneration != runtimeV2RequestGeneration {
                    throw error
                }
                return try await admin.runtimeEventPage(
                    page: 1,
                    pageSize: runtimeHistoryPageSize,
                    filter: runtimeV2Filter()
                )
            }
            throw error
        }
    }

    private func applyRuntimeHistoryPage(_ page: AdminWire.RuntimeHistoryPage) {
        if runtimeHistoryPage?.snapshotSeq != page.snapshotSeq
            || runtimeHistoryPage?.historyGeneration != page.historyGeneration {
            runtimeExportEstimate = nil
            runtimeExportEstimateError = nil
        }
        if runtimeHistoryPage != page { runtimeHistoryPage = page }
        if runtimeHistorySnapshotSeq != page.snapshotSeq {
            runtimeHistorySnapshotSeq = page.snapshotSeq
        }
        if runtimeHistoryGeneration != page.historyGeneration {
            runtimeHistoryGeneration = page.historyGeneration
        }
    }

    func loadRuntimeHistoryPage(_ page: Int) {
        guard page >= 1 else { return }
        // Paging supersedes an aggregate refresh.  The old refresh task will
        // observe the generation mismatch and leave all state to this request.
        runtimeV2Loading = false
        runtimeErrorPageLoading = false
        runtimeDimensionPageLoading = false
        runtimeDimensionsLoading = false
        runtimeRequestChainTask?.cancel()
        runtimeRequestChainTask = nil
        runtimeRequestChainRequestGeneration &+= 1
        runtimeRequestChainLoading = false
        runtimeRequestChain = nil
        runtimeErrorPageRequestGeneration &+= 1
        runtimeDimensionRequestGeneration &+= 1
        runtimeHistoryLoading = true
        runtimeV2RequestGeneration &+= 1
        let generation = runtimeV2RequestGeneration
        Task { [weak self] in
            guard let self, let admin = self.admin else {
                self?.runtimeHistoryLoading = false
                self?.runtimeV2Loading = false
                return
            }
            do {
                let value = try await self.fetchRuntimeHistoryPage(
                    using: admin,
                    page: page,
                    resetSnapshot: self.runtimeHistoryPage == nil,
                    retrySnapshot: true,
                    requestGeneration: generation
                )
                guard generation == self.runtimeV2RequestGeneration else { return }
                self.applyRuntimeHistoryPage(value)
                self.runtimeHistoryError = nil
                self.runtimeHistoryLoading = false
                self.runtimeV2Loading = false
            } catch {
                guard generation == self.runtimeV2RequestGeneration else { return }
                self.runtimeHistoryError = "\(error)"
                self.runtimeHistoryLoading = false
                self.runtimeV2Loading = false
                if self.isRuntimeSnapshotError(error) {
                    self.clearRuntimeV2Snapshot(expectedGeneration: generation)
                    self.flash("历史快照已变化，请重新加载")
                }
            }
        }
    }

    private func loadRuntimeV2Board(_ board: String, force: Bool) {
        guard statisticsVisible, runtimeHistoryPage != nil else { return }
        switch board {
        case "overview":
            if force || runtimeEndpointsPage == nil || runtimeProjectsPage == nil || runtimeSessionsPage == nil || runtimeModelsPage == nil {
                loadRuntimeDimensionsForBoard()
            }
        case "errors":
            if force || runtimeErrorPage == nil {
                loadRuntimeV2ErrorPage(page: 1)
            }
        case "tokens":
            if force || runtimeEndpointsPage == nil || runtimeProjectsPage == nil || runtimeSessionsPage == nil || runtimeModelsPage == nil {
                loadRuntimeDimensionsForBoard()
            }
        case "dimensions":
            if force || runtimeDimensionPage?.kind != Self.runtimeDimensionWireKind(runtimeDimensionKind) {
                loadRuntimeDimensionPage(kind: runtimeDimensionKind, page: 1)
            }
        default:
            break
        }
    }

    private static let runtimeDimensionKinds = [
        "endpoint", "model", "clientKind", "purpose", "failureKind",
        "failurePhase", "protocol", "streamTerminal", "project", "session",
    ]

    private static func runtimeDimensionWireKind(_ kind: String) -> String {
        switch kind {
        case "clientKind": "client_kind"
        case "failureKind": "failure_kind"
        case "failurePhase": "failure_phase"
        case "streamTerminal": "stream_terminal"
        default: kind
        }
    }

    func setRuntimeDimensionKind(_ kind: String) {
        guard Self.runtimeDimensionKinds.contains(kind) else { return }
        runtimeDimensionKind = kind
        runtimeDimensionSearch = ""
        runtimeDimensionSort = "last_seen"
        runtimeDimensionOrder = "desc"
        runtimeDimensionPage = nil
        guard statisticsVisible, statisticsBoard == "dimensions" else { return }
        loadRuntimeDimensionPage(kind: kind, page: 1)
    }

    func setRuntimeDimensionSearch(_ search: String) {
        runtimeDimensionSearch = String(search.prefix(256))
        guard statisticsVisible, ["overview", "dimensions", "tokens"].contains(statisticsBoard) else { return }
        let kind = statisticsBoard == "overview"
            ? "project"
            : statisticsBoard == "tokens" ? "endpoint" : runtimeDimensionKind
        loadRuntimeDimensionPage(kind: kind, page: 1)
    }

    func setRuntimeDimensionSort(_ sort: String, order: String? = nil) {
        let allowed = [
            "name", "requests", "success_rate", "failures", "input_tokens",
            "output_tokens", "cache_read", "cache_write", "tokens",
            "average_duration", "last_seen",
        ]
        guard allowed.contains(sort) else { return }
        let nextOrder = order.flatMap { ["asc", "desc"].contains($0) ? $0 : nil }
        let changed = runtimeDimensionSort != sort
            || (nextOrder != nil && runtimeDimensionOrder != nextOrder)
        runtimeDimensionSort = sort
        if let nextOrder { runtimeDimensionOrder = nextOrder }
        guard changed else { return }
        guard statisticsVisible, ["overview", "dimensions", "tokens"].contains(statisticsBoard) else { return }
        let kind = statisticsBoard == "overview"
            ? "project"
            : statisticsBoard == "tokens" ? "endpoint" : runtimeDimensionKind
        loadRuntimeDimensionPage(kind: kind, page: 1)
    }

    func setRuntimeDimensionPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runtimeDimensionPageSize = pageSize
        guard statisticsVisible, ["overview", "dimensions", "tokens"].contains(statisticsBoard) else { return }
        // The Token board reuses the same dimension table, but it is fixed to
        // the endpoint dimension.  Keep its per-page selector functional too;
        // previously the setter silently ignored changes while that board was
        // visible.
        let kind = statisticsBoard == "overview"
            ? "project"
            : statisticsBoard == "tokens" ? "endpoint" : runtimeDimensionKind
        loadRuntimeDimensionPage(kind: kind, page: 1)
    }

    func setRuntimeErrorPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runtimeErrorPageSize = pageSize
        guard statisticsVisible, statisticsBoard == "errors" else { return }
        loadRuntimeV2ErrorPage(page: 1)
    }

    func setRuntimeProjectPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runtimeProjectPageSize = pageSize
        guard statisticsVisible, ["overview", "tokens", "dimensions"].contains(statisticsBoard) else { return }
        loadRuntimeProjects(page: 1)
    }

    func setRuntimeSessionPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runtimeSessionPageSize = pageSize
        guard statisticsVisible, ["overview", "tokens", "dimensions"].contains(statisticsBoard) else { return }
        loadRuntimeSessions(page: 1)
    }

    func setRuntimeModelPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runtimeModelPageSize = pageSize
        guard statisticsVisible, ["overview", "tokens", "dimensions"].contains(statisticsBoard) else { return }
        loadRuntimeModels(page: 1)
    }

    func setRuntimeEndpointPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runtimeEndpointPageSize = pageSize
        guard statisticsVisible, ["overview", "tokens"].contains(statisticsBoard) else { return }
        loadRuntimeEndpoints(page: 1)
    }

    func setRuntimeEndpointSort(_ sort: String, order: String = "desc") {
        runtimeEndpointSort = sort
        runtimeEndpointOrder = ["asc", "desc"].contains(order) ? order : "desc"
        guard statisticsVisible, ["overview", "tokens"].contains(statisticsBoard) else { return }
        loadRuntimeEndpoints(page: 1)
    }

    func setRuntimeModelSort(_ sort: String, order: String = "desc") {
        runtimeModelSort = sort
        runtimeModelOrder = ["asc", "desc"].contains(order) ? order : "desc"
        guard statisticsVisible, ["overview", "tokens"].contains(statisticsBoard) else { return }
        loadRuntimeModels(page: 1)
    }

    func setRuntimeProjectSort(_ sort: String, order: String = "desc") {
        runtimeProjectSort = sort
        runtimeProjectOrder = ["asc", "desc"].contains(order) ? order : "desc"
        guard statisticsVisible, ["overview", "tokens", "dimensions"].contains(statisticsBoard) else { return }
        loadRuntimeProjects(page: 1)
    }

    func setRuntimeSessionSort(_ sort: String, order: String = "desc") {
        runtimeSessionSort = sort
        runtimeSessionOrder = ["asc", "desc"].contains(order) ? order : "desc"
        guard statisticsVisible, ["overview", "tokens", "dimensions"].contains(statisticsBoard) else { return }
        loadRuntimeSessions(page: 1)
    }

    func loadRuntimeDimensionPage(kind: String? = nil, page: Int = 1) {
        let kind = kind ?? runtimeDimensionKind
        guard Self.runtimeDimensionKinds.contains(kind), page >= 1 else { return }
        runtimeDimensionKind = kind
        guard let admin, let anchor = runtimeHistoryPage else {
            runtimeDimensionPageLoading = false
            refreshRuntimeV2(resetSnapshot: true)
            return
        }
        runtimeDimensionRequestGeneration &+= 1
        let generation = runtimeDimensionRequestGeneration
        let snapshotGeneration = runtimeV2RequestGeneration
        runtimeDimensionPageLoading = true
        let search = runtimeDimensionSearch
        let sort = runtimeDimensionSort
        let order = runtimeDimensionOrder
        let pageSize = runtimeDimensionPageSize
        let filter = runtimeV2Filter()
        Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.runtimeDimensionRequestGeneration,
                   snapshotGeneration == self.runtimeV2RequestGeneration {
                    self.runtimeDimensionPageLoading = false
                }
            }
            do {
                let value = try await admin.runtimeDimensions(
                    kind: kind,
                    page: page,
                    pageSize: pageSize,
                    search: search.isEmpty ? nil : search,
                    sort: sort,
                    order: order,
                    snapshotSeq: anchor.snapshotSeq,
                    historyGeneration: anchor.historyGeneration,
                    filter: filter
                )
                guard generation == self.runtimeDimensionRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if self.runtimeDimensionPage != value {
                    self.runtimeDimensionPage = value
                }
                self.runtimeV2Error = nil
            } catch {
                guard generation == self.runtimeDimensionRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if self.isRuntimeSnapshotError(error) {
                    self.clearRuntimeV2Snapshot(expectedGeneration: snapshotGeneration)
                    self.refreshRuntimeV2(resetSnapshot: true)
                } else {
                    self.runtimeV2Error = "读取\(kind)统计列表失败：\(error)"
                }
            }
        }
    }

    /// 入口、项目、会话与模型是同一看板的首屏表，使用同一个 generation
    /// 并发读取；每张表仍保留自己的页、搜索和排序状态。
    private func loadRuntimeDimensionsForBoard() {
        guard let admin, let anchor = runtimeHistoryPage else { return }
        runtimeDimensionRequestGeneration &+= 1
        let generation = runtimeDimensionRequestGeneration
        let snapshotGeneration = runtimeV2RequestGeneration
        let filter = runtimeV2Filter()
        let sessionFilter = runtimeV2Filter(includeLocalProject: true)
        let modelFilter = runtimeV2Filter(includeLocalProject: true)
        runtimeDimensionsLoading = true
        let endpointPageSize = runtimeEndpointPageSize
        let projectPageSize = runtimeProjectPageSize
        let sessionPageSize = runtimeSessionPageSize
        let endpointSearch = runtimeEndpointSearch
        let projectSearch = runtimeProjectSearch
        let sessionSearch = runtimeSessionSearch
        let endpointSort = runtimeEndpointSort
        let endpointOrder = runtimeEndpointOrder
        let projectSort = runtimeProjectSort
        let projectOrder = runtimeProjectOrder
        let sessionSort = runtimeSessionSort
        let sessionOrder = runtimeSessionOrder
        let modelPageSize = runtimeModelPageSize
        let modelSearch = runtimeModelSearch
        let modelSort = runtimeModelSort
        let modelOrder = runtimeModelOrder
        Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.runtimeDimensionRequestGeneration,
                   snapshotGeneration == self.runtimeV2RequestGeneration {
                    self.runtimeDimensionsLoading = false
                }
            }
            do {
                async let endpoints = admin.runtimeDimensions(
                    kind: "endpoint",
                    page: 1,
                    pageSize: endpointPageSize,
                    search: endpointSearch,
                    sort: endpointSort,
                    order: endpointOrder,
                    snapshotSeq: anchor.snapshotSeq,
                    historyGeneration: anchor.historyGeneration,
                    filter: filter
                )
                async let projects = admin.runtimeProjects(
                    page: 1,
                    pageSize: projectPageSize,
                    search: projectSearch,
                    sort: projectSort,
                    order: projectOrder,
                    snapshotSeq: anchor.snapshotSeq,
                    historyGeneration: anchor.historyGeneration,
                    filter: filter
                )
                async let sessions = admin.runtimeSessions(
                    page: 1,
                    pageSize: sessionPageSize,
                    search: sessionSearch,
                    sort: sessionSort,
                    order: sessionOrder,
                    snapshotSeq: anchor.snapshotSeq,
                    historyGeneration: anchor.historyGeneration,
                    filter: sessionFilter
                )
                async let models = admin.runtimeDimensions(
                    kind: "model",
                    page: 1,
                    pageSize: modelPageSize,
                    search: modelSearch,
                    sort: modelSort,
                    order: modelOrder,
                    snapshotSeq: anchor.snapshotSeq,
                    historyGeneration: anchor.historyGeneration,
                    filter: modelFilter
                )
                let (endpointValue, projectValue, sessionValue, modelValue) = try await (endpoints, projects, sessions, models)
                guard generation == self.runtimeDimensionRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if self.runtimeEndpointsPage != endpointValue {
                    self.runtimeEndpointsPage = endpointValue
                }
                if self.runtimeProjectsPage != projectValue {
                    self.runtimeProjectsPage = projectValue
                }
                if self.runtimeSessionsPage != sessionValue {
                    self.runtimeSessionsPage = sessionValue
                }
                if self.runtimeModelsPage != modelValue {
                    self.runtimeModelsPage = modelValue
                }
                self.runtimeV2Error = nil
            } catch {
                guard generation == self.runtimeDimensionRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if self.isRuntimeSnapshotError(error) {
                    self.clearRuntimeV2Snapshot(expectedGeneration: snapshotGeneration)
                    self.refreshRuntimeV2(resetSnapshot: true)
                } else {
                    self.runtimeV2Error = "读取入口、项目和会话列表失败：\(error)"
                }
            }
        }
    }

    func loadRuntimeV2ErrorPage(page: Int) {
        guard page >= 1, let admin, let anchor = runtimeHistoryPage else { return }
        runtimeErrorPageRequestGeneration &+= 1
        let generation = runtimeErrorPageRequestGeneration
        let snapshotGeneration = runtimeV2RequestGeneration
        runtimeErrorPageLoading = true
        Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.runtimeErrorPageRequestGeneration,
                   snapshotGeneration == self.runtimeV2RequestGeneration {
                    self.runtimeErrorPageLoading = false
                }
            }
            do {
                let value = try await admin.runtimeErrors(
                    page: page,
                    pageSize: self.runtimeErrorPageSize,
                    snapshotSeq: anchor.snapshotSeq,
                    historyGeneration: anchor.historyGeneration,
                    filter: self.runtimeV2Filter()
                )
                guard generation == self.runtimeErrorPageRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if self.runtimeErrorPage != value {
                    self.runtimeErrorPage = value
                }
                self.runtimeV2Error = nil
            } catch {
                guard generation == self.runtimeErrorPageRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if self.isRuntimeSnapshotError(error) {
                    self.clearRuntimeV2Snapshot(expectedGeneration: snapshotGeneration)
                    self.refreshRuntimeV2(resetSnapshot: true)
                } else {
                    self.runtimeV2Error = "读取错误分页失败：\(error)"
                }
            }
        }
    }

    func setRuntimeHistoryPageSize(_ pageSize: Int) {
        guard AdminWire.RuntimeHistoryPage.allowedPageSizes.contains(pageSize) else { return }
        runtimeHistoryPageSize = pageSize
        clearRuntimeV2Snapshot()
        refreshRuntimeV2(resetSnapshot: true)
    }

    func loadRuntimeRequestChain(for eventID: String?) {
        runtimeRequestChainTask?.cancel()
        runtimeRequestChainTask = nil
        runtimeRequestChainRequestGeneration &+= 1
        let generation = runtimeRequestChainRequestGeneration
        runtimeRequestChain = nil
        runtimeRequestChainError = nil
        guard let eventID, !eventID.isEmpty else { return }
        let item = runHistoryPage?.events.first(where: { $0.id == eventID })
            ?? runtimeHistoryPage?.events.first(where: { $0.id == eventID })
        let requestID = item?.requestID
            ?? runtime.recentEvents.first(where: { $0.id == eventID })?.requestID
            ?? runtimeEventDetail?.event.requestID
        guard let requestID, !requestID.isEmpty, let admin else { return }
        runtimeRequestChainLoading = true
        let snapshotGeneration = runtimeV2RequestGeneration
        let connectionGeneration = adminConnectionGeneration
        let task = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.runtimeRequestChainRequestGeneration,
                   snapshotGeneration == self.runtimeV2RequestGeneration,
                   connectionGeneration == self.adminConnectionGeneration {
                    self.runtimeRequestChainLoading = false
                    self.runtimeRequestChainTask = nil
                }
            }
            do {
                let chain = try await admin.runtimeRequestChain(requestID: requestID)
                guard !Task.isCancelled,
                      generation == self.runtimeRequestChainRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimeRequestChain = chain
            } catch {
                guard !Task.isCancelled,
                      generation == self.runtimeRequestChainRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimeRequestChainError = "\(error)"
            }
        }
        runtimeRequestChainTask = task
    }

    func loadRuntimeProjects(page: Int = 1) {
        loadRuntimeDimension(kind: .project, page: page)
    }

    func loadRuntimeSessions(page: Int = 1) {
        loadRuntimeDimension(kind: .session, page: page)
    }

    func loadRuntimeModels(page: Int = 1) {
        loadRuntimeDimension(kind: .model, page: page)
    }

    func loadRuntimeEndpoints(page: Int = 1) {
        loadRuntimeDimension(kind: .endpoint, page: page)
    }

    private enum RuntimeDimension { case endpoint, project, session, model }

    private func loadRuntimeDimension(kind: RuntimeDimension, page: Int) {
        guard let admin, let anchor = runtimeHistoryPage else {
            runtimeDimensionsLoading = false
            refreshRuntimeV2(resetSnapshot: true)
            return
        }
        runtimeDimensionRequestGeneration &+= 1
        let generation = runtimeDimensionRequestGeneration
        let snapshotGeneration = runtimeV2RequestGeneration
        runtimeDimensionsLoading = true
        let search: String
        let sort: String
        let order: String
        let pageSize: Int
        switch kind {
        case .endpoint:
            search = runtimeEndpointSearch; sort = runtimeEndpointSort; order = runtimeEndpointOrder; pageSize = runtimeEndpointPageSize
        case .project:
            search = runtimeProjectSearch; sort = runtimeProjectSort; order = runtimeProjectOrder; pageSize = runtimeProjectPageSize
        case .session:
            search = runtimeSessionSearch; sort = runtimeSessionSort; order = runtimeSessionOrder; pageSize = runtimeSessionPageSize
        case .model:
            search = runtimeModelSearch; sort = runtimeModelSort; order = runtimeModelOrder; pageSize = runtimeModelPageSize
        }
        Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.runtimeDimensionRequestGeneration,
                   snapshotGeneration == self.runtimeV2RequestGeneration {
                    self.runtimeDimensionsLoading = false
                }
            }
            do {
                let value: AdminWire.RuntimeDimensionPage
                if kind == .endpoint {
                    value = try await admin.runtimeDimensions(
                        kind: "endpoint", page: page, pageSize: pageSize, search: search,
                        sort: sort, order: order, snapshotSeq: anchor.snapshotSeq,
                        historyGeneration: anchor.historyGeneration, filter: self.runtimeV2Filter()
                    )
                } else if kind == .project {
                    value = try await admin.runtimeProjects(
                        page: page, pageSize: pageSize, search: search, sort: sort,
                        order: self.runtimeProjectOrder, snapshotSeq: anchor.snapshotSeq,
                        historyGeneration: anchor.historyGeneration, filter: self.runtimeV2Filter()
                    )
                } else if kind == .session {
                    value = try await admin.runtimeSessions(
                        page: page, pageSize: pageSize, search: search, sort: sort,
                        order: self.runtimeSessionOrder, snapshotSeq: anchor.snapshotSeq,
                        historyGeneration: anchor.historyGeneration, filter: self.runtimeV2Filter(includeLocalProject: true)
                    )
                } else {
                    value = try await admin.runtimeDimensions(
                        kind: "model", page: page, pageSize: pageSize, search: search,
                        sort: sort, order: order, snapshotSeq: anchor.snapshotSeq,
                        historyGeneration: anchor.historyGeneration, filter: self.runtimeV2Filter(includeLocalProject: true)
                    )
                }
                guard generation == self.runtimeDimensionRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if kind == .endpoint {
                    if self.runtimeEndpointsPage != value { self.runtimeEndpointsPage = value }
                } else if kind == .project {
                    if self.runtimeProjectsPage != value {
                        self.runtimeProjectsPage = value
                    }
                } else if kind == .session {
                    if self.runtimeSessionsPage != value { self.runtimeSessionsPage = value }
                } else if kind == .model {
                    if self.runtimeModelsPage != value { self.runtimeModelsPage = value }
                }
                self.runtimeV2Error = nil
            } catch {
                guard generation == self.runtimeDimensionRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration else { return }
                if self.isRuntimeSnapshotError(error) {
                    self.clearRuntimeV2Snapshot(expectedGeneration: snapshotGeneration)
                    self.refreshRuntimeV2(resetSnapshot: true)
                } else {
                    self.runtimeV2Error = "读取统计列表分页失败：\(error)"
                }
            }
        }
    }

    func setRuntimeV2Project(_ projectID: String, projectName: String? = nil) {
        // Keep the historical method name for older views, but preserve the
        // current contract: a project row is a local drill-down that filters
        // only the session list, never the global analytics query.
        setRuntimeLocalProject(projectID, projectName: projectName)
    }

    func setRuntimeV2Session(_ sessionID: String) {
        runtimeV2SessionID = sessionID
        clearRuntimeV2Snapshot()
        refreshRuntimeV2(resetSnapshot: true)
    }

    func refreshRuntimeMaintenance() {
        runtimeMaintenanceRequestGeneration &+= 1
        let generation = runtimeMaintenanceRequestGeneration
        guard let admin else { return }
        let connectionGeneration = adminConnectionGeneration
        Task { [weak self] in
            guard let self else { return }
            async let storage = try? admin.runtimeStorage()
            async let retention = try? admin.runtimeRetention()
            async let pricing = try? admin.runtimePricing()
            let values = await (storage, retention, pricing)
            guard generation == self.runtimeMaintenanceRequestGeneration,
                  connectionGeneration == self.adminConnectionGeneration else { return }
            self.runtimeStorageProbe = values.0
            self.runtimeRetention = values.1
            self.runtimePricing = values.2
        }
    }

    func updateRuntimeRetention(_ update: AdminWire.RuntimeRetentionUpdate) {
        runtimeMaintenanceRequestGeneration &+= 1
        let generation = runtimeMaintenanceRequestGeneration
        guard let admin else { return }
        let connectionGeneration = adminConnectionGeneration
        Task { [weak self] in
            guard let self else { return }
            do {
                let retention = try await admin.updateRuntimeRetention(update)
                guard generation == self.runtimeMaintenanceRequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimeRetention = retention
                self.clearRuntimeV2Snapshot()
                self.flash("保留策略已更新")
                self.refreshRuntimeMaintenance()
                self.refreshRuntimeV2(resetSnapshot: true)
            } catch {
                guard generation == self.runtimeMaintenanceRequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimeV2Error = "保留策略更新失败：\(error)"
                self.flash("保留策略更新失败")
            }
        }
    }

    func updateRuntimePricing(_ update: AdminWire.RuntimePricingUpdate) {
        runtimeMaintenanceRequestGeneration &+= 1
        let generation = runtimeMaintenanceRequestGeneration
        guard let admin else { return }
        let connectionGeneration = adminConnectionGeneration
        Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await admin.updateRuntimePricing(update)
                let pricing = try await admin.runtimePricing()
                guard generation == self.runtimeMaintenanceRequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimePricing = pricing
                self.flash("模型价格表已更新")
                self.refreshRuntimeV2(resetSnapshot: true)
            } catch {
                guard generation == self.runtimeMaintenanceRequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimeV2Error = "模型价格表更新失败：\(error)"
                self.flash("模型价格表更新失败")
            }
        }
    }

    func estimateRuntimeExport(
        scope: String = "events",
        format: String = "jsonl",
        privacy: String = "stored",
        confirmStored: Bool = false
    ) {
        runtimeExportEstimateRequestGeneration &+= 1
        let generation = runtimeExportEstimateRequestGeneration
        guard let admin else {
            runtimeExportEstimate = nil
            runtimeExportEstimateError = "请先启动代理"
            return
        }
        let anchor = runtimeHistoryPage
        let snapshotGeneration = runtimeV2RequestGeneration
        let connectionGeneration = adminConnectionGeneration
        runtimeExportEstimateError = nil
        Task { [weak self] in
            guard let self else { return }
            do {
                let estimate = try await admin.runtimeExportEstimate(
                    scope: scope, format: format, privacy: privacy,
                    confirmStored: confirmStored,
                    snapshotSeq: anchor?.snapshotSeq,
                    historyGeneration: anchor?.historyGeneration,
                    filter: self.runtimeV2Filter()
                )
                guard !Task.isCancelled,
                      generation == self.runtimeExportEstimateRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimeExportEstimate = estimate
                self.runtimeExportEstimateError = nil
            } catch {
                guard generation == self.runtimeExportEstimateRequestGeneration,
                      snapshotGeneration == self.runtimeV2RequestGeneration,
                      connectionGeneration == self.adminConnectionGeneration else { return }
                self.runtimeExportEstimateError = "\(error)"
            }
        }
    }

    func exportRuntimeAnalytics(
        scope: String = "events",
        format: String = "jsonl",
        privacy: String = "stored",
        confirmStored: Bool = false
    ) {
        guard !runtimeExportBusy, let admin else { return }
        let anchor = runtimeHistoryPage
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "sumpter-runtime-\(scope).\(format)"
        panel.message = privacy == "stored"
            ? "stored 仅包含 SQLite 已保存的运行字段，不含完整诊断捕获；可能包含敏感标识。"
            : "导出为脱敏统计字段。"
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        runtimeExportRequestGeneration &+= 1
        let generation = runtimeExportRequestGeneration
        runtimeExportBusy = true
        Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.runtimeExportRequestGeneration { self.runtimeExportBusy = false }
            }
            do {
                try await admin.downloadRuntimeExport(
                    to: destination, scope: scope, format: format, privacy: privacy,
                    confirmStored: confirmStored,
                    snapshotSeq: anchor?.snapshotSeq,
                    historyGeneration: anchor?.historyGeneration,
                    filter: self.runtimeV2Filter()
                )
                guard generation == self.runtimeExportRequestGeneration else { return }
                self.flash("运行统计导出完成")
            } catch {
                guard generation == self.runtimeExportRequestGeneration else { return }
                self.runtimeExportEstimateError = "导出失败：\(error)"
                self.flash("运行统计导出失败")
            }
        }
    }

    private func isRuntimeSnapshotError(_ error: Error) -> Bool {
        guard let error = error as? AdminClient.AdminError else { return false }
        return error.serverCode == "runtime_snapshot_expired"
            || error.serverCode == "runtime_snapshot_trimmed"
    }

    private func clearRuntimeV2Snapshot(expectedGeneration: Int? = nil) {
        if let expectedGeneration,
           expectedGeneration != runtimeV2RequestGeneration {
            return
        }
        // Invalidate child requests before dropping the anchor.  Otherwise a
        // response for the old snapshot can repopulate a table after a reset,
        // filter change, or sidecar restart.
        runtimeErrorPageRequestGeneration &+= 1
        runtimeDimensionRequestGeneration &+= 1
        runtimeRequestChainRequestGeneration &+= 1
        runtimeExportEstimateRequestGeneration &+= 1
        runtimeRequestChainTask?.cancel()
        runtimeRequestChainTask = nil
        runtimeErrorPageLoading = false
        runtimeDimensionPageLoading = false
        runtimeDimensionsLoading = false
        runtimeRequestChainLoading = false
        runtimeHistoryPage = nil
        runtimeHistorySnapshotSeq = nil
        runtimeHistoryGeneration = nil
        runtimeTrendSeries = nil
        runtimeErrorPage = nil
        runtimeDimensionPage = nil
        runtimeProjectsPage = nil
        runtimeSessionsPage = nil
        runtimeEndpointsPage = nil
        runtimeModelsPage = nil
        runtimeRequestChain = nil
        runtimeExportEstimate = nil
        runtimeHistoryError = nil
        runtimeExportEstimateError = nil
    }

    func loadMoreRuntimeEvents() {
        Task {
            guard let admin else { return }
            do {
                var beforeSeq = runtimePage?.events.last?.seq
                var page: AdminWire.RuntimeEventPage?
                var merged = runtimePage?.events ?? []
                let known = Set(merged.map(\.id))
                var knownIDs = known
                for _ in 0..<5 {
                    let next = try await admin.runtimeEvents(beforeSeq: beforeSeq, limit: 200)
                    page = next
                    for event in next.events where knownIDs.insert(event.id).inserted {
                        merged.append(event)
                    }
                    guard next.hasMore, let nextBeforeSeq = next.events.last?.seq else { break }
                    beforeSeq = nextBeforeSeq
                }
                guard let lastPage = page else { return }
                runtimePage = AdminWire.RuntimeEventPage(
                    events: merged,
                    hasMore: lastPage.hasMore,
                    resetGeneration: lastPage.resetGeneration,
                    cursorValid: lastPage.cursorValid
                )
                let existing = Dictionary(uniqueKeysWithValues: runtime.recentEvents.map { ($0.id, $0) })
                runtime.recentEvents = merged.map { $0.mergedRuntimeEvent(with: existing[$0.id]) }
            } catch {
                runtimeEventsError = "\(error)"
                flash("加载历史事件失败")
            }
        }
    }

    func loadRuntimeEvent(id: String?) {
        detailRequestGeneration &+= 1
        let generation = detailRequestGeneration
        guard let id, !id.isEmpty else {
            runtimeEventDetail = nil
            return
        }
        // Do not leave the previous selection visible while this selection is
        // still loading; a slower response is rejected by the generation check.
        if runtimeEventDetail?.event.id != id {
            runtimeEventDetail = nil
        }
        Task {
            guard let admin else { return }
            do {
                let detail = try await admin.runtimeEvent(id: id)
                guard generation == detailRequestGeneration else { return }
                runtimeEventDetail = detail
                var events = runtime.recentEvents
                if let index = events.firstIndex(where: { $0.id == id }) {
                    events[index] = detail.event
                } else {
                    events.insert(detail.event, at: 0)
                }
                runtime.recentEvents = RuntimeEvent.trimmed(events, perKindLimit: 200)
            }
            catch {
                guard generation == detailRequestGeneration else { return }
                flash("读取事件详情失败")
            }
        }
    }

    /// 读取诊断捕获索引。索引请求可被下一次刷新取消，且只有最新代次能更新 UI。
    func refreshDiagnosticCapture() {
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureRequestGeneration &+= 1
        let generation = diagnosticCaptureRequestGeneration
        diagnosticCaptureBusy = true
        diagnosticCaptureTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureRequestGeneration {
                    self.diagnosticCaptureBusy = false
                    self.diagnosticCaptureTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "请先启动代理"
                return
            }
            do {
                let index = try await admin.diagnosticCaptureIndex()
                guard !Task.isCancelled, generation == self.diagnosticCaptureRequestGeneration else { return }
                if self.diagnosticCapture != index {
                    self.diagnosticCapture = index
                }
                self.diagnosticCaptureError = nil
                if let selected = self.diagnosticCaptureDetail?.requestID,
                   !index.records.contains(where: { $0.requestID == selected }) {
                    self.diagnosticCaptureDetailTask?.cancel()
                    self.diagnosticCaptureDetailRequestGeneration &+= 1
                    self.diagnosticCaptureDetail = nil
                    self.diagnosticCaptureDetailError = nil
                    self.diagnosticCaptureDetailBusy = false
                }
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "\(error)"
                self.flash("读取诊断捕获索引失败")
            }
        }
    }

    /// 只为用户当前选中的请求读取明文详情；快速切换时取消上一条并采用 latest-wins。
    func loadDiagnosticCaptureDetail(id: String?) {
        diagnosticCaptureDetailTask?.cancel()
        diagnosticCaptureDetailRequestGeneration &+= 1
        let generation = diagnosticCaptureDetailRequestGeneration
        guard let id, !id.isEmpty else {
            diagnosticCaptureDetail = nil
            diagnosticCaptureDetailError = nil
            diagnosticCaptureDetailBusy = false
            return
        }
        diagnosticCaptureDetail = nil
        diagnosticCaptureDetailError = nil
        diagnosticCaptureDetailBusy = true
        diagnosticCaptureDetailTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureDetailRequestGeneration {
                    self.diagnosticCaptureDetailBusy = false
                    self.diagnosticCaptureDetailTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureDetailRequestGeneration else { return }
                self.diagnosticCaptureDetailError = "请先启动代理"
                return
            }
            do {
                // 详情正文可能接近捕获容量上限。显式把网络读取和 Codable 解码
                // 放到 utility executor，只有最终的小状态更新回主 actor，避免把
                // 大记录解析与详情页的首帧布局耦合在一起。
                let detailTask = Task.detached(priority: .utility) {
                    try await admin.diagnosticCaptureDetail(id: id)
                }
                let detail = try await withTaskCancellationHandler(operation: {
                    try await detailTask.value
                }, onCancel: {
                    detailTask.cancel()
                })
                guard !Task.isCancelled, generation == self.diagnosticCaptureDetailRequestGeneration else { return }
                self.diagnosticCaptureDetail = detail
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureDetailRequestGeneration else { return }
                self.diagnosticCaptureDetailError = "\(error)"
                self.flash("读取诊断捕获详情失败")
            }
        }
    }

    /// 将最近一次原子落盘的完整捕获快照直接下载到用户选择的文件，不把正文解码或
    /// 累积进 AppModel。URLSession download task 会在临时文件中接收流，适合接近
    /// 512 MiB 的捕获；导出文件包含所有记录（索引最多只展示最近 200 条）。
    func exportDiagnosticCapture(
        privacy: String = "raw",
        confirmRaw: Bool = false,
        format: String = "jsonl",
        scope: String = "all",
        requestID: String? = nil
    ) {
        guard !diagnosticCaptureExportBusy else { return }
        guard let admin, (diagnosticCapture?.recordCount ?? 0) > 0 else {
            flash("暂无可导出的诊断捕获")
            return
        }
        guard privacy == "redacted" || (privacy == "raw" && confirmRaw) else {
            flash("未确认原始诊断捕获导出")
            return
        }
        let panel = NSSavePanel()
        let extensionName = format == "json" ? "json" : "jsonl"
        panel.nameFieldStringValue = "sumpter-diagnostic-capture-\(privacy)-\(Int(Date().timeIntervalSince1970)).\(extensionName)"
        panel.message = privacy == "raw"
            ? "文件包含未脱敏请求、响应、Headers 和流式 Chunk；仅保存到可信位置。"
            : "服务端会按需生成脱敏快照；原始捕获文件不会加载到 App 内存。"
        guard panel.runModal() == .OK, let destination = panel.url else { return }

        diagnosticCaptureExportRequestGeneration &+= 1
        let generation = diagnosticCaptureExportRequestGeneration
        diagnosticCaptureExportBusy = true
        let task = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureExportRequestGeneration {
                    self.diagnosticCaptureExportTask = nil
                    self.diagnosticCaptureExportBusy = false
                }
            }
            do {
                let downloadTask = Task.detached(priority: .utility) {
                    try await admin.downloadDiagnosticCapture(
                        to: destination,
                        privacy: privacy,
                        confirmRaw: confirmRaw,
                        scope: scope,
                        format: format,
                        requestID: requestID
                    )
                }
                try await withTaskCancellationHandler(operation: {
                    try await downloadTask.value
                }, onCancel: {
                    downloadTask.cancel()
                })
                guard !Task.isCancelled else { return }
                self.flash(privacy == "raw" ? "原始诊断捕获已导出" : "脱敏诊断捕获已导出")
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                self.flash("完整诊断捕获导出失败：\(error.localizedDescription)")
            }
        }
        diagnosticCaptureExportTask = task
    }

    func setDiagnosticCapture(enabled: Bool, maxBytes: Int? = nil) {
        guard !diagnosticCaptureBusy else { return }
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureRequestGeneration &+= 1
        let generation = diagnosticCaptureRequestGeneration
        diagnosticCaptureBusy = true
        diagnosticCaptureTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureRequestGeneration {
                    self.diagnosticCaptureBusy = false
                    self.diagnosticCaptureTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "请先启动代理"
                return
            }
            do {
                let index = try await admin.setDiagnosticCapture(enabled: enabled, maxBytes: maxBytes)
                guard !Task.isCancelled, generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCapture = index
                self.diagnosticCaptureError = nil
                self.flash(enabled ? "完整诊断捕获已开始" : "完整诊断捕获已停止")
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "\(error)"
                self.flash("切换诊断捕获失败")
            }
        }
    }

    func clearDiagnosticCapture() {
        guard !diagnosticCaptureBusy else { return }
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureRequestGeneration &+= 1
        let generation = diagnosticCaptureRequestGeneration
        diagnosticCaptureBusy = true
        diagnosticCaptureDetailTask?.cancel()
        diagnosticCaptureDetailRequestGeneration &+= 1
        diagnosticCaptureDetail = nil
        diagnosticCaptureDetailError = nil
        diagnosticCaptureDetailBusy = false
        diagnosticCaptureTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureRequestGeneration {
                    self.diagnosticCaptureBusy = false
                    self.diagnosticCaptureTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "请先启动代理"
                return
            }
            do {
                try await admin.clearDiagnosticCapture()
                let index = try await admin.diagnosticCaptureIndex()
                guard !Task.isCancelled, generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCapture = index
                self.diagnosticCaptureError = nil
                self.flash("捕获记录已清空")
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "\(error)"
                self.flash("清空诊断捕获失败")
            }
        }
    }

    /// 同步等待刷新完成;冻结模式的手动刷新用它拿到最新快照再重新冻结。
    func refreshNow() async {
        await refreshStatus(loadLatestEvents: true)
    }

    /// 退出前先停 sidecar(其 SIGTERM 路径会把统计落盘),避免防抖窗口内的最近事件丢失。
    func shutdownAndQuit() {
        Task {
            await stopSidecar()
            await MainActor.run {
                NSApplication.shared.terminate(nil)
            }
        }
    }

    func saveConfig() {
        Task {
            do {
                try await persistConfigAndRefresh()
            } catch {
                lastError = "\(error)"
                flash("保存失败")
            }
        }
    }

    /// 从磁盘重新加载 `config.json` 并推给引擎(读 `SumpterPaths.configURL()`;
    /// 旧 keys.json 早已不参与加载)。
    /// 用于手动编辑配置文件后无需重启即可生效(app 不监听文件变化)。
    /// 也可由控制端点 `POST /__reload` 经 `setReloadHandler` 触发。
    func reloadConfigFromDisk() {
        Task {
            do {
                try await performReloadConfigFromDisk()
                flash("已从磁盘重新加载配置")
            } catch {
                lastError = "\(error)"
                flash("重新加载失败")
            }
        }
    }

    /// 可 await 的磁盘重载实现,供菜单使用(外部 `/__reload` 由 sumpterd 自主处理并经 SSE 对账)。
    func performReloadConfigFromDisk() async throws {
        let url = try SumpterPaths.configURL()
        let store = self.store ?? ConfigStore(url: url)
        self.store = store
        configPath = url.path
        let result = try store.loadWithMigration()
        presentMigrationNoticeIfNeeded(result.migrationNotice)
        let raw = result.config
        let loaded = raw.normalizedBuiltInFeatureRules()
        if loaded != raw {
            try store.save(loaded)
        }
        config = loaded
        try await pushConfigToSidecar()
        await refreshStatus()
    }

    /// 配置已落盘后让 sumpterd 生效:监听地址变了要重启进程(热 reload 不重绑端口),
    /// 否则 admin reload 热加载并拿 generation/warnings 回执。
    private func pushConfigToSidecar() async throws {
        guard sidecar.isRunning else { return }
        if let applied = lastAppliedListener, applied != config.listener {
            // 监听地址/端口变了:热 reload 不重绑端口,必须重启进程。
            await stopSidecar()
            await startSidecar()
            flash("监听配置已变更,引擎已重启")
            return
        }
        guard let admin else { return }
        let ack = try await admin.reload()
        lastAppliedListener = config.listener
        // 对账:generation 是 sumpterd 对刚读到的配置算的代号,对不上说明引擎没跟上。
        engineGeneration = ack.generation
        // reload 回执仍完整解码以兼容 daemon wire，但不再在 App 内展示
        // 配置风险监测；这里只反馈 reload 本身。
        flash("配置已生效")
    }

    func resetRuntimeStats() {
        Task {
            guard let admin else { return }
            do {
                try await admin.resetRuntime()
                runtimeChangeSeq = 0
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
                runtime = RuntimeSnapshot()
                clearRuntimeV2Snapshot()
                runtimeV2RequestGeneration &+= 1
                runtimeErrorPageRequestGeneration &+= 1
                runtimeDimensionRequestGeneration &+= 1
                resetRuntimeLocalDrillDownState()
                flash("SQLite 新统计已清空")
            } catch {
                lastError = "\(error)"
                flash("重置统计失败")
            }
            await refreshStatus(loadLatestEvents: true)
        }
    }

    func recreateRuntimeStats() {
        Task {
            guard let admin else { return }
            do {
                try await admin.recreateRuntime()
                runtimeChangeSeq = 0
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
                runtime = RuntimeSnapshot()
                clearRuntimeV2Snapshot()
                runtimeV2RequestGeneration &+= 1
                runtimeErrorPageRequestGeneration &+= 1
                runtimeDimensionRequestGeneration &+= 1
                runtimeMaintenanceRequestGeneration &+= 1
                resetRuntimeLocalDrillDownState()
                flash("已重置并新建 SQLite 数据库")
            } catch {
                lastError = "\(error)"
                flash("新建数据库失败")
            }
            await refreshStatus(loadLatestEvents: true)
        }
    }

    func previewRuntimeCleanup(olderThan: Double) async throws -> AdminWire.RuntimeCleanupPreview {
        guard let admin else { throw AppModelError.invalidInput("管理连接尚未就绪") }
        return try await admin.previewRuntimeCleanup(olderThan: olderThan)
    }

    func cleanupRuntime(olderThan: Double) async throws -> AdminWire.RuntimeCleanupMutation {
        guard let admin else { throw AppModelError.invalidInput("管理连接尚未就绪") }
        let mutation = try await admin.cleanupRuntime(olderThan: olderThan)
        runtimeChangeSeq = 0
        runtimeEventDetail = nil
        runtimePage = nil
        runHistoryRequestGeneration &+= 1
        runHistoryPage = nil
        runHistoryError = nil
        clearRuntimeV2Snapshot()
        runtimeV2RequestGeneration &+= 1
        runtimeErrorPageRequestGeneration &+= 1
        runtimeDimensionRequestGeneration &+= 1
        resetRuntimeLocalDrillDownState()
        runtime.recentEvents = []
        flash("已按时间清理 \(mutation.deletedEvents) 条统计事件")
        await refreshStatus(loadLatestEvents: true)
        refreshRuntimeMaintenance()
        return mutation
    }

    func deleteRuntimeSession(sessionID: String, confirmUnidentified: Bool = false) {
        Task {
            guard let admin else { return }
            do {
                let mutation = try await admin.deleteRuntimeSession(
                    sessionID: sessionID,
                    confirmUnidentified: confirmUnidentified
                )
                runtimeChangeSeq = 0
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
                clearRuntimeV2Snapshot()
                runtimeV2RequestGeneration &+= 1
                runtimeErrorPageRequestGeneration &+= 1
                runtimeDimensionRequestGeneration &+= 1
                if runtimeLocalSessionID == sessionID || runtimeLocalSessionName == sessionID {
                    resetRuntimeLocalSessionState()
                }
                if runtimeAnalyticsSessionID == sessionID {
                    runtimeAnalyticsSessionID = ""
                }
                flash("已删除会话 · \(mutation.deletedRequests) 个请求 / \(mutation.deletedEvents) 条事件")
                await refreshStatus(loadLatestEvents: true)
            } catch {
                lastError = "\(error)"
                flash("删除会话失败")
            }
        }
    }

    func exportRuntimeSession(sessionID: String) {
        Task {
            guard let admin else { return }
            do {
                let data = try await admin.exportRuntimeSession(sessionID: sessionID)
                let panel = NSSavePanel()
                panel.nameFieldStringValue = "sumpter-session-\(sessionID).json"
                if panel.runModal() == .OK, let url = panel.url {
                    try data.write(to: url, options: .atomic)
                    flash("会话 JSON 已导出")
                }
            } catch {
                lastError = "\(error)"
                flash("导出会话失败")
            }
        }
    }

    func setClaudeNotifications(enabled: Bool) {
        do {
            try ClaudeNotificationHooks.setEnabled(enabled, port: config.listener.port)
            refreshNotificationHookState()
            if enabled {
                requestNotificationAuthorization()
            }
        } catch {
            lastError = "\(error)"
            flash("通知设置失败")
        }
    }

    func setCodexNotifications(enabled: Bool) {
        do {
            try CodexNotificationHooks.setEnabled(enabled, port: config.listener.port)
            refreshNotificationHookState()
            if enabled {
                requestNotificationAuthorization()
            }
        } catch {
            refreshNotificationHookState()
            lastError = "\(error)"
            flash("Codex 通知设置失败")
        }
    }

    func setNotifications(enabled: Bool) {
        do {
            try ClaudeNotificationHooks.setEnabled(enabled, port: config.listener.port)
            try CodexNotificationHooks.setEnabled(enabled, port: config.listener.port)
            try GrokNotificationHooks.setEnabled(enabled, port: config.listener.port)
            refreshNotificationHookState()
            if enabled {
                requestNotificationAuthorization()
            }
        } catch {
            refreshNotificationHookState()
            lastError = "\(error)"
            flash("通知设置失败")
        }
    }

    func setClaudeNotification(argument: String, enabled: Bool) {
        do {
            var arguments = claudeNotificationArguments
            if enabled {
                arguments.insert(argument)
            } else {
                arguments.remove(argument)
            }
            try ClaudeNotificationHooks.setSelectedArguments(arguments, port: config.listener.port)
            refreshNotificationHookState()
            if !arguments.isEmpty {
                requestNotificationAuthorization()
            }
        } catch {
            lastError = "\(error)"
            flash("通知类型设置失败")
        }
    }

    func setNotificationCategory(_ category: String, enabled: Bool) {
        switch category {
        case "action_required": actionNotificationsEnabled = enabled
        case "status": statusNotificationsEnabled = enabled
        case "turn_completed": turnCompletionNotificationsEnabled = enabled
        case "subtask_completed": subtaskNotificationsEnabled = enabled
        case "turn_failed": failureNotificationsEnabled = enabled
        default: return
        }
        UserDefaults.standard.set(enabled, forKey: notificationCategoryKey(category))
        if enabled { requestNotificationAuthorization() }
    }

    private func refreshNotificationHookState() {
        claudeNotificationArguments = ClaudeNotificationHooks.enabledArguments()
        claudeNotificationsEnabled = !claudeNotificationArguments.isEmpty
        codexNotificationHookPath = CodexNotificationHooks.resolvedHooksJSONPath()
        codexNotificationArguments = CodexNotificationHooks.enabledArguments()
        codexNotificationHookStatus = CodexNotificationHooks.currentStatus()
        codexNotificationsEnabled = !codexNotificationArguments.isEmpty
        grokNotificationHookPath = GrokNotificationHooks.resolvedHooksJSONPath()
        grokNotificationsEnabled = GrokNotificationHooks.isEnabled()
    }

    func sendTestNotification() {
        Task {
            await deliverNotification(title: "Sumpter", message: "通知已连接")
        }
    }

    func previewNotificationSound() {
        NativeNotifier.shared.playSound(notificationSoundPreference)
    }

    func requestNotificationAuthorization() {
        Task {
            do {
                notificationAuthorizationStatus = try await NativeNotifier.shared.requestAuthorization()
                notificationError = nil
            } catch {
                notificationError = "\(error)"
                lastError = "\(error)"
            }
        }
    }

    func refreshNotificationAuthorizationStatus() {
        Task {
            NativeNotifier.shared.configure()
            notificationAuthorizationStatus = await NativeNotifier.shared.authorizationStatus()
        }
    }

    func openNotificationSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.notifications") {
            NSWorkspace.shared.open(url)
        } else {
            NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/System Settings.app"))
        }
    }

    /// 在访达里打开Sumpter的配置目录(config.json / runtime.sqlite3 / 旧 stats.json / proxy.log 所在处)。
    func openConfigDirectory() {
        do {
            let directory = try SumpterPaths.appSupportDirectory()
            NSWorkspace.shared.activateFileViewerSelecting([directory])
        } catch {
            flash("打开配置目录失败")
            lastError = "\(error)"
        }
    }

    func setNotificationSoundPreference(_ preference: NotificationSoundPreference) {
        notificationSoundPreference = preference
        preference.save()
    }

    func refreshLoginItemStatus() {
        loginItemEnabled = SMAppService.mainApp.status == .enabled
    }

    func setLoginItem(enabled: Bool) {
        do {
            if enabled {
                if SMAppService.mainApp.status != .enabled {
                    try SMAppService.mainApp.register()
                }
            } else if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            }
            refreshLoginItemStatus()
        } catch {
            lastError = "\(error)"
            flash("自启动设置失败")
            refreshLoginItemStatus()
        }
    }

    // MARK: - 入口

    func addProviderAccount(
        idText: String,
        name: String,
        baseURLText: String,
        protocolName: String,
        enabled: Bool,
        apiKey: String,
        pinnedIPsText: String = "",
        priority: Int = 0,
        pin: Bool = false,
        stickyGroup: String = "",
        keepAlive: Bool = true
    ) async throws {
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        // 空 Key 合法:= 无鉴权转发(不发鉴权头,本地/内网 llama.cpp 等上游用),
        // 与引擎行为不变量一致;空 Key 仍允许无鉴权的本地/内网 HTTP 上游。
        let cleanKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty else {
            throw AppModelError.invalidInput("入口名称不能为空")
        }
        let url = try validatedURLString(baseURLText, field: "API 地址")
        guard let baseURL = URL(string: url) else {
            throw AppModelError.invalidInput("API 地址无效")
        }
        let trimmedID = idText.trimmingCharacters(in: .whitespacesAndNewlines)
        let id = try validatedProviderID(trimmedID.isEmpty ? uniqueEndpointID(cleanName) : trimmedID)
        let ips = parseCSV(pinnedIPsText)
        let group = stickyGroup.trimmingCharacters(in: .whitespacesAndNewlines)
        guard priority >= 0 else {
            throw AppModelError.invalidInput("优先级必须是非负整数")
        }

        try await mutateConfig { config in
            guard config.endpoint(id: id) == nil else {
                throw AppModelError.invalidInput("入口 ID 已存在: \(id)")
            }
            config.endpoints.append(Endpoint(
                id: id,
                name: cleanName,
                baseURL: baseURL,
                protocolMode: try Self.endpointProtocolMode(protocolName),
                enabled: enabled,
                apiKey: cleanKey,
                pinnedIPs: ips,
                priority: priority,
                pinnedIPExclusive: pin,
                stickyGroup: group.isEmpty ? nil : group,
                keepAlive: keepAlive
            ))
        }
    }

    func updateProviderAccount(
        id: String,
        name: String,
        baseURLText: String,
        protocolName: String,
        enabled: Bool,
        apiKey: String,
        pinnedIPsText: String = "",
        priority: Int = 0,
        pin: Bool = false,
        stickyGroup: String = "",
        keepAlive: Bool = false
    ) async throws {
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        // 空 Key 合法(无鉴权转发),同 addProviderAccount。
        let cleanKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty else {
            throw AppModelError.invalidInput("入口名称不能为空")
        }
        let url = try validatedURLString(baseURLText, field: "API 地址")
        guard let baseURL = URL(string: url) else {
            throw AppModelError.invalidInput("API 地址无效")
        }
        let ips = parseCSV(pinnedIPsText)
        let group = stickyGroup.trimmingCharacters(in: .whitespacesAndNewlines)
        guard priority >= 0 else {
            throw AppModelError.invalidInput("优先级必须是非负整数")
        }

        try await mutateConfig { config in
            let location = try Self.locate(endpointID: id, in: config)
            config.endpoints[location.endpoint].name = cleanName
            config.endpoints[location.endpoint].baseURL = baseURL
            config.endpoints[location.endpoint].apiKey = cleanKey
            config.endpoints[location.endpoint].protocolMode =
                try Self.endpointProtocolMode(protocolName)
            config.endpoints[location.endpoint].enabled = enabled
            config.endpoints[location.endpoint].pinnedIPs = ips
            config.endpoints[location.endpoint].priority = priority
            config.endpoints[location.endpoint].pinnedIPExclusive = pin
            config.endpoints[location.endpoint].stickyGroup = group.isEmpty ? nil : group
            config.endpoints[location.endpoint].keepAlive = keepAlive
        }
    }

    /// 快捷启停只改一个字段，并在落盘/reload 任一步失败时恢复原状态。
    /// 回滚基于失败时的最新配置草稿，避免覆盖同时完成的其它字段修改。
    func setProviderEnabled(id: String, enabled: Bool) async throws {
        let location = try Self.locate(endpointID: id, in: config)
        let previousEnabled = config.endpoints[location.endpoint].enabled
        guard previousEnabled != enabled else { return }

        var draft = config
        draft.endpoints[location.endpoint].enabled = enabled
        config = draft

        do {
            try await persistConfigAndRefresh()
        } catch {
            let saveError = error
            var rollback = config
            if let rollbackLocation = try? Self.locate(endpointID: id, in: rollback),
               rollback.endpoints[rollbackLocation.endpoint].enabled == enabled {
                rollback.endpoints[rollbackLocation.endpoint].enabled = previousEnabled
                config = rollback
                do {
                    try await persistConfigAndRefresh()
                } catch {
                    throw AppModelError.invalidInput(
                        "入口启停保存失败（\(saveError.localizedDescription)），回滚也失败：\(error.localizedDescription)"
                    )
                }
            }
            throw saveError
        }
    }

    func deleteProviderAccounts(ids: Set<String>) async throws {
        guard !ids.isEmpty else { return }
        try await mutateConfig { config in
            config.endpoints.removeAll { ids.contains($0.id) }
        }
    }

    func moveProviderAccount(id: String, direction: Int) async throws {
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: id, in: config)
            let target = location.endpoint + direction
            guard config.endpoints.indices.contains(target) else {
                return
            }
            let endpoint = config.endpoints.remove(at: location.endpoint)
            config.endpoints.insert(endpoint, at: target)
        }
    }

    /// 多选整块移动(一次落盘;顺序语义 = TableSelection.moved,抵边行原地不动)。
    func moveProviderAccounts(ids: Set<String>, direction: Int) async throws {
        guard !ids.isEmpty else { return }
        try await mutateConfig { config in
            let order = config.endpoints.map(\.id)
            let target = TableSelection.moved(ids: order, selection: ids, direction: direction)
            guard target != order else { return }
            let byID = Dictionary(uniqueKeysWithValues: config.endpoints.map { ($0.id, $0) })
            config.endpoints = target.compactMap { byID[$0] }
        }
    }

    /// 拖拽入口后的任意位置移动。目标下标按移除源入口后的数组计算，
    /// 与 macOS Table 的拖放回调一致；仍走统一配置落盘/sidecar 刷新管线。
    func reorderProviderAccount(id: String, toIndex: Int) async throws {
        try await mutateConfig { config in
            let order = config.endpoints.map(\.id)
            let target = TableSelection.moved(id: id, orderedIDs: order, toIndex: toIndex)
            guard target != order else { return }
            let byID = Dictionary(uniqueKeysWithValues: config.endpoints.map { ($0.id, $0) })
            config.endpoints = target.compactMap { byID[$0] }
        }
    }

    // MARK: - 入口的模型映射

    func addProviderMapping(
        endpointID: String,
        clientPattern: String,
        upstreamModel: String,
        thinking: ThinkingMode,
        context: ContextMode,
        failoverTimeoutSeconds: Double? = nil
    ) async throws {
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            let client = ModelName.clean(clientPattern)
            let upstream = ModelName.clean(upstreamModel)
            guard !client.isEmpty else {
                throw AppModelError.invalidInput("客户端模型不能为空")
            }
            guard !config.endpoints[location.endpoint].hasMapping(clientPattern: client) else {
                throw AppModelError.invalidInput("该入口已存在客户端模型映射: \(client)")
            }
            config.endpoints[location.endpoint].mappings.append(ModelMapping(
                clientPattern: ModelPattern(client),
                upstreamModel: upstream,
                thinking: thinking,
                context: context,
                failoverTimeoutSeconds: failoverTimeoutSeconds
            ))
        }
    }

    /// 从已知模型目录里批量把选中的真实模型加成映射。
    func addProviderMappingsFromCatalog(endpointID: String, models: Set<String>) async throws {
        let specs: [(client: String, upstream: String)] = models.compactMap { raw in
            let original = ModelName.clean(raw)
            guard !original.isEmpty else { return nil }
            return (client: original, upstream: original)
        }
        guard !specs.isEmpty else { return }
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            var existing = Set(
                config.endpoints[location.endpoint].mappings.map {
                    ModelName.clean($0.clientPattern.rawValue)
                }
            )
            for spec in specs where !existing.contains(spec.client) {
                config.endpoints[location.endpoint].mappings.append(ModelMapping(
                    clientPattern: ModelPattern(spec.client),
                    upstreamModel: spec.upstream,
                    thinking: .passthrough,
                    context: ModelName.defaultContext(for: spec.client)
                ))
                existing.insert(spec.client)
            }
        }
    }

    /// 某入口已获取但尚未映射的真实模型(供「从已知模型添加」挑选)。
    func unmappedCatalogModels(endpointID: String) -> [String] {
        guard let endpoint = config.endpoint(id: endpointID) else { return [] }
        let mapped = Set(endpoint.mappings.map { ModelName.clean($0.clientPattern.rawValue) })
        var seen: Set<String> = []
        var result: [String] = []
        for model in endpoint.catalog.models {
            let cleaned = ModelName.clean(model)
            guard !cleaned.isEmpty, !mapped.contains(cleaned), !seen.contains(cleaned) else { continue }
            seen.insert(cleaned)
            result.append(cleaned)
        }
        return result
    }

    func updateProviderMapping(
        endpointID: String,
        mappingID: String,
        clientPattern: String,
        upstreamModel: String,
        thinking: ThinkingMode,
        context: ContextMode,
        failoverTimeoutSeconds: Double?
    ) async throws {
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            let endpoint = config.endpoints[location.endpoint]
            guard let mappingIndex = endpoint.mappings.firstIndex(where: { $0.id == mappingID }) else {
                throw AppModelError.invalidInput("模型映射不存在")
            }
            let client = ModelName.clean(clientPattern)
            let upstream = ModelName.clean(upstreamModel)
            guard !client.isEmpty else {
                throw AppModelError.invalidInput("客户端模型不能为空")
            }
            guard !endpoint.hasMapping(clientPattern: client, excluding: mappingID) else {
                throw AppModelError.invalidInput("该入口已存在客户端模型映射: \(client)")
            }
            // Capability declarations are part of the routing contract. The
            // editor currently does not expose a picker, so an update must
            // carry the existing values forward instead of silently turning
            // an explicit video/live/files mapping back into name inference.
            let existingCapabilities = endpoint.mappings[mappingIndex].capabilities
            config.endpoints[location.endpoint].mappings[mappingIndex] = ModelMapping(
                clientPattern: ModelPattern(client),
                upstreamModel: upstream,
                thinking: thinking,
                context: context,
                failoverTimeoutSeconds: failoverTimeoutSeconds,
                capabilities: existingCapabilities
            )
        }
    }

    func deleteProviderMapping(endpointID: String, mappingID: String) async throws {
        try await deleteProviderMappings(endpointID: endpointID, mappingIDs: [mappingID])
    }

    func deleteProviderMappings(endpointID: String, mappingIDs: Set<String>) async throws {
        guard !mappingIDs.isEmpty else { return }
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            config.endpoints[location.endpoint].mappings.removeAll {
                mappingIDs.contains($0.id)
            }
        }
    }

    func fetchProviderModels(endpointID: String) async throws {
        fetchingModelEndpointIDs.insert(endpointID)
        defer { fetchingModelEndpointIDs.remove(endpointID) }

        guard config.endpoint(id: endpointID) != nil else {
            throw AppModelError.invalidInput("入口不存在: \(endpointID)")
        }
        guard let admin else {
            throw AppModelError.invalidInput("代理服务尚未启动，无法获取模型")
        }
        let result = try await admin.providerModels(endpointID: endpointID)
        let models = ModelCatalog.deduplicatedModels(result.models)
        let source = result.source
        let stamp = modelCatalogTimestamp(result.updatedAt)

        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            config.endpoints[location.endpoint].catalog = ModelCatalog(
                models: models,
                source: source,
                status: "已获取",
                error: "",
                updatedAt: stamp
            )
        }
        flash("已获取 \(models.count) 个模型")
    }

    func markFetchModelsFailure(endpointID: String, error: Error) async {
        let message = String((error.localizedDescription.isEmpty ? "\(error)" : error.localizedDescription).prefix(300))
        do {
            try await mutateConfig { config in
                let location = try Self.locate(endpointID: endpointID, in: config)
                config.endpoints[location.endpoint].catalog.status = "获取失败"
                config.endpoints[location.endpoint].catalog.error = message
                config.endpoints[location.endpoint].catalog.updatedAt = modelCatalogTimestamp()
            }
        } catch {
            lastError = "\(error)"
        }
    }

    // MARK: - 分流规则

    func updateFeatureRuleAndSave(
        id: String,
        enabled: Bool,
        model: String,
        effortOverride: ReasoningEffort?,
        protocolOverride: ProviderProtocol?,
        endpointID: String?,
        toolTypePrefix: String,
        systemContains: String,
        messagesContain: String,
        modelEquals: String
    ) async throws {
        let cleanedModel = ModelName.clean(model)
        guard !cleanedModel.isEmpty else {
            throw AppModelError.invalidInput("目标模型不能为空")
        }
        let trimmedEndpointID = endpointID?.trimmingCharacters(in: .whitespacesAndNewlines)
        let targetEndpointID = (trimmedEndpointID?.isEmpty == false) ? trimmedEndpointID : nil

        try await mutateConfig { config in
            if let targetEndpointID {
                guard config.endpoint(id: targetEndpointID) != nil else {
                    throw AppModelError.invalidInput("Provider 不存在: \(targetEndpointID)")
                }
            }
            guard let index = config.featureRules.firstIndex(where: { $0.id == id }) else {
                throw AppModelError.invalidInput("分流规则不存在: \(id)")
            }
            config.featureRules[index].enabled = enabled
            config.featureRules[index].target = RouteTarget(
                model: cleanedModel,
                protocolOverride: protocolOverride,
                endpointID: targetEndpointID,
                effortOverride: effortOverride
            )
            if let canonical = BuiltInFeatureRules.canonicalRule(id: id) {
                config.featureRules[index].name = canonical.name
                config.featureRules[index].match = canonical.match
            } else {
                config.featureRules[index].match = FeatureMatch(
                    toolTypePrefix: nilIfBlank(toolTypePrefix),
                    systemContains: nilIfBlank(systemContains),
                    messagesContain: nilIfBlank(messagesContain),
                    modelEquals: nilIfBlank(modelEquals)
                )
            }
        }
    }

    func updateFeatureRule(
        id: String,
        enabled: Bool,
        model: String,
        effortOverride: ReasoningEffort? = nil,
        protocolOverride: ProviderProtocol? = nil,
        endpointID: String? = nil,
        toolTypePrefix: String,
        systemContains: String,
        messagesContain: String,
        modelEquals: String
    ) {
        Task {
            do {
                try await updateFeatureRuleAndSave(
                    id: id,
                    enabled: enabled,
                    model: model,
                    effortOverride: effortOverride,
                    protocolOverride: protocolOverride,
                    endpointID: endpointID,
                    toolTypePrefix: toolTypePrefix,
                    systemContains: systemContains,
                    messagesContain: messagesContain,
                    modelEquals: modelEquals
                )
            } catch {
                lastError = "\(error)"
                flash("更新分流规则失败")
            }
        }
    }

    // MARK: - 转发与重试参数(全局)

    func updateRetryPolicyAndSave(
        responseTimeoutText: String,
        streamIdleTimeoutText: String,
        max500RetriesText: String,
        failoverOn500: Bool = true,
        retryDelaySecondsText: String,
        passThroughRetryDelay: Bool = true,
        maxDeferredRoundsText: String,
        maxRetryDurationSecondsText: String,
        sessionStickyRetriesText: String,
        pinnedIPConcurrencyText: String
    ) async throws {
        let tuning = try mapInputValidation {
            try InputValidation.retryPolicy(
                responseTimeoutText: responseTimeoutText,
                streamIdleTimeoutText: streamIdleTimeoutText,
                max500RetriesText: max500RetriesText,
                failoverOn500: failoverOn500,
                retryDelaySecondsText: retryDelaySecondsText,
                passThroughRetryDelay: passThroughRetryDelay,
                maxDeferredRoundsText: maxDeferredRoundsText,
                maxRetryDurationSecondsText: maxRetryDurationSecondsText,
                sessionStickyRetriesText: sessionStickyRetriesText,
                pinnedIPConcurrencyText: pinnedIPConcurrencyText
            )
        }
        try await mutateConfig { config in
            config.retry = RetryPolicy(
                responseTimeoutSeconds: tuning.responseTimeoutSeconds,
                streamIdleTimeoutSeconds: tuning.streamIdleTimeoutSeconds,
                max500Retries: tuning.max500Retries,
                failoverOn500: tuning.failoverOn500,
                retryDelaySeconds: tuning.retryDelaySeconds,
                passThroughRetryDelay: tuning.passThroughRetryDelay,
                maxDeferredRounds: tuning.maxDeferredRounds,
                maxRetryDurationSeconds: tuning.maxRetryDurationSeconds,
                sessionStickyRetries: tuning.sessionStickyRetries,
                pinnedIPConcurrency: tuning.pinnedIPConcurrency
            )
        }
    }

    // MARK: - Claude Code 配置备份

    func backupClaudeSettings() {
        do {
            try ClaudeNotificationHooks.backupSettings()
            flash("Claude Code 配置已备份")
        } catch {
            lastError = "\(error)"
            flash("备份 Claude Code 配置失败")
        }
    }

    func restoreClaudeSettingsBackup() {
        do {
            try ClaudeNotificationHooks.restoreBackup()
            refreshNotificationHookState()
            flash("Claude Code 配置已还原")
        } catch {
            lastError = "\(error)"
            flash("还原 Claude Code 配置失败")
        }
    }

    // MARK: - 入站认证

    func setInboundAuthToken(_ token: String) {
        Task {
            do {
                try await mutateConfig { config in
                    config.listener.authToken = token.trimmingCharacters(in: .whitespacesAndNewlines)
                }
            } catch {
                lastError = "\(error)"
                flash("更新入站认证失败")
            }
        }
    }

    func clearInboundAuthToken() {
        setInboundAuthToken("")
    }

    /// 提交监听配置(安全页草稿的唯一落地入口)。
    /// 走 mutateConfig 统一管线:归一化 → 落盘 → 推给引擎(必要时重启进程)。
    /// 端口变更时同步重写通知 hook 脚本(脚本内嵌端口,不重写会静默失联)。
    func updateListener(host: String, port: Int, allowedCIDRs: [String]) {
        Task {
            do {
                let portChanged = port != config.listener.port
                try await mutateConfig { draft in
                    draft.listener.host = host
                    draft.listener.port = port
                    draft.listener.allowedCIDRs = allowedCIDRs
                }
                if portChanged {
                    do {
                        try ClaudeNotificationHooks.rewriteScriptIfEnabled(port: port)
                        try CodexNotificationHooks.rewriteScriptIfEnabled(port: port)
                        try GrokNotificationHooks.rewriteScriptIfEnabled(port: port)
                        refreshNotificationHookState()
                    } catch {
                        flash("通知脚本更新失败,请重新切换一次通知开关")
                    }
                }
            } catch {
                lastError = "\(error)"
                flash("保存监听配置失败")
            }
        }
    }

    // MARK: - 配置改写基础设施

    /// 所有配置修改的唯一入口:在草稿上改,归一化内建规则,落盘并刷新引擎。
    private func mutateConfig(_ body: (inout AppConfig) throws -> Void) async throws {
        var draft = config
        try body(&draft)
        draft.normalizeBuiltInFeatureRules()
        config = draft
        try await persistConfigAndRefresh()
    }

    private struct EndpointLocation {
        let endpoint: Int
    }

    private static func locate(endpointID: String, in config: AppConfig) throws -> EndpointLocation {
        guard let endpoint = config.endpoints.firstIndex(where: { $0.id == endpointID }) else {
            throw AppModelError.invalidInput("Provider 不存在: \(endpointID)")
        }
        return EndpointLocation(endpoint: endpoint)
    }

    private static func endpointProtocolMode(_ rawValue: String) throws -> EndpointProtocolMode {
        guard let mode = EndpointProtocolMode(rawValue: rawValue) else {
            throw AppModelError.invalidInput("入口协议无效: \(rawValue)")
        }
        return mode
    }

    private func presentMigrationNoticeIfNeeded(_ notice: ConfigMigrationNotice?) {
        guard let notice else { return }
        let defaultsKey = "sumpter.configMigrationNotice.\(notice.id)"
        guard !UserDefaults.standard.bool(forKey: defaultsKey) else { return }
        UserDefaults.standard.set(true, forKey: defaultsKey)
        configMigrationNotice = notice
        let expanded = notice.autoEndpointIDs.count
        let suffix = expanded > 0 ? "，其中 \(expanded) 个入口已转为自动（三协议）" : ""
        flash("配置已迁移到 v\(notice.toSchema)\(suffix)；旧文件已备份")
    }

    private func validatedURLString(_ text: String, field: String) throws -> String {
        try mapInputValidation { try InputValidation.url(text, field: field) }
    }

    private func validatedProviderID(_ text: String) throws -> String {
        try mapInputValidation { try InputValidation.providerID(text) }
    }

    /// 把 Core 的 `InputValidationError` 归一成 AppModel 的 `invalidInput`，保持既有对外错误契约。
    private func mapInputValidation<T>(_ body: () throws -> T) throws -> T {
        do {
            return try body()
        } catch let error as InputValidationError {
            throw AppModelError.invalidInput(error.message)
        }
    }


    private func bootstrap() async {
        do {
            let url = try SumpterPaths.configURL()
            configPath = url.path
            tightenSensitiveFilePermissions()
            let store = ConfigStore(url: url)
            self.store = store
            if FileManager.default.fileExists(atPath: url.path) {
                let result = try store.loadWithMigration()
                presentMigrationNoticeIfNeeded(result.migrationNotice)
                let loaded = result.config
                config = loaded.normalizedBuiltInFeatureRules()
                if config != loaded {
                    try store.save(config)
                }
            } else {
                config = .bootstrap
                try store.save(config)
            }
            controlToken = try ControlTokenStore.ensureToken(at: try SumpterPaths.controlTokenURL())
            notificationAuthorizationStatus = await NativeNotifier.shared.authorizationStatus()
            sidecar.onUnexpectedExit = { [weak self] code in
                guard let self else { return }
                self.eventsTask?.cancel()
                self.eventsTask = nil
                self.countersRefreshTask?.cancel()
                self.countersRefreshTask = nil
                self.admin = nil
                self.sidecarState = .crashed(code)
                self.isProxyRunning = false
                self.engineGeneration = nil
                self.statusText = "引擎异常退出"
                self.lastError = "sumpterd 异常退出,code=\(code)"
                self.flash("代理引擎异常退出")
                self.health = ProxyHealthEvaluator.evaluate(
                    events: self.runtime.recentEvents,
                    isRunning: false
                )
            }
            refreshNotificationHookState()
            // hooks 已启用则启动时重写脚本:脚本模板升级(如 v2 转发 stdin)随 app
            // 更新自动生效,不用等用户碰通知开关或改端口。
            try? ClaudeNotificationHooks.rewriteScriptIfEnabled(port: config.listener.port)
            if codexNotificationsEnabled {
                try? CodexNotificationHooks.rewriteScriptIfEnabled(port: config.listener.port)
            }
            if grokNotificationsEnabled {
                try? GrokNotificationHooks.rewriteScriptIfEnabled(port: config.listener.port)
            }
            if codexNotificationsEnabled || grokNotificationsEnabled {
                refreshNotificationHookState()
            }
            refreshLoginItemStatus()
            if FileManager.default.fileExists(atPath: try SumpterPaths.autostartURL().path) {
                await startSidecar()
            }
            await refreshStatus()
        } catch {
            lastError = "\(error)"
            statusText = "初始化失败"
        }
    }

    private func persistConfigAndRefresh() async throws {
        let predecessor = configPersistenceTail
        let operation = Task { @MainActor [weak self] in
            if let predecessor {
                await predecessor.value
            }
            guard let self else { return }
            if self.store == nil {
                let url = try SumpterPaths.configURL()
                self.store = ConfigStore(url: url)
                self.configPath = url.path
            }
            self.config.normalizeBuiltInFeatureRules()
            if let store = self.store {
                let snapshot = self.config
                // 磁盘写挪出 MainActor,避免保存时界面卡顿。
                try await Task.detached(priority: .utility) {
                    try store.save(snapshot)
                }.value
            }
            try await self.pushConfigToSidecar()
            await self.refreshStatus()
        }
        configPersistenceTail = Task { _ = try? await operation.value }
        try await operation.value
    }

    private func deliverNotification(
        title: String,
        message: String,
        kind: String? = nil,
        category: String? = nil,
        sessionID: String? = nil,
        cwd: String? = nil,
        clientKind: String = "claude_code"
    ) async {
        do {
            // 来源前缀保证 Claude 与 Codex 即使复用 session id 也不会混组。
            let thread = "\(clientKind):\(sessionID ?? category ?? kind ?? "sumpter")"
            let subtitle = cwd.map { URL(fileURLWithPath: $0).lastPathComponent } ?? ""
            notificationAuthorizationStatus = try await NativeNotifier.shared.deliver(
                title: title,
                message: message,
                subtitle: subtitle,
                threadIdentifier: thread,
                soundPreference: notificationSoundPreference
            )
            notificationError = nil
        } catch {
            notificationAuthorizationStatus = await NativeNotifier.shared.authorizationStatus()
            notificationError = "\(error)"
            lastError = "\(error)"
        }
    }

    private func shouldDeliverNotification(category: String?) -> Bool {
        guard let category else { return true }
        return notificationCategoryEnabled(category)
    }

    func notificationCategoryEnabled(_ category: String) -> Bool {
        switch category {
        case "action_required": return actionNotificationsEnabled
        case "status": return statusNotificationsEnabled
        case "turn_completed": return turnCompletionNotificationsEnabled
        case "subtask_completed": return subtaskNotificationsEnabled
        case "turn_failed": return failureNotificationsEnabled
        default: return true
        }
    }

    private func notificationCategoryKey(_ category: String) -> String {
        switch category {
        case "action_required": return "notificationActionRequiredEnabled"
        case "status": return "notificationStatusEnabled"
        case "turn_completed": return "notificationTurnCompletedEnabled"
        case "subtask_completed": return "notificationSubtaskCompletedEnabled"
        case "turn_failed": return "notificationTurnFailedEnabled"
        default: return "notificationUnknownEnabled"
        }
    }

    /// 兜底刷新:周期轮询只拉 status/summary；统计页自己的 v3 快照独立加载。
    /// 首次加载、手动刷新、reset
    /// 或增量游标失效时才替换最新事件页，避免覆盖用户已加载的历史分页。
    /// 每类请求独立处理错误：analytics/storage 失败不能伪装成 daemon 不可达。
    private func refreshStatus(
        loadLatestEvents: Bool = false,
        reconcileEvents: Bool = false
    ) async {
        refreshRequestGeneration &+= 1
        let generation = refreshRequestGeneration
        guard sidecar.isRunning, let admin else {
            if isProxyRunning { isProxyRunning = false }
            if case .crashed = sidecarState {
                // 崩溃态由 onUnexpectedExit 设置,这里不覆盖。
            } else if sidecarState != .stopped {
                sidecarState = .stopped
            }
            if statusText != "启动失败" && statusText != "引擎异常退出" && statusText != "初始化失败" {
                if statusText != "已停止" { statusText = "已停止" }
            }
            let summary = ProxyHealthEvaluator.evaluate(events: runtime.recentEvents, isRunning: false)
            if summary != health { health = summary }
            return
        }
        let status: AdminWire.Status
        do {
            status = try await admin.status()
        } catch {
            guard generation == refreshRequestGeneration else { return }
            // Only the liveness request controls the unreachable state.
            if sidecarState != .unreachable { sidecarState = .unreachable }
            if statusText != "引擎无响应" { statusText = "引擎无响应" }
            let evaluated = ProxyHealthEvaluator.evaluate(events: runtime.recentEvents, isRunning: false)
            if evaluated != health { health = evaluated }
            return
        }
        guard generation == refreshRequestGeneration else { return }
        guard status.runtimeApiVersion == nil || status.runtimeApiVersion == Self.runtimeAPIVersion else {
            lastError = "daemon 运行统计 API 版本为 v\(status.runtimeApiVersion ?? -1)，App 需要 v\(Self.runtimeAPIVersion)。请同步升级。"
            return
        }

        var summary: AdminWire.RuntimeSummary?
        do {
            let value = try await admin.runtimeSummary()
            guard value.apiVersion == Self.runtimeAPIVersion else {
                throw AppModelError.invalidInput(
                    "运行统计 API 版本不匹配：需要 v\(Self.runtimeAPIVersion)，当前为 v\(value.apiVersion)。请同步升级。"
                )
            }
            summary = value
            runtimeSummaryError = nil
        } catch {
            guard generation == refreshRequestGeneration else { return }
            runtimeSummaryError = "\(error)"
        }

        var page: AdminWire.RuntimeEventPage?
        if loadLatestEvents || runtimePage == nil {
            do {
                page = try await admin.runtimeEvents(limit: 10)
                runtimeEventsError = nil
            } catch {
                guard generation == refreshRequestGeneration else { return }
                runtimeEventsError = "\(error)"
            }
        }
        guard generation == refreshRequestGeneration else { return }

        if let summary {
            let resetGenerationChanged = runtimeSummary.map {
                $0.resetGeneration != summary.resetGeneration
            } ?? false
            if resetGenerationChanged, page == nil {
                page = try? await admin.runtimeEvents(limit: 10)
                guard generation == refreshRequestGeneration else { return }
            }
            if resetGenerationChanged {
                runtimeChangeSeq = 0
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
            }
            let counters = summary.counters
            let existing = Dictionary(uniqueKeysWithValues: runtime.recentEvents.map { ($0.id, $0) })
            let mergedEvents = page?.events.map { $0.mergedRuntimeEvent(with: existing[$0.id]) } ?? runtime.recentEvents
            let snapshot = RuntimeSnapshot(
                clientRequests: counters.clientRequests,
                clientSuccesses: counters.clientSuccesses,
                clientFailures: counters.clientFailures,
                upstreamAttempts: counters.upstreamAttempts,
                upstreamSuccesses: counters.upstreamSuccesses,
                upstreamFailures: counters.upstreamFailures,
                failovers: counters.failovers,
                recentEvents: mergedEvents
            )
            if runtime != snapshot { runtime = snapshot }
            if runtimeSummary != summary { runtimeSummary = summary }
            if let page, runtimePage != page { runtimePage = page }
            if let page {
                let pageCursor = page.events.map(\.changeSeq).max() ?? 0
                runtimeChangeSeq = resetGenerationChanged ? pageCursor : max(runtimeChangeSeq, pageCursor)
            }
        }
        if !isProxyRunning { isProxyRunning = true }
        if sidecarState != .running { sidecarState = .running }
        if endpointCount != status.endpoints { endpointCount = status.endpoints }
        if engineGeneration != status.generation { engineGeneration = status.generation }
        // status.lastError is the daemon's own operational error only.
        if lastError != status.lastError { lastError = status.lastError }
        if statusText != "运行中" { statusText = "运行中" }
        let evaluatedHealth = ProxyHealthEvaluator.evaluate(events: runtime.recentEvents, isRunning: true)
        if evaluatedHealth != health { health = evaluatedHealth }

        if reconcileEvents && autoRefreshEnabled && !loadLatestEvents {
            await reconcileRuntimeChanges(using: admin)
        }
    }

    /// best-effort:把配置目录里可能含明文 key 的文件(含历史备份)收紧到 0600。
    private func tightenSensitiveFilePermissions() {
        guard let directory = try? SumpterPaths.appSupportDirectory() else {
            return
        }
        let manager = FileManager.default
        guard let entries = try? manager.contentsOfDirectory(atPath: directory.path) else {
            return
        }
        for name in entries {
            let isSensitive = name == "keys.json"
                || name.hasPrefix("app-config")
                || name.hasPrefix("keys")
            guard isSensitive, name.hasSuffix(".json") else {
                continue
            }
            let path = directory.appendingPathComponent(name).path
            try? manager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path)
        }
    }

    private func writeAutostartMarker() throws {
        let url = try SumpterPaths.autostartURL()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try "1".write(to: url, atomically: true, encoding: .utf8)
    }

    private func removeAutostartMarker() throws {
        let url = try SumpterPaths.autostartURL()
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
    }

    private func uniqueEndpointID(_ name: String) -> String {
        let base = name
            .lowercased()
            .map { char in
                char.isLetter || char.isNumber ? char : "-"
            }
            .reduce(into: "") { $0.append($1) }
            .split(separator: "-")
            .joined(separator: "-")
        let prefix = base.isEmpty ? "provider" : base
        let existing = Set(config.endpoints.map(\.id))
        if !existing.contains(prefix) {
            return prefix
        }
        var index = 2
        while existing.contains("\(prefix)-\(index)") {
            index += 1
        }
        return "\(prefix)-\(index)"
    }

    private func modelCatalogTimestamp() -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter.string(from: Date())
    }

    /// Admin API 返回 Unix 秒数字符串；配置目录在 macOS UI 中沿用可读的
    /// 本地时间格式，避免把 `1786233600` 直接展示给用户。
    private func modelCatalogTimestamp(_ raw: String) -> String {
        guard let seconds = Double(raw), seconds.isFinite, seconds > 0 else {
            return raw.isEmpty ? modelCatalogTimestamp() : raw
        }
        let date = Date(timeIntervalSince1970: seconds)
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter.string(from: date)
    }

    private func parseCSV(_ text: String) -> [String] {
        text
            .split { $0 == "," || $0 == "\n" || $0 == "\t" || $0 == " " }
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }

    private func nilIfBlank(_ text: String) -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
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
