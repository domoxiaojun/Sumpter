//! Awaitable public queries. SQL/cursor work is confined to the bounded runtime executor.
use super::*;

pub async fn events_page(path: PathBuf, request: EventPageQuery) -> QueryResult<EventPage> {
    crate::query_executor::run(move || super::events_page(&path, &request))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn trends(path: PathBuf, request: TrendQuery) -> QueryResult<TrendSeries> {
    crate::query_executor::run(move || super::trends(&path, &request))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn facets(path: PathBuf, filter: RuntimeFilter) -> QueryResult<RuntimeFacetSnapshot> {
    crate::query_executor::run(move || super::facets(&path, &filter))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn error_groups(path: PathBuf, request: ErrorPageQuery) -> QueryResult<ErrorPage> {
    crate::query_executor::run(move || super::error_groups(&path, &request))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn storage_details(path: PathBuf) -> QueryResult<StorageProbe> {
    crate::query_executor::run(move || super::storage_details(&path))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn pricing(path: PathBuf) -> QueryResult<Pricing> {
    crate::query_executor::run(move || super::pricing(&path))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn export_estimate(path: PathBuf, query: ExportQuery) -> QueryResult<ExportEstimate> {
    crate::query_executor::run(move || super::export_estimate(&path, &query))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn analytics(
    path: PathBuf,
    range: String,
    filter: RuntimeFilter,
) -> QueryResult<AnalyticsSummary> {
    crate::query_executor::run(move || super::analytics(&path, &range, &filter))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn dimension_page(
    path: PathBuf,
    kind: DimensionKind,
    query: DimensionPageQuery,
) -> QueryResult<DimensionPage> {
    crate::query_executor::run(move || super::dimension_page(&path, kind, &query))
        .await
        .map_err(RuntimeQueryError::Output)?
}

pub async fn request_chain_with_recent(
    path: PathBuf,
    request_id: String,
    recent: Vec<RuntimeChange>,
) -> QueryResult<RequestChain> {
    crate::query_executor::run(move || {
        super::request_chain_with_recent(&path, &request_id, &recent)
    })
    .await
    .map_err(RuntimeQueryError::Output)?
}

pub async fn stream_export<F>(
    path: PathBuf,
    query: ExportQuery,
    sink: F,
) -> QueryResult<ExportManifest>
where
    F: FnMut(Vec<u8>) -> Result<(), String> + Send + 'static,
{
    crate::query_executor::run(move || super::stream_export(&path, &query, sink))
        .await
        .map_err(RuntimeQueryError::Output)?
}
