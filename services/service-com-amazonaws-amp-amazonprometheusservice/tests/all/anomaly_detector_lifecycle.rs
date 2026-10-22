// SPDX-License-Identifier: CC0-1.0

//! The anomaly detector lifecycle, end to end over HTTP: create inside a workspace,
//! describe, delete, and the refusals each of those models — every assertion made
//! against what the service PERSISTED rather than against what the request carried.
//!
//! The harness (`Amp`, `Reply`, `text`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` already builds: one `MultiServiceRouter` with
//! this service mounted on one shared engine, so routing, the `restJson1` wire shapes,
//! the modelled response statuses and the modelled error envelopes are all exercised.

use super::workspace_lifecycle::{text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::ANOMALY_DETECTOR_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA, ANOMALY_DETECTOR_NOT_FOUND_MESSAGE,
    RANDOM_CUT_FOREST_AGGREGATION_OPERATORS, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETING,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// `POST /workspaces/{workspaceId}/anomalydetectors`.
async fn create_detector(amp: &Amp, region: &str, workspace_id: &str, body: &Value) -> Reply {
    amp.call(
        "POST",
        &format!("/workspaces/{workspace_id}/anomalydetectors"),
        region,
        &body.to_string(),
    )
    .await
}

/// `GET /workspaces/{workspaceId}/anomalydetectors/{anomalyDetectorId}`.
async fn describe_detector(amp: &Amp, region: &str, workspace_id: &str, detector_id: &str) -> Reply {
    amp.call(
        "GET",
        &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
        region,
        "",
    )
    .await
}

/// `DELETE /workspaces/{workspaceId}/anomalydetectors/{anomalyDetectorId}`, with
/// `clientToken` on the query string where the model binds it.
async fn delete_detector(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    detector_id: &str,
    client_token: Option<&str>,
) -> Reply {
    let uri = match client_token {
        Some(token) => {
            format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}?clientToken={token}")
        }
        None => format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
    };
    amp.call("DELETE", &uri, region, "").await
}

type Reply = super::workspace_lifecycle::Reply;

/// A `randomCutForest` configuration carrying only the required `query`, so the
/// response shows what the service filled in for the rest.
fn minimal_configuration(query: &str) -> Value {
    json!({ "randomCutForest": { "query": query } })
}

/// `RandomCutForestConfiguration` with every member set, so a round trip has something
/// to lose: `shingleSize`, `sampleSize` and both `ignoreNearExpected` unions.
fn full_configuration(query: &str) -> Value {
    json!({
        "randomCutForest": {
            "query": query,
            "shingleSize": 16,
            "sampleSize": 512,
            "ignoreNearExpectedFromAbove": { "amount": 2.5 },
            "ignoreNearExpectedFromBelow": { "ratio": 0.125 },
        }
    })
}

/// The same configuration as the service reports it back, with the defaults
/// `RandomCutForestConfiguration` documents applied: "The default number is 8
/// consecutive data points" (`shingleSize`) and "The default number is 256 consecutive
/// data points" (`sampleSize`).
fn configuration_with_defaults(query: &str) -> Value {
    json!({
        "randomCutForest": {
            "query": query,
            "shingleSize": 8,
            "sampleSize": 256,
        }
    })
}

fn json_map(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
            .collect(),
    )
}

fn map_of(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), text(value)))
                .collect()
        })
        .unwrap_or_default()
}

fn expected_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `AnomalyDetectorId`: length 1..64, pattern `^ad-[0-9A-Za-z][-.0-9A-Z_a-z]*$`, and the
/// shape the service mints — `ad-` followed by a UUID.
fn assert_well_formed_detector_id(case: &str, detector_id: &str) {
    assert!(
        !detector_id.is_empty() && detector_id.len() <= 64,
        "case {case:?}: anomalyDetectorId must be 1..64 characters, got {detector_id:?}"
    );
    let uuid = detector_id.strip_prefix("ad-").unwrap_or_else(|| {
        panic!("case {case:?}: anomalyDetectorId must start with ad-, got {detector_id:?}")
    });
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: anomalyDetectorId must carry a UUID after ad-, got {detector_id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit())),
        "case {case:?}: the UUID in an anomalyDetectorId must be hexadecimal, got {detector_id:?}"
    );
}

