// SPDX-License-Identifier: CC0-1.0

//! `PutAnomalyDetector`, end to end over HTTP.
//!
//! The rule under test: the operation REPLACES the whole specification of the detector
//! its two path labels address — the algorithm configuration, the evaluation interval,
//! the missing-data action and the metric labels, with an optional member the request
//! omits left holding nothing — leaves every member that identifies the resource alone
//! (id, ARN, alias, creation instant, tags), moves only `modifiedAt`, and reports the
//! detector `UPDATING` until the clock owes its move back to `ACTIVE`. It only ever
//! updates: a workspace or a detector that does not resolve is
//! `ResourceNotFoundException` (404), a detector or workspace that is not `ACTIVE` is
//! `ConflictException` (409), and a specification the service does not accept is
//! `ValidationException` (400) with nothing stored.
//!
//! Each suite spans what that rule generalises over — the configuration's members and
//! their documented defaults, the evaluation interval across its published bounds, the
//! `missingDataAction` union's members (and its absence), the number and content of the
//! metric labels, the number of successive updates, the tags the detector already
//! carries, and the region the request is signed for — rather than one request's values.
//!
//! The request harness (`Amp`, `Reply`, `text`, `tags_of`, `wait_for_due_transition`) is
//! the one `tests/all/workspace_lifecycle.rs` already builds.

use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::ANOMALY_DETECTOR_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    ANOMALY_DETECTOR_NOT_FOUND_MESSAGE, ANOMALY_DETECTOR_RESOURCE_TYPE,
    RANDOM_CUT_FOREST_AGGREGATION_OPERATORS, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETING,
    STATUS_UPDATING, WORKSPACE_NOT_FOUND_MESSAGE, WORKSPACE_RESOURCE_TYPE,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// `POST /workspaces/{workspaceId}/anomalydetectors` — CreateAnomalyDetector, the only
/// operation that brings a detector into existence (it is where `alias` is given).
async fn create_detector(amp: &Amp, region: &str, workspace_id: &str, body: &Value) -> Reply {
    amp.call(
        "POST",
        &format!("/workspaces/{workspace_id}/anomalydetectors"),
        region,
        &body.to_string(),
    )
    .await
}

/// `PUT /workspaces/{workspaceId}/anomalydetectors/{anomalyDetectorId}` —
/// PutAnomalyDetector, the operation under test.
async fn put_detector(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    detector_id: &str,
    body: &Value,
) -> Reply {
    amp.call(
        "PUT",
        &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
        region,
        &body.to_string(),
    )
    .await
}

/// `GET /workspaces/{workspaceId}/anomalydetectors/{anomalyDetectorId}` — the reader the
/// update is asserted through, so every assertion is made against what was PERSISTED.
async fn describe_detector(amp: &Amp, region: &str, workspace_id: &str, detector_id: &str) -> Reply {
    amp.call(
        "GET",
        &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
        region,
        "",
    )
    .await
}

