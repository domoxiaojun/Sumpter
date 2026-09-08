import AppKit
import Foundation
import ServiceManagement
import SwiftUI
import UserNotifications
import SumpterCore

@MainActor
extension AppModel {
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

    func setRuntimeAnalyticsFilters(clientKind: String? = nil, clientVariant: String? = nil, agentRole: String? = nil, agentName: String? = nil, parentThreadID: String? = nil, parentTurnID: String? = nil, rootTurnID: String? = nil, endpointID: String? = nil, project: String? = nil, sessionID: String? = nil, model: String? = nil, requestPurpose: String? = nil, outcome: String? = nil, failureKind: String? = nil, failurePhase: String? = nil) {
        if let clientKind { runtimeAnalyticsClientKind = clientKind }
        if let clientVariant { runtimeAnalyticsClientVariant = clientVariant }
        if let agentRole { runtimeAnalyticsAgentRole = agentRole }
        if let agentName { runtimeAnalyticsAgentName = agentName }
        if let parentThreadID { runtimeAnalyticsParentThreadID = parentThreadID }
        if let parentTurnID { runtimeAnalyticsParentTurnID = parentTurnID }
        if let rootTurnID { runtimeAnalyticsRootTurnID = rootTurnID }
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

    func resetRuntimeLocalProjectState() {
        runtimeLocalProjectID = ""
        runtimeLocalProjectName = ""
    }

    func resetRuntimeLocalSessionState() {
        runtimeLocalSessionID = ""
        runtimeLocalSessionName = ""
    }

    func resetRuntimeLocalDrillDownState() {
        resetRuntimeLocalProjectState()
        resetRuntimeLocalSessionState()
    }

    func reloadRuntimeAnalytics() {
        // Compatibility callers may still use this method name, but the old
        // full-table analytics request is intentionally retired from the macOS
        // UI. Keep the call latest-wins and use the v3 projections instead.
        reloadRuntimeFacets()
        refreshRuntimeV2(resetSnapshot: true)
    }

    /// Refresh only the picker dimensions.  This request is deliberately
    /// independent from the compatibility aggregate so a slow high-cardinality
    /// table cannot block selectors or the automatic status loop.
    func reloadRuntimeFacets() {
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

    func clearInvalidAnalyticsFacetFilters(_ snapshot: AdminWire.RuntimeFacetSnapshot) {
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
    func clearInvalidAnalyticsFilters(_ analytics: AdminWire.RuntimeAnalytics) {
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

    var runHistoryFilter: AdminWire.RuntimeFilter {
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

    func runtimeV2Filter(includeLocalProject: Bool = false) -> AdminWire.RuntimeFilter {
        let localTodayStart = Calendar.autoupdatingCurrent
            .startOfDay(for: Date())
            .timeIntervalSinceReferenceDate
        let base = AdminWire.RuntimeFilter(
            outcome: runtimeAnalyticsOutcome.isEmpty ? nil : runtimeAnalyticsOutcome,
            clientKind: runtimeAnalyticsClientKind.isEmpty ? nil : runtimeAnalyticsClientKind,
            clientVariant: runtimeAnalyticsClientVariant.isEmpty ? nil : runtimeAnalyticsClientVariant,
            agentRole: runtimeAnalyticsAgentRole.isEmpty ? nil : runtimeAnalyticsAgentRole,
            agentName: runtimeAnalyticsAgentName.isEmpty ? nil : runtimeAnalyticsAgentName,
            parentThreadID: runtimeAnalyticsParentThreadID.isEmpty ? nil : runtimeAnalyticsParentThreadID,
            parentTurnID: runtimeAnalyticsParentTurnID.isEmpty ? nil : runtimeAnalyticsParentTurnID,
            rootTurnID: runtimeAnalyticsRootTurnID.isEmpty ? nil : runtimeAnalyticsRootTurnID,
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

    func refreshRuntimeV2Async(
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

    func runtimeTrendIfNeeded(
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

    func runtimeStorageIfNeeded(
        for board: String,
        using admin: AdminClient
    ) async throws -> AdminWire.RuntimeStorageProbe? {
        guard board == "overview" else { return nil }
        return try await admin.runtimeStorage()
    }

    func runtimePricingIfNeeded(
        for board: String,
        using admin: AdminClient
    ) async throws -> AdminWire.RuntimePricing? {
        guard board == "tokens" else { return nil }
        return try await admin.runtimePricing()
    }

    func runtimeDiagnosticAnalyticsIfNeeded(
        for board: String,
        using admin: AdminClient
    ) async throws -> AdminWire.RuntimeAnalytics? {
        guard board == "errors" else { return nil }
        return try await admin.runtimeAnalytics(range: runtimeAnalyticsRange, filter: runtimeV2Filter())
    }

    func fetchRuntimeHistoryPage(
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

    func applyRuntimeHistoryPage(_ page: AdminWire.RuntimeHistoryPage) {
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

    func loadRuntimeV2Board(_ board: String, force: Bool) {
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

    static let runtimeDimensionKinds = [
        "endpoint", "model", "clientKind", "clientVariant", "agentRole", "agentName", "parentThread", "parentTurn", "rootTurn", "purpose", "failureKind",
        "failurePhase", "protocol", "streamTerminal", "project", "session",
    ]

    static func runtimeDimensionWireKind(_ kind: String) -> String {
        switch kind {
        case "clientKind": "client_kind"
        case "clientVariant": "client_variant"
        case "agentRole": "agent_role"
        case "agentName": "agent_name"
        case "parentThread": "parent_thread_id"
        case "parentTurn": "parent_turn_id"
        case "rootTurn": "root_turn_id"
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
    func loadRuntimeDimensionsForBoard() {
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

    enum RuntimeDimension { case endpoint, project, session, model }

    func loadRuntimeDimension(kind: RuntimeDimension, page: Int) {
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

    func isRuntimeSnapshotError(_ error: Error) -> Bool {
        guard let error = error as? AdminClient.AdminError else { return false }
        return error.serverCode == "runtime_snapshot_expired"
            || error.serverCode == "runtime_snapshot_trimmed"
    }

    func clearRuntimeV2Snapshot(expectedGeneration: Int? = nil) {
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
}