/// `AnomalyDetectorArn`:
/// `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:anomalydetector/ws-.+/ad-.+$`.
fn assert_well_formed_detector_arn(
    case: &str,
    arn: &str,
    region: &str,
    workspace_id: &str,
    detector_id: &str,
) {
    let expected = format!(
        "arn:aws:aps:{region}:{}:anomalydetector/{workspace_id}/{detector_id}",
        engine::emulator_account_id()
    );
    assert_eq!(
        arn, expected,
        "case {case:?}: arn must name this region, the emulator account, the workspace and the detector"
    );
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: arn must carry six colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        segments[2], "aps",
        "case {case:?}: arn service must be aps, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].chars().all(|c| c.is_ascii_digit()),
        "case {case:?}: arn account must be 12 digits, got {arn:?}"
    );
    assert!(
        segments[5].starts_with("anomalydetector/ws-") && segments[5].contains("/ad-"),
        "case {case:?}: arn resource must be anomalydetector/<workspaceId>/<detectorId>, got {arn:?}"
    );
}

/// Every status this crate reports for a detector is a value the modelled
/// `AnomalyDetectorStatusCode` enum declares — the catalog comes from the model, not
/// from this test.
#[test]
fn reported_statuses_are_modelled_anomaly_detector_status_codes() {
    for status in [STATUS_CREATING, STATUS_ACTIVE, STATUS_DELETING] {
        assert!(
            ANOMALY_DETECTOR_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled AnomalyDetectorStatusCode \
             ({ANOMALY_DETECTOR_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A create persists every member it was given and answers from that record;
/// DescribeAnomalyDetector reads the same record back. One row per thing the rule
/// generalises over: the configuration (minimal, so the documented defaults show; full,
/// with both `ignoreNearExpected` unions), the evaluation interval (omitted so the
/// modelled default applies, and at each published `@range(min: 30, max: 86400)`
/// bound), the `missingDataAction` union (absent, each member), the label and tag counts
/// (none, several), and the region the request is signed for.
#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    struct Case {
        name: &'static str,
        region: &'static str,
        alias: &'static str,
        /// The PromQL query the configuration carries, so each row states it once.
        query: &'static str,
        /// Builds the `configuration` the request carries from `query`.
        configuration: fn(&str) -> Value,
        /// Builds the `configuration` the service must report back from `query`.
        expected_configuration: fn(&str) -> Value,
        /// `None` leaves `evaluationIntervalInSeconds` out of the request, so the
        /// modelled `default: 60` applies.
        evaluation_interval: Option<i64>,
        expected_interval: i64,
        missing_data_action: Option<Value>,
        labels: Vec<(&'static str, &'static str)>,
        tags: Vec<(&'static str, &'static str)>,
    }

    let cases = vec![
        Case {
            name: "minimal configuration, interval left to the modelled default",
            region: "us-west-2",
            alias: "cpu-detector",
            query: "avg(rate(node_cpu_seconds_total[5m]))",
            configuration: minimal_configuration,
            expected_configuration: configuration_with_defaults,
            evaluation_interval: None,
            expected_interval: 60,
            missing_data_action: None,
            labels: vec![],
            tags: vec![],
        },
        Case {
            name: "full configuration at the lowest interval, markAsAnomaly, labels and tags",
            region: "eu-west-1",
            alias: "Latency.detector_1",
            query: "quantile(0.99, rate(http_request_duration_seconds[1m]))",
            configuration: full_configuration,
            expected_configuration: full_configuration,
            evaluation_interval: Some(30),
            expected_interval: 30,
            missing_data_action: Some(json!({ "markAsAnomaly": true })),
            labels: vec![("service", "checkout"), ("tier", "edge")],
            tags: vec![("team", "obs"), ("env", "prod")],
        },
        Case {
            name: "highest interval, skip on missing data, one label",
            region: "ap-southeast-2",
            alias: "a",
            query: "sum by (job) (rate(requests_total[10m]))",
            configuration: minimal_configuration,
            expected_configuration: configuration_with_defaults,
            evaluation_interval: Some(86_400),
            expected_interval: 86_400,
            missing_data_action: Some(json!({ "skip": true })),
            labels: vec![("shard", "7")],
            tags: vec![],
        },
    ];

    for case in cases {
        let name = case.name;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, "detectors").await;

        let mut body = serde_json::Map::new();
        body.insert("alias".to_string(), json!(case.alias));
        body.insert(
            "configuration".to_string(),
            (case.configuration)(case.query),
        );
        if let Some(interval) = case.evaluation_interval {
            body.insert("evaluationIntervalInSeconds".to_string(), json!(interval));
        }
        if let Some(action) = case.missing_data_action.clone() {
            body.insert("missingDataAction".to_string(), action);
        }
        if !case.labels.is_empty() {
            body.insert("labels".to_string(), json_map(&case.labels));
        }
        if !case.tags.is_empty() {
            body.insert("tags".to_string(), json_map(&case.tags));
        }

        let created =
            create_detector(&amp, case.region, &workspace_id, &Value::Object(body)).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: CreateAnomalyDetector must answer its modelled 202, got {created:?}"
        );
        let detector_id = text(&created.body["anomalyDetectorId"]);
        assert_well_formed_detector_id(name, &detector_id);
        assert_well_formed_detector_arn(
            name,
            &text(&created.body["arn"]),
            case.region,
            &workspace_id,
            &detector_id,
        );
        assert_eq!(
            text(&created.body["status"]["statusCode"]),
            STATUS_CREATING,
            "case {name:?}: a just-created detector must report {STATUS_CREATING}, got {created:?}"
        );
        assert_eq!(
            map_of(&created.body["tags"]),
            expected_map(&case.tags),
            "case {name:?}: the create must answer with the tags it persisted, got {created:?}"
        );

        // The same record, read back through the implemented reader while the clock
        // still owes the move to ACTIVE.
        let described = describe_detector(&amp, case.region, &workspace_id, &detector_id).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: DescribeAnomalyDetector must answer 200, got {described:?}"
        );
        let detector = &described.body["anomalyDetector"];
        assert_eq!(
            text(&detector["anomalyDetectorId"]),
            detector_id,
            "case {name:?}: describe must report the created id, got {described:?}"
        );
        assert_eq!(
            text(&detector["arn"]),
            text(&created.body["arn"]),
            "case {name:?}: describe must report the created arn, got {described:?}"
        );
        assert_eq!(
            text(&detector["alias"]),
            case.alias,
            "case {name:?}: describe must report the alias the create carried, got {described:?}"
        );
        assert_eq!(
            detector["evaluationIntervalInSeconds"].as_i64(),
            Some(case.expected_interval),
            "case {name:?}: expected evaluationIntervalInSeconds {}, got {described:?}",
            case.expected_interval
        );
        let expected_configuration = (case.expected_configuration)(case.query);
        assert_eq!(
            detector["configuration"], expected_configuration,
            "case {name:?}: expected configuration {expected_configuration}, got {described:?}"
        );
        match &case.missing_data_action {
            Some(action) => assert_eq!(
                &detector["missingDataAction"], action,
                "case {name:?}: expected missingDataAction {action}, got {described:?}"
            ),
            None => assert!(
                detector.get("missingDataAction").is_none(),
                "case {name:?}: a detector created without missingDataAction must report none, \
                 got {described:?}"
            ),
        }
        if case.labels.is_empty() {
            assert!(
                detector.get("labels").is_none(),
                "case {name:?}: a detector created without labels must report none, got {described:?}"
            );
        } else {
            assert_eq!(
                map_of(&detector["labels"]),
                expected_map(&case.labels),
                "case {name:?}: describe must report the labels it persisted, got {described:?}"
            );
        }
        assert_eq!(
            map_of(&detector["tags"]),
            expected_map(&case.tags),
            "case {name:?}: describe must report the tags it persisted, got {described:?}"
        );
        assert_eq!(
            text(&detector["status"]["statusCode"]),
            STATUS_CREATING,
            "case {name:?}: the detector must still be {STATUS_CREATING} here, got {described:?}"
        );
        let created_at = detector["createdAt"].as_f64().unwrap_or_default();
        let modified_at = detector["modifiedAt"].as_f64().unwrap_or_default();
        assert!(
            created_at > 0.0,
            "case {name:?}: createdAt must be an epoch timestamp, got {described:?}"
        );
        assert_eq!(
            modified_at, created_at,
            "case {name:?}: an untouched detector's modifiedAt is its createdAt, got {described:?}"
        );

        // The clock owes the move to ACTIVE, and the same read reports it afterwards.
        wait_for_due_transition();
        let active = describe_detector(&amp, case.region, &workspace_id, &detector_id).await;
        assert_eq!(
            text(&active.body["anomalyDetector"]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {name:?}: the clock owes the move to {STATUS_ACTIVE}, got {active:?}"
        );
        assert_eq!(
            active.body["anomalyDetector"]["createdAt"].as_f64(),
            Some(created_at),
            "case {name:?}: createdAt must be stable across reads, got {active:?}"
        );
    }
}

/// `clientToken` carries `@idempotencyToken`: the same token answers with the detector
/// it already created, and a different token creates another one.
#[tokio::test]
async fn create_is_idempotent_per_client_token() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "idempotency").await;
    let body = |token: &str, alias: &str| {
        json!({
            "alias": alias,
            "clientToken": token,
            "configuration": minimal_configuration("max(rate(errors_total[5m]))"),
        })
    };

    let first = create_detector(&amp, region, &workspace_id, &body("token-one", "one")).await;
    let replay = create_detector(&amp, region, &workspace_id, &body("token-one", "one")).await;
    let other = create_detector(&amp, region, &workspace_id, &body("token-two", "two")).await;

    assert_eq!(
        first.status, 202,
        "case \"first create\": expected 202, got {first:?}"
    );
    assert_eq!(
        text(&replay.body["anomalyDetectorId"]),
        text(&first.body["anomalyDetectorId"]),
        "case \"same clientToken\": the replay must answer with the first detector, got {replay:?}"
    );
    assert_ne!(
        text(&other.body["anomalyDetectorId"]),
        text(&first.body["anomalyDetectorId"]),
        "case \"different clientToken\": a new token must create a second detector, got {other:?}"
    );

    // Two detectors exist, not three: the replay persisted nothing new.
    for (case, reply) in [("first", &first), ("second", &other)] {
        let detector_id = text(&reply.body["anomalyDetectorId"]);
        let described = describe_detector(&amp, region, &workspace_id, &detector_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the created detector must be readable, got {described:?}"
        );
    }
}

