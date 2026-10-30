import Foundation

// Keep shared defaults outside main.swift so library/test hosts initialize
// them lazily without running the application entry point.
let autoRefreshEnabledDefaultsKey = "runtimeAutoRefreshEnabled"
let runtimeAutoRefreshIntervalDefaultsKey = "runtimeAutoRefreshIntervalSeconds"
let statisticsAutoRefreshIntervalDefaultsKey = "statisticsAutoRefreshIntervalSeconds"
let runHistoryPageSizeDefaultsKey = "runtimeRunHistoryPageSize"
let runtimeHistoryPageSizeDefaultsKey = "runtimeStatisticsHistoryPageSize"
let runtimeErrorPageSizeDefaultsKey = "runtimeStatisticsErrorPageSize"
let runtimeDimensionPageSizeDefaultsKey = "runtimeStatisticsDimensionPageSize"
let runtimeEndpointPageSizeDefaultsKey = "runtimeStatisticsEndpointPageSize"
let runtimeProjectPageSizeDefaultsKey = "runtimeStatisticsProjectPageSize"
let runtimeSessionPageSizeDefaultsKey = "runtimeStatisticsSessionPageSize"
let runtimeModelPageSizeDefaultsKey = "runtimeStatisticsModelPageSize"
let autoRefreshIntervalChoices: [Double] = [1, 2, 5, 10, 15, 30]
