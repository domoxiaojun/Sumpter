//! Runtime query export domain.

use rusqlite::params_from_iter;

use super::{
    API_VERSION, Connection, ExportEstimate, ExportEventRow, ExportFormat, ExportManifest,
    ExportPrivacy, ExportQuery, ExportScope, PROJECTION_VERSION, Path, QueryResult, RuntimeFilter,
    RuntimeQueryError, SqlFilter, Transaction, TransactionBehavior, append_runtime_filter,
    history_snapshot, read_connection, require_projection,
};

pub fn export_estimate(path: &Path, query: &ExportQuery) -> QueryResult<ExportEstimate> {
    let mut connection = read_connection(path)?;
    export_estimate_on(&mut connection, query)
}

pub fn export_estimate_on(
    connection: &mut Connection,
    query: &ExportQuery,
) -> QueryResult<ExportEstimate> {
    let filters = query.filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(&transaction, query.snapshot_seq, query.history_generation)?;
    let mut builder = export_filter(&filters, snapshot.snapshot_seq, query.scope);
    let where_sql = builder.where_sql();
    let (row_count, source_bytes) = match query.scope {
        ExportScope::Events => transaction.query_row(
            &format!(
                "SELECT COUNT(*),COALESCE(SUM(payload_bytes),0) FROM runtime_events{where_sql}"
            ),
            params_from_iter(builder.values.iter()),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?.max(0) as u64)),
        )?,
        ExportScope::Projects | ExportScope::Sessions => {
            let (key, name, source) = export_dimension_expressions(query.scope);
            let count = transaction.query_row(
                &format!(
                    "SELECT COUNT(*) FROM (SELECT 1 FROM runtime_events{where_sql} \
                     GROUP BY {key},{name},{source})"
                ),
                params_from_iter(builder.values.iter()),
                |row| row.get::<_, i64>(0),
            )?;
            (count, (count.max(0) as u64).saturating_mul(512))
        }
    };
    let format_overhead = match query.format {
        ExportFormat::Csv => 192_u64,
        ExportFormat::Jsonl => 256_u64,
    };
    let estimated_bytes =
        if query.scope == ExportScope::Events && query.privacy == ExportPrivacy::Stored {
            source_bytes.saturating_add((row_count.max(0) as u64).saturating_mul(format_overhead))
        } else {
            (row_count.max(0) as u64).saturating_mul(format_overhead.max(384))
        };
    // Keep the builder live through the count call only; it can contain a
    // large number of owned filter strings but never row data.
    builder.values.clear();
    transaction.commit()?;
    Ok(ExportEstimate {
        api_version: API_VERSION,
        scope: query.scope,
        format: query.format,
        privacy: query.privacy,
        privacy_scope: export_privacy_scope(query.privacy),
        row_count,
        estimated_bytes,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        retained_from_seq: snapshot.retained_from_seq,
    })
}

pub fn stream_export<F>(path: &Path, query: &ExportQuery, sink: F) -> QueryResult<ExportManifest>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    let mut connection = read_connection(path)?;
    stream_export_on(&mut connection, query, sink)
}

