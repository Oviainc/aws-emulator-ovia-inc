// SPDX-License-Identifier: CC0-1.0

//! A map's KEYS hold to their key shape's modelled constraints, end to end over HTTP.
//!
//! The rule under test is one sentence: a request carrying a map member is refused HTTP
//! 400 `ValidationException` — naming the MAP member and the requirement the key failed,
//! and persisting nothing — whenever any key of that map violates the key shape's
//! modelled `@length` or `@pattern`. It is asserted over a TABLE per map shape (the rows
//! differing along which clause of the constraint they miss, the key's character set, its
//! length at each published bound and one past it, and the number of entries the map
//! carries) and over every operation that accepts a map of that shape.
//!
//! The three map shapes this service's requests carry, each with the constrained key the
//! rows exercise:
//!
//! * `PrometheusMetricLabelMap` / `PrometheusMetricLabelKey` — `CreateAnomalyDetector$labels`
//! * `TagMap` / `TagKey` — the `tags` every create carries, and `TagResource$tags`
//! * `LabelSet` / `LabelName` — `UpdateWorkspaceConfiguration$limitsPerLabelSet[].labelSet`
//!
//! The harness (`Amp`, `Reply`, `text`, `encode_label`) is the one
//! `tests/all/workspace_lifecycle.rs` owns, so these suites drive the same
//! `MultiServiceRouter::dispatch` request path as every other suite in this target.

use serde_json::{json, Value};

use super::workspace_lifecycle::{encode_label, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    METRIC_NAME_LABEL, VALIDATION_REASON,
};

// The constraints below are the model's, read with
// `smithy_query(query="shape", shape="PrometheusMetricLabelKey,TagKey,LabelName")`:
//
// * `PrometheusMetricLabelKey` — @length(min: 1, max: 7168),
//   @pattern("^(?!__)[a-zA-Z_][a-zA-Z0-9_]*$")
// * `TagKey` — @length(min: 1, max: 128), @pattern("^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$")
// * `LabelName` — @length(min: 1), @pattern("^[a-zA-Z_][a-zA-Z0-9_]*$")
//
// Each requirement below is that constraint as this service words it
// (`state::constraint_requirement`, with a `@pattern`'s `^`/`$` anchors dropped) — the
// same wording a refusal made at the request boundary carries.

/// `PrometheusMetricLabelKey`'s modelled maximum length.
const LABEL_KEY_MAX_LENGTH: usize = 7168;

/// `TagKey`'s modelled maximum length.
const TAG_KEY_MAX_LENGTH: usize = 128;

const LABEL_KEY_PATTERN_REQUIREMENT: &str =
    "Member must satisfy regular expression pattern: (?!__)[a-zA-Z_][a-zA-Z0-9_]*";

const LABEL_NAME_PATTERN_REQUIREMENT: &str =
    "Member must satisfy regular expression pattern: [a-zA-Z_][a-zA-Z0-9_]*";

const TAG_KEY_PATTERN_REQUIREMENT: &str =
    r"Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)";

const MIN_LENGTH_REQUIREMENT: &str = "Member must have length greater than or equal to 1";

const LABEL_KEY_MAX_LENGTH_REQUIREMENT: &str =
    "Member must have length less than or equal to 7168";

const TAG_KEY_MAX_LENGTH_REQUIREMENT: &str = "Member must have length less than or equal to 128";

/// One row of a table that submits a map and expects a refusal: the case name, the map's
/// entries in source order, and the requirement the refusal must report (empty where the
/// row must be ACCEPTED instead).
type MapCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, &'a str);

/// One row of a table that submits a map and expects it to be stored: the case name and
/// the map's entries in source order.
type AcceptedMapCase<'a> = (&'a str, Vec<(&'a str, &'a str)>);

/// A `randomCutForest` configuration whose query is wrapped by a supported PromQL
/// aggregation operator, so `CreateAnomalyDetector` reaches the map members under test.
fn detector_configuration() -> Value {
    json!({ "randomCutForest": { "query": "sum(up)" } })
}

