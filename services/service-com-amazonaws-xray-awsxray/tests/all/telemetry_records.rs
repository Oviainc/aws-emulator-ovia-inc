// SPDX-License-Identifier: CC0-1.0

//! `PutTelemetryRecords` — the X-Ray daemon's counter upload, end to end over
//! the wire.
//!
//! Every case drives the same front door the production router does: a
//! `MultiServiceRouter` with this service mounted, addressed by the `xray`
//! endpoint host, speaking restJson1.
//!
//! `PutTelemetryRecordsResult` models NO members and X-Ray publishes no
//! operation that reads telemetry back
//! (`smithy_query(query="closure",operation="PutTelemetryRecords")`), so the
//! response body cannot be what the ingest asserts: these cases assert the
//! service's own store, reached through the SAME shared engine the router hands
//! the handler. The rules checked are:
//!
//! * an accepted call persists every submitted record under the reporting source
//!   the request identifies, and answers 200 with an empty body;
//! * records accumulate across calls — each source's totals are the sums over
//!   every record it ever had accepted, one field per modelled counter;
//! * an absent counter contributes nothing, rather than counting as zero-or-one;
//! * the three reporter members identify the source together, and an
//!   empty-string member identifies nothing, so it reports as the same source as
//!   an omitted one;
//! * caller-driven work and growth are bounded: the per-call batch, the records
//!   kept per source and the number of sources each have a cap, reported as a
//!   refusal or as an eviction and never silently exceeded;
//! * an invalid parameter (a negative counter, an over-long batch) is refused
//!   with `InvalidRequestException` and persists NOTHING.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_xray_awsxray::state::{
    TelemetryReporter, TelemetrySource, TelemetryTotals, MAX_TELEMETRY_RECORDS_PER_CALL,
    MAX_TELEMETRY_RECORDS_PER_SOURCE, MAX_TELEMETRY_SOURCES,
};
use service_com_amazonaws_xray_awsxray::ComAmazonawsXrayAwsxrayState;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator with an empty store, holding the engine the router shares with
/// every handler of this service — the only way to observe an operation whose
/// result shape has no members.
struct Xray {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// restJson1 resolves an error shape from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Xray {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // These tests exercise handlers, not SigV4; the signing name still has to
        // route, which the `Host` header below does.
        router.set_request_authentication(false);
        Self { router, engine }
    }

    async fn call(&self, path: &str, body: &Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://xray.us-east-1.amazonaws.com{path}"))
            .header("host", "xray.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Answer {
            status,
            error_type,
            body,
        }
    }

    async fn put_telemetry(&self, body: &Value) -> Answer {
        self.call("/TelemetryRecords", body).await
    }

    /// The stored reporter for `source`, if the store holds one.
    fn reporter(&self, source: &TelemetrySource) -> Option<TelemetryReporter> {
        self.engine
            .state::<ComAmazonawsXrayAwsxrayState>()
            .lock()
            .telemetry
            .get(source)
            .cloned()
    }

    /// How many reporting sources the store holds.
    fn source_count(&self) -> usize {
        self.engine
            .state::<ComAmazonawsXrayAwsxrayState>()
            .lock()
            .telemetry
            .len()
    }
}

/// One submitted `TelemetryRecord`, as the row of a table: its `Timestamp` plus
/// each counter the row chooses to carry.
///
/// The members are the modelled ones — `SegmentsReceivedCount`,
/// `SegmentsSentCount`, `SegmentsSpilloverCount`, `SegmentsRejectedCount` and
/// `BackendConnectionErrors`'s `TimeoutCount`, `ConnectionRefusedCount`,
/// `HTTPCode4XXCount`, `HTTPCode5XXCount`, `UnknownHostCount`, `OtherCount`
/// (`smithy_query(query="closure",operation="PutTelemetryRecords")`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Record {
    timestamp: i64,
    received: Option<i32>,
    sent: Option<i32>,
    spillover: Option<i32>,
    rejected: Option<i32>,
    /// `true` once any backend counter below is carried: an absent
    /// `BackendConnectionErrors` is distinct from a present, empty one.
    backend: bool,
    timeout: Option<i32>,
    refused: Option<i32>,
    http4xx: Option<i32>,
    http5xx: Option<i32>,
    unknown_host: Option<i32>,
    other: Option<i32>,
}

impl Record {
    /// A record carrying only its required `Timestamp`.
    fn at(timestamp: i64) -> Self {
        Self {
            timestamp,
            ..Self::default()
        }
    }

    fn segments(mut self, received: i32, sent: i32) -> Self {
        self.received = Some(received);
        self.sent = Some(sent);
        self
    }

