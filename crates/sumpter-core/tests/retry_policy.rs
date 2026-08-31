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
    assert!(!RetryPolicy::is_deferred_status(500));
}
