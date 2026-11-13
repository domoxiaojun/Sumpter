import SwiftUI

struct SettingsRootView: View {
    // 详情页各自观察 AppModel。根导航不订阅全量运行状态，避免每次
    // SSE/轮询更新都重新计算左侧 List 和 NavigationSplitView。
    let model: AppModel
    @ObservedObject private var runtimeViewState: RuntimeViewState
    @ObservedObject private var windowVisibility: SumpterWindowVisibility
    @State private var selection: SettingsSection? = .run
    @State private var columnVisibility: NavigationSplitViewVisibility = .all
    @State private var wideColumnVisibility: NavigationSplitViewVisibility = .all
    @Environment(\.colorScheme) private var systemColorScheme
    @AppStorage("sumpterAppearanceMode") private var appearanceModeRaw = SumpterAppearanceMode.system.rawValue

    init(model: AppModel, windowVisibility: SumpterWindowVisibility = SumpterWindowVisibility()) {
        self.model = model
        self._runtimeViewState = ObservedObject(wrappedValue: model.runtimeViewState)
        self._windowVisibility = ObservedObject(wrappedValue: windowVisibility)
    }

    private var appearanceMode: SumpterAppearanceMode {
        SumpterAppearanceMode(rawValue: appearanceModeRaw) ?? .system
    }

    private var resolvedColorScheme: ColorScheme {
        switch appearanceMode {
        case .system:
            systemColorScheme
        case .light:
            .light
        case .dark:
            .dark
        }
    }

    private var palette: SumpterTheme.Palette {
        SumpterTheme.palette(for: resolvedColorScheme)
    }

    var body: some View {
        GeometryReader { proxy in
            NavigationSplitView(columnVisibility: $columnVisibility) {
                List(SettingsSection.allCases, selection: $selection) { section in
                    NavigationLink(value: section) {
                        Label(section.title, systemImage: section.systemImage)
                            .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                    }
                    .accessibilityLabel(section.title)
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)
                .background(palette.sidebar)
                .navigationTitle("Sumpter")
                .navigationSplitViewColumnWidth(min: 180, ideal: 200, max: 240)
            } detail: {
                Group {
                    if runtimeViewState.retainsPageContent || ![SettingsSection.run, .statistics].contains(selection ?? .run) {
                        detailView(for: selection ?? .run)
                    } else {
                        Color.clear
                    }
                }
                .navigationTitle((selection ?? .run).title)
            }
            .navigationSplitViewStyle(.balanced)
            // Collapse only when crossing the breakpoint. The native sidebar
            // button remains available, and resizing never recreates the detail.
            .onChange(of: proxy.size.width < 980, initial: true) { _, compact in
                if compact {
                    wideColumnVisibility = columnVisibility
                    columnVisibility = .detailOnly
                } else {
                    columnVisibility = wideColumnVisibility
                }
            }
        }
        .onAppear {
            if selection == nil {
                selection = .run
            }
        }
        .onChange(of: selection) { _, value in model.setRuntimeSection(value ?? .run) }
        .tint(palette.brand)
        .background(palette.canvas)
        .environment(\.sumpterPalette, palette)
        .environment(\.sumpterWindowVisible, windowVisibility.isVisible)
        .preferredColorScheme(appearanceMode.preferredColorScheme)
        .toolbar {
            ToolbarItem(placement: .automatic) {
                Menu {
                    Picker("主题", selection: $appearanceModeRaw) {
                        ForEach(SumpterAppearanceMode.allCases) { mode in
                            Label(mode.title, systemImage: mode.icon)
                                .tag(mode.rawValue)
                        }
                    }
                } label: {
                    Label("外观：\(appearanceMode.title)", systemImage: appearanceMode.icon)
                }
                .help("切换与 Linux WebUI 一致的浅色/深色语义主题")
            }
        }
        // Keep the sidebar/detail split usable in a narrow window. Wide tables
        // own their horizontal scroll region; the shell itself must not force
        // an unnecessarily large minimum that prevents side-by-side parity
        // checks with the responsive Linux dashboard.
        .frame(minWidth: 720, minHeight: 540)
    }

    @ViewBuilder
    private func detailView(for section: SettingsSection) -> some View {
        switch section {
        case .run:
            OverviewPane(model: model)
        case .providers:
            ProvidersPane(model: model)
        case .modelGroups:
            ModelGroupsPane(model: model)
        case .routing:
            RoutingPane(model: model)
        case .security:
            SecurityPane(model: model)
        case .notifications:
            NotificationsPane(model: model)
        case .statistics:
            UsagePane(model: model)
        case .diagnostics:
            DiagnosticsPane(model: model)
        case .help:
            HelpPane(model: model) { destination in
                selection = destination
            }
        case .about:
            AboutPane(model: model)
        }
    }
}
