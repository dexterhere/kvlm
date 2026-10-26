use super::*;

fn record(gaps_ms: &[u64], reported: Option<u64>) -> StreamRecord {
    let mut record = StreamRecord {
        reported_tokens: reported,
        ..StreamRecord::default()
    };
    let mut at = 400;
    record.push_content(Duration::from_millis(at));
    for gap in gaps_ms {
        at += gap;
        record.push_content(Duration::from_millis(at));
    }
    record
}

#[test]
fn nearest_rank_matches_the_textbook_definition() {
    let v: Vec<f64> = (1..=10).map(f64::from).collect();
    assert_eq!(nearest_rank(&v, 50), 5.0);
    assert_eq!(nearest_rank(&v, 95), 10.0);
    assert_eq!(nearest_rank(&v, 10), 1.0);
    assert_eq!(nearest_rank(&[7.0], 95), 7.0);
}

#[test]
fn a_single_token_per_event_stream_gives_itl_and_ttft() {
    let gaps = [10, 10, 10, 10, 10, 10, 10, 10, 10, 100];
    let record = record(&gaps, Some(11));
    assert_eq!(record.ttft_ms(), Some(400.0));
    let itl = record.itl().unwrap();
    assert_eq!(itl.intervals, 10);
    assert_eq!(itl.median_ms, 10.0);
    assert_eq!(itl.p95_ms, 100.0);
}

#[test]
fn every_way_the_proof_can_fail_leaves_itl_unknown_with_a_reason() {
    let gaps = [10; 12];
    let reason = |r: StreamRecord| r.itl().unwrap_err();

    assert!(reason(record(&gaps, None)).contains("did not report"));
    assert!(reason(record(&gaps, Some(40))).contains("13 stream events for 40"));
    assert!(reason(record(&gaps, Some(5))).contains("13 stream events for 5"));

    let mut merged = record(&gaps, Some(13));
    merged.events_merged = true;
    assert!(reason(merged).contains("combined"));

    let mut reasoning = record(&gaps, Some(13));
    reasoning.reasoning_events = 2;
    assert!(reason(reasoning).contains("reasoning"));

    assert!(reason(record(&[10; 7], Some(8))).contains("only 7 intervals"));
    assert!(reason(record(&[], Some(1))).contains("only 0 intervals"));
    assert!(reason(StreamRecord::default()).contains("did not report"));

    let mut unordered = record(&gaps, Some(13));
    unordered.content_arrivals.swap(3, 4);
    assert!(reason(unordered).contains("not in order"));
}

#[test]
fn a_stream_past_the_cap_is_not_guessed_at() {
    let mut record = StreamRecord {
        reported_tokens: Some(MAX_EVENTS as u64 + 10),
        ..StreamRecord::default()
    };
    for i in 0..MAX_EVENTS as u64 + 10 {
        record.push_content(Duration::from_millis(i));
    }
    assert_eq!(record.content_arrivals.len(), MAX_EVENTS);
    assert!(record.truncated);
    assert!(record.itl().unwrap_err().contains("longer than"));
}

#[test]
fn speed_uses_server_counters_and_refuses_empty_or_unmeasurable_answers() {
    assert_eq!(tokens_per_second(Some(50), Some(1000.0)), Some(50.0));
    assert_eq!(tokens_per_second(Some(0), Some(1000.0)), None);
    assert_eq!(tokens_per_second(Some(1), Some(0.0)), None);
    assert_eq!(tokens_per_second(Some(1), Some(-4.0)), None);
    assert_eq!(tokens_per_second(Some(1), Some(f64::NAN)), None);
    assert_eq!(tokens_per_second(Some(1), Some(f64::INFINITY)), None);
    assert_eq!(tokens_per_second(None, Some(10.0)), None);
    assert_eq!(tokens_per_second(Some(5), None), None);
}
