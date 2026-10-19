SELECT event_count,completed_events,in_flight_events,payload_bytes,
       (SELECT seq FROM runtime_events ORDER BY seq ASC LIMIT 1),
       (SELECT seq FROM runtime_events ORDER BY seq DESC LIMIT 1),
       (SELECT timestamp FROM runtime_events ORDER BY timestamp ASC LIMIT 1),
       (SELECT timestamp FROM runtime_events ORDER BY timestamp DESC LIMIT 1)
FROM runtime_storage_metrics WHERE id=1