fn json_map(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

fn expected_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One detector specification, as a request carries it and as the service reports it
/// back. Every member but `query` is optional, which is what a replacement has to be
/// able to drop.
#[derive(Clone)]
struct Spec {
    query: String,
    /// `shingleSize` and `sampleSize`, or `None` to leave them to their documented
    /// defaults. Modelled `@range(min: 2, max: 1024)` and `@range(min: 256, max: 1024)`.
    sizes: Option<(i64, i64)>,
    /// The `ignoreNearExpected` unions, each absent or one of its two members.
    ignore_above: Option<Value>,
    ignore_below: Option<Value>,
    /// `evaluationIntervalInSeconds`, or `None` to let the modelled default apply.
    interval: Option<i64>,
    /// The `missingDataAction` union: absent, or one of its members.
    missing_data_action: Option<Value>,
    labels: Vec<(&'static str, &'static str)>,
}

/// The modelled default of `AnomalyDetectorEvaluationInterval` (`default: 60`), applied
/// by the request walk when the member is absent.
const DEFAULT_EVALUATION_INTERVAL: i64 = 60;

/// `RandomCutForestConfiguration$shingleSize`: "The default number is 8 consecutive data
/// points".
const DEFAULT_SHINGLE_SIZE: i64 = 8;

/// `RandomCutForestConfiguration$sampleSize`: "The default number is 256 consecutive data
/// points".
const DEFAULT_SAMPLE_SIZE: i64 = 256;

impl Spec {
    fn new(query: &str) -> Self {
        Self {
            query: query.to_string(),
            sizes: None,
            ignore_above: None,
            ignore_below: None,
            interval: None,
            missing_data_action: None,
            labels: vec![],
        }
    }

    fn sizes(mut self, shingle: i64, sample: i64) -> Self {
        self.sizes = Some((shingle, sample));
        self
    }

    fn ignore_above(mut self, member: Value) -> Self {
        self.ignore_above = Some(member);
        self
    }

    fn ignore_below(mut self, member: Value) -> Self {
        self.ignore_below = Some(member);
        self
    }

    fn interval(mut self, seconds: i64) -> Self {
        self.interval = Some(seconds);
        self
    }

    fn missing_data_action(mut self, action: Value) -> Self {
        self.missing_data_action = Some(action);
        self
    }

    fn labels(mut self, labels: &[(&'static str, &'static str)]) -> Self {
        self.labels = labels.to_vec();
        self
    }

    /// The `configuration` union as the request carries it.
    fn configuration_sent(&self) -> Value {
        let mut forest = serde_json::Map::new();
        forest.insert("query".to_string(), json!(self.query.clone()));
        if let Some((shingle, sample)) = self.sizes {
            forest.insert("shingleSize".to_string(), json!(shingle));
            forest.insert("sampleSize".to_string(), json!(sample));
        }
        if let Some(above) = self.ignore_above.clone() {
            forest.insert("ignoreNearExpectedFromAbove".to_string(), above);
        }
        if let Some(below) = self.ignore_below.clone() {
            forest.insert("ignoreNearExpectedFromBelow".to_string(), below);
        }
        json!({ "randomCutForest": Value::Object(forest) })
    }

    /// The same configuration as the service reports it back: the two sizes always carry
    /// a value, because the ones the request omits take the defaults their members
    /// document.
    fn configuration_reported(&self) -> Value {
        let (shingle, sample) = self
            .sizes
            .unwrap_or((DEFAULT_SHINGLE_SIZE, DEFAULT_SAMPLE_SIZE));
        self.clone().sizes(shingle, sample).configuration_sent()
    }

    fn reported_interval(&self) -> i64 {
        self.interval.unwrap_or(DEFAULT_EVALUATION_INTERVAL)
    }

    /// The `PutAnomalyDetector` request body for this specification.
    fn put_body(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("configuration".to_string(), self.configuration_sent());
        if let Some(interval) = self.interval {
            body.insert("evaluationIntervalInSeconds".to_string(), json!(interval));
        }
        if let Some(action) = self.missing_data_action.clone() {
            body.insert("missingDataAction".to_string(), action);
        }
        if !self.labels.is_empty() {
            body.insert("labels".to_string(), json_map(&self.labels));
        }
        Value::Object(body)
    }

    /// The `CreateAnomalyDetector` request body for this specification — the same members
    /// plus the two this operation does not carry, `alias` and `tags`.
    fn create_body(&self, alias: &str, tags: &[(&str, &str)]) -> Value {
        let mut body = self.put_body();
        let map = body.as_object_mut().expect("an object body");
        map.insert("alias".to_string(), json!(alias));
        if !tags.is_empty() {
            map.insert("tags".to_string(), json_map(tags));
        }
        body
    }
}

/// Assert the detector the service PERSISTED holds exactly `spec`, with `status`.
fn assert_persisted(case: &str, described: &Reply, spec: &Spec, status: &str) {
    assert_eq!(
        described.status, 200,
        "case {case:?}: DescribeAnomalyDetector must answer 200, got {described:?}"
    );
    let detector = &described.body["anomalyDetector"];
    assert_eq!(
        &detector["configuration"],
        &spec.configuration_reported(),
        "case {case:?}: expected configuration {}, got {detector:?}",
        spec.configuration_reported()
    );
    assert_eq!(
        detector["evaluationIntervalInSeconds"].as_i64(),
        Some(spec.reported_interval()),
        "case {case:?}: expected evaluationIntervalInSeconds {}, got {detector:?}",
        spec.reported_interval()
    );
    match &spec.missing_data_action {
        Some(action) => assert_eq!(
            &detector["missingDataAction"], action,
            "case {case:?}: expected missingDataAction {action}, got {detector:?}"
        ),
        None => assert!(
            detector.get("missingDataAction").is_none(),
            "case {case:?}: a specification without missingDataAction must report none, \
             got {detector:?}"
        ),
    }
    if spec.labels.is_empty() {
        assert!(
            detector.get("labels").is_none(),
            "case {case:?}: a specification without labels must report none, got {detector:?}"
        );
    } else {
        assert_eq!(
            tags_of(&detector["labels"]),
            expected_map(&spec.labels),
            "case {case:?}: expected labels {:?}, got {detector:?}",
            expected_map(&spec.labels)
        );
    }
    let status_code = text(&detector["status"]["statusCode"]);
    assert_eq!(
        status_code, status,
        "case {case:?}: expected status {status}, got {detector:?}"
    );
    assert!(
        ANOMALY_DETECTOR_STATUS_CODE_VALUES.contains(&status_code.as_str()),
        "case {case:?}: {status_code:?} is not a modelled AnomalyDetectorStatusCode, \
         got {detector:?}"
    );
}

/// The epoch seconds a timestamp member carries.
fn instant(case: &str, value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected an epoch-seconds timestamp, got {value:?}"))
}

/// A workspace id and a detector id that are well formed but name nothing — the shapes
/// the service mints (`ws-`/`ad-` followed by a UUID), so these reach the handler rather
/// than being refused by `WorkspaceId`/`AnomalyDetectorId`'s own patterns.
const ABSENT_WORKSPACE_ID: &str = "ws-00000000-0000-4000-8000-000000000000";
const ABSENT_DETECTOR_ID: &str = "ad-00000000-0000-4000-8000-000000000000";

/// An update replaces the WHOLE specification of an existing detector and the describe
/// reads the replacement back, while the members that identify the resource — id, ARN,
/// alias, `createdAt` and the tags this operation never carries — stay as they were and
/// `modifiedAt` moves.
///
/// One row per thing the rule generalises over: the configuration (minimal so the
/// documented defaults show, full with both `ignoreNearExpected` unions), the evaluation
/// interval (set, and omitted so the modelled default applies), the `missingDataAction`
/// union (absent, each member), the label count (none, one, several), the number of
/// successive updates, whether the detector already carries tags, and the signing region.
#[tokio::test]
async fn put_replaces_the_whole_specification_and_describe_reads_it_back() {
    struct Case {
        name: &'static str,
        region: &'static str,
        alias: &'static str,
        tags: Vec<(&'static str, &'static str)>,
        created: Spec,
        updates: Vec<Spec>,
    }

    let cases = vec![
        Case {
            name: "a minimal specification replaced by a full one",
            region: "us-east-1",
            alias: "requests",
            tags: vec![("team", "obs")],
            created: Spec::new("sum(rate(requests_total[5m]))"),
            updates: vec![Spec::new("avg(rate(requests_total[1m]))")
                .sizes(16, 512)
                .ignore_above(json!({ "amount": 2.5 }))
                .ignore_below(json!({ "ratio": 0.125 }))
                .interval(30)
                .missing_data_action(json!({ "markAsAnomaly": true }))
                .labels(&[("service", "checkout"), ("tier", "edge")])],
        },
        Case {
            name: "a full specification replaced by a minimal one clears every optional member",
            region: "eu-west-1",
            alias: "Latency.detector_1",
            tags: vec![("env", "prod"), ("team", "obs")],
            created: Spec::new("quantile(0.99, rate(http_request_duration_seconds[1m]))")
                .sizes(1024, 1024)
                .ignore_above(json!({ "ratio": 0.5 }))
                .ignore_below(json!({ "amount": 1.0 }))
                .interval(86_400)
                .missing_data_action(json!({ "skip": true }))
                .labels(&[("service", "checkout"), ("tier", "edge")]),
            updates: vec![Spec::new("max(node_memory_bytes)")],
        },
        Case {
            name: "two successive updates, each replacing the one before",
            region: "ap-southeast-2",
            alias: "a",
            tags: vec![],
            created: Spec::new("sum by (job) (rate(requests_total[10m]))")
                .missing_data_action(json!({ "markAsAnomaly": true }))
                .labels(&[("shard", "7")]),
            updates: vec![
                Spec::new("count without (instance) (up)")
                    .sizes(2, 256)
                    .interval(3_600)
                    .missing_data_action(json!({ "skip": true }))
                    .labels(&[("shard", "9"), ("zone", "b")]),
                Spec::new("stddev(rate(errors_total[5m]))")
                    .sizes(32, 1024)
                    .ignore_below(json!({ "ratio": 0.0 }))
                    .interval(30)
                    .labels(&[("zone", "c")]),
            ],
        },
        Case {
            name: "an update that changes nothing but the interval still retrains",
            region: "us-west-2",
            alias: "group0",
            tags: vec![("owner", "team-a")],
            created: Spec::new("group(up)").interval(120),
            updates: vec![Spec::new("group(up)").interval(121)],
        },
    ];

    for case in cases {
        let name = case.name;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, "detectors").await;

        let created = create_detector(
            &amp,
            case.region,
            &workspace_id,
            &case.created.create_body(case.alias, &case.tags),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: CreateAnomalyDetector must answer 202, got {created:?}"
        );
        let detector_id = text(&created.body["anomalyDetectorId"]);
        let arn = text(&created.body["arn"]);
        wait_for_due_transition();

        let before = describe_detector(&amp, case.region, &workspace_id, &detector_id).await;
        assert_persisted(name, &before, &case.created, STATUS_ACTIVE);
        let created_at = instant(name, &before.body["anomalyDetector"]["createdAt"]);
        let mut last_modified = instant(name, &before.body["anomalyDetector"]["modifiedAt"]);
        let expected_tags = expected_map(&case.tags);

        for (step, spec) in case.updates.iter().enumerate() {
            let case_name = format!("{name} (update {step})");
            let put = put_detector(
                &amp,
                case.region,
                &workspace_id,
                &detector_id,
                &spec.put_body(),
            )
            .await;
            assert_eq!(
                put.status, 202,
                "case {case_name:?}: PutAnomalyDetector must answer 202, got {put:?}"
            );
            // The response is read off the record, which is why it reports the ARN and the
            // tags the detector holds although this operation carries neither.
            assert_eq!(
                text(&put.body["anomalyDetectorId"]),
                detector_id,
                "case {case_name:?}: expected anomalyDetectorId {detector_id:?}, got {put:?}"
            );
            assert_eq!(
                text(&put.body["arn"]),
                arn,
                "case {case_name:?}: expected arn {arn:?}, got {put:?}"
            );
            assert_eq!(
                text(&put.body["status"]["statusCode"]),
                STATUS_UPDATING,
                "case {case_name:?}: expected status {STATUS_UPDATING}, got {put:?}"
            );
            assert_eq!(
                tags_of(&put.body["tags"]),
                expected_tags,
                "case {case_name:?}: expected the stored tags {expected_tags:?}, got {put:?}"
            );

            // The record itself holds the replacement, and only the replacement.
            let described = describe_detector(&amp, case.region, &workspace_id, &detector_id).await;
            assert_persisted(&case_name, &described, spec, STATUS_UPDATING);
            let detector = &described.body["anomalyDetector"];
            assert_eq!(
                text(&detector["anomalyDetectorId"]),
                detector_id,
                "case {case_name:?}: the id must not change, got {detector:?}"
            );
            assert_eq!(
                text(&detector["arn"]),
                arn,
                "case {case_name:?}: the ARN must not change, got {detector:?}"
            );
            assert_eq!(
                text(&detector["alias"]),
                case.alias,
                "case {case_name:?}: this operation carries no alias, so the stored one must \
                 not change, got {detector:?}"
            );
            assert_eq!(
                tags_of(&detector["tags"]),
                expected_tags,
                "case {case_name:?}: this operation carries no tags, so the stored ones must \
                 not change, got {detector:?}"
            );
            assert_eq!(
                instant(&case_name, &detector["createdAt"]),
                created_at,
                "case {case_name:?}: the creation instant must not change, got {detector:?}"
            );
            let modified_at = instant(&case_name, &detector["modifiedAt"]);
            assert!(
                modified_at > last_modified,
                "case {case_name:?}: an applied update must move modifiedAt to its own \
                 instant, got {modified_at} against the previous {last_modified}"
            );
            assert!(
                modified_at >= created_at,
                "case {case_name:?}: an updated detector was modified no earlier than it was \
                 created, got modifiedAt {modified_at} and createdAt {created_at}"
            );

            // The clock owes the move back to ACTIVE, and settling it moves the status
            // alone: the record was last changed by this call, not by the clock.
            wait_for_due_transition();
            let settled = describe_detector(&amp, case.region, &workspace_id, &detector_id).await;
            assert_persisted(&case_name, &settled, spec, STATUS_ACTIVE);
            assert_eq!(
                instant(&case_name, &settled.body["anomalyDetector"]["modifiedAt"]),
                modified_at,
                "case {case_name:?}: settling the status must not move modifiedAt, got {settled:?}"
            );
            last_modified = modified_at;
        }
    }
}

