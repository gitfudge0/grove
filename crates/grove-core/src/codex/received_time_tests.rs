use super::*;

#[test]
fn receipt_time_survives_streaming_without_backfilling_history() {
    let mut item = json!({"id":"reply", "type":"agentMessage", "text":"first"});
    stamp_message_received(&mut item, None);
    let timestamp = item["groveReceivedAt"].as_u64().unwrap();
    assert!(timestamp > 0);
    let mut replacement = json!({"id":"reply", "type":"agentMessage", "text":"complete"});
    stamp_message_received(&mut replacement, Some(&item));
    assert_eq!(replacement["groveReceivedAt"], timestamp);
    let historical = json!({"id":"old", "type":"userMessage"});
    let mut replacement = historical.clone();
    stamp_message_received(&mut replacement, Some(&historical));
    assert!(replacement.get("groveReceivedAt").is_none());
}