// Kept separate from the path-opening wrapper so the estimate/stream contract
// can be exercised against one read transaction in unit tests. Production
// callers still use `stream_export`, which opens the database read-only.
pub(super) fn stream_export_on<F>(
    connection: &mut Connection,
    query: &ExportQuery,
    mut sink: F,
) -> QueryResult<ExportManifest>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    if query.privacy == ExportPrivacy::Stored && !query.confirm_stored {
        return Err(RuntimeQueryError::InvalidInput(
            "privacy=stored requires confirmStored=true".into(),
        ));
    }
    let filters = query.filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(&transaction, query.snapshot_seq, query.history_generation)?;
    let builder = export_filter(&filters, snapshot.snapshot_seq, query.scope);
    let mut row_count = 0_i64;
    let mut bytes_written = 0_u64;
    if query.format == ExportFormat::Csv {
        let header = match query.scope {
            ExportScope::Events if query.privacy == ExportPrivacy::Stored => {
                "seq,changeSeq,eventID,requestID,timestamp,kind,payloadJSON\n"
            }
            ExportScope::Events => {
                "seq,changeSeq,eventID,requestID,timestamp,kind,outcome,statusCode,clientKind,requestPurpose,endpointID,endpointName,model,projectID,projectName,sessionID,failureKind,failurePhase,upstreamStatusCode,durationMS,ttfbMS,failover,inputTokens,outputTokens,cacheReadInputTokens,cacheCreationInputTokens,reasoningTokens,processedInputTokens,processedTotalTokens,clientVariant,agentRole,agentName,parentThreadID,parentTurnID,rootTurnID\n"
            }
            ExportScope::Projects | ExportScope::Sessions => {
                "key,name,source,requests,successes,failures,cancelled,failovers,inputTokens,outputTokens,cacheReadInputTokens,cacheCreationInputTokens,processedTotalTokens,firstSeen,lastSeen,relatedCount,workspacePaths\n"
            }
        };
        export_send(&mut sink, header.as_bytes().to_vec(), &mut bytes_written)?;
    }
    match query.scope {
        ExportScope::Events => stream_event_rows(
            &transaction,
            &builder,
            query,
            &mut sink,
            &mut row_count,
            &mut bytes_written,
        )?,
        ExportScope::Projects | ExportScope::Sessions => stream_dimension_rows(
            &transaction,
            &builder,
            query,
            &mut sink,
            &mut row_count,
            &mut bytes_written,
        )?,
    }
    transaction.commit()?;
    Ok(ExportManifest {
        row_count,
        bytes_written,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        privacy: query.privacy,
    })
}

fn export_filter(filter: &RuntimeFilter, snapshot_seq: i64, scope: ExportScope) -> SqlFilter {
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.raw(format!("projection_version = {PROJECTION_VERSION}"));
    if scope != ExportScope::Events {
        builder.raw("kind = 'client'");
        if scope == ExportScope::Projects {
            builder.raw("COALESCE(attribution_scope,'unknown') != 'internal_feature'");
        }
    }
    builder.le_i64("seq", snapshot_seq);
    append_runtime_filter(&mut builder, filter);
    builder
}

fn export_dimension_expressions(scope: ExportScope) -> (&'static str, &'static str, &'static str) {
    match scope {
        ExportScope::Projects => (
            "COALESCE(project_id,'unidentified_project')",
            "COALESCE(project_name,project_id,'unidentified_project')",
            "COALESCE(project_source,'missing_workspace_metadata')",
        ),
        ExportScope::Sessions => (
            "COALESCE(session_key,'unidentified_session')",
            "COALESCE(session_key,'unidentified_session')",
            "COALESCE(session_source,'missing_session_metadata')",
        ),
        ExportScope::Events => unreachable!("events are not a grouped dimension"),
    }
}

fn export_privacy_scope(privacy: ExportPrivacy) -> &'static str {
    match privacy {
        ExportPrivacy::Redacted => {
            "stable masks for event/request/session IDs; workspace locators removed"
        }
        ExportPrivacy::Stored => {
            "SQLite-stored runtime fields only; no prompt, body, headers, credentials, or full paths"
        }
    }
}

