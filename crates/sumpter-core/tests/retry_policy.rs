use sumpter_core::config::RetryPolicy;

#[test]
fn retry_and_deferred_status_sets_match_runtime_contract() {
    assert_eq!(
        RetryPolicy::RETRYABLE_STATUS_CODES,
        [
            401, 402, 403, 429, 502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 529, 530,
        ]
    );
    assert_eq!(
        RetryPolicy::DEFERRED_STATUS_CODES,
        RetryPolicy::RETRYABLE_STATUS_CODES
    );

    for status in RetryPolicy::RETRYABLE_STATUS_CODES {
        assert!(RetryPolicy::is_retryable_status(status));
        assert!(RetryPolicy::is_deferred_status(status));
    }
    assert!(!RetryPolicy::is_retryable_status(400));
    assert!(!RetryPolicy::is_retryable_status(500));
    assert!(!RetryPolicy::is_deferred_status(500));
    assert!(RetryPolicy::is_endpoint_retryable_status(500));
}

#[test]
fn retry_policy_new_fields_keep_safe_defaults_and_wire_names() {
    let defaults = RetryPolicy::default();
    assert_eq!(defaults.max_500_retries, 0);
    assert!(defaults.failover_on_500);
    assert!(defaults.pass_through_retry_delay);
    assert_eq!(defaults.retry_delay_seconds, None);

    let mut configured = defaults;
    configured.max_500_retries = 3;
    configured.failover_on_500 = false;
    configured.pass_through_retry_delay = false;
    configured.retry_delay_seconds = Some(2.5);
    let value = serde_json::to_value(configured).unwrap();
    assert_eq!(value["max500Retries"], 3);
    assert_eq!(value["failoverOn500"], false);
    assert_eq!(value["passThroughRetryDelay"], false);
    assert_eq!(value["retryDelaySeconds"], 2.5);
    assert!(value.get("max_500_retries").is_none());
}

#[test]
fn retry_policy_legacy_json_defaults_500_failover_on() {
    let policy: RetryPolicy = serde_json::from_value(serde_json::json!({
        "max500Retries": 2,
    }))
    .unwrap();
    assert!(policy.failover_on_500);
}