    fn spillover(mut self, spillover: i32) -> Self {
        self.spillover = Some(spillover);
        self
    }

    fn rejected(mut self, rejected: i32) -> Self {
        self.rejected = Some(rejected);
        self
    }

    /// Every backend counter at once.
    fn backend_errors(mut self, counts: [i32; 6]) -> Self {
        self.backend = true;
        self.timeout = Some(counts[0]);
        self.refused = Some(counts[1]);
        self.http4xx = Some(counts[2]);
        self.http5xx = Some(counts[3]);
        self.unknown_host = Some(counts[4]);
        self.other = Some(counts[5]);
        self
    }

    /// A present but empty `BackendConnectionErrors`.
    fn empty_backend_errors(mut self) -> Self {
        self.backend = true;
        self
    }

    /// This record as the wire JSON a daemon would send.
    fn to_json(self) -> Value {
        let mut record = json!({ "Timestamp": self.timestamp });
        let mut set = |member: &str, count: Option<i32>| {
            if let Some(count) = count {
                record[member] = json!(count);
            }
        };
        set("SegmentsReceivedCount", self.received);
        set("SegmentsSentCount", self.sent);
        set("SegmentsSpilloverCount", self.spillover);
        set("SegmentsRejectedCount", self.rejected);
        if self.backend {
            let mut errors = json!({});
            let mut set = |member: &str, count: Option<i32>| {
                if let Some(count) = count {
                    errors[member] = json!(count);
                }
            };
            set("TimeoutCount", self.timeout);
            set("ConnectionRefusedCount", self.refused);
            set("HTTPCode4XXCount", self.http4xx);
            set("HTTPCode5XXCount", self.http5xx);
            set("UnknownHostCount", self.unknown_host);
            set("OtherCount", self.other);
            record["BackendConnectionErrors"] = errors;
        }
        record
    }
}

/// The totals a batch must add up to, summed independently of the service.
fn expected_totals(records: &[Record]) -> TelemetryTotals {
    let sum = |pick: fn(&Record) -> Option<i32>| -> i64 {
        records
            .iter()
            .filter_map(pick)
            .map(i64::from)
            .sum::<i64>()
    };
    TelemetryTotals {
        segments_received: sum(|record| record.received),
        segments_sent: sum(|record| record.sent),
        segments_spillover: sum(|record| record.spillover),
        segments_rejected: sum(|record| record.rejected),
        backend_timeouts: sum(|record| record.timeout),
        backend_connection_refused: sum(|record| record.refused),
        backend_http_code_4xx: sum(|record| record.http4xx),
        backend_http_code_5xx: sum(|record| record.http5xx),
        backend_unknown_host: sum(|record| record.unknown_host),
        backend_other: sum(|record| record.other),
    }
}

/// A stored `TelemetryRecord` read back as the test's own row, so a case can
/// compare what the store holds against what it submitted.
fn stored_as_row(
    record: &service_com_amazonaws_xray_awsxray::ops::types::TelemetryRecord,
) -> Record {
    let errors = record.backend_connection_errors.as_ref();
    Record {
        timestamp: record.timestamp.secs(),
        received: record.segments_received_count,
        sent: record.segments_sent_count,
        spillover: record.segments_spillover_count,
        rejected: record.segments_rejected_count,
        backend: errors.is_some(),
        timeout: errors.and_then(|errors| errors.timeout_count),
        refused: errors.and_then(|errors| errors.connection_refused_count),
        http4xx: errors.and_then(|errors| errors.http_code4_xx_count),
        http5xx: errors.and_then(|errors| errors.http_code5_xx_count),
        unknown_host: errors.and_then(|errors| errors.unknown_host_count),
        other: errors.and_then(|errors| errors.other_count),
    }
}

/// The request body for one batch from one reporting source.
fn body(records: &[Record], source: &[(&str, &str)]) -> Value {
    let mut body = json!({
        "TelemetryRecords": records.iter().copied().map(Record::to_json).collect::<Vec<_>>(),
    });
    for (member, value) in source {
        body[*member] = json!(value);
    }
    body
}