fn stream_event_rows<F>(
    transaction: &Transaction<'_>,
    builder: &SqlFilter,
    query: &ExportQuery,
    sink: &mut F,
    row_count: &mut i64,
    bytes_written: &mut u64,
) -> QueryResult<()>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    let stored = query.privacy == ExportPrivacy::Stored;
    // Redacted exports are assembled entirely from normalized columns.  Do
    // not select the potentially large detail blob unless the caller has
    // explicitly confirmed a stored export.
    let payload_column = if stored { ",payload_json" } else { "" };
    let sql = format!(
        "SELECT seq,change_seq,event_id,request_id,timestamp,kind,outcome,status_code,\
                client_kind,request_purpose,endpoint_id,endpoint_name,effective_model,\
                project_id,project_name,session_key,failure_kind,failure_phase,\
                upstream_status_code,duration_ms,ttfb_ms,failover,input_tokens,output_tokens,\
                cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,\
                processed_input_tokens,processed_total_tokens,client_variant,agent_role,agent_name,parent_thread_id,parent_turn_id,root_turn_id{payload_column} \
         FROM runtime_events{} ORDER BY seq ASC",
        builder.where_sql()
    );
    let mut statement = transaction.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(builder.values.iter()))?;
    while let Some(row) = rows.next()? {
        let event = ExportEventRow {
            seq: row.get(0)?,
            change_seq: row.get(1)?,
            event_id: row.get(2)?,
            request_id: row.get(3)?,
            timestamp: row.get(4)?,
            kind: row.get(5)?,
            outcome: row.get(6)?,
            status_code: row.get(7)?,
            client_kind: row.get(8)?,
            request_purpose: row.get(9)?,
            endpoint_id: row.get(10)?,
            endpoint_name: row.get(11)?,
            effective_model: row.get(12)?,
            project_id: row.get(13)?,
            project_name: row.get(14)?,
            session_key: row.get(15)?,
            failure_kind: row.get(16)?,
            failure_phase: row.get(17)?,
            upstream_status_code: row.get(18)?,
            duration_ms: row.get(19)?,
            ttfb_ms: row.get(20)?,
            failover: row.get::<_, Option<i64>>(21)?.unwrap_or(0) != 0,
            input_tokens: row.get(22)?,
            output_tokens: row.get(23)?,
            cache_read_input_tokens: row.get(24)?,
            cache_creation_input_tokens: row.get(25)?,
            reasoning_tokens: row.get(26)?,
            processed_input_tokens: row.get(27)?,
            processed_total_tokens: row.get(28)?,
            client_variant: row.get(29)?,
            agent_role: row.get(30)?,
            agent_name: row.get(31)?,
            parent_thread_id: row.get(32)?,
            parent_turn_id: row.get(33)?,
            root_turn_id: row.get(34)?,
            payload_json: stored.then(|| row.get(35)).transpose()?,
        };
        let bytes = export_event_bytes(&event, query)?;
        export_send(sink, bytes, bytes_written)?;
        *row_count = row_count.saturating_add(1);
    }
    Ok(())
}

