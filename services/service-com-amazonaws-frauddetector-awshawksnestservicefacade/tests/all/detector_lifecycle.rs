// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Fraud Detector detector lifecycle.
//!
//! Every case drives the service over its real wire protocol (`awsJson1_1`, `POST /`
//! with `X-Amz-Target: AWSHawksNestServiceFacade.<Op>`) through the same
//! `MultiServiceRouter` the emulator boots, so what is asserted here is what a client
//! sees: ids allocated by the service, values read back out of its state, and the
//! modelled errors with their modelled status codes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `RuleExecutionMode` values the model declares, from
/// `src/ops/types/shared_types.rs`'s `RULE_EXECUTION_MODE_VALUES` (generated from the
/// Smithy model's `RuleExecutionMode` enum).
const RULE_EXECUTION_MODES: &[&str] = &["ALL_MATCHED", "FIRST_MATCHED"];

/// The `DetectorVersionStatus` values the model declares, from
/// `src/ops/types/shared_types.rs`'s `DETECTOR_VERSION_STATUS_VALUES`.
const DETECTOR_VERSION_STATUSES: &[&str] = &["ACTIVE", "DRAFT", "INACTIVE"];

/// One emulator: a single shared engine behind the router, so every call in a test sees
/// the same state, exactly as the booted emulator does.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4: the `Authorization` header
        // below is still required, because it is what routes the request to this
        // service's signing name.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_frauddetector_awshawksnestservicefacade::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Call one operation and return `(status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSHawksNestServiceFacade.{operation}"),
            )
            // The SigV4 credential scope is the routing key: its 4th field is this
            // service's signing name and its 3rd is the region the ARNs carry.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/frauddetector/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0123456789abcdef",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation, requiring a 200.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected HTTP 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// `PutDetector` with the minimum members.
    async fn put_detector(&self, detector_id: &str) -> Value {
        self.ok(
            "PutDetector",
            json!({"detectorId": detector_id, "eventTypeName": "payment_event"}),
        )
        .await
    }
}

/// The wire error code (`__type`) of an error response, with any namespace prefix
/// stripped — `com.amazonaws.frauddetector#ValidationException` -> `ValidationException`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '.'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// An ISO-8601 timestamp this service rendered, as epoch milliseconds.
///
/// Timestamps must be COMPARED as instants, not as strings: the writer trims a
/// trailing-zero (and an all-zero) millisecond fraction, so `…:00Z` and `…:00.101Z`
/// are the same second but sort the wrong way round lexicographically (`'Z' > '0'`).
fn timestamp_millis(case: &str, rendered: &str) -> i64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        rendered,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|e| panic!("case {case:?}: {rendered:?} is not an ISO-8601 timestamp: {e}"));
    parsed.to_millis().unwrap_or_else(|e| {
        panic!("case {case:?}: {rendered:?} is not representable in milliseconds: {e}")
    })
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn string_member(body: &Value, name: &str) -> String {
    member(body, name)
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A detector ARN must be the modelled `fraudDetectorArn` shape:
/// `arn:aws:frauddetector:<region>:<account12>:<resource>`.
fn assert_fraud_detector_arn(case: &str, arn: &str, expected_resource: &str) {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected a 6-field ARN, got {arn:?}"
    );
    assert_eq!(
        (fields[0], fields[1], fields[2]),
        ("arn", "aws", "frauddetector"),
        "case {case:?}: expected an aws frauddetector ARN, got {arn:?}"
    );
    assert!(
        (3..=20).contains(&fields[3].len())
            && fields[3]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "case {case:?}: region segment {:?} is outside the modelled [a-z0-9-]{{3,20}}, in {arn:?}",
        fields[3]
    );
    assert!(
        fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: account segment {:?} is not 12 digits, in {arn:?}",
        fields[4]
    );
    assert_eq!(
        fields[5], expected_resource,
        "case {case:?}: wrong ARN resource in {arn:?}"
    );
}

/// An ISO-8601 instant within the `time` shape's modelled length of 11..30.
fn assert_timestamp(case: &str, value: &str) {
    assert!(
        (11..=30).contains(&value.len()),
        "case {case:?}: timestamp {value:?} is outside the modelled length 11..30"
    );
    assert!(
        value.len() > 11 && value.as_bytes()[10] == b'T' && value.ends_with('Z'),
        "case {case:?}: timestamp {value:?} is not an ISO-8601 UTC instant"
    );
}