/// `clientToken` carries `@idempotencyToken`: re-sending the call that started the
/// pending retraining answers from the record that call wrote instead of applying the
/// change twice, while any other call arriving on a detector that is still `UPDATING` is
/// refused — and once the clock has settled the detector, that same token is a new
/// request.
#[tokio::test]
async fn put_is_idempotent_per_client_token() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "idempotency").await;

    let first_spec = Spec::new("sum(rate(requests_total[5m]))");
    let created = create_detector(
        &amp,
        region,
        &workspace_id,
        &first_spec.create_body("requests", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"seed\": CreateAnomalyDetector must answer 202, got {created:?}"
    );
    let detector_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();

    let token = "tok-1";
    let update = Spec::new("avg(rate(requests_total[1m]))").interval(90);
    let mut body = update.put_body();
    body.as_object_mut()
        .expect("an object body")
        .insert("clientToken".to_string(), json!(token));

    let accepted = put_detector(&amp, region, &workspace_id, &detector_id, &body).await;
    assert_eq!(
        accepted.status, 202,
        "case \"first put\": expected 202, got {accepted:?}"
    );
    let described = describe_detector(&amp, region, &workspace_id, &detector_id).await;
    assert_persisted("first put", &described, &update, STATUS_UPDATING);
    let modified_at = instant("first put", &described.body["anomalyDetector"]["modifiedAt"]);

    // The same call again, while that retraining is still owed: accepted, and the record
    // is the one the first call wrote — nothing was applied a second time.
    let replay = put_detector(&amp, region, &workspace_id, &detector_id, &body).await;
    assert_eq!(
        replay.status, 202,
        "case \"replayed token\": expected 202, got {replay:?}"
    );
    assert_eq!(
        text(&replay.body["status"]["statusCode"]),
        STATUS_UPDATING,
        "case \"replayed token\": expected the pending status, got {replay:?}"
    );
    let after_replay = describe_detector(&amp, region, &workspace_id, &detector_id).await;
    assert_persisted("replayed token", &after_replay, &update, STATUS_UPDATING);
    assert_eq!(
        instant(
            "replayed token",
            &after_replay.body["anomalyDetector"]["modifiedAt"]
        ),
        modified_at,
        "case \"replayed token\": a replay must not change the record, got {after_replay:?}"
    );

    // Any OTHER call on a detector that is not usable yet is refused, whether it carries
    // a different token or none at all.
    let other_spec = Spec::new("min(node_memory_bytes)");
    let mut other_token_body = other_spec.put_body();
    other_token_body
        .as_object_mut()
        .expect("an object body")
        .insert("clientToken".to_string(), json!("tok-2"));
    for (case, refused_body) in [
        ("another token", other_token_body),
        ("no token", other_spec.put_body()),
    ] {
        let refused = put_detector(&amp, region, &workspace_id, &detector_id, &refused_body).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: a put on an UPDATING detector must answer 409, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: unexpected error code, got {refused:?}"
        );
        let untouched = describe_detector(&amp, region, &workspace_id, &detector_id).await;
        assert_persisted(case, &untouched, &update, STATUS_UPDATING);
    }

    // Once the retraining has settled the token stands for no pending change, so the same
    // token carried again is a new request and is applied.
    wait_for_due_transition();
    let reused = put_detector(&amp, region, &workspace_id, &detector_id, &body).await;
    assert_eq!(
        reused.status, 202,
        "case \"token reused after settling\": expected 202, got {reused:?}"
    );
    let after_reuse = describe_detector(&amp, region, &workspace_id, &detector_id).await;
    assert_persisted("token reused after settling", &after_reuse, &update, STATUS_UPDATING);
    assert!(
        instant(
            "token reused after settling",
            &after_reuse.body["anomalyDetector"]["modifiedAt"]
        ) > modified_at,
        "case \"token reused after settling\": the update must be applied again, \
         got {after_reuse:?}"
    );
}