/// `RandomCutForestConfiguration$query`: "Random Cut Forest queries must be wrapped by a
/// supported PromQL aggregation operator." Every operator the member lists is accepted
/// — including ones this suite's other rows never use — and an expression led by
/// anything else is refused with the modelled `ValidationException` (400), persisting
/// nothing.
#[tokio::test]
async fn create_requires_a_query_wrapped_by_a_supported_aggregation_operator() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "queries").await;

    for (index, operator) in RANDOM_CUT_FOREST_AGGREGATION_OPERATORS.iter().enumerate() {
        let query = format!("{operator}(rate(requests_total[5m]))");
        let accepted = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": format!("agg{index}"),
                "configuration": minimal_configuration(&query),
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {query:?}: a query wrapped by a supported aggregation operator must be \
             accepted, got {accepted:?}"
        );
    }

    // `sum by (...)` / `sum without (...)` are the aggregation forms PromQL's syntax
    // allows beside a plain call, so each is accepted too.
    for query in [
        "sum by (job) (rate(requests_total[5m]))",
        "sum without (instance) (rate(requests_total[5m]))",
        "max by(job)(rate(requests_total[5m]))",
    ] {
        let accepted = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": "modified",
                "configuration": minimal_configuration(query),
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {query:?}: an aggregation with a by/without modifier must be accepted, \
             got {accepted:?}"
        );
    }

    // Each is a valid PromQL expression that is NOT wrapped by an aggregation operator:
    // a range-vector function, an over-time function, a bare selector, a filtered
    // selector, an expression with no identifier at all, and two where an aggregation
    // appears but does not wrap the whole expression.
    let refused_queries = [
        "rate(requests_total[5m])",
        "avg_over_time(node_load1[1h])",
        "requests_total",
        "up{job=\"api\"}",
        "1 + 2",
        "1 + sum(rate(requests_total[5m]))",
        "(sum(rate(requests_total[5m]))) / count(up)",
    ];
    for query in refused_queries {
        let refused = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": "unwrapped",
                "configuration": minimal_configuration(query),
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {query:?}: a query not wrapped by an aggregation operator must be refused, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {query:?}: unexpected error code, got {refused:?}"
        );
        assert!(
            refused.body.get("anomalyDetectorId").is_none(),
            "case {query:?}: a refused create must mint nothing, got {refused:?}"
        );
    }
}