// ---------------------------------------------------------------------------
// PutDetector + GetDetectors: what was created is what is reported
// ---------------------------------------------------------------------------

#[tokio::test]
async fn created_detectors_are_reported_with_the_values_they_were_created_with() {
    let h = Harness::new();

    // The class this spans: the optional members present or absent, and ids/event types
    // of different shapes (digits, underscores, hyphens, the modelled 64-char maximum).
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("my_detector", "my_event_type", Some("demo")),
        ("detector-2", "event-2", None),
        ("d3", "e3", Some("a")),
        (
            "aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgggg",
            "0123456789",
            Some("the modelled 64-character maximum id"),
        ),
    ];

    for (detector_id, event_type_name, description) in cases {
        let mut request = json!({"detectorId": detector_id, "eventTypeName": event_type_name});
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        let created = h.ok("PutDetector", request).await;
        assert_eq!(
            created,
            json!({}),
            "case {detector_id:?}: PutDetector models no output members, got {created:?}"
        );
    }

    let listed = h.ok("GetDetectors", json!({})).await;
    let detectors = member(&listed, "detectors")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        detectors.len(),
        cases.len(),
        "expected every created detector to be listed, got {detectors:?}"
    );

    for (detector_id, event_type_name, description) in cases {
        let found = detectors
            .iter()
            .find(|d| string_member(d, "detectorId") == *detector_id)
            .unwrap_or_else(|| panic!("case {detector_id:?}: not listed in {detectors:?}"));
        assert_eq!(
            string_member(found, "eventTypeName"),
            *event_type_name,
            "case {detector_id:?}: wrong eventTypeName in {found:?}"
        );
        assert_eq!(
            member(found, "description").as_str(),
            *description,
            "case {detector_id:?}: wrong description in {found:?}"
        );
        assert_fraud_detector_arn(
            detector_id,
            &string_member(found, "arn"),
            &format!("detector/{detector_id}"),
        );
        assert_timestamp(detector_id, &string_member(found, "createdTime"));
        assert_timestamp(detector_id, &string_member(found, "lastUpdatedTime"));
    }

    // The single-detector filter answers out of the same records.
    for (detector_id, event_type_name, _) in cases {
        let filtered = h
            .ok("GetDetectors", json!({"detectorId": detector_id}))
            .await;
        let detectors = member(&filtered, "detectors")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            detectors.len(),
            1,
            "case {detector_id:?}: a filtered GetDetectors must report exactly that detector, got {detectors:?}"
        );
        assert_eq!(
            string_member(&detectors[0], "eventTypeName"),
            *event_type_name,
            "case {detector_id:?}: wrong eventTypeName in {:?}",
            detectors[0]
        );
    }

    let (status, body) = h
        .call("GetDetectors", json!({"detectorId": "no_such_detector"}))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {:?}: filtering on an unknown detector must be a 404, got {body:?}",
        "no_such_detector"
    );
}

