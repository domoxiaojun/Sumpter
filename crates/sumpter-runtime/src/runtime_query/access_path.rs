use super::RuntimeFilter;

/// Do not override selective dimension indexes or force a wide COUNT through
/// the time index. Ordered pages can stop after LIMIT; counts cannot.
pub(super) fn event_query_table(
    filter: &RuntimeFilter,
    ordered_page: bool,
    now: f64,
) -> &'static str {
    let ordinary = filter.inbound_token_id.is_none()
        && filter.client_kind.is_none()
        && filter.client_variant.is_none()
        && filter.agent_role.is_none()
        && filter.agent_name.is_none()
        && filter.parent_thread_id.is_none()
        && filter.parent_turn_id.is_none()
        && filter.root_turn_id.is_none()
        && filter.request_purpose.is_none()
        && filter.request_id.is_none()
        && filter.endpoint_id.is_none()
        && filter.model.is_none()
        && filter.project_id.is_none()
        && filter.project_name.is_none()
        && filter.session_id.is_none()
        && filter.failure_kind.is_none()
        && filter.failure_phase.is_none();
    let eligible_range = filter.from.is_some_and(|from| {
        let to = filter.to.unwrap_or(now);
        from.is_finite()
            && to.is_finite()
            && to >= from
            && (ordered_page || to - from <= 8.0 * 86_400.0)
    });
    if ordinary && eligible_range {
        "runtime_events INDEXED BY runtime_events_time_seq_v2"
    } else {
        "runtime_events"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_and_page_choose_paths_without_overriding_selective_filters() {
        let mut filter = RuntimeFilter {
            kind: Some("client".into()),
            from: Some(100.0),
            ..Default::default()
        };
        let time = "runtime_events INDEXED BY runtime_events_time_seq_v2";
        assert_eq!(
            event_query_table(&filter, false, 100.0 + 7.0 * 86_400.0),
            time
        );
        assert_eq!(
            event_query_table(&filter, false, 100.0 + 30.0 * 86_400.0),
            "runtime_events"
        );
        assert_eq!(
            event_query_table(&filter, true, 100.0 + 30.0 * 86_400.0),
            time
        );
        filter.session_id = Some("selected".into());
        assert_eq!(event_query_table(&filter, true, 200.0), "runtime_events");
        filter.session_id = None;
        filter.from = None;
        assert_eq!(event_query_table(&filter, true, 200.0), "runtime_events");
    }
}