/// The operation only ever updates a detector that already exists: neither path label
/// may name something this account does not hold, and a refusal stores nothing.
///
/// The rows span both labels and the scope each is resolved in — an unknown workspace, an
/// unknown detector, and a detector that exists but belongs to ANOTHER workspace, which
/// is not reachable through this one.
#[tokio::test]
async fn put_refuses_a_label_that_resolves_to_nothing() {
    let amp = Amp::new();
    let region = "eu-central-1";
    let left = amp.active_workspace(region, "left").await;
    let right = amp.active_workspace(region, "right").await;

    let spec = Spec::new("sum(rate(requests_total[5m]))");
    let created = create_detector(&amp, region, &left, &spec.create_body("requests", &[])).await;
    assert_eq!(
        created.status, 202,
        "case \"seed\": CreateAnomalyDetector must answer 202, got {created:?}"
    );
    let detector_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();

    let replacement = Spec::new("max(node_memory_bytes)").interval(300);
    let cases = [
        (
            "unknown workspace",
            ABSENT_WORKSPACE_ID.to_string(),
            detector_id.clone(),
            ABSENT_WORKSPACE_ID.to_string(),
            WORKSPACE_RESOURCE_TYPE,
            WORKSPACE_NOT_FOUND_MESSAGE,
        ),
        (
            "unknown detector",
            left.clone(),
            ABSENT_DETECTOR_ID.to_string(),
            ABSENT_DETECTOR_ID.to_string(),
            ANOMALY_DETECTOR_RESOURCE_TYPE,
            ANOMALY_DETECTOR_NOT_FOUND_MESSAGE,
        ),
        (
            "a detector of another workspace",
            right.clone(),
            detector_id.clone(),
            detector_id.clone(),
            ANOMALY_DETECTOR_RESOURCE_TYPE,
            ANOMALY_DETECTOR_NOT_FOUND_MESSAGE,
        ),
    ];

    for (case, workspace, detector, resource_id, resource_type, message) in cases {
        let refused =
            put_detector(&amp, region, &workspace, &detector, &replacement.put_body()).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: PutAnomalyDetector must answer 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["resourceId"]),
            resource_id,
            "case {case:?}: expected resourceId {resource_id:?}, got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["resourceType"]),
            resource_type,
            "case {case:?}: expected resourceType {resource_type:?}, got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["message"]),
            message,
            "case {case:?}: expected message {message:?}, got {refused:?}"
        );
        // Nothing was stored: the detector that DOES exist still holds what it held.
        let untouched = describe_detector(&amp, region, &left, &detector_id).await;
        assert_persisted(case, &untouched, &spec, STATUS_ACTIVE);
    }
}