/// One row of the ingest table: its label, the batch it submits and the
/// reporter members the request carries.
type IngestRow = (&'static str, Vec<Record>, Vec<(&'static str, &'static str)>);

#[tokio::test]
async fn an_accepted_batch_is_persisted_under_its_reporting_source() {
    // Rows differ along what the rule generalises over: how many records the
    // batch carries (none, one, several), which counters each record carries
    // (none, some, every one, a present-but-empty `BackendConnectionErrors`),
    // and which members identify the reporter.
    let rows: Vec<IngestRow> = vec![
        (
            "empty batch, ec2 reporter",
            vec![],
            vec![("EC2InstanceId", "i-0123456789abcdef")],
        ),
        (
            "one record, only the required Timestamp",
            vec![Record::at(1_480_615_200)],
            vec![("Hostname", "ip-10-0-0-1.ec2.internal")],
        ),
        (
            "one record, segment counters only",
            vec![Record::at(1_480_615_260).segments(12, 11).spillover(1)],
            vec![("EC2InstanceId", "i-00000000000000001")],
        ),
        (
            "three records, every counter the model declares",
            vec![
                Record::at(1_480_615_300)
                    .segments(5, 5)
                    .rejected(0)
                    .backend_errors([1, 2, 3, 4, 5, 6]),
                Record::at(1_480_615_360)
                    .segments(7, 6)
                    .spillover(1)
                    .rejected(2)
                    .backend_errors([0, 0, 1, 0, 2, 0]),
                Record::at(1_480_615_420).backend_errors([9, 8, 7, 6, 5, 4]),
            ],
            vec![
                ("EC2InstanceId", "i-0abcdef012345678"),
                ("Hostname", "worker-1.internal"),
                ("ResourceARN", "arn:aws:ecs:us-east-1:123456789012:task/abc"),
            ],
        ),
        (
            "a present but empty BackendConnectionErrors contributes nothing",
            vec![Record::at(1_480_615_480).empty_backend_errors()],
            vec![("ResourceARN", "arn:aws:ec2:us-east-1:123456789012:instance/i-1")],
        ),
        (
            "ten records, no reporter member at all",
            (0..10)
                .map(|step| Record::at(1_480_615_500 + i64::from(step)).segments(step, step))
                .collect(),
            vec![],
        ),
    ];

    for (label, records, source) in rows {
        let xray = Xray::new();
        let answer = xray.put_telemetry(&body(&records, &source)).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a valid batch must be accepted, got {answer:?}"
        );
        assert_eq!(
            answer.body,
            json!({}),
            "case {label:?}: PutTelemetryRecordsResult models no members, got {:?}",
            answer.body
        );

        let expected_source = TelemetrySource::new(
            source
                .iter()
                .find(|(member, _)| *member == "EC2InstanceId")
                .map(|(_, value)| *value),
            source
                .iter()
                .find(|(member, _)| *member == "Hostname")
                .map(|(_, value)| *value),
            source
                .iter()
                .find(|(member, _)| *member == "ResourceARN")
                .map(|(_, value)| *value),
        );
        let reporter = xray
            .reporter(&expected_source)
            .unwrap_or_else(|| panic!("case {label:?}: nothing was persisted for the reporter"));
        assert_eq!(
            reporter.records.len(),
            records.len(),
            "case {label:?}: expected {} stored records, got {:?}",
            records.len(),
            reporter.records.len()
        );
        assert_eq!(
            reporter.records_accepted,
            records.len() as u64,
            "case {label:?}: expected {} accepted records, got {:?}",
            records.len(),
            reporter.records_accepted
        );
        assert_eq!(
            reporter.reports, 1,
            "case {label:?}: one call is one report, got {:?}",
            reporter.reports
        );
        let stored: Vec<Record> = reporter.records.iter().map(stored_as_row).collect();
        assert_eq!(
            stored, records,
            "case {label:?}: expected the submitted records stored verbatim and in order, got {stored:?}"
        );
        assert_eq!(
            reporter.totals,
            expected_totals(&records),
            "case {label:?}: expected totals {:?}, got {:?}",
            expected_totals(&records),
            reporter.totals
        );
    }
}