fn export_event_bytes(event: &ExportEventRow, query: &ExportQuery) -> QueryResult<Vec<u8>> {
    let event_id = match query.privacy {
        ExportPrivacy::Redacted => stable_mask("event", &event.event_id),
        ExportPrivacy::Stored => event.event_id.clone(),
    };
    let request_id = redact_optional("request", event.request_id.as_deref(), query.privacy);
    if query.format == ExportFormat::Jsonl {
        let value = if query.privacy == ExportPrivacy::Stored {
            let payload_json =
                event
                    .payload_json
                    .as_deref()
                    .ok_or_else(|| RuntimeQueryError::CorruptPayload {
                        event_id: event.event_id.clone(),
                        detail: "stored export row did not include payload_json".into(),
                    })?;
            let payload =
                serde_json::from_str::<serde_json::Value>(payload_json).map_err(|error| {
                    RuntimeQueryError::CorruptPayload {
                        event_id: event.event_id.clone(),
                        detail: error.to_string(),
                    }
                })?;
            serde_json::json!({
                "seq": event.seq,
                "changeSeq": event.change_seq,
                "eventID": event_id,
                "requestID": request_id,
                "event": payload,
            })
        } else {
            serde_json::json!({
                "seq": event.seq, "changeSeq": event.change_seq,
                "eventID": event_id, "requestID": request_id,
                "timestamp": event.timestamp, "kind": event.kind, "outcome": event.outcome,
                "statusCode": event.status_code, "clientKind": event.client_kind,
                "requestPurpose": event.request_purpose, "endpointID": event.endpoint_id,
                "endpointName": event.endpoint_name, "model": event.effective_model,
                "projectID": event.project_id, "projectName": event.project_name,
                "sessionID": redact_optional("session", event.session_key.as_deref(), query.privacy),
                "failureKind": event.failure_kind, "failurePhase": event.failure_phase,
                "upstreamStatusCode": event.upstream_status_code,
                "durationMS": event.duration_ms, "ttfbMS": event.ttfb_ms,
                "failover": event.failover, "inputTokens": event.input_tokens,
                "outputTokens": event.output_tokens,
                "cacheReadInputTokens": event.cache_read_input_tokens,
                "cacheCreationInputTokens": event.cache_creation_input_tokens,
                "reasoningTokens": event.reasoning_tokens,
                "processedInputTokens": event.processed_input_tokens,
                "processedTotalTokens": event.processed_total_tokens,
                "clientVariant": event.client_variant,
                "agentRole": event.agent_role,
                "agentName": redact_optional("agent", event.agent_name.as_deref(), query.privacy),
                "parentThreadID": redact_optional("thread", event.parent_thread_id.as_deref(), query.privacy),
                "parentTurnID": redact_optional("turn", event.parent_turn_id.as_deref(), query.privacy),
                "rootTurnID": redact_optional("turn", event.root_turn_id.as_deref(), query.privacy),

            })
        };
        let mut bytes = serde_json::to_vec(&value)
            .map_err(|error| RuntimeQueryError::Output(error.to_string()))?;
        bytes.push(b'\n');
        return Ok(bytes);
    }
    let fields = if query.privacy == ExportPrivacy::Stored {
        vec![
            event.seq.to_string(),
            event.change_seq.to_string(),
            event_id,
            request_id.unwrap_or_default(),
            event.timestamp.to_string(),
            event.kind.clone(),
            event.payload_json.clone().unwrap_or_default(),
        ]
    } else {
        vec![
            event.seq.to_string(),
            event.change_seq.to_string(),
            event_id,
            request_id.unwrap_or_default(),
            event.timestamp.to_string(),
            event.kind.clone(),
            event.outcome.clone().unwrap_or_default(),
            event.status_code.to_string(),
            event.client_kind.clone().unwrap_or_default(),
            event.request_purpose.clone().unwrap_or_default(),
            event.endpoint_id.clone().unwrap_or_default(),
            event.endpoint_name.clone().unwrap_or_default(),
            event.effective_model.clone().unwrap_or_default(),
            event.project_id.clone().unwrap_or_default(),
            event.project_name.clone().unwrap_or_default(),
            redact_optional("session", event.session_key.as_deref(), query.privacy)
                .unwrap_or_default(),
            event.failure_kind.clone().unwrap_or_default(),
            event.failure_phase.clone().unwrap_or_default(),
            option_csv(event.upstream_status_code),
            option_csv(event.duration_ms),
            option_csv(event.ttfb_ms),
            event.failover.to_string(),
            option_csv(event.input_tokens),
            option_csv(event.output_tokens),
            option_csv(event.cache_read_input_tokens),
            option_csv(event.cache_creation_input_tokens),
            option_csv(event.reasoning_tokens),
            option_csv(event.processed_input_tokens),
            option_csv(event.processed_total_tokens),
            event.client_variant.clone().unwrap_or_default(),
            event.agent_role.clone().unwrap_or_default(),
            redact_optional("agent", event.agent_name.as_deref(), query.privacy)
                .unwrap_or_default(),
            redact_optional("thread", event.parent_thread_id.as_deref(), query.privacy)
                .unwrap_or_default(),
            redact_optional("turn", event.parent_turn_id.as_deref(), query.privacy)
                .unwrap_or_default(),
            redact_optional("turn", event.root_turn_id.as_deref(), query.privacy)
                .unwrap_or_default(),
        ]
    };
    Ok(csv_line(fields).into_bytes())
}