/// A map on the wire, built from keys a `json!` literal cannot carry (an empty key, a key
/// 7,169 characters long, a key holding a control character).
fn wire_map(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

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

/// `PATCH /workspaces/{workspaceId}/configuration`.
async fn update_configuration(amp: &Amp, region: &str, workspace_id: &str, body: &Value) -> Reply {
    amp.call(
        "PATCH",
        &format!("/workspaces/{workspace_id}/configuration"),
        region,
        &body.to_string(),
    )
    .await
}

/// One member refusal, asserted the way every aps member refusal reads: HTTP 400
/// `ValidationException`, `reason` set, exactly ONE `fieldList` entry naming the MAP
/// member and the requirement its key failed, the same requirement in `message`, and
/// nothing from the request anywhere in the body.
fn assert_member_refusal(case: &str, reply: &Reply, member: &str, requirement: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: a key outside the key shape's constraints must be refused 400, \
         got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["reason"]),
        VALIDATION_REASON,
        "case {case:?}: `reason` is @required, got {reply:?}"
    );
    assert_eq!(
        reply.body["fieldList"],
        json!([{ "name": member, "message": requirement }]),
        "case {case:?}: expected one fieldList entry naming {member:?} and the requirement \
         the key failed, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        format!("Invalid {member}: {requirement}"),
        "case {case:?}: expected the message to name the member and its requirement, \
         got {reply:?}"
    );
}

