import AppKit
import Combine
import Foundation
import SumpterCore

@MainActor
final class RuntimeViewState: ObservableObject {
    @Published var windowVisible = false
    @Published var retainsPageContent = true
    var section: SettingsSection = .run
}

@MainActor
extension AppModel {
    var runPageVisible: Bool { runtimeViewState.windowVisible && runtimeViewState.section == .run }
    var statisticsPageVisible: Bool { runtimeViewState.windowVisible && runtimeViewState.section == .statistics }
    var runtimeDataVisible: Bool { runPageVisible || statisticsPageVisible }

    func setRuntimeSection(_ section: SettingsSection) {
        guard runtimeViewState.section != section else { return }
        cancelRuntimePageRequests()
        runtimeViewState.section = section
        setStatisticsVisible(section == .statistics && runtimeViewState.windowVisible)
    }

    func setRuntimeWindowVisible(_ visible: Bool) {
        guard runtimeViewState.windowVisible != visible else { return }
        runtimeCacheReleaseTask?.cancel()
        runtimeCacheReleaseTask = nil
        runtimeViewState.windowVisible = visible
        if visible {
            runtimeViewState.retainsPageContent = true
            setStatisticsVisible(runtimeViewState.section == .statistics)
        } else {
            cancelRuntimePageRequests()
            setStatisticsVisible(false)
            runtimeCacheReleaseTask = Task { [weak self] in
                do { try await Task.sleep(for: .seconds(60)) } catch { return }
                guard let self, !Task.isCancelled, !self.runtimeViewState.windowVisible else { return }
                self.releaseRuntimePageCaches()
            }
        }
        startStatusPolling()
    }

    func observeRuntimeMemoryPressure() {
        let source = DispatchSource.makeMemoryPressureSource(eventMask: [.warning, .critical], queue: .main)
        source.setEventHandler { [weak self] in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.runtimeEventDetails.removeAll()
                if !self.runtimeViewState.windowVisible { self.releaseRuntimePageCaches() }
            }
        }
        runtimeMemoryPressureSource = source
        source.resume()
    }

    func cancelRuntimePageRequests() {
        refreshRequestGeneration &+= 1
        statusRefreshTask?.cancel()
        statusRefreshTask = nil
        statusRefreshPending = false
        statusRefreshWantsLatest = false
        statusRefreshWantsReconcile = false
        countersRefreshTask?.cancel()
        countersRefreshTask = nil
        runHistoryAutoRefreshTask?.cancel()
        runHistoryAutoRefreshTask = nil
        runHistoryTask?.cancel()
        runHistoryTask = nil
        runHistoryRefreshPending = false
        runHistoryRequestGeneration &+= 1
        runHistoryLoading = false
        runOverviewTask?.cancel()
        runOverviewTask = nil
        runOverviewRefreshPending = false
        runOverviewRequestGeneration &+= 1
        runtimeEventDetailTask?.cancel()
        runtimeEventDetailTask = nil
        detailRequestGeneration &+= 1
        cancelStatisticsRequests()
        runtimeV2RequestGeneration &+= 1
        runtimeFacetsRequestGeneration &+= 1
        stopRuntimeRequestChainRefresh()
        runtimeV2Loading = false
        runtimeHistoryLoading = false
    }

    func releaseRuntimePageCaches() {
        guard !runtimeViewState.windowVisible else { return }
        cancelRuntimePageRequests()
        runtimeViewState.retainsPageContent = false
        runtimeEventDetails.removeAll()
        runtimeEventDetail = nil
        runtimePage = nil
        runtime.recentEvents = []
        runtimeSummary = nil
        runHistoryPage = nil
        runHistoryError = nil
        resetRunOverview()
        clearRuntimeV2Snapshot()
        runtimeFacets = nil
        runtimeAnalytics = nil
        runtimeStorageProbe = nil
        runtimePricing = nil
        lastRuntimeFacetsRefreshAt = nil
        lastRuntimeV2RefreshAt = nil
        runtimeFacetsDirty = true
        runtimeChangeSeq = 0
    }

    func applyDaemonStatus(_ status: AdminWire.Status) {
        if isProxyRunning != status.running { isProxyRunning = status.running }
        if sidecarState != .running { sidecarState = .running }
        if endpointCount != status.endpoints { endpointCount = status.endpoints }
        if engineGeneration != status.generation { engineGeneration = status.generation }
        if lastError != status.lastError { lastError = status.lastError }
        if statusText != "运行中" { statusText = "运行中" }
        let value = status.health
        let state = status.running ? (ProxyHealth(rawValue: value.state) ?? .idle) : .stopped
        let percent = Int(((value.successRate ?? 0) * 100).rounded())
        let headline: String
        switch state {
        case .stopped: headline = "已停止"
        case .idle: headline = "运行中 · 暂无有效请求"
        case .healthy: headline = "正常 · 近 \(value.sampleCount) 请求 \(percent)% 成功"
        case .degraded: headline = "降级 · 近 \(value.sampleCount) 请求 \(percent)% 成功"
        case .down: headline = "上游全部失败 · 近 \(value.sampleCount) 请求 0% 成功"
        }
        let next = ProxyHealthSummary(state: state, considered: value.sampleCount,
            successes: Int(((value.successRate ?? 0) * Double(value.sampleCount)).rounded()),
            successRate: value.successRate, lastSuccess: value.lastSuccess, headline: headline)
        if health != next { health = next }
    }
}