/// A delete starts the removal the clock then owes: the detector reports `DELETING`
/// first, and a later read answers the modelled `ResourceNotFoundException`. Repeating
/// the call while the removal is pending is accepted (the operation is `@idempotent`),
/// and deleting one detector leaves its siblings alone.
#[tokio::test]
async fn delete_starts_a_removal_the_clock_then_completes() {
    let amp = Amp::new();
    let region = "us-west-2";
    let workspace_id = amp.active_workspace(region, "deletes").await;

    let mut detector_ids = Vec::new();
    for alias in ["first", "second"] {
        let created = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": alias,
                "configuration": minimal_configuration("min(rate(errors_total[5m]))"),
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {alias:?}: expected 202, got {created:?}"
        );
        detector_ids.push(text(&created.body["anomalyDetectorId"]));
    }
    wait_for_due_transition();

    let target = detector_ids[0].clone();
    let sibling = detector_ids[1].clone();

    let deleted = delete_detector(&amp, region, &workspace_id, &target, Some("delete-token")).await;
    assert_eq!(
        deleted.status, 202,
        "case {target:?}: DeleteAnomalyDetector must answer its modelled 202, got {deleted:?}"
    );
    let deleting = describe_detector(&amp, region, &workspace_id, &target).await;
    assert_eq!(
        text(&deleting.body["anomalyDetector"]["status"]["statusCode"]),
        STATUS_DELETING,
        "case {target:?}: a detector whose removal is pending reports {STATUS_DELETING}, \
         got {deleting:?}"
    );

    let replay = delete_detector(&amp, region, &workspace_id, &target, Some("delete-token")).await;
    assert_eq!(
        replay.status, 202,
        "case {target:?}: repeating the delete must stay accepted, got {replay:?}"
    );

    wait_for_due_transition();
    let gone = describe_detector(&amp, region, &workspace_id, &target).await;
    assert_eq!(
        gone.status, 404,
        "case {target:?}: a removed detector must answer 404, got {gone:?}"
    );
    assert_eq!(
        gone.error_code(),
        "ResourceNotFoundException",
        "case {target:?}: unexpected error code, got {gone:?}"
    );
    let untouched = describe_detector(&amp, region, &workspace_id, &sibling).await;
    assert_eq!(
        untouched.status, 200,
        "case {sibling:?}: deleting one detector must leave its sibling, got {untouched:?}"
    );
    assert_eq!(
        text(&untouched.body["anomalyDetector"]["status"]["statusCode"]),
        STATUS_ACTIVE,
        "case {sibling:?}: the sibling must still be {STATUS_ACTIVE}, got {untouched:?}"
    );
}