#[tokio::test]
async fn a_second_put_detector_updates_the_same_record() {
    let h = Harness::new();
    h.ok(
        "PutDetector",
        json!({"detectorId": "upsert_me", "eventTypeName": "first_event", "description": "first"}),
    )
    .await;
    let first = h.ok("GetDetectors", json!({"detectorId": "upsert_me"})).await;
    let first = member(&first, "detectors")[0].clone();

    // Each row changes a different member of the SAME id; none may create a second
    // record or move `createdTime`.
    let cases: &[(&str, Option<&str>)] = &[
        ("second_event", Some("second")),
        ("third_event", None),
        ("third_event", Some("back again")),
    ];
    for (event_type_name, description) in cases {
        let mut request = json!({"detectorId": "upsert_me", "eventTypeName": event_type_name});
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        h.ok("PutDetector", request).await;

        let listed = h.ok("GetDetectors", json!({})).await;
        let detectors = member(&listed, "detectors")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            detectors.len(),
            1,
            "case {event_type_name:?}: an upsert must not duplicate the detector, got {detectors:?}"
        );
        let current = &detectors[0];
        assert_eq!(
            string_member(current, "eventTypeName"),
            *event_type_name,
            "case {event_type_name:?}: eventTypeName was not updated in {current:?}"
        );
        assert_eq!(
            member(current, "description").as_str(),
            *description,
            "case {event_type_name:?}: description was not updated in {current:?}"
        );
        assert_eq!(
            string_member(current, "createdTime"),
            string_member(&first, "createdTime"),
            "case {event_type_name:?}: createdTime must survive an update, got {current:?}"
        );
        assert_eq!(
            string_member(current, "arn"),
            string_member(&first, "arn"),
            "case {event_type_name:?}: the ARN must survive an update, got {current:?}"
        );
        assert!(
            timestamp_millis(
                event_type_name,
                &string_member(current, "lastUpdatedTime")
            ) >= timestamp_millis(event_type_name, &string_member(&first, "createdTime")),
            "case {event_type_name:?}: lastUpdatedTime must not precede createdTime, got {current:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetDetectors paging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_walks_every_detector_once() {
    let h = Harness::new();
    const TOTAL: usize = 23;
    for index in 0..TOTAL {
        h.put_detector(&format!("detector_{index:02}")).await;
    }

    // `maxResults` is modelled with range 5..10, so both published bounds and a value
    // inside the range are exercised; `None` takes the documented default of 10.
    for page_size in [None, Some(5), Some(7), Some(10)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(page_size) = page_size {
                request["maxResults"] = json!(page_size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let page = h.ok("GetDetectors", request).await;
            let detectors = member(&page, "detectors")
                .as_array()
                .cloned()
                .unwrap_or_default();
            let expected_max = page_size.unwrap_or(10) as usize;
            assert!(
                detectors.len() <= expected_max,
                "case {page_size:?}: a page must hold at most maxResults entries, got {}",
                detectors.len()
            );
            for detector in &detectors {
                seen.push(string_member(detector, "detectorId"));
            }
            pages += 1;
            assert!(
                pages <= TOTAL + 1,
                "case {page_size:?}: paging did not terminate after {pages} pages"
            );
            match member(&page, "nextToken").as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {page_size:?}: paging repeated a detector: {seen:?}"
        );
        assert_eq!(
            seen.len(),
            TOTAL,
            "case {page_size:?}: paging must report every detector exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn paging_refuses_a_page_size_or_token_it_never_issued() {
    let h = Harness::new();
    for index in 0..6 {
        h.put_detector(&format!("detector_{index:02}")).await;
    }

    // `maxResults` is modelled with range 5..10: each published bound is served, and one
    // past each bound is refused rather than quietly reshaped.
    let page_sizes: &[(i32, u16)] = &[(4, 400), (5, 200), (10, 200), (11, 400), (0, 400)];
    for (max_results, expected_status) in page_sizes {
        let (status, body) = h
            .call("GetDetectors", json!({"maxResults": max_results}))
            .await;
        assert_eq!(
            status, *expected_status,
            "case maxResults={max_results}: wrong status, got {body:?}"
        );
        if *expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case maxResults={max_results}: wrong error code, got {body:?}"
            );
        }
    }

    // A pagination token the service never issued names no position — whatever shape it
    // takes: empty, non-numeric, naming an id that was never allocated, or "0", which no
    // allocator ever hands out (both token spaces start at 1).
    let bad_tokens: &[&str] = &["", "not-a-detector", "detector_99", "0", "99"];
    for token in bad_tokens {
        let (status, body) = h.call("GetDetectors", json!({"nextToken": token})).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException".to_string()),
            "case {token:?}: an unissued GetDetectors token must be refused, got {body:?}"
        );
        let (status, body) = h
            .call(
                "DescribeDetector",
                json!({"detectorId": "detector_00", "nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException".to_string()),
            "case {token:?}: an unissued DescribeDetector token must be refused, got {body:?}"
        );
    }

    // `DescribeDetector`'s own modelled range is 1000..2500, and it states the same
    // rule: both bounds are served, one past each is refused.
    let describe_page_sizes: &[(i32, u16)] = &[(999, 400), (1000, 200), (2500, 200), (2501, 400)];
    for (max_results, expected_status) in describe_page_sizes {
        let (status, body) = h
            .call(
                "DescribeDetector",
                json!({"detectorId": "detector_00", "maxResults": max_results}),
            )
            .await;
        assert_eq!(
            status, *expected_status,
            "case DescribeDetector maxResults={max_results}: wrong status, got {body:?}"
        );
        if *expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case DescribeDetector maxResults={max_results}: wrong error code, got {body:?}"
            );
        }
    }

    // A `detectorId` filter is never paged, so a token on that path was never issued
    // either — even one naming a real detector.
    let (status, body) = h
        .call(
            "GetDetectors",
            json!({"detectorId": "detector_00", "nextToken": "detector_01"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException".to_string()),
        "a filtered GetDetectors must refuse a pagination token, got {body:?}"
    );
}

#[tokio::test]
async fn rule_versions_are_allocated_per_detector_and_rule() {
    let h = Harness::new();
    h.put_detector("detector_a").await;
    h.put_detector("detector_b").await;

    // Rows differ in the (detector, rule) pair and in the expression stored, so the
    // allocation is shown to be per `(detectorId, ruleId)` rather than global.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("detector_a", "high_risk", "$amount > 100", "1"),
        ("detector_a", "high_risk", "$amount > 200", "2"),
        ("detector_a", "low_risk", "$amount < 10", "1"),
        ("detector_b", "high_risk", "$amount > 100", "1"),
        ("detector_a", "high_risk", "$amount > 300", "3"),
        ("detector_b", "high_risk", "$amount > 400", "2"),
    ];

    for (detector_id, rule_id, expression, expected_version) in cases {
        let created = h
            .ok(
                "CreateRule",
                json!({
                    "detectorId": detector_id,
                    "ruleId": rule_id,
                    "language": "DETECTORPL",
                    "expression": expression,
                    "outcomes": ["review"],
                }),
            )
            .await;
        let rule = member(&created, "rule");
        assert_eq!(
            (
                string_member(rule, "detectorId"),
                string_member(rule, "ruleId"),
                string_member(rule, "ruleVersion"),
            ),
            (
                (*detector_id).to_string(),
                (*rule_id).to_string(),
                (*expected_version).to_string(),
            ),
            "case {detector_id:?}/{rule_id:?}: wrong rule identity, got {rule:?}"
        );
    }
}

#[tokio::test]
async fn create_rule_refuses_what_it_cannot_attach() {
    let h = Harness::new();
    h.put_detector("known_detector").await;

    // Every row is a request the service cannot satisfy, with the status the model
    // declares for it.
    let cases: &[(&str, Value, u16, &str)] = &[
        (
            "unknown detector",
            json!({
                "detectorId": "missing_detector",
                "ruleId": "r1",
                "language": "DETECTORPL",
                "expression": "$amount > 1",
                "outcomes": ["review"],
            }),
            400,
            "ValidationException",
        ),
        (
            "unmodelled language",
            json!({
                "detectorId": "known_detector",
                "ruleId": "r1",
                "language": "PYTHON",
                "expression": "$amount > 1",
                "outcomes": ["review"],
            }),
            400,
            "ValidationException",
        ),
        (
            "empty outcomes (modelled len >= 1)",
            json!({
                "detectorId": "known_detector",
                "ruleId": "r1",
                "language": "DETECTORPL",
                "expression": "$amount > 1",
                "outcomes": [],
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, body) = h.call("CreateRule", request.clone()).await;
        assert_eq!(
            (status, error_code(&body)),
            (*expected_status, (*expected_code).to_string()),
            "case {case:?}: wrong refusal, got {body:?}"
        );
    }

    // And nothing was stored by any of them.
    let rules_of_detector = h
        .ok(
            "CreateRule",
            json!({
                "detectorId": "known_detector",
                "ruleId": "r1",
                "language": "DETECTORPL",
                "expression": "$amount > 1",
                "outcomes": ["review"],
            }),
        )
        .await;
    assert_eq!(
        string_member(member(&rules_of_detector, "rule"), "ruleVersion"),
        "1",
        "a refused CreateRule must not have consumed a rule version, got {rules_of_detector:?}"
    );
}

// ---------------------------------------------------------------------------
// CreateDetectorVersion + GetDetectorVersion
// ---------------------------------------------------------------------------

/// Create a detector with `rule_count` rules and return the rule references.
async fn detector_with_rules(h: &Harness, detector_id: &str, rule_count: usize) -> Vec<Value> {
    h.put_detector(detector_id).await;
    let mut refs = Vec::new();
    for index in 0..rule_count {
        let created = h
            .ok(
                "CreateRule",
                json!({
                    "detectorId": detector_id,
                    "ruleId": format!("rule_{index}"),
                    "language": "DETECTORPL",
                    "expression": format!("$amount > {index}"),
                    "outcomes": ["review", "block"],
                }),
            )
            .await;
        refs.push(member(&created, "rule").clone());
    }
    refs
}

#[tokio::test]
async fn detector_versions_are_sequential_per_detector_and_read_back_from_state() {
    let h = Harness::new();
    let rules_a = detector_with_rules(&h, "detector_a", 3).await;
    let rules_b = detector_with_rules(&h, "detector_b", 1).await;

    // Rows differ in the detector, the number of rules carried, the execution mode
    // (absent, and each modelled value) and the description.
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, usize, Option<&str>, Option<&str>, &str)] = &[
        ("detector_a", 1, None, None, "1"),
        ("detector_a", 3, Some("ALL_MATCHED"), Some("all three"), "2"),
        ("detector_b", 1, Some("FIRST_MATCHED"), None, "1"),
        ("detector_a", 0, Some("FIRST_MATCHED"), Some("no rules"), "3"),
    ];

    for (detector_id, rule_count, mode, description, expected_version_id) in cases {
        let rules: Vec<Value> = if *detector_id == "detector_a" {
            rules_a.iter().take(*rule_count).cloned().collect()
        } else {
            rules_b.iter().take(*rule_count).cloned().collect()
        };
        let mut request = json!({"detectorId": detector_id, "rules": rules});
        if let Some(mode) = mode {
            request["ruleExecutionMode"] = json!(mode);
        }
        if let Some(description) = description {
            request["description"] = json!(description);
        }

        let created = h.ok("CreateDetectorVersion", request).await;
        assert_eq!(
            (
                string_member(&created, "detectorId"),
                string_member(&created, "detectorVersionId"),
                string_member(&created, "status"),
            ),
            (
                (*detector_id).to_string(),
                (*expected_version_id).to_string(),
                "DRAFT".to_string(),
            ),
            "case {detector_id:?}#{expected_version_id:?}: wrong creation result, got {created:?}"
        );
        assert!(
            DETECTOR_VERSION_STATUSES.contains(&string_member(&created, "status").as_str()),
            "case {detector_id:?}#{expected_version_id:?}: status is outside the modelled enum, got {created:?}"
        );

        // Round-trip: the version reads back out of state with everything it was
        // created with.
        let read = h
            .ok(
                "GetDetectorVersion",
                json!({"detectorId": detector_id, "detectorVersionId": expected_version_id}),
            )
            .await;
        assert_eq!(
            string_member(&read, "detectorVersionId"),
            *expected_version_id,
            "case {detector_id:?}#{expected_version_id:?}: wrong version read back, got {read:?}"
        );
        assert_eq!(
            string_member(&read, "status"),
            "DRAFT",
            "case {detector_id:?}#{expected_version_id:?}: a new version is DRAFT, got {read:?}"
        );
        // An absent `ruleExecutionMode` takes the documented default.
        let expected_mode = mode.unwrap_or("FIRST_MATCHED");
        assert_eq!(
            string_member(&read, "ruleExecutionMode"),
            expected_mode,
            "case {detector_id:?}#{expected_version_id:?}: wrong ruleExecutionMode, got {read:?}"
        );
        assert!(
            RULE_EXECUTION_MODES.contains(&string_member(&read, "ruleExecutionMode").as_str()),
            "case {detector_id:?}#{expected_version_id:?}: ruleExecutionMode is outside the modelled enum, got {read:?}"
        );
        assert_eq!(
            member(&read, "description").as_str(),
            *description,
            "case {detector_id:?}#{expected_version_id:?}: wrong description, got {read:?}"
        );
        assert_eq!(
            member(&read, "rules").as_array().cloned().unwrap_or_default(),
            rules,
            "case {detector_id:?}#{expected_version_id:?}: wrong rules read back, got {read:?}"
        );
        assert_fraud_detector_arn(
            &format!("{detector_id}#{expected_version_id}"),
            &string_member(&read, "arn"),
            &format!("detector-version/{detector_id}/{expected_version_id}"),
        );
        assert_timestamp(detector_id, &string_member(&read, "createdTime"));
        assert_timestamp(detector_id, &string_member(&read, "lastUpdatedTime"));
    }
}

#[tokio::test]
async fn detector_version_reads_and_writes_refuse_what_state_does_not_hold() {
    let h = Harness::new();
    let rules = detector_with_rules(&h, "detector_a", 1).await;
    h.put_detector("detector_b").await;
    h.ok(
        "CreateDetectorVersion",
        json!({"detectorId": "detector_a", "rules": rules}),
    )
    .await;

    let create_cases: &[(&str, Value, u16, &str)] = &[
        (
            "unknown detector",
            json!({"detectorId": "missing_detector", "rules": []}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown rule id",
            json!({"detectorId": "detector_a", "rules": [
                {"detectorId": "detector_a", "ruleId": "no_such_rule", "ruleVersion": "1"}]}),
            400,
            "ValidationException",
        ),
        (
            "unknown rule version",
            json!({"detectorId": "detector_a", "rules": [
                {"detectorId": "detector_a", "ruleId": "rule_0", "ruleVersion": "9"}]}),
            400,
            "ValidationException",
        ),
        (
            "rule of another detector",
            json!({"detectorId": "detector_b", "rules": [
                {"detectorId": "detector_a", "ruleId": "rule_0", "ruleVersion": "1"}]}),
            400,
            "ValidationException",
        ),
        (
            "unmodelled ruleExecutionMode",
            json!({"detectorId": "detector_a", "rules": [], "ruleExecutionMode": "SOMETIMES"}),
            400,
            "ValidationException",
        ),
    ];
    for (case, request, expected_status, expected_code) in create_cases {
        let (status, body) = h.call("CreateDetectorVersion", request.clone()).await;
        assert_eq!(
            (status, error_code(&body)),
            (*expected_status, (*expected_code).to_string()),
            "case {case:?}: wrong refusal, got {body:?}"
        );
    }

    let get_cases: &[(&str, &str, &str)] = &[
        ("unknown detector", "missing_detector", "1"),
        ("version never created", "detector_a", "2"),
        ("version of the wrong detector", "detector_b", "1"),
    ];
    for (case, detector_id, detector_version_id) in get_cases {
        let (status, body) = h
            .call(
                "GetDetectorVersion",
                json!({"detectorId": detector_id, "detectorVersionId": detector_version_id}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: an absent detector version must be a 404, got {body:?}"
        );
    }

    // A refused CreateDetectorVersion must not have consumed an id.
    let created = h
        .ok(
            "CreateDetectorVersion",
            json!({"detectorId": "detector_a", "rules": rules}),
        )
        .await;
    assert_eq!(
        string_member(&created, "detectorVersionId"),
        "2",
        "refused requests must not consume a detectorVersionId, got {created:?}"
    );
}

// ---------------------------------------------------------------------------
// DescribeDetector
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_detector_summarizes_every_stored_version_in_version_order() {
    let h = Harness::new();
    let rules = detector_with_rules(&h, "detector_a", 1).await;

    // Eleven versions, so an id-ordering bug (lexicographic "10" before "2") shows.
    const VERSIONS: usize = 11;
    for index in 0..VERSIONS {
        h.ok(
            "CreateDetectorVersion",
            json!({
                "detectorId": "detector_a",
                "rules": rules,
                "description": format!("version {index}"),
            }),
        )
        .await;
    }

    let described = h
        .ok("DescribeDetector", json!({"detectorId": "detector_a"}))
        .await;
    assert_eq!(
        string_member(&described, "detectorId"),
        "detector_a",
        "wrong detectorId in {described:?}"
    );
    assert_fraud_detector_arn(
        "detector_a",
        &string_member(&described, "arn"),
        "detector/detector_a",
    );
    let summaries = member(&described, "detectorVersionSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ids: Vec<String> = summaries
        .iter()
        .map(|s| string_member(s, "detectorVersionId"))
        .collect();
    let expected: Vec<String> = (1..=VERSIONS).map(|index| index.to_string()).collect();
    assert_eq!(
        ids, expected,
        "expected every stored version in version order, got {summaries:?}"
    );
    for (index, summary) in summaries.iter().enumerate() {
        assert_eq!(
            string_member(summary, "status"),
            "DRAFT",
            "case {index}: wrong status in {summary:?}"
        );
        assert_eq!(
            string_member(summary, "description"),
            format!("version {index}"),
            "case {index}: wrong description in {summary:?}"
        );
        assert_timestamp(&index.to_string(), &string_member(summary, "lastUpdatedTime"));
    }

    let (status, body) = h
        .call("DescribeDetector", json!({"detectorId": "missing_detector"}))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException".to_string()),
        "describing an unknown detector must be a 404, got {body:?}"
    );

    // A token naming a stored version resumes the listing at that version.
    for from in [1_usize, 5, VERSIONS] {
        let resumed = h
            .ok(
                "DescribeDetector",
                json!({"detectorId": "detector_a", "nextToken": from.to_string()}),
            )
            .await;
        let resumed_ids: Vec<String> = member(&resumed, "detectorVersionSummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|s| string_member(s, "detectorVersionId"))
            .collect();
        let expected: Vec<String> = (from..=VERSIONS).map(|index| index.to_string()).collect();
        assert_eq!(
            resumed_ids, expected,
            "case nextToken={from}: paging must resume at that version, got {resumed_ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteDetector
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_detector_removes_it_unless_something_is_still_attached() {
    let h = Harness::new();

    // Rows span what can still be attached to a detector: nothing, rule versions only,
    // and rule versions plus a detector version.
    h.put_detector("empty_detector").await;
    detector_with_rules(&h, "detector_with_rules", 2).await;
    let rules = detector_with_rules(&h, "detector_with_versions", 1).await;
    h.ok(
        "CreateDetectorVersion",
        json!({"detectorId": "detector_with_versions", "rules": rules}),
    )
    .await;

    let conflicts: &[&str] = &["detector_with_rules", "detector_with_versions"];
    for detector_id in conflicts {
        let (status, body) = h
            .call("DeleteDetector", json!({"detectorId": detector_id}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (409, "ConflictException".to_string()),
            "case {detector_id:?}: deleting a detector with children must conflict, got {body:?}"
        );
        let still_there = h
            .ok("GetDetectors", json!({"detectorId": detector_id}))
            .await;
        assert_eq!(
            member(&still_there, "detectors").as_array().map(Vec::len),
            Some(1),
            "case {detector_id:?}: a refused delete must leave the detector in place, got {still_there:?}"
        );
    }

    // A detector with nothing attached deletes, and stops being reported.
    h.ok("DeleteDetector", json!({"detectorId": "empty_detector"}))
        .await;
    let (status, body) = h
        .call("GetDetectors", json!({"detectorId": "empty_detector"}))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException".to_string()),
        "a deleted detector must no longer be reported, got {body:?}"
    );
    let listed = h.ok("GetDetectors", json!({})).await;
    let ids: Vec<String> = member(&listed, "detectors")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|d| string_member(d, "detectorId"))
        .collect();
    assert_eq!(
        ids,
        vec![
            "detector_with_rules".to_string(),
            "detector_with_versions".to_string()
        ],
        "a deleted detector must be gone from the listing, got {ids:?}"
    );

    // Deleting it again is the same end state, and this operation models no
    // not-found error: it succeeds.
    h.ok("DeleteDetector", json!({"detectorId": "empty_detector"}))
        .await;
}

// ---------------------------------------------------------------------------
// The operations this slice does not implement still answer 501
// ---------------------------------------------------------------------------

#[tokio::test]
async fn operations_outside_this_slice_still_answer_not_implemented() {
    let h = Harness::new();
    h.put_detector("detector_a").await;

    for (operation, request) in [
        ("GetRules", json!({"detectorId": "detector_a"})),
        (
            "DeleteDetectorVersion",
            json!({"detectorId": "detector_a", "detectorVersionId": "1"}),
        ),
        ("GetEventTypes", json!({})),
    ] {
        let (status, body) = h.call(operation, request).await;
        assert_eq!(
            (status, error_code(&body)),
            (501, "NotImplemented".to_string()),
            "case {operation:?}: an unimplemented operation must answer 501, got {body:?}"
        );
    }
}