#[tokio::test]
async fn records_accumulate_per_source_across_calls() {
    let xray = Xray::new();
    // Rows differ along the reporter identity: which of the three members the
    // source names, and whether a member is empty rather than absent.
    let daemon_a: Vec<(&str, &str)> = vec![("EC2InstanceId", "i-aaaaaaaaaaaaaaaa1")];
    let daemon_a_with_empties: Vec<(&str, &str)> = vec![
        ("EC2InstanceId", "i-aaaaaaaaaaaaaaaa1"),
        ("Hostname", ""),
        ("ResourceARN", ""),
    ];
    let daemon_b: Vec<(&str, &str)> = vec![("Hostname", "host-b.internal")];

    let first = vec![Record::at(1_480_615_200).segments(10, 9).spillover(1)];
    let second = vec![
        Record::at(1_480_615_260).segments(4, 4).rejected(1),
        Record::at(1_480_615_320).backend_errors([1, 0, 2, 0, 0, 3]),
    ];
    let third = vec![Record::at(1_480_615_380).segments(100, 100)];

    for (label, records, source) in [
        ("A's first report", &first, &daemon_a),
        ("A's second report", &second, &daemon_a),
        (
            "A's third report, with empty-string members",
            &third,
            &daemon_a_with_empties,
        ),
        ("B's only report", &first, &daemon_b),
    ] {
        let answer = xray.put_telemetry(&body(records, source)).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {answer:?}"
        );
    }

    // An empty-string member identifies nothing, so A's three calls are ONE
    // source: the store holds exactly two.
    assert_eq!(
        xray.source_count(),
        2,
        "expected 2 reporting sources, got {:?}",
        xray.source_count()
    );

    let all_of_a: Vec<Record> = first
        .iter()
        .chain(second.iter())
        .chain(third.iter())
        .copied()
        .collect();
    let a = xray
        .reporter(&TelemetrySource::new(Some("i-aaaaaaaaaaaaaaaa1"), None, None))
        .expect("case \"A\": A's telemetry is persisted");
    assert_eq!(
        a.reports, 3,
        "case \"A\": three calls are three reports, got {:?}",
        a.reports
    );
    assert_eq!(
        a.records_accepted,
        all_of_a.len() as u64,
        "case \"A\": expected {} accepted records, got {:?}",
        all_of_a.len(),
        a.records_accepted
    );
    let stored: Vec<Record> = a.records.iter().map(stored_as_row).collect();
    assert_eq!(
        stored, all_of_a,
        "case \"A\": a later call appends rather than replacing, got {stored:?}"
    );
    assert_eq!(
        a.totals,
        expected_totals(&all_of_a),
        "case \"A\": expected the sums over every accepted record {:?}, got {:?}",
        expected_totals(&all_of_a),
        a.totals
    );

    // B's counters are its own: one source's report never lands on another's.
    let b = xray
        .reporter(&TelemetrySource::new(None, Some("host-b.internal"), None))
        .expect("case \"B\": B's telemetry is persisted");
    assert_eq!(
        b.totals,
        expected_totals(&first),
        "case \"B\": expected only B's own report {:?}, got {:?}",
        expected_totals(&first),
        b.totals
    );
    assert_eq!(
        b.reports, 1,
        "case \"B\": one call is one report, got {:?}",
        b.reports
    );
}

