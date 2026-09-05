use super::{
    AnalyticsFilter, Arc, AtomicBool, AtomicUsize, BACKPRESSURE_BYTES, BACKPRESSURE_EVENTS,
    BATCH_BYTES, BATCH_EVENTS, Command, Connection, Duration, HashMap, HashSet, Inner, Mutex,
    OptionalExtension, Ordering, PENDING_BYTES_LIMIT, PENDING_EVENTS_LIMIT, Path, PathBuf,
    PendingBatch, RETENTION_IDLE_CHECK_INTERVAL, RETRY_INITIAL, RETRY_MAX, RuntimeChange,
    RuntimeCleanupMutation, RuntimeCleanupPreview, RuntimeCounters, RuntimeEvent,
    RuntimeEventListItem, RuntimePricingMutation, RuntimePricingUpdate, RuntimeRetentionMutation,
    RuntimeRetentionUpdate, RuntimeSnapshot, RuntimeStorageStatus, RuntimeStore, RuntimeSummary,
    SessionMutation, Value, WriteMessage, check_existing_schema, cleanup_before_database,
    cleanup_before_preview_database, commit_pending, database_file_sizes, delete_session_database,
    export_session_json, harden_database_file, load_snapshot, load_state, meta_i64, mpsc,
    normalize_startup, now, option_token, params, projection_maintenance_needed, read_connection,
    recreate_database, refresh_cached_storage, replace_pricing_database, reset_database,
    rotate_retention_now, run_projection_maintenance, run_retention_maintenance, set_meta,
    set_retention_database, setup_connection, thread, validate_cleanup_cutoff,
    validate_pricing_update, validate_retention_update,
};