fn stream_dimension_rows<F>(
    transaction: &Transaction<'_>,
    builder: &SqlFilter,
    query: &ExportQuery,
    sink: &mut F,
    row_count: &mut i64,
    bytes_written: &mut u64,
) -> QueryResult<()>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    let (key, name, source) = export_dimension_expressions(query.scope);
    let (related, workspace) = match query.scope {
        ExportScope::Projects => ("session_key", "MAX(workspace_paths_json)"),
        ExportScope::Sessions => ("project_id", "NULL"),
        ExportScope::Events => unreachable!(),
    };
    let sql = format!(
        "SELECT {key},{name},{source},COUNT(*),\
                SUM(CASE WHEN outcome='succeeded' THEN 1 ELSE 0 END),\
                SUM(CASE WHEN outcome='failed' THEN 1 ELSE 0 END),\
                SUM(CASE WHEN outcome='cancelled' THEN 1 ELSE 0 END),\
                SUM(CASE WHEN failover=1 THEN 1 ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(input_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(output_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(cache_read_input_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(cache_creation_input_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(processed_total_tokens,0) ELSE 0 END),\
                MIN(timestamp),MAX(timestamp),COUNT(DISTINCT {related}),{workspace} \
         FROM runtime_events{} GROUP BY {key},{name},{source} ORDER BY {key} ASC",
        builder.where_sql()
    );
    let mut statement = transaction.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(builder.values.iter()))?;
    while let Some(row) = rows.next()? {
        let mut key_value = row.get::<_, String>(0)?;
        if query.privacy == ExportPrivacy::Redacted && query.scope == ExportScope::Sessions {
            key_value = stable_mask("session", &key_value);
        }
        let workspace_paths = if query.privacy == ExportPrivacy::Stored {
            row.get::<_, Option<String>>(16)?
                .unwrap_or_else(|| "[]".into())
        } else {
            "[]".into()
        };
        let fields = vec![
            key_value.clone(),
            if query.scope == ExportScope::Sessions {
                key_value.clone()
            } else {
                row.get::<_, String>(1)?
            },
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?.to_string(),
            row.get::<_, i64>(4)?.to_string(),
            row.get::<_, i64>(5)?.to_string(),
            row.get::<_, i64>(6)?.to_string(),
            row.get::<_, i64>(7)?.to_string(),
            row.get::<_, i64>(8)?.to_string(),
            row.get::<_, i64>(9)?.to_string(),
            row.get::<_, i64>(10)?.to_string(),
            row.get::<_, i64>(11)?.to_string(),
            row.get::<_, i64>(12)?.to_string(),
            row.get::<_, f64>(13)?.to_string(),
            row.get::<_, f64>(14)?.to_string(),
            row.get::<_, i64>(15)?.to_string(),
            workspace_paths,
        ];
        let bytes = if query.format == ExportFormat::Csv {
            csv_line(fields).into_bytes()
        } else {
            let value = serde_json::json!({
                "key": fields[0], "name": fields[1], "source": fields[2],
                "requests": fields[3].parse::<i64>().unwrap_or(0),
                "successes": fields[4].parse::<i64>().unwrap_or(0),
                "failures": fields[5].parse::<i64>().unwrap_or(0),
                "cancelled": fields[6].parse::<i64>().unwrap_or(0),
                "failovers": fields[7].parse::<i64>().unwrap_or(0),
                "inputTokens": fields[8].parse::<i64>().unwrap_or(0),
                "outputTokens": fields[9].parse::<i64>().unwrap_or(0),
                "cacheReadInputTokens": fields[10].parse::<i64>().unwrap_or(0),
                "cacheCreationInputTokens": fields[11].parse::<i64>().unwrap_or(0),
                "processedTotalTokens": fields[12].parse::<i64>().unwrap_or(0),
                "firstSeen": fields[13].parse::<f64>().unwrap_or(0.0),
                "lastSeen": fields[14].parse::<f64>().unwrap_or(0.0),
                "relatedCount": fields[15].parse::<i64>().unwrap_or(0),
                "workspacePaths": serde_json::from_str::<Vec<String>>(&fields[16]).unwrap_or_default(),
            });
            let mut bytes = serde_json::to_vec(&value)
                .map_err(|error| RuntimeQueryError::Output(error.to_string()))?;
            bytes.push(b'\n');
            bytes
        };
        export_send(sink, bytes, bytes_written)?;
        *row_count = row_count.saturating_add(1);
    }
    Ok(())
}

fn redact_optional(label: &str, value: Option<&str>, privacy: ExportPrivacy) -> Option<String> {
    value.map(|value| match privacy {
        ExportPrivacy::Redacted => stable_mask(label, value),
        ExportPrivacy::Stored => value.to_owned(),
    })
}

fn stable_mask(label: &str, value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{label}_{hash:016x}")
}

fn option_csv<T: ToString>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn csv_line(fields: Vec<String>) -> String {
    let mut line = fields
        .into_iter()
        .map(|value| {
            if value.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    line.push('\n');
    line
}

fn export_send<F>(sink: &mut F, bytes: Vec<u8>, bytes_written: &mut u64) -> QueryResult<()>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    *bytes_written = bytes_written.saturating_add(bytes.len() as u64);
    sink(bytes).map_err(RuntimeQueryError::Output)
}