/// `ACTIVE` is the one usable status: a detector that is still being created, still
/// settling a previous retraining or being deleted cannot take a new specification, and
/// neither can one inside a workspace that is not usable. Each refusal is the modelled
/// `ConflictException` (409) naming the resource whose status refused it, and stores
/// nothing.
#[tokio::test]
async fn put_refuses_a_resource_that_is_not_active() {
    let region = "us-west-2";
    let spec = Spec::new("sum(rate(requests_total[5m]))");
    let replacement = Spec::new("min(node_memory_bytes)").interval(600);

    // A detector the clock has not finished creating.
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "creating").await;
    let created = create_detector(
        &amp,
        region,
        &workspace_id,
        &spec.create_body("requests", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"detector CREATING\": CreateAnomalyDetector must answer 202, got {created:?}"
    );
    let detector_id = text(&created.body["anomalyDetectorId"]);
    let refused = put_detector(
        &amp,
        region,
        &workspace_id,
        &detector_id,
        &replacement.put_body(),
    )
    .await;
    assert_conflict("detector CREATING", &refused, &detector_id, ANOMALY_DETECTOR_RESOURCE_TYPE);
    assert_persisted(
        "detector CREATING",
        &describe_detector(&amp, region, &workspace_id, &detector_id).await,
        &spec,
        STATUS_CREATING,
    );
    // The refusal changed nothing the clock owed: the create still settles.
    wait_for_due_transition();
    assert_persisted(
        "detector CREATING then settled",
        &describe_detector(&amp, region, &workspace_id, &detector_id).await,
        &spec,
        STATUS_ACTIVE,
    );

    // A detector whose removal has started.
    let deleted = amp
        .call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
            region,
            "",
        )
        .await;
    assert_eq!(
        deleted.status, 202,
        "case \"detector DELETING\": DeleteAnomalyDetector must answer 202, got {deleted:?}"
    );
    let refused = put_detector(
        &amp,
        region,
        &workspace_id,
        &detector_id,
        &replacement.put_body(),
    )
    .await;
    assert_conflict("detector DELETING", &refused, &detector_id, ANOMALY_DETECTOR_RESOURCE_TYPE);
    assert_persisted(
        "detector DELETING",
        &describe_detector(&amp, region, &workspace_id, &detector_id).await,
        &spec,
        STATUS_DELETING,
    );

    // A detector inside a workspace whose own deletion has started: the workspace is the
    // resource that refuses the call.
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "cascade").await;
    let created = create_detector(
        &amp,
        region,
        &workspace_id,
        &spec.create_body("requests", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"workspace DELETING\": CreateAnomalyDetector must answer 202, got {created:?}"
    );
    let detector_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();
    let deleted = amp.delete(region, &workspace_id, None).await;
    assert_eq!(
        deleted.status, 202,
        "case \"workspace DELETING\": DeleteWorkspace must answer 202, got {deleted:?}"
    );
    let refused = put_detector(
        &amp,
        region,
        &workspace_id,
        &detector_id,
        &replacement.put_body(),
    )
    .await;
    assert_conflict("workspace DELETING", &refused, &workspace_id, WORKSPACE_RESOURCE_TYPE);
    assert_persisted(
        "workspace DELETING",
        &describe_detector(&amp, region, &workspace_id, &detector_id).await,
        &spec,
        STATUS_ACTIVE,
    );
}

