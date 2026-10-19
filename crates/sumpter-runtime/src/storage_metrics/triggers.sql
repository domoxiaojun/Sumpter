DROP TRIGGER IF EXISTS runtime_storage_metrics_insert_v1;
DROP TRIGGER IF EXISTS runtime_storage_metrics_update_v1;
DROP TRIGGER IF EXISTS runtime_storage_metrics_delete_v1;

CREATE TRIGGER runtime_storage_metrics_insert_v1 AFTER INSERT ON runtime_events
BEGIN
    UPDATE runtime_storage_metrics SET
        event_count=event_count+1,
        completed_events=completed_events+(NEW.is_in_flight=0),
        in_flight_events=in_flight_events+(NEW.is_in_flight=1),
        payload_bytes=payload_bytes+NEW.payload_bytes
    WHERE id=1;
END;

CREATE TRIGGER runtime_storage_metrics_update_v1
AFTER UPDATE OF is_in_flight,payload_bytes ON runtime_events
WHEN OLD.is_in_flight<>NEW.is_in_flight OR OLD.payload_bytes<>NEW.payload_bytes
BEGIN
    UPDATE runtime_storage_metrics SET
        completed_events=completed_events-(OLD.is_in_flight=0)+(NEW.is_in_flight=0),
        in_flight_events=in_flight_events-(OLD.is_in_flight=1)+(NEW.is_in_flight=1),
        payload_bytes=payload_bytes-OLD.payload_bytes+NEW.payload_bytes
    WHERE id=1;
END;

CREATE TRIGGER runtime_storage_metrics_delete_v1 AFTER DELETE ON runtime_events
BEGIN
    UPDATE runtime_storage_metrics SET
        event_count=event_count-1,
        completed_events=completed_events-(OLD.is_in_flight=0),
        in_flight_events=in_flight_events-(OLD.is_in_flight=1),
        payload_bytes=payload_bytes-OLD.payload_bytes
    WHERE id=1;
END;