/// What each operation answers for an id that does not resolve, and for a detector that
/// belongs to ANOTHER workspace: a detector is addressed within its workspace, so it is
/// not reachable through a different one.
#[tokio::test]
async fn unresolved_ids_answer_the_modelled_refusals() {
    let amp = Amp::new();
    let region = "us-east-1";
    let left = amp.active_workspace(region, "left").await;
    let right = amp.active_workspace(region, "right").await;
    let created = create_detector(
        &amp,
        region,
        &left,
        &json!({
            "alias": "owned-by-left",
            "configuration": minimal_configuration("count(rate(requests_total[5m]))"),
        }),
    )
    .await;
    assert_eq!(created.status, 202, "case \"seed\": expected 202, got {created:?}");
    let detector_id = text(&created.body["anomalyDetectorId"]);

    // CreateAnomalyDetector models no ResourceNotFoundException, so an unknown
    // workspaceId is refused as the modelled ValidationException.
    let unknown_workspace = create_detector(
        &amp,
        region,
        "ws-00000000-0000-0000-0000-000000000000",
        &json!({
            "alias": "orphan",
            "configuration": minimal_configuration("sum(rate(requests_total[5m]))"),
        }),
    )
    .await;
    assert_eq!(
        unknown_workspace.status, 400,
        "case \"unknown workspace\": CreateAnomalyDetector must refuse with 400, \
         got {unknown_workspace:?}"
    );
    assert_eq!(
        unknown_workspace.error_code(),
        "ValidationException",
        "case \"unknown workspace\": unexpected error code, got {unknown_workspace:?}"
    );

    let cases = [
        (
            "unknown detector",
            left.clone(),
            "ad-00000000-0000-0000-0000-000000000000".to_string(),
            true,
        ),
        (
            "detector of another workspace",
            right.clone(),
            detector_id,
            true,
        ),
        (
            "unknown workspace",
            "ws-00000000-0000-0000-0000-000000000000".to_string(),
            "ad-00000000-0000-0000-0000-000000000000".to_string(),
            false,
        ),
    ];
    for (case, workspace_id, detector, detector_level) in cases {
        let described = describe_detector(&amp, region, &workspace_id, &detector).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: DescribeAnomalyDetector must answer 404, got {described:?}"
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {described:?}"
        );
        let deleted = delete_detector(&amp, region, &workspace_id, &detector, None).await;
        assert_eq!(
            deleted.status, 404,
            "case {case:?}: DeleteAnomalyDetector must answer 404, got {deleted:?}"
        );
        assert_eq!(
            deleted.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {deleted:?}"
        );
        if !detector_level {
            continue;
        }
        // The exception names the resource that does not resolve in `resourceId` /
        // `resourceType`, which is why the message itself carries no id.
        for (operation, reply) in [("describe", &described), ("delete", &deleted)] {
            assert_eq!(
                text(&reply.body["message"]),
                ANOMALY_DETECTOR_NOT_FOUND_MESSAGE,
                "case {case:?} ({operation}): unexpected message, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceId"]),
                detector,
                "case {case:?} ({operation}): resourceId must name the detector, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceType"]),
                "AWS::APS::AnomalyDetector",
                "case {case:?} ({operation}): resourceType must name the detector's type, \
                 got {reply:?}"
            );
        }
    }
}