/// The modelled `ConflictException` a status refusal answers with, naming the resource
/// whose status refused the call.
fn assert_conflict(case: &str, refused: &Reply, resource_id: &str, resource_type: &str) {
    assert_eq!(
        refused.status, 409,
        "case {case:?}: PutAnomalyDetector must answer 409, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ConflictException",
        "case {case:?}: unexpected error code, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["resourceId"]),
        resource_id,
        "case {case:?}: expected resourceId {resource_id:?}, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["resourceType"]),
        resource_type,
        "case {case:?}: expected resourceType {resource_type:?}, got {refused:?}"
    );
}

/// The configuration rule holds for the specification an update stores, exactly as it
/// does for the one a create stores: a Random Cut Forest query must BE a call to one of
/// the aggregation operators the model's `RandomCutForestConfiguration$query`
/// documentation lists, and anything else is the modelled `ValidationException` (400)
/// naming the member, with nothing stored.
///
/// Every operator in that published catalog is exercised, including ones no issue cited,
/// each with and without an aggregation modifier.
#[tokio::test]
async fn put_requires_a_query_wrapped_by_a_supported_aggregation_operator() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "queries").await;

    let seed = Spec::new("sum(rate(requests_total[5m]))");
    let created = create_detector(
        &amp,
        region,
        &workspace_id,
        &seed.create_body("requests", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"seed\": CreateAnomalyDetector must answer 202, got {created:?}"
    );
    let detector_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();

    let mut held = seed.clone();
    // Every operator the catalog publishes, plus the aggregation modifiers PromQL's
    // `<aggr-op> [by|without (<labels>)] (<expression>)` syntax allows between an
    // operator and its argument list — still a query wrapped by that operator.
    let mut accepted_queries: Vec<String> = RANDOM_CUT_FOREST_AGGREGATION_OPERATORS
        .iter()
        .map(|operator| format!("{operator}(rate(requests_total[5m]))"))
        .collect();
    accepted_queries.push("sum by (job) (rate(requests_total[5m]))".to_string());
    accepted_queries.push("max without (instance) (up)".to_string());

    for form in accepted_queries {
        let accepted = Spec::new(&form);
        let reply = put_detector(
            &amp,
            region,
            &workspace_id,
            &detector_id,
            &accepted.put_body(),
        )
        .await;
        assert_eq!(
            reply.status, 202,
            "case {form:?}: a query wrapped by a supported operator must be accepted, \
             got {reply:?}"
        );
        assert_persisted(
            &form,
            &describe_detector(&amp, region, &workspace_id, &detector_id).await,
            &accepted,
            STATUS_UPDATING,
        );
        held = accepted;
        wait_for_due_transition();
    }

    // Forms that are NOT a call to one of those operators: a bare selector, an operator
    // the catalog does not list, an aggregation that is not outermost, and a parenthesised
    // expression.
    for query in [
        "rate(requests_total[5m])",
        "requests_total",
        "topk(5, rate(requests_total[5m]))",
        "1 + sum(rate(requests_total[5m]))",
        "(sum(rate(requests_total[5m])))",
        "summary(rate(requests_total[5m]))",
    ] {
        let refused_spec = Spec::new(query);
        let refused = put_detector(
            &amp,
            region,
            &workspace_id,
            &detector_id,
            &refused_spec.put_body(),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {query:?}: a query not wrapped by a supported operator must be refused, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {query:?}: unexpected error code, got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["fieldList"][0]["name"]),
            "configuration.randomCutForest.query",
            "case {query:?}: the refusal must name the member it refused, got {refused:?}"
        );
        // Refused before anything was stored: the detector still holds the last accepted
        // specification, and is still usable.
        assert_persisted(
            query,
            &describe_detector(&amp, region, &workspace_id, &detector_id).await,
            &held,
            STATUS_ACTIVE,
        );
    }

    // A metric label whose key is outside `PrometheusMetricLabelKey`'s modelled pattern
    // (`^(?!__)[a-zA-Z_][a-zA-Z0-9_]*$`) is refused the same way, naming `labels`.
    for key in ["__reserved", "7shard", "has-dash"] {
        let mut body = Spec::new("sum(rate(requests_total[5m]))").put_body();
        body.as_object_mut()
            .expect("an object body")
            .insert("labels".to_string(), json!({ key: "value" }));
        let refused = put_detector(&amp, region, &workspace_id, &detector_id, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {key:?}: a label key outside the modelled pattern must be refused, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {key:?}: unexpected error code, got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["fieldList"][0]["name"]),
            "labels",
            "case {key:?}: the refusal must name the member it refused, got {refused:?}"
        );
        assert_persisted(
            key,
            &describe_detector(&amp, region, &workspace_id, &detector_id).await,
            &held,
            STATUS_ACTIVE,
        );
    }
}