/// `CreateAnomalyDetector$labels`: every clause of `PrometheusMetricLabelKey` refuses the
/// call, and the detector is never minted. The rows differ along which clause is missed
/// (the reserved double-underscore prefix, the leading character class, the trailing
/// character class, each published length bound) and along the key's character set; the
/// last row shows that a well-formed key beside a bad one does not rescue the map.
#[tokio::test]
async fn a_label_key_outside_the_key_shapes_constraints_is_refused() {
    let amp = Amp::new();
    let region = "us-west-2";
    let workspace_id = amp.active_workspace(region, "labels").await;
    let too_long = "a".repeat(LABEL_KEY_MAX_LENGTH + 1);

    // (case, the labels map submitted, the requirement the refusal must report)
    let cases: Vec<MapCase> = vec![
        (
            "the metric name label",
            vec![(METRIC_NAME_LABEL, "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "another reserved double-underscore name",
            vec![("__anything", "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "the reserved prefix on its own",
            vec![("__", "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "a leading digit",
            vec![("1bad", "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "a hyphen",
            vec![("bad-label", "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "a space",
            vec![("bad label", "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            // A colon is a character a Prometheus METRIC name admits and a label name
            // does not, so the two character sets are not interchangeable.
            "a colon",
            vec![("job:rate", "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "a non-ASCII letter",
            vec![("étage", "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "one below the published minimum length",
            vec![("", "x")],
            MIN_LENGTH_REQUIREMENT,
        ),
        (
            "one past the published maximum length",
            vec![(too_long.as_str(), "x")],
            LABEL_KEY_MAX_LENGTH_REQUIREMENT,
        ),
        (
            "a well-formed key beside a reserved one",
            vec![("service", "checkout"), (METRIC_NAME_LABEL, "x")],
            LABEL_KEY_PATTERN_REQUIREMENT,
        ),
    ];

    for (case, labels, requirement) in cases {
        let reply = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": "labelled",
                "configuration": detector_configuration(),
                "labels": wire_map(&labels),
            }),
        )
        .await;
        assert_member_refusal(case, &reply, "labels", requirement);
        assert!(
            reply.body.get("anomalyDetectorId").is_none(),
            "case {case:?}: a refused create must mint nothing, got {reply:?}"
        );
    }
}

/// The accepting half over the same member: every key form `PrometheusMetricLabelKey`
/// admits is stored and read back through `DescribeAnomalyDetector` — including a key at
/// each published length bound and a single underscore-led name, which the reserved-prefix
/// clause must not catch.
#[tokio::test]
async fn every_label_key_the_key_shape_admits_round_trips() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let workspace_id = amp.active_workspace(region, "labels").await;
    let at_max_length = "a".repeat(LABEL_KEY_MAX_LENGTH);

    // (case, the labels map submitted)
    let cases: Vec<AcceptedMapCase> = vec![
        ("a plain label name", vec![("service", "checkout")]),
        ("one underscore, not two", vec![("_private", "1")]),
        ("an underscore then a digit", vec![("_1", "1")]),
        ("a single character, the published minimum length", vec![("a", "1")]),
        ("mixed case and digits", vec![("A_b9Z", "1")]),
        ("at the published maximum length", vec![(at_max_length.as_str(), "1")]),
        (
            "several labels at once",
            vec![("service", "checkout"), ("tier", "edge"), ("shard", "7")],
        ),
    ];

    for (case, labels) in cases {
        let created = create_detector(
            &amp,
            region,
            &workspace_id,
            &json!({
                "alias": "labelled",
                "configuration": detector_configuration(),
                "labels": wire_map(&labels),
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a key the key shape admits must be accepted, got {created:?}"
        );

        let detector_id = text(&created.body["anomalyDetectorId"]);
        let described = describe_detector(&amp, region, &workspace_id, &detector_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the minted detector must read back, got {described:?}"
        );
        assert_eq!(
            described.body["anomalyDetector"]["labels"],
            wire_map(&labels),
            "case {case:?}: describe must report the labels the service persisted, \
             got {described:?}"
        );
    }
}

/// A refused create writes NOTHING: it mints no detector, and it does not record the
/// `clientToken` it carried — so the same token sent again with a well-formed map creates
/// the detector rather than replaying a refusal's half-finished record.
#[tokio::test]
async fn a_refused_create_persists_nothing_and_keeps_no_token() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "tokens").await;
    let token = "aabbccdd-0011-2233-4455-667788990011";

    let refused = create_detector(
        &amp,
        region,
        &workspace_id,
        &json!({
            "alias": "labelled",
            "clientToken": token,
            "configuration": detector_configuration(),
            "labels": wire_map(&[(METRIC_NAME_LABEL, "x")]),
        }),
    )
    .await;
    assert_member_refusal(
        "refused create",
        &refused,
        "labels",
        LABEL_KEY_PATTERN_REQUIREMENT,
    );

    let accepted = create_detector(
        &amp,
        region,
        &workspace_id,
        &json!({
            "alias": "labelled",
            "clientToken": token,
            "configuration": detector_configuration(),
            "labels": wire_map(&[("service", "checkout")]),
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case \"replayed token\": the token the refused call carried must still be free, \
         got {accepted:?}"
    );
    let detector_id = text(&accepted.body["anomalyDetectorId"]);
    let described = describe_detector(&amp, region, &workspace_id, &detector_id).await;
    assert_eq!(
        described.body["anomalyDetector"]["labels"],
        wire_map(&[("service", "checkout")]),
        "case \"replayed token\": the stored detector must hold only the labels the \
         accepted call carried, got {described:?}"
    );
}

/// One request carrying a `tags` map: the operation it names and its HTTP binding, with
/// the map spliced into the body each operation carries it in.
struct TagSink {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: fn(Value) -> Value,
}

/// Every implemented operation that accepts a `TagMap`, so the rule is asserted over the
/// whole class rather than over one of them.
fn tag_sinks(workspace_id: &str, workspace_arn: &str) -> Vec<TagSink> {
    vec![
        TagSink {
            operation: "CreateWorkspace",
            method: "POST",
            path: "/workspaces".to_string(),
            body: |tags| json!({ "alias": "tagged", "tags": tags }),
        },
        TagSink {
            operation: "CreateAnomalyDetector",
            method: "POST",
            path: format!("/workspaces/{workspace_id}/anomalydetectors"),
            body: |tags| {
                json!({
                    "alias": "tagged",
                    "configuration": detector_configuration(),
                    "tags": tags,
                })
            },
        },
        TagSink {
            operation: "CreateRuleGroupsNamespace",
            method: "POST",
            path: format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
            // `data` is the base64 of `groups: []`, a rules file with no rule groups.
            body: |tags| json!({ "name": "rules", "data": "Z3JvdXBzOiBbXQ==", "tags": tags }),
        },
        TagSink {
            operation: "TagResource",
            method: "POST",
            path: format!("/tags/{}", encode_label(workspace_arn)),
            body: |tags| json!({ "tags": tags }),
        },
    ]
}

/// `TagKey`'s modelled constraints over every operation that accepts a `TagMap`: a key
/// missing either length bound or carrying a character the pattern's classes exclude is
/// refused, whichever operation the map was sent to, and nothing is tagged.
#[tokio::test]
async fn a_tag_key_outside_the_key_shapes_constraints_is_refused() {
    let amp = Amp::new();
    let region = "us-west-2";
    let created = amp
        .call(
            "POST",
            "/workspaces",
            region,
            &json!({ "alias": "tags" }).to_string(),
        )
        .await;
    assert_eq!(created.status, 202, "case \"seed\": {created:?}");
    let workspace_id = text(&created.body["workspaceId"]);
    let workspace_arn = text(&created.body["arn"]);
    wait_for_due_transition();
    let too_long = "t".repeat(TAG_KEY_MAX_LENGTH + 1);

    // (case, the tags map submitted, the requirement the refusal must report)
    let cases: Vec<MapCase> = vec![
        (
            "an asterisk, which no class of the pattern admits",
            vec![("tag*key", "v")],
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "a hash",
            vec![("tag#key", "v")],
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            // An ASCII control character is not a letter, separator or number.
            "a control character",
            vec![("tag\u{7}key", "v")],
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "one below the published minimum length",
            vec![("", "v")],
            MIN_LENGTH_REQUIREMENT,
        ),
        (
            "one past the published maximum length",
            vec![(too_long.as_str(), "v")],
            TAG_KEY_MAX_LENGTH_REQUIREMENT,
        ),
        (
            "a well-formed key beside a bad one",
            vec![("team", "obs"), ("tag#key", "v")],
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
    ];

    for (case, tags, requirement) in cases {
        for sink in tag_sinks(&workspace_id, &workspace_arn) {
            let case = format!("{} with {case}", sink.operation);
            let body = (sink.body)(wire_map(&tags));
            let reply = amp
                .call(sink.method, &sink.path, region, &body.to_string())
                .await;
            assert_member_refusal(&case, &reply, "tags", requirement);
        }
    }

    // Nothing the refused calls carried was tagged, and no refused create minted a
    // resource of its own.
    let listed = amp.list_tags(region, &workspace_arn).await;
    assert_eq!(
        listed.status, 200,
        "case \"nothing tagged\": the seed workspace must still read back, got {listed:?}"
    );
    assert_eq!(
        listed.body["tags"],
        json!({}),
        "case \"nothing tagged\": a refused tag call must persist nothing, got {listed:?}"
    );
}

/// The accepting half: every key form `TagKey`'s classes admit — Unicode letters,
/// separators and numbers beside the punctuation the pattern lists, at the published
/// maximum length — is accepted and read back through `ListTagsForResource`.
#[tokio::test]
async fn every_tag_key_the_key_shape_admits_round_trips() {
    let amp = Amp::new();
    let region = "ap-southeast-2";
    let at_max_length = "t".repeat(TAG_KEY_MAX_LENGTH);

    // (case, the tags map submitted)
    let cases: Vec<AcceptedMapCase> = vec![
        ("a plain key", vec![("team", "obs")]),
        (
            "the punctuation the pattern lists",
            vec![("cost.center:eu/west=1+2-3@aws", "v")],
        ),
        ("a Unicode letter key", vec![("ключ", "v")]),
        ("a separator and a number", vec![("cost center 2", "v")]),
        ("a single character, the published minimum length", vec![("t", "v")]),
        ("at the published maximum length", vec![(at_max_length.as_str(), "v")]),
    ];

    for (case, tags) in cases {
        let created = amp
            .call(
                "POST",
                "/workspaces",
                region,
                &json!({ "alias": "tagged", "tags": wire_map(&tags) }).to_string(),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a key the key shape admits must be accepted, got {created:?}"
        );
        let workspace_arn = text(&created.body["arn"]);
        let listed = amp.list_tags(region, &workspace_arn).await;
        assert_eq!(
            listed.body["tags"],
            wire_map(&tags),
            "case {case:?}: the tags the service persisted must read back, got {listed:?}"
        );
    }
}

/// `UpdateWorkspaceConfiguration$limitsPerLabelSet[].labelSet`: `LabelName`'s modelled
/// constraints are applied to the label set's keys too, and a refused update leaves the
/// stored configuration alone. `LabelName` carries no reserved-prefix clause, so the
/// metric name label — which `PrometheusMetricLabelKey` refuses — is a label set a
/// caller may limit.
#[tokio::test]
async fn a_label_set_name_outside_the_key_shapes_constraints_is_refused() {
    let amp = Amp::new();
    let region = "eu-central-1";

    // (case, the labelSet submitted, the requirement a refusal must report, or "" when
    // the row must be accepted)
    let cases: Vec<MapCase> = vec![
        (
            "a leading digit",
            vec![("1team", "obs")],
            LABEL_NAME_PATTERN_REQUIREMENT,
        ),
        (
            "a hyphen",
            vec![("bad-label", "obs")],
            LABEL_NAME_PATTERN_REQUIREMENT,
        ),
        (
            "a colon",
            vec![("job:rate", "obs")],
            LABEL_NAME_PATTERN_REQUIREMENT,
        ),
        (
            "one below the published minimum length",
            vec![("", "obs")],
            MIN_LENGTH_REQUIREMENT,
        ),
        ("a plain label name", vec![("env", "prod")], ""),
        // `LabelSet`'s key is `LabelName`, whose pattern admits the double-underscore
        // prefix: the metric name label is limitable, and its VALUE is what the operation
        // then holds to a metric name.
        ("the metric name label", vec![(METRIC_NAME_LABEL, "up")], ""),
    ];

    for (case, label_set, requirement) in cases {
        // A fresh workspace per row: an accepted row writes, and a refused one must be
        // refused on its own merits.
        let workspace_id = amp.active_workspace(region, "labelsets").await;
        let body = json!({
            "limitsPerLabelSet": [{ "labelSet": wire_map(&label_set), "limits": { "maxSeries": 1 } }],
        });
        let reply = update_configuration(&amp, region, &workspace_id, &body).await;
        if requirement.is_empty() {
            assert_eq!(
                reply.status, 202,
                "case {case:?}: a label name the key shape admits must be accepted, \
                 got {reply:?}"
            );
            wait_for_due_transition();
            let described = amp
                .call(
                    "GET",
                    &format!("/workspaces/{workspace_id}/configuration"),
                    region,
                    "",
                )
                .await;
            assert_eq!(
                described.body["workspaceConfiguration"]["limitsPerLabelSet"][0]["labelSet"],
                wire_map(&label_set),
                "case {case:?}: the label set the service persisted must read back, \
                 got {described:?}"
            );
            continue;
        }

        assert_member_refusal(case, &reply, "limitsPerLabelSet", requirement);
        let described = amp
            .call(
                "GET",
                &format!("/workspaces/{workspace_id}/configuration"),
                region,
                "",
            )
            .await;
        assert_eq!(
            described.body["workspaceConfiguration"]["limitsPerLabelSet"],
            json!([]),
            "case {case:?}: a refused update must persist no label set, got {described:?}"
        );
    }
}