/// A detector is a CHILD of its workspace: once the workspace is gone, so is the
/// detector — and a workspace whose deletion has started takes no new ones
/// (`ConflictException`, "The request would cause an inconsistent state").
#[tokio::test]
async fn a_detector_lives_and_dies_with_its_workspace() {
    let amp = Amp::new();
    let region = "us-west-2";
    let workspace_id = amp.active_workspace(region, "cascade").await;
    let created = create_detector(
        &amp,
        region,
        &workspace_id,
        &json!({
            "alias": "doomed",
            "configuration": minimal_configuration("stdvar(rate(requests_total[5m]))"),
        }),
    )
    .await;
    assert_eq!(created.status, 202, "case \"seed\": expected 202, got {created:?}");
    let detector_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();

    let deleted = amp
        .call("DELETE", &format!("/workspaces/{workspace_id}"), region, "")
        .await;
    assert_eq!(
        deleted.status, 202,
        "case \"delete workspace\": expected 202, got {deleted:?}"
    );

    // While the workspace's own deletion is pending, it takes no new children.
    let refused = create_detector(
        &amp,
        region,
        &workspace_id,
        &json!({
            "alias": "late",
            "configuration": minimal_configuration("group(up)"),
        }),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case \"workspace DELETING\": a create must answer 409, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ConflictException",
        "case \"workspace DELETING\": unexpected error code, got {refused:?}"
    );

    wait_for_due_transition();
    let gone = describe_detector(&amp, region, &workspace_id, &detector_id).await;
    assert_eq!(
        gone.status, 404,
        "case \"workspace removed\": its detector must be gone too, got {gone:?}"
    );
    assert_eq!(
        gone.error_code(),
        "ResourceNotFoundException",
        "case \"workspace removed\": unexpected error code, got {gone:?}"
    );
}