/// `AnomalyDetectorEvaluationInterval` is modelled `@range(min: 30, max: 86400)`: an
/// update at each published bound is accepted and one step past either is refused with
/// 400, so a caller cannot store an interval outside that range through this operation
/// either.
#[tokio::test]
async fn put_holds_the_evaluation_interval_to_its_modelled_range() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "intervals").await;

    let seed = Spec::new("sum(rate(requests_total[5m]))").interval(60);
    let created = create_detector(
        &amp,
        region,
        &workspace_id,
        &seed.create_body("requests", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"seed\": CreateAnomalyDetector must answer 202, got {created:?}"
    );
    let detector_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();

    let mut held = seed.clone();
    for (interval, expected) in [(29_i64, 400_u16), (30, 202), (86_400, 202), (86_401, 400)] {
        let spec = Spec::new("sum(rate(requests_total[5m]))").interval(interval);
        let reply = put_detector(&amp, region, &workspace_id, &detector_id, &spec.put_body()).await;
        assert_eq!(
            reply.status, expected,
            "case {interval:?}: expected HTTP {expected}, got {reply:?}"
        );
        if expected == 202 {
            assert_persisted(
                &interval.to_string(),
                &describe_detector(&amp, region, &workspace_id, &detector_id).await,
                &spec,
                STATUS_UPDATING,
            );
            held = spec;
            wait_for_due_transition();
        } else {
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {interval:?}: unexpected error code, got {reply:?}"
            );
            assert_persisted(
                &interval.to_string(),
                &describe_detector(&amp, region, &workspace_id, &detector_id).await,
                &held,
                STATUS_ACTIVE,
            );
        }
    }
}