impl RuntimeStore {
    pub fn new(path: impl Into<PathBuf>) -> Result<(Self, RuntimeSnapshot), String> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        check_existing_schema(&path)?;
        let mut connection = Connection::open(&path).map_err(|error| error.to_string())?;
        setup_connection(&mut connection).map_err(|error| error.to_string())?;
        harden_database_file(&path);
        let normalized_changes =
            normalize_startup(&mut connection).map_err(|error| error.to_string())?;
        let startup_rotation =
            rotate_retention_now(&mut connection).map_err(|error| error.to_string())?;
        let rotated_ids = startup_rotation
            .deleted_event_ids
            .into_iter()
            .collect::<HashSet<_>>();
        let mut state = load_state(&connection).map_err(|error| error.to_string())?;
        for change in normalized_changes
            .into_iter()
            .filter(|change| !rotated_ids.contains(&change.event.id))
        {
            state.remember_change(change);
        }
        let snapshot = load_snapshot(&connection)?;
        let (sender, receiver) = mpsc::sync_channel::<Command>(PENDING_EVENTS_LIMIT);
        let worker_path = path.clone();
        let inner = Arc::new(Inner {
            path,
            sender,
            state: Mutex::new(state),
            pending_events: AtomicUsize::new(0),
            pending_bytes: AtomicUsize::new(0),
            backpressure: AtomicBool::new(false),
            hard_backpressure: AtomicBool::new(false),
            #[cfg(test)]
            fail_writes: AtomicBool::new(false),
        });
        // The worker must not keep an `Arc<Inner>` alive: `Inner` owns the
        // channel sender, so holding a strong reference here would create a
        // self-retaining cycle and leave one blocked thread per store.
        let worker_ref = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("runtime-sqlite".into())
            .spawn(move || {
                let mut connection = match Connection::open(worker_path) {
                    Ok(connection) => connection,
                    Err(error) => {
                        if let Some(worker_inner) = worker_ref.upgrade() {
                            worker_inner.state.lock().unwrap().last_error = Some(error.to_string());
                        }
                        return;
                    }
                };
                if let Err(error) = setup_connection(&mut connection) {
                    if let Some(worker_inner) = worker_ref.upgrade() {
                        worker_inner.state.lock().unwrap().last_error = Some(error.to_string());
                    }
                    return;
                }
                let mut projection_maintenance =
                    projection_maintenance_needed(&connection).unwrap_or(true);
                let mut pending = PendingBatch::default();
                let mut retry_delay = RETRY_INITIAL;
                loop {
                    let command = if pending.is_empty() {
                        if projection_maintenance {
                            match receiver.recv_timeout(Duration::from_millis(25)) {
                                Ok(command) => command,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let Some(worker_inner) = worker_ref.upgrade() else {
                                        break;
                                    };
                                    match run_projection_maintenance(&mut connection) {
                                        Ok(more) => {
                                            projection_maintenance = more;
                                            let _ = refresh_cached_storage(
                                                &worker_inner,
                                                &connection,
                                                !more,
                                            );
                                        }
                                        Err(error) => {
                                            let failed =
                                                meta_i64(&connection, "hourly_rollup_failed")
                                                    .ok()
                                                    .flatten()
                                                    .unwrap_or(0)
                                                    .saturating_add(1);
                                            let _ = set_meta(
                                                &connection,
                                                "hourly_rollup_failed",
                                                failed,
                                            );
                                            worker_inner.state.lock().unwrap().last_error =
                                                Some(format!(
                                                    "runtime projection maintenance failed: {error}"
                                                ));
                                        }
                                    }
                                    continue;
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        } else {
                            match receiver.recv_timeout(RETENTION_IDLE_CHECK_INTERVAL) {
                                Ok(command) => command,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let Some(worker_inner) = worker_ref.upgrade() else {
                                        break;
                                    };
                                    match run_retention_maintenance(&worker_inner, &mut connection)
                                    {
                                        Ok(true) => projection_maintenance = true,
                                        Ok(false) => {}
                                        Err(error) => {
                                            worker_inner.state.lock().unwrap().last_error =
                                                Some(format!(
                                                    "runtime retention maintenance failed: {error}"
                                                ));
                                        }
                                    }
                                    continue;
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                    } else {
                        match receiver.recv_timeout(pending.flush_wait()) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                let Some(worker_inner) = worker_ref.upgrade() else {
                                    break;
                                };
                                match commit_pending(&worker_inner, &mut connection, &mut pending) {
                                    Ok(()) => {
                                        retry_delay = RETRY_INITIAL;
                                        projection_maintenance = true;
                                    }
                                    Err(error) => {
                                        worker_inner.state.lock().unwrap().last_error =
                                            Some(error.to_string());
                                        thread::sleep(retry_delay);
                                        retry_delay = retry_delay.saturating_mul(2).min(RETRY_MAX);
                                    }
                                }
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    };
                    match command {
                        Command::Write(message) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                break;
                            };
                            pending.upsert(message, &worker_inner);
                            if pending.len() >= BATCH_EVENTS || pending.bytes >= BATCH_BYTES {
                                match commit_pending(&worker_inner, &mut connection, &mut pending) {
                                    Ok(()) => {
                                        retry_delay = RETRY_INITIAL;
                                        projection_maintenance = true;
                                    }
                                    Err(error) => {
                                        worker_inner.state.lock().unwrap().last_error =
                                            Some(error.to_string());
                                        thread::sleep(retry_delay);
                                        retry_delay = retry_delay.saturating_mul(2).min(RETRY_MAX);
                                    }
                                }
                            }
                        }
                        Command::Flush(reply) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .map_err(|error| error.to_string());
                            if result.is_ok() {
                                retry_delay = RETRY_INITIAL;
                                projection_maintenance = true;
                            } else if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                        }
                        Command::Reset(reply) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| reset_database(&worker_inner, &mut connection))
                                    .map_err(|error| error.to_string());
                            let succeeded = result.is_ok();
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                            if succeeded {
                                projection_maintenance = true;
                                let _ = connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
                                if std::fs::metadata(&worker_inner.path)
                                    .map(|metadata| metadata.len() >= 64 * 1024 * 1024)
                                    .unwrap_or(false)
                                {
                                    let _ = connection.execute_batch("VACUUM;");
                                }
                            }
                        }
                        Command::Recreate(reply) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| recreate_database(&worker_inner, &mut connection))
                                    .map_err(|error| error.to_string());
                            let succeeded = result.is_ok();
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                            if succeeded {
                                projection_maintenance = false;
                                let _ =
                                    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
                            }
                        }
                        Command::CleanupBefore { older_than, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| {
                                        cleanup_before_database(
                                            &worker_inner,
                                            &mut connection,
                                            older_than,
                                        )
                                    })
                                    .map_err(|error| error.to_string());
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            } else {
                                projection_maintenance = true;
                            }
                            let _ = reply.send(result);
                        }
                        Command::DeleteSession { session_id, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| {
                                        delete_session_database(
                                            &worker_inner,
                                            &mut connection,
                                            &session_id,
                                        )
                                    })
                                    .map_err(|error| error.to_string());
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            } else {
                                projection_maintenance = true;
                            }
                            let _ = reply.send(result);
                        }
                        Command::SetRetention { update, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .map_err(|error| error.to_string())
                                    .and_then(|_| {
                                        set_retention_database(
                                            &worker_inner,
                                            &mut connection,
                                            update,
                                        )
                                    });
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            } else {
                                projection_maintenance = true;
                            }
                            let _ = reply.send(result);
                        }
                        Command::ReplacePricing { update, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .map_err(|error| error.to_string())
                                    .and_then(|_| {
                                        replace_pricing_database(&mut connection, update)
                                    });
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                        }
                    }
                }
                if let Some(worker_inner) = worker_ref.upgrade() {
                    let _ = commit_pending(&worker_inner, &mut connection, &mut pending);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok((Self { inner }, snapshot))
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn database_path(&self) -> &Path {
        &self.inner.path
    }

    pub fn recent_changes_for_request(&self, request_id: &str) -> Vec<RuntimeChange> {
        let request_id = request_id.trim();
        if request_id.is_empty() {
            return Vec::new();
        }
        self.inner
            .state
            .lock()
            .unwrap()
            .recent_changes
            .iter()
            .filter(|change| change.event.request_id.as_deref() == Some(request_id))
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(super) fn set_test_write_failure(&self, enabled: bool) {
        self.inner.fail_writes.store(enabled, Ordering::Release);
    }

    pub fn enqueue(
        &self,
        event: RuntimeEvent,
        counters: RuntimeCounters,
    ) -> Result<RuntimeChange, String> {
        let payload_size = serde_json::to_vec(&event)
            .map_err(|error| error.to_string())?
            .len();
        let mut state = self.inner.state.lock().unwrap();
        let previous_counters = state.counters;
        let previous_latest_event = state.latest_event.clone();
        let previous_active_sequence = state.active_sequences.get(&event.id).copied();
        let replacing_active = state.active_sequences.contains_key(&event.id);
        if self.inner.pending_bytes.load(Ordering::Acquire) + payload_size > PENDING_BYTES_LIMIT
            || (!replacing_active
                && self.inner.pending_events.load(Ordering::Acquire) >= PENDING_EVENTS_LIMIT)
        {
            self.inner.backpressure.store(true, Ordering::Release);
            self.inner.hard_backpressure.store(true, Ordering::Release);
            state.last_error = Some("runtime_storage_backpressure".into());
            return Err("runtime_storage_backpressure".into());
        }
        let seq = if let Some(seq) = state.active_sequences.get(&event.id).copied() {
            if !event.is_in_flight() {
                state.active_sequences.remove(&event.id);
            }
            seq
        } else {
            let seq = state.next_seq;
            state.next_seq += 1;
            if event.is_in_flight() {
                state.active_sequences.insert(event.id.clone(), seq);
            }
            seq
        };
        let change_seq = state.next_change_seq;
        state.next_change_seq += 1;
        state.counters = counters;
        state.latest_event = Some(event.clone());
        // Admission and reservation share the state lock. Without reserving
        // before releasing it, concurrent request threads could all pass the
        // hard-limit check and temporarily exceed the memory budget.
        self.inner.pending_events.fetch_add(1, Ordering::AcqRel);
        self.inner
            .pending_bytes
            .fetch_add(payload_size, Ordering::AcqRel);
        let message = WriteMessage {
            seq,
            change_seq,
            event: event.clone(),
            counters,
            bytes: payload_size,
        };
        if let Err(error) = self.inner.sender.try_send(Command::Write(message)) {
            self.inner.pending_events.fetch_sub(1, Ordering::AcqRel);
            self.inner
                .pending_bytes
                .fetch_sub(payload_size, Ordering::AcqRel);
            state.counters = previous_counters;
            state.latest_event = previous_latest_event;
            match previous_active_sequence {
                Some(previous) => {
                    state.active_sequences.insert(event.id.clone(), previous);
                }
                None => {
                    state.active_sequences.remove(&event.id);
                }
            }
            self.inner.backpressure.store(true, Ordering::Release);
            self.inner.hard_backpressure.store(true, Ordering::Release);
            state.last_error = Some(error.to_string());
            return Err(error.to_string());
        }
        drop(state);
        let change = RuntimeChange {
            seq,
            change_seq,
            event,
        };
        {
            let mut state = self.inner.state.lock().unwrap();
            state.remember_change(change.clone());
        }
        if self.inner.pending_bytes.load(Ordering::Acquire) >= BACKPRESSURE_BYTES
            || self.inner.pending_events.load(Ordering::Acquire) >= BACKPRESSURE_EVENTS
        {
            self.inner.backpressure.store(true, Ordering::Release);
        }
        Ok(change)
    }

    pub fn flush(&self) -> Result<(), String> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::Flush(sender))
            .map_err(|e| e.to_string())?;
        receiver.recv().map_err(|e| e.to_string())??;
        Ok(())
    }

    pub fn set_retention(
        &self,
        update: RuntimeRetentionUpdate,
    ) -> Result<RuntimeRetentionMutation, String> {
        validate_retention_update(&update)?;
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::SetRetention {
                update,
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    }

    pub fn replace_pricing(
        &self,
        update: RuntimePricingUpdate,
    ) -> Result<RuntimePricingMutation, String> {
        validate_pricing_update(&update)?;
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::ReplacePricing {
                update,
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    }

    pub fn reset(&self) -> Result<i64, String> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::Reset(sender))
            .map_err(|e| e.to_string())?;
        let generation = receiver.recv().map_err(|e| e.to_string())??;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = RuntimeCounters::default();
        state.latest_event = None;
        state.reset_generation = generation;
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_error = None;
        self.inner.backpressure.store(false, Ordering::Release);
        self.inner.hard_backpressure.store(false, Ordering::Release);
        Ok(generation)
    }

    /// Replace the runtime statistics schema with a fresh current-version
    /// database. Unlike `reset`, this removes legacy table columns as well as
    /// all retained rows, while leaving diagnostic capture outside the scope
    /// of the operation.
    pub fn recreate(&self) -> Result<i64, String> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::Recreate(sender))
            .map_err(|e| e.to_string())?;
        let generation = receiver.recv().map_err(|e| e.to_string())??;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = RuntimeCounters::default();
        state.latest_event = None;
        state.reset_generation = generation;
        state.history_generation = state.history_generation.saturating_add(1);
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_error = None;
        self.inner.backpressure.store(false, Ordering::Release);
        self.inner.hard_backpressure.store(false, Ordering::Release);
        Ok(generation)
    }

    /// Preview a one-off age cleanup after flushing queued events, so the
    /// confirmation count is based on the same durable request groups the
    /// mutation will evaluate. The cutoff uses Apple reference-date seconds,
    /// matching every runtime event timestamp exposed by Admin.
    pub fn cleanup_before_preview(&self, older_than: f64) -> Result<RuntimeCleanupPreview, String> {
        validate_cleanup_cutoff(older_than)?;
        self.flush()?;
        let connection = read_connection(&self.inner.path)?;
        cleanup_before_preview_database(&connection, older_than).map_err(|error| error.to_string())
    }

    pub fn cleanup_before(&self, older_than: f64) -> Result<RuntimeCleanupMutation, String> {
        validate_cleanup_cutoff(older_than)?;
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::CleanupBefore {
                older_than,
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        let mutation = receiver.recv().map_err(|error| error.to_string())??;
        let refreshed =
            load_state(&read_connection(&self.inner.path)?).map_err(|error| error.to_string())?;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = refreshed.counters;
        state.latest_event = refreshed.latest_event;
        state.history_generation = mutation.history_generation;
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_commit_at = Some(now());
        state.last_error = None;
        Ok(mutation)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<SessionMutation, String> {
        self.delete_session_confirmed(session_id, false)
    }

    /// Delete a session, optionally allowing the aggregate unidentified
    /// bucket.  The explicit confirmation is deliberately kept at the store
    /// boundary so non-HTTP callers cannot accidentally turn a UI affordance
    /// into a destructive all-unknown operation.
    pub fn delete_session_confirmed(
        &self,
        session_id: &str,
        confirm_unidentified: bool,
    ) -> Result<SessionMutation, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("不能删除空会话；请使用完整 sessionID/threadID".into());
        }
        if session_id == "unidentified_session" && !confirm_unidentified {
            return Err("不能删除未识别会话；请使用完整 sessionID/threadID".into());
        }
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::DeleteSession {
                session_id: session_id.to_owned(),
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        let mutation = receiver.recv().map_err(|error| error.to_string())??;
        let refreshed =
            load_state(&read_connection(&self.inner.path)?).map_err(|error| error.to_string())?;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = refreshed.counters;
        state.latest_event = refreshed.latest_event;
        state.reset_generation = mutation.reset_generation;
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_commit_at = Some(now());
        state.last_error = None;
        Ok(mutation)
    }

    pub fn export_session(&self, session_id: &str) -> Result<Value, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("sessionID 不能为空".into());
        }
        self.flush()?;
        export_session_json(&read_connection(&self.inner.path)?, session_id)
    }

    pub fn summary(&self) -> RuntimeSummary {
        let (
            reset_generation,
            history_generation,
            counters,
            latest_event,
            last_commit_at,
            last_error,
            storage,
        ) = {
            let state = self.inner.state.lock().unwrap();
            (
                state.reset_generation,
                state.history_generation,
                state.counters,
                state.latest_event.clone(),
                state.last_commit_at,
                state.last_error.clone(),
                state.storage.clone(),
            )
        };
        let (db_bytes, wal_bytes) = database_file_sizes(&self.inner.path);
        let pending_events = self.inner.pending_events.load(Ordering::Acquire);
        let pending_bytes = self.inner.pending_bytes.load(Ordering::Acquire);
        RuntimeSummary {
            api_version: 1,
            storage: RuntimeStorageStatus {
                backend: "sqlite",
                state: if self.inner.backpressure.load(Ordering::Acquire)
                    || pending_bytes >= BACKPRESSURE_BYTES
                    || pending_events >= BACKPRESSURE_EVENTS
                {
                    "backpressure"
                } else if last_error.is_some() {
                    "degraded"
                } else {
                    "ready"
                },
                pending_events,
                pending_bytes,
                event_count: storage.event_count,
                completed_event_count: storage.completed_event_count,
                in_flight_event_count: storage.in_flight_event_count,
                oldest_event_at: storage.oldest_event_at,
                newest_event_at: storage.newest_event_at,
                retained_from_seq: storage.retained_from_seq,
                payload_bytes: storage.payload_bytes,
                live_bytes: storage.live_bytes,
                allocated_bytes: storage.allocated_bytes,
                db_bytes,
                wal_bytes,
                schema_version: storage.schema_version,
                backfill_cursor: storage.backfill_cursor,
                backfill_complete: storage.backfill_complete,
                backfill_failed: storage.backfill_failed,
                indexes_ready: storage.indexes_ready,
                rollup_complete: storage.rollup_complete,
                rollup_max_seq: storage.rollup_max_seq,
                rollup_history_generation: storage.rollup_history_generation,
                rollup_failed: storage.rollup_failed,
                rollup_dirty_buckets: storage.rollup_dirty_buckets,
                user_deleted_events: storage.user_deleted_events,
                user_deleted_requests: storage.user_deleted_requests,
                last_commit_at,
                last_error,
            },
            reset_generation,
            history_generation,
            counters,
            latest_event,
        }
    }

    pub fn snapshot(&self) -> Result<RuntimeSnapshot, String> {
        load_snapshot(&read_connection(&self.inner.path)?)
    }

    pub fn is_backpressured(&self) -> bool {
        self.inner.hard_backpressure.load(Ordering::Acquire)
            || self.inner.pending_bytes.load(Ordering::Acquire) >= PENDING_BYTES_LIMIT
            || self.inner.pending_events.load(Ordering::Acquire) >= PENDING_EVENTS_LIMIT
    }

    // 事件查询的过滤条件就是这么多维,合并成 struct 会让调用方多写一层构造。
    #[allow(clippy::too_many_arguments)]
    pub fn events(
        &self,
        before_seq: Option<i64>,
        after_change_seq: Option<i64>,
        limit: usize,
        kind: Option<&str>,
        request_id: Option<&str>,
        outcome: Option<&str>,
        from: Option<f64>,
        to: Option<f64>,
    ) -> Result<Vec<RuntimeEventListItem>, String> {
        // Zero is the client-side sentinel for "no cursor". Treating it as
        // an actual cursor would return an empty/incorrect page (`seq < 0` or
        // the oldest in-memory changes) rather than the newest page expected
        // by the list API.
        let before_seq = before_seq.filter(|value| *value > 0);
        let after_change_seq = after_change_seq.filter(|value| *value > 0);
        let limit = limit.clamp(1, 200);
        let query_limit = limit + 1;
        let recent_changes = self
            .inner
            .state
            .lock()
            .unwrap()
            .recent_changes
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        if let Some(cursor) = after_change_seq {
            let mut changes = recent_changes
                .into_iter()
                .filter(|change| change.change_seq > cursor)
                .filter(|change| {
                    let event = &change.event;
                    !before_seq.is_some_and(|value| change.seq >= value)
                        && !kind.is_some_and(|value| value != event.kind)
                        && !request_id
                            .is_some_and(|value| event.request_id.as_deref() != Some(value))
                        && !outcome
                            .is_some_and(|value| option_token(event.outcome) != Some(value.into()))
                        && !from.is_some_and(|value| event.timestamp < value)
                        && !to.is_some_and(|value| event.timestamp > value)
                })
                .map(|change| {
                    RuntimeEventListItem::from_change(change.seq, change.change_seq, change.event)
                })
                .collect::<Vec<_>>();
            changes.sort_by_key(|item| item.change_seq);
            changes.truncate(query_limit);
            return Ok(changes);
        }

        let connection = read_connection(&self.inner.path)?;
        let order = if after_change_seq.is_some() {
            "change_seq ASC"
        } else {
            "seq DESC"
        };
        // Keep one positional parameter set for every branch.  This avoids
        // binding optional named parameters that are absent from a dynamically
        // assembled statement (rusqlite correctly rejects those bindings).
        let sql = format!(
            "SELECT seq,change_seq,payload_json FROM runtime_events
             WHERE (?1 IS NULL OR seq < ?1)
               AND (?2 IS NULL OR change_seq > ?2)
               AND (?3 IS NULL OR kind = ?3)
               AND (?4 IS NULL OR request_id = ?4)
               AND (?5 IS NULL OR outcome = ?5)
               AND (?6 IS NULL OR timestamp >= ?6)
               AND (?7 IS NULL OR timestamp <= ?7)
             ORDER BY {order} LIMIT ?8"
        );
        let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = statement
            .query(params![
                before_seq,
                after_change_seq,
                kind,
                request_id,
                outcome,
                from,
                to,
                query_limit as i64,
            ])
            .map_err(|e| e.to_string())?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let seq: i64 = row.get(0).map_err(|e| e.to_string())?;
            let change_seq: i64 = row.get(1).map_err(|e| e.to_string())?;
            let payload: String = row.get(2).map_err(|e| e.to_string())?;
            let event = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
            result.push(RuntimeEventListItem::from_change(seq, change_seq, event));
        }
        let mut merged = result
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect::<HashMap<_, _>>();
        for change in recent_changes {
            let event = &change.event;
            if before_seq.is_some_and(|value| change.seq >= value)
                || after_change_seq.is_some_and(|value| change.change_seq <= value)
                || kind.is_some_and(|value| value != event.kind)
                || request_id.is_some_and(|value| event.request_id.as_deref() != Some(value))
                || outcome.is_some_and(|value| option_token(event.outcome) != Some(value.into()))
                || from.is_some_and(|value| event.timestamp < value)
                || to.is_some_and(|value| event.timestamp > value)
            {
                continue;
            }
            let item =
                RuntimeEventListItem::from_change(change.seq, change.change_seq, change.event);
            if merged
                .get(&item.id)
                .is_none_or(|current| item.change_seq > current.change_seq)
            {
                merged.insert(item.id.clone(), item);
            }
        }
        let mut result = merged.into_values().collect::<Vec<_>>();
        result.sort_by_key(|row| std::cmp::Reverse(row.seq));
        result.truncate(query_limit);
        Ok(result)
    }

    /// A cursor older than the oldest retained change cannot be completed by
    /// this in-memory change window. The caller must reload summary and the
    /// newest page instead of treating the current database row as every
    /// intermediate in-flight/completed update.
    pub fn change_cursor_valid(&self, cursor: Option<i64>) -> Result<bool, String> {
        let Some(cursor) = cursor.filter(|value| *value > 0) else {
            return Ok(true);
        };
        let (latest, recent_oldest) = {
            let state = self.inner.state.lock().unwrap();
            (
                state.next_change_seq.saturating_sub(1),
                state.recent_changes.front().map(|change| change.change_seq),
            )
        };
        if cursor > latest {
            return Ok(false);
        }
        Ok(recent_oldest.map_or(cursor == latest, |value| cursor >= value - 1))
    }

    pub fn event(&self, id: &str) -> Result<Option<RuntimeChange>, String> {
        if let Some(change) = self
            .inner
            .state
            .lock()
            .unwrap()
            .recent_changes
            .iter()
            .rev()
            .find(|change| change.event.id == id)
            .cloned()
        {
            return Ok(Some(change));
        }
        let connection = read_connection(&self.inner.path)?;
        connection
            .query_row(
                "SELECT seq,change_seq,payload_json FROM runtime_events WHERE event_id=?1",
                params![id],
                |row| {
                    let event: RuntimeEvent = serde_json::from_str(&row.get::<_, String>(2)?)
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                    Ok(RuntimeChange {
                        seq: row.get(0)?,
                        change_seq: row.get(1)?,
                        event,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn analytics(&self, range: &str) -> Result<Value, String> {
        self.analytics_filtered(range, &AnalyticsFilter::default())
    }

    pub fn analytics_filtered(
        &self,
        range: &str,
        filter: &AnalyticsFilter,
    ) -> Result<Value, String> {
        let filter = filter.normalized();
        let query_filter = crate::runtime_query::RuntimeFilter {
            client_kind: filter.client_kind,
            endpoint_id: filter.endpoint_id,
            project_id: filter.project_id,
            project_name: filter.project,
            session_id: filter.session_id,
            from: filter.from,
            to: filter.to,
            ..crate::runtime_query::RuntimeFilter::default()
        };
        let summary = crate::runtime_query::analytics(&self.inner.path, range, &query_filter)
            .map_err(|error| error.to_string())?;
        serde_json::to_value(summary).map_err(|error| error.to_string())
    }
}