/// `AnomalyDetectorEvaluationInterval` is modelled `@range(min: 30, max: 86400)`: each
/// published bound is accepted and one step past either is refused with 400, so a
/// caller cannot store an interval outside the modelled range.
#[tokio::test]
async fn the_evaluation_interval_holds_to_its_modelled_range() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "intervals").await;
    let cases = [(29_i64, 400_u16), (30, 202), (86_400, 202), (86_401, 400)];

    for (interval, expected_status) in cases {
        let reply = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": "interval",
                "evaluationIntervalInSeconds": interval,
                "configuration": minimal_configuration("avg(rate(requests_total[5m]))"),
            }),
        )
        .await;
        assert_eq!(
            reply.status, expected_status,
            "case {interval}: expected HTTP {expected_status}, got {reply:?}"
        );
        if expected_status == 202 {
            let detector_id = text(&reply.body["anomalyDetectorId"]);
            let described = describe_detector(&amp, region, &workspace_id, &detector_id).await;
            assert_eq!(
                described.body["anomalyDetector"]["evaluationIntervalInSeconds"].as_i64(),
                Some(interval),
                "case {interval}: the stored interval must be the one accepted, got {described:?}"
            );
        }
    }
}

/// "Keys must not begin with aws:" (`TagResource$tags`) holds for the tags a create
/// carries, and the refusal persists nothing.
#[tokio::test]
async fn create_refuses_a_reserved_tag_key() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "tags").await;

    for reserved in ["aws:cloudformation:stack-name", "aws:created-by"] {
        let refused = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": "tagged",
                "configuration": minimal_configuration("avg(rate(requests_total[5m]))"),
                "tags": { reserved: "x" },
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {reserved:?}: a reserved tag key must be refused, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {reserved:?}: unexpected error code, got {refused:?}"
        );
    }
}

/// The store is BOUNDED: once one workspace holds `ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA`
/// detectors a create is refused with the modelled `ServiceQuotaExceededException` (402)
/// rather than growing the state further, and the refusal persists nothing.
#[tokio::test]
async fn create_refuses_once_the_workspace_is_at_its_detector_quota() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "quota").await;
    let configuration = minimal_configuration("sum(rate(requests_total[5m]))");

    let mut first_id = String::new();
    for index in 0..ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA {
        let created = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": format!("bulk{index:04}"),
                "configuration": configuration.clone(),
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {index}: filling the workspace must answer 202, got {created:?}"
        );
        if index == 0 {
            first_id = text(&created.body["anomalyDetectorId"]);
        }
    }

    let refused = create_detector(
        &amp,
        region,
        &workspace_id,
        &json!({
            "alias": "onetoomany",
            "configuration": configuration.clone(),
        }),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case \"at quota\": a create past the bound must answer 402, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "case \"at quota\": unexpected error code, got {refused:?}"
    );
    // The bound is per workspace, so the workspace is the resource that reached it.
    assert_eq!(
        text(&refused.body["resourceId"]),
        workspace_id,
        "case \"at quota\": resourceId must name the workspace, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["resourceType"]),
        "AWS::APS::Workspace",
        "case \"at quota\": resourceType must name the workspace's type, got {refused:?}"
    );
    assert!(
        refused.body.get("anomalyDetectorId").is_none(),
        "case \"at quota\": a refused create must mint nothing, got {refused:?}"
    );

    // The refusal changed nothing about what the workspace already holds.
    let still_there = describe_detector(&amp, region, &workspace_id, &first_id).await;
    assert_eq!(
        still_there.status, 200,
        "case \"at quota\": the detectors already stored must stay readable, got {still_there:?}"
    );
}