#[tokio::test]
async fn an_invalid_parameter_is_refused_and_persists_nothing() {
    // One row per modelled counter, so the refusal is not keyed to a single
    // member, plus the batch bound at its published value and one past it.
    let negatives: Vec<(&str, Record)> = vec![
        (
            "SegmentsReceivedCount",
            Record {
                received: Some(-1),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "SegmentsSentCount",
            Record {
                sent: Some(-7),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "SegmentsSpilloverCount",
            Record {
                spillover: Some(-2),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "SegmentsRejectedCount",
            Record {
                rejected: Some(i32::MIN),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "BackendConnectionErrors.TimeoutCount",
            Record {
                backend: true,
                timeout: Some(-1),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "BackendConnectionErrors.ConnectionRefusedCount",
            Record {
                backend: true,
                refused: Some(-3),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "BackendConnectionErrors.HTTPCode4XXCount",
            Record {
                backend: true,
                http4xx: Some(-4),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "BackendConnectionErrors.HTTPCode5XXCount",
            Record {
                backend: true,
                http5xx: Some(-5),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "BackendConnectionErrors.UnknownHostCount",
            Record {
                backend: true,
                unknown_host: Some(-6),
                ..Record::at(1_480_615_200)
            },
        ),
        (
            "BackendConnectionErrors.OtherCount",
            Record {
                backend: true,
                other: Some(-8),
                ..Record::at(1_480_615_200)
            },
        ),
    ];

    for (member, bad) in negatives {
        let xray = Xray::new();
        // The bad record sits behind a good one, so a refusal must not have
        // persisted the records it already walked.
        let records = vec![Record::at(1_480_615_100).segments(1, 1), bad];
        let refused = xray.put_telemetry(&body(&records, &[])).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {member:?}: a negative counter is an invalid parameter, got {refused:?}"
        );
        assert!(
            refused.body.to_string().contains(member),
            "case {member:?}: the refusal must name the member at fault, got {:?}",
            refused.body
        );
        assert!(
            xray.reporter(&TelemetrySource::default()).is_none(),
            "case {member:?}: a refused call must persist nothing, got {:?}",
            xray.reporter(&TelemetrySource::default())
        );
    }

    // The batch bound: at the cap, and one past it.
    for (count, expect_status) in [
        (MAX_TELEMETRY_RECORDS_PER_CALL, 200),
        (MAX_TELEMETRY_RECORDS_PER_CALL + 1, 400),
    ] {
        let xray = Xray::new();
        let records: Vec<Record> = (0..count)
            .map(|step| Record::at(1_480_615_200 + step as i64).segments(1, 1))
            .collect();
        let answer = xray.put_telemetry(&body(&records, &[])).await;
        assert_eq!(
            answer.status, expect_status,
            "case {count} records: expected {expect_status}, got {answer:?}"
        );
        let stored = xray
            .reporter(&TelemetrySource::default())
            .map_or(0, |reporter| reporter.records.len());
        let expected_stored = if expect_status == 200 { count } else { 0 };
        assert_eq!(
            stored, expected_stored,
            "case {count} records: expected {expected_stored} stored, got {stored:?}"
        );
        if expect_status == 400 {
            assert_eq!(
                answer.error_type.as_deref(),
                Some("InvalidRequestException"),
                "case {count} records: an over-long batch is an invalid request, got {answer:?}"
            );
        }
    }
}

#[tokio::test]
async fn growth_is_bounded_by_the_record_window_and_the_source_cap() {
    // Per-source window: submit past the cap and the OLDEST records fall out,
    // while the running totals keep counting every record accepted.
    let xray = Xray::new();
    let source = TelemetrySource::new(Some("i-window00000000001"), None, None);
    let batch = MAX_TELEMETRY_RECORDS_PER_CALL;
    let calls = MAX_TELEMETRY_RECORDS_PER_SOURCE / batch + 1;
    let mut submitted: Vec<Record> = Vec::new();
    for call in 0..calls {
        let records: Vec<Record> = (0..batch)
            .map(|step| Record::at(1_000_000 + (call * batch + step) as i64).segments(1, 0))
            .collect();
        let answer = xray
            .put_telemetry(&body(&records, &[("EC2InstanceId", "i-window00000000001")]))
            .await;
        assert_eq!(
            answer.status, 200,
            "case window call {call}: expected 200, got {answer:?}"
        );
        submitted.extend(records);
    }

    let reporter = xray
        .reporter(&source)
        .expect("case \"window\": the reporter is persisted");
    assert_eq!(
        reporter.records.len(),
        MAX_TELEMETRY_RECORDS_PER_SOURCE,
        "case \"window\": the window must hold at most {MAX_TELEMETRY_RECORDS_PER_SOURCE}, got {:?}",
        reporter.records.len()
    );
    assert_eq!(
        reporter.records_accepted,
        submitted.len() as u64,
        "case \"window\": expected {} accepted records, got {:?}",
        submitted.len(),
        reporter.records_accepted
    );
    let kept: Vec<Record> = reporter.records.iter().map(stored_as_row).collect();
    let newest = &submitted[submitted.len() - MAX_TELEMETRY_RECORDS_PER_SOURCE..];
    assert_eq!(
        kept, newest,
        "case \"window\": the window must keep the most recent records, got {:?}..{:?}",
        kept.first(),
        kept.last()
    );
    assert_eq!(
        reporter.totals,
        expected_totals(&submitted),
        "case \"window\": totals count every accepted record, not just the window, got {:?}",
        reporter.totals
    );

    // Source cap: one report from each of cap + 1 distinct sources, and the
    // least recently reporting one is evicted rather than the store growing.
    let xray = Xray::new();
    let hostname = |index: usize| format!("host-{index}.internal");
    for index in 0..=MAX_TELEMETRY_SOURCES {
        let answer = xray
            .put_telemetry(&body(
                &[Record::at(2_000_000 + index as i64).segments(1, 1)],
                &[("Hostname", hostname(index).as_str())],
            ))
            .await;
        assert_eq!(
            answer.status, 200,
            "case source {index}: expected 200, got {answer:?}"
        );
    }
    assert_eq!(
        xray.source_count(),
        MAX_TELEMETRY_SOURCES,
        "case \"source cap\": at most {MAX_TELEMETRY_SOURCES} sources are kept, got {:?}",
        xray.source_count()
    );
    assert!(
        xray.reporter(&TelemetrySource::new(None, Some(&hostname(0)), None))
            .is_none(),
        "case \"source cap\": the least recently reporting source must be evicted"
    );
    assert!(
        xray.reporter(&TelemetrySource::new(
            None,
            Some(&hostname(MAX_TELEMETRY_SOURCES)),
            None
        ))
        .is_some(),
        "case \"source cap\": the newest source must be kept"
    );
}
