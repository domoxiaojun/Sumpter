//! Small derived counters; analytics never scan large event payloads.
use crate::database::{Connection, params};
use sumpter_core::events::{ClientKind, RuntimeEvent, codex_request_kind};

pub(crate) fn ensure(connection: &Connection) -> crate::database::Result<()> {
    let existed: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='runtime_codex_requests' AND type='table')",
        params![], |row| row.get(0),
    )?;
    if !existed {
        connection.execute(
            "DELETE FROM runtime_meta WHERE key='codex_role_revision'",
            params![],
        )?;
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS runtime_codex_requests (
        event_seq INTEGER PRIMARY KEY REFERENCES runtime_events(seq) ON DELETE CASCADE,
        prewarm INTEGER NOT NULL, turn INTEGER NOT NULL, compaction INTEGER NOT NULL,
        memory INTEGER NOT NULL, unknown INTEGER NOT NULL,
        unobserved_connections INTEGER NOT NULL, unobserved_frames INTEGER NOT NULL
    );
    CREATE TRIGGER IF NOT EXISTS runtime_codex_requests_delete AFTER DELETE ON runtime_events
    BEGIN DELETE FROM runtime_codex_requests WHERE event_seq=OLD.seq; END;",
    )
}

pub(crate) fn write(
    connection: &Connection,
    seq: i64,
    event: &RuntimeEvent,
) -> crate::database::Result<()> {
    if event.kind != "client"
        || (event.client_kind != Some(ClientKind::Codex) && event.codex_metadata.is_none())
    {
        connection.execute(
            "DELETE FROM runtime_codex_requests WHERE event_seq=?1",
            [seq],
        )?;
        return Ok(());
    }
    let ws = event
        .stream_trace
        .as_ref()
        .and_then(|trace| trace.websocket_trace.as_ref());
    let summary = ws.and_then(|trace| trace.requests.as_ref());
    let counts = ["prewarm", "turn", "compaction", "memory", "unknown"].map(|kind| {
        if let Some(summary) = summary {
            summary
                .counts
                .get(kind)
                .copied()
                .unwrap_or(0)
                .min(i64::MAX as u64) as i64
        } else if ws.is_none() {
            i64::from(codex_request_kind(event.codex_metadata.as_ref()) == kind)
        } else {
            0
        }
    });
    connection.execute("INSERT INTO runtime_codex_requests VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
        ON CONFLICT(event_seq) DO UPDATE SET prewarm=excluded.prewarm,turn=excluded.turn,compaction=excluded.compaction,memory=excluded.memory,unknown=excluded.unknown,unobserved_connections=excluded.unobserved_connections,unobserved_frames=excluded.unobserved_frames",
        params![seq,counts[0],counts[1],counts[2],counts[3],counts[4],i64::from(ws.is_some() && summary.is_none()),summary.map_or(0,|s|s.unobserved_frames.min(i64::MAX as u64) as i64)])?;
    Ok(())
}
