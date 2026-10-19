//! Constant-size storage totals plus indexed event boundaries. This auxiliary
//! structure is compatible with existing event schemas and never changes the
//! public storage contract or the event retention policy.

use crate::database::{Connection, Result, params};

const TRIGGERS: &str = include_str!("storage_metrics/triggers.sql");
pub(crate) const SELECT: &str = include_str!("storage_metrics/select.sql");

#[derive(Debug, PartialEq)]
pub(crate) struct StorageMetrics {
    pub event_count: i64,
    pub completed_events: i64,
    pub in_flight_events: i64,
    pub payload_bytes: i64,
    pub min_seq: Option<i64>,
    pub max_seq: Option<i64>,
    pub earliest_timestamp: Option<f64>,
    pub latest_timestamp: Option<f64>,
}

/// Bootstrap only when the singleton or one of its triggers is missing. In
/// particular, an older binary may have dropped/recreated runtime_events while
/// leaving this auxiliary table behind; checking the triggers detects that case.
/// DDL, the baseline scan and trigger installation commit or roll back together.
pub(crate) fn ensure(connection: &mut Connection) -> Result<()> {
    use crate::entities::runtime_storage_metrics;
    use sea_orm::{ConnectionTrait, DbBackend, Schema, sea_query::Expr};

    let transaction = connection.transaction()?;
    transaction.orm(|db| async move {
        let mut table = Schema::new(DbBackend::Sqlite)
            .create_table_from_entity(runtime_storage_metrics::Entity);
        table.check(Expr::col(runtime_storage_metrics::Column::Id).eq(1));
        db.execute(DbBackend::Sqlite.build(table.if_not_exists()))
            .await
            .map(|_| ())
    })?;
    let ready = transaction.query_row(
        "SELECT (SELECT COUNT(*) FROM sqlite_master
                    WHERE type='trigger' AND tbl_name='runtime_events' AND name IN (
                        'runtime_storage_metrics_insert_v1',
                        'runtime_storage_metrics_update_v1',
                        'runtime_storage_metrics_delete_v1')) = 3
                AND EXISTS(SELECT 1 FROM runtime_storage_metrics WHERE id=1)",
        params![],
        |row| row.get::<_, bool>(0),
    )?;
    if !ready {
        transaction.execute_batch(
            "INSERT INTO runtime_storage_metrics
                 (id,event_count,completed_events,in_flight_events,payload_bytes)
             SELECT 1,COUNT(*),
                    COALESCE(SUM(CASE WHEN is_in_flight=0 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN is_in_flight=1 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(payload_bytes),0)
             FROM runtime_events WHERE true
             ON CONFLICT(id) DO UPDATE SET
                 event_count=excluded.event_count,
                 completed_events=excluded.completed_events,
                 in_flight_events=excluded.in_flight_events,
                 payload_bytes=excluded.payload_bytes;",
        )?;
        transaction.execute_batch(TRIGGERS)?;
    }
    transaction.commit()
}

/// One SQL statement so the totals and four index boundary lookups share a
/// snapshot even when the writer calls this without an explicit read transaction.
pub(crate) fn read(connection: &Connection) -> Result<StorageMetrics> {
    connection.query_row(SELECT, params![], |row| {
        Ok(StorageMetrics {
            event_count: row.get(0)?,
            completed_events: row.get(1)?,
            in_flight_events: row.get(2)?,
            payload_bytes: row.get(3)?,
            min_seq: row.get(4)?,
            max_seq: row.get(5)?,
            earliest_timestamp: row.get(6)?,
            latest_timestamp: row.get(7)?,
        })
    })
}

#[cfg(test)]
pub(crate) mod tests;
