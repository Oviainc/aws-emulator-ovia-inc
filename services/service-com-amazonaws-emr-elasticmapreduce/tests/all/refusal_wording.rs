// SPDX-License-Identifier: CC0-1.0

//! What an Amazon EMR refusal says, WORD for word, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends.
//!
//! THE RULE every table below asserts: a refused call answers the service's own sentence
//! for that operation and that error class — its fixed text exactly, its variable parts
//! taken from the request and from state, and NOTHING else: no clause, value or
//! diagnostic the service does not word there, and no sentence borrowed from another
//! operation's class. The status and the error code are asserted beside the text,
//! because a client reads all three.
//!
//! The recorded instances this suite was written from are ONE row each; the tables span
//! the class the rule is stated over — every form of the offending value, every operation
//! that answers the class, and (where the sentence is constant) several inputs that must
//! all produce the SAME text.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_refused, kept_alive_request, Emr};
use crate::refusal_messages::message;
use crate::supported_instance_types::BAD_RELEASE_LABELS;

/// The sentence the LEGACY job-flow surface answers an id it resolves no cluster for:
/// constant, with no id in it.
///
/// `pub(crate)` so a sibling suite asserts the same sentence through one constant rather
/// than its own copy.
pub(crate) const JOB_FLOW_SENTENCE: &str = "Specified job flow ID not valid.";

/// Ids no cluster is held under. Rows differ along the form of the id: the shape a real
/// id has, a short and a long body, a bare prefix, the empty string (`ClusterId` is
/// `len 0..256`, so the boundary validator hands it to the handler) and a value of
/// another form entirely.
const UNKNOWN_CLUSTER_IDS: &[&str] = &[
    "j-ZZZZZZZZZZZZZ",
    "j-3ABCDEFGHIJKL",
    "j-1",
    "j-000000000000000000000000",
    "j-",
    "",
    "not-a-cluster-id",
];

// ---------------------------------------------------------------------------
// PutBlockPublicAccessConfiguration: an inverted port range
// ---------------------------------------------------------------------------

/// THE RULE: a permitted-port range whose `MinRange` exceeds its `MaxRange` is refused
/// with `InvalidRequestException` (HTTP 400) and the service's sentence for that class,
/// which states the pair it rejected as `(MinRange, MaxRange)` in the order the request
/// sent it and then the rule the two values broke.
///
/// Rows span the inverted pairs the member admits: a pair well inside the port space,
/// the pair from the published bounds, two pairs one apart, a pair of equal magnitude
/// either side of zero, and an inversion sitting behind disjoint ranges the walk reaches
/// first.
#[tokio::test]
async fn an_inverted_port_range_is_refused_with_the_port_range_sentence() {
    let emr = Emr::new();

    // `PortRange$MinRange` / `$MaxRange` both target `Port`, `@range min -1 max 65535`
    // (smithy_query(query="shape", shape="PortRange,Port")), so every pair below is one
    // the boundary validator admits and the handler has to judge.
    let cases: Vec<(&str, Value, (i64, i64))> = vec![
        (
            "a pair well inside the port space",
            json!([{ "MinRange": 200, "MaxRange": 100 }]),
            (200, 100),
        ),
        (
            "another pair inside the port space",
            json!([{ "MinRange": 90, "MaxRange": 20 }]),
            (90, 20),
        ),
        (
            "the published bounds, inverted",
            json!([{ "MinRange": 65535, "MaxRange": -1 }]),
            (65535, -1),
        ),
        (
            "a pair one apart",
            json!([{ "MinRange": 23, "MaxRange": 22 }]),
            (23, 22),
        ),
        (
            "a pair either side of zero",
            json!([{ "MinRange": 0, "MaxRange": -1 }]),
            (0, -1),
        ),
        (
            "an inversion behind two disjoint ranges",
            json!([
                { "MinRange": 20, "MaxRange": 21 },
                { "MinRange": 80, "MaxRange": 80 },
                { "MinRange": 9000, "MaxRange": 8999 },
            ]),
            (9000, 8999),
        ),
    ];

    for (case, ranges, (min, max)) in cases {
        let (status, payload) = emr
            .call(
                "PutBlockPublicAccessConfiguration",
                json!({
                    "BlockPublicAccessConfiguration": {
                        "BlockPublicSecurityGroupRules": true,
                        "PermittedPublicSecurityGroupRuleRanges": ranges,
                    },
                }),
            )
            .await;
        assert_refused(case, status, &payload, "InvalidRequestException");
        let expected = format!(
            "The specified port range ({min}, {max}) is not valid. Min Range must be \
             less than or equal to Max Range."
        );
        assert_eq!(
            message(case, &payload),
            expected,
            "case {case:?}: expected {expected:?}, got {payload}"
        );
    }

    // The control: an upward range of the same values is stored, so the refusals above
    // are about the ORDER of the pair and not about the operation.
    emr.ok(
        "PutBlockPublicAccessConfiguration",
        json!({
            "BlockPublicAccessConfiguration": {
                "BlockPublicSecurityGroupRules": true,
                "PermittedPublicSecurityGroupRuleRanges": [{ "MinRange": 100, "MaxRange": 200 }],
            },
        }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// The security-configuration surface
// ---------------------------------------------------------------------------

/// Names a security configuration can be created under. Rows differ along the text of
/// the name — `Name` is an `XmlString`, so the class is wide: ASCII, a name carrying the
/// quote character the template itself uses, spaces, non-ASCII text and the empty string.
fn security_configuration_names() -> Vec<String> {
    vec![
        "analytics-prod".to_string(),
        "a name with 'single' quotes".to_string(),
        "a name with \"double\" quotes".to_string(),
        "a name with spaces".to_string(),
        "конфигурация".to_string(),
        String::new(),
    ]
}

/// THE RULE: a `Name` the account holds no security configuration under is refused with
/// the service's sentence for an absent configuration, which quotes the name the request
/// sent with DOUBLE quotes — the same sentence whether the caller described or deleted
/// it, and whether the name was never created or has just been deleted.
#[tokio::test]
async fn an_absent_security_configuration_is_worded_with_the_name_double_quoted() {
    let emr = Emr::new();
    // One configuration is held throughout, so every refusal below is a lookup.
    emr.ok(
        "CreateSecurityConfiguration",
        json!({ "Name": "held", "SecurityConfiguration": "{}" }),
    )
    .await;

    for name in security_configuration_names() {
        let expected = format!("Security configuration with name \"{name}\" does not exist.");
        // Leg one: a name that was never created. Leg two: the same name after a create
        // and a delete, which is the same absent resource.
        for leg in ["never created", "created then deleted"] {
            if leg == "created then deleted" {
                emr.ok(
                    "CreateSecurityConfiguration",
                    json!({ "Name": &name, "SecurityConfiguration": "{}" }),
                )
                .await;
                emr.ok("DeleteSecurityConfiguration", json!({ "Name": &name }))
                    .await;
            }
            for operation in [
                "DescribeSecurityConfiguration",
                "DeleteSecurityConfiguration",
            ] {
                let case = format!("{operation} on {name:?} ({leg})");
                let (status, payload) = emr.call(operation, json!({ "Name": &name })).await;
                assert_refused(&case, status, &payload, "InvalidRequestException");
                assert_eq!(
                    message(&case, &payload),
                    expected,
                    "case {case:?}: expected {expected:?}, got {payload}"
                );
            }
        }
    }

    // The held configuration still reads back, so the refusals above resolved names.
    let held = emr
        .ok("DescribeSecurityConfiguration", json!({ "Name": "held" }))
        .await;
    assert_eq!(
        held["Name"].as_str(),
        Some("held"),
        "the held configuration must still read back, got {held}"
    );
}

/// THE RULE: a create under a name the account already holds a DIFFERENT configuration
/// under is refused with the service's conflict sentence, which names the shape and
/// single-quotes the name the request sent.
#[tokio::test]
async fn a_taken_security_configuration_name_is_worded_as_a_conflict() {
    let emr = Emr::new();

    for name in security_configuration_names() {
        emr.ok(
            "CreateSecurityConfiguration",
            json!({ "Name": &name, "SecurityConfiguration": "{}" }),
        )
        .await;

        // Rows differ along the document the second create submits: any document other
        // than the stored one is the conflict.
        for (label, document) in [
            ("an empty object with a member added", json!({ "Other": 1 }).to_string()),
            (
                "a document configuring encryption",
                json!({ "EncryptionConfiguration": { "EnableAtRestEncryption": false } })
                    .to_string(),
            ),
        ] {
            let case = format!("a second create of {name:?} with {label}");
            let (status, payload) = emr
                .call(
                    "CreateSecurityConfiguration",
                    json!({ "Name": &name, "SecurityConfiguration": document }),
                )
                .await;
            assert_refused(&case, status, &payload, "InvalidRequestException");
            let expected = format!("SecurityConfiguration with name '{name}' already exists.");
            assert_eq!(
                message(&case, &payload),
                expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
        }

        // The stored document is untouched by the refused creates.
        let described = emr
            .ok("DescribeSecurityConfiguration", json!({ "Name": &name }))
            .await;
        assert_eq!(
            described["SecurityConfiguration"].as_str(),
            Some("{}"),
            "case {name:?}: a refused create must not replace the stored document, got \
             {described}"
        );
    }
}

/// THE RULE: a `SecurityConfiguration` document that is not JSON is refused with the
/// service's CONSTANT sentence for a malformed document — the same text for every such
/// document, naming neither the parser, nor where it stopped, nor anything the document
/// carried.
///
/// Rows span the ways a document fails to parse — each would make a JSON parser stop at a
/// different place, and the refusal must not vary with that.
#[tokio::test]
async fn a_malformed_security_configuration_document_is_worded_constantly() {
    let emr = Emr::new();
    let expected = "Invalid security configuration; Malformed Json string.";

    let documents = [
        ("bare text", "this-is-not-json"),
        ("an unquoted key", "{not valid json"),
        ("an unterminated object", "{\"EncryptionConfiguration\":"),
        ("an unterminated array", "["),
        ("an empty document", ""),
        ("a missing value", "{\"EncryptionConfiguration\":}"),
        ("a trailing comma", "{\"a\": 1,}"),
        ("single-quoted JSON", "{'EncryptionConfiguration': {}}"),
        ("an XML document", "<EncryptionConfiguration/>"),
    ];

    for (case, document) in documents {
        let (status, payload) = emr
            .call(
                "CreateSecurityConfiguration",
                json!({ "Name": format!("malformed-{case}"), "SecurityConfiguration": document }),
            )
            .await;
        assert_refused(case, status, &payload, "InvalidRequestException");
        assert_eq!(
            message(case, &payload),
            expected,
            "case {case:?}: expected {expected:?}, got {payload}"
        );

        // A refused create stored nothing, so the name is still absent.
        let (status, payload) = emr
            .call(
                "DescribeSecurityConfiguration",
                json!({ "Name": format!("malformed-{case}") }),
            )
            .await;
        assert_refused(case, status, &payload, "InvalidRequestException");
    }
}

// ---------------------------------------------------------------------------
// The release-label surface
// ---------------------------------------------------------------------------

/// THE RULE: each operation that resolves a release label words an unoffered label its
/// OWN way — `DescribeReleaseLabel` with a constant sentence that echoes nothing, and
/// `ListSupportedInstanceTypes` with one that names the label the request sent — so the
/// two are not interchangeable.
#[tokio::test]
async fn an_unoffered_release_label_is_worded_per_operation() {
    let emr = Emr::new();

    for label in BAD_RELEASE_LABELS {
        // `ListSupportedInstanceTypes` requires the member, so every row reaches it.
        let case = format!("ListSupportedInstanceTypes with {label:?}");
        let (status, payload) = emr
            .call(
                "ListSupportedInstanceTypes",
                json!({ "ReleaseLabel": label }),
            )
            .await;
        assert_refused(&case, status, &payload, "InvalidRequestException");
        let expected = format!("The supplied release label is invalid: {label}.");
        assert_eq!(
            message(&case, &payload),
            expected,
            "case {case:?}: expected {expected:?}, got {payload}"
        );

        // `DescribeReleaseLabel`'s member is optional, and a request carrying none names
        // no release at all — a different class, with its own sentence — so the empty
        // label is not a row of this one.
        if label.is_empty() {
            continue;
        }
        let case = format!("DescribeReleaseLabel with {label:?}");
        let (status, payload) = emr
            .call("DescribeReleaseLabel", json!({ "ReleaseLabel": label }))
            .await;
        assert_refused(&case, status, &payload, "InvalidRequestException");
        let said = message(&case, &payload);
        assert_eq!(
            said, "The release label isn't valid.",
            "case {case:?}: unexpected message, got {payload}"
        );
        assert!(
            !said.contains(label),
            "case {case:?}: the refusal must not echo the label, got {said:?}"
        );
    }

    // The control: a label the catalog does offer is served by both operations, so the
    // refusals above are lookups of the catalog.
    let described = emr
        .ok("DescribeReleaseLabel", json!({ "ReleaseLabel": "emr-7.5.0" }))
        .await;
    assert_eq!(
        described["ReleaseLabel"].as_str(),
        Some("emr-7.5.0"),
        "an offered release must still be described, got {described}"
    );
    let listed = emr
        .ok(
            "ListSupportedInstanceTypes",
            json!({ "ReleaseLabel": "emr-7.5.0" }),
        )
        .await;
    assert!(
        listed["SupportedInstanceTypes"]
            .as_array()
            .is_some_and(|types| !types.is_empty()),
        "an offered release must still report instance types, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// GetSession: the shape of the id, then the resources it names
// ---------------------------------------------------------------------------

/// Session ids no session could ever be minted under: each carries a character outside
/// the class an EMR identifier is drawn from — a space, a slash, a control character,
/// punctuation, a tab, a newline.
const MALFORMED_SESSION_IDS: &[&str] = &[
    "not a valid !! session /// id \u{1}\u{2}",
    "has a space",
    "has/a/slash",
    "has\u{1}a\u{2}control",
    "has\ta\ttab",
    "has\na\nnewline",
    "percent%encoded",
    "is-0000000000000;drop",
];

/// Session ids that COULD have been minted — the class is letters, digits, hyphen and
/// underscore — so each reaches the lookups rather than the shape gate.
const WELL_FORMED_SESSION_IDS: &[&str] = &[
    "is-0000000000000",
    "is-ad6bbdf71f480",
    "s-SESSIONZZZZZ",
    "under_scored",
    "0",
    "",
];

/// THE RULE: `GetSession` settles the SHAPE of `SessionId` before it resolves anything,
/// so a value outside the class a session id is drawn from is a bad request —
/// `ValidationException`, HTTP 400, naming the session's qualified path exactly as the
/// request spelled it — whether or not the cluster beside it exists; a well-formed id
/// instead reaches the lookups, where an unresolvable cluster and an absent session each
/// answer their own sentence.
#[tokio::test]
async fn get_session_judges_the_session_ids_shape_before_the_cluster() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    // Leg one: the shape gate. Rows span the malformed ids and both kinds of cluster id
    // beside them, which is what shows the gate runs BEFORE the cluster is resolved.
    for session_id in MALFORMED_SESSION_IDS {
        for (cluster_label, cluster_id) in [
            ("a live cluster", live.clone()),
            ("an unknown cluster", "j-INVALIDCLUSTERID1".to_string()),
            ("an empty cluster id", String::new()),
        ] {
            let case = format!("GetSession({cluster_label}, {session_id:?})");
            let (status, payload) = emr
                .call(
                    "GetSession",
                    json!({ "ClusterId": &cluster_id, "SessionId": session_id }),
                )
                .await;
            assert_refused(&case, status, &payload, "ValidationException");
            let expected =
                format!("sessionId '{cluster_id}/session/{session_id}' is not valid.");
            assert_eq!(
                message(&case, &payload),
                expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
        }
    }

    // Leg two: a well-formed id reaches the lookups. On an unknown cluster the refusal
    // is about the CLUSTER; on a live one it is about the SESSION, and names only the
    // session id.
    for session_id in WELL_FORMED_SESSION_IDS {
        for unknown in UNKNOWN_CLUSTER_IDS {
            let case = format!("GetSession({unknown:?}, {session_id:?})");
            let (status, payload) = emr
                .call(
                    "GetSession",
                    json!({ "ClusterId": unknown, "SessionId": session_id }),
                )
                .await;
            assert_refused(&case, status, &payload, "InvalidRequestException");
            let expected = format!("Cluster id '{unknown}' is not valid.");
            assert_eq!(
                message(&case, &payload),
                expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
        }

        let case = format!("GetSession(the live cluster, {session_id:?})");
        let (status, payload) = emr
            .call(
                "GetSession",
                json!({ "ClusterId": &live, "SessionId": session_id }),
            )
            .await;
        assert_refused(&case, status, &payload, "InvalidRequestException");
        let expected = format!("Session id '{session_id}' is not valid.");
        let said = message(&case, &payload);
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {payload}"
        );
        assert!(
            !said.contains(live.as_str()),
            "case {case:?}: the session sentence names no cluster, got {said:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Unknown-cluster refusals, per surface
// ---------------------------------------------------------------------------

/// Every operation that refuses an id it resolves no cluster for through the
/// `ValidationException` channel, with a body naming `cluster_id` and the sentence that
/// operation's surface words: the legacy job-flow surface's constant, or the cluster-id
/// sentence that quotes the id.
fn job_flow_surface_cases(cluster_id: &str) -> Vec<(&'static str, &'static str, Value, String)> {
    let cluster_sentence = format!("Cluster id '{cluster_id}' is not valid.");
    vec![
        (
            "a policy the service could hold",
            "PutManagedScalingPolicy",
            json!({
                "ClusterId": cluster_id,
                "ManagedScalingPolicy": {
                    "ComputeLimits": {
                        "UnitType": "Instances",
                        "MinimumCapacityUnits": 1,
                        "MaximumCapacityUnits": 4,
                    },
                },
            }),
            JOB_FLOW_SENTENCE.to_string(),
        ),
        (
            // The SAME constant when the policy itself could not be held either: the
            // cluster is resolved first, so the refusal is about the id.
            "a policy with inverted compute limits",
            "PutManagedScalingPolicy",
            json!({
                "ClusterId": cluster_id,
                "ManagedScalingPolicy": {
                    "ComputeLimits": {
                        "UnitType": "Instances",
                        "MinimumCapacityUnits": 12,
                        "MaximumCapacityUnits": 3,
                    },
                },
            }),
            JOB_FLOW_SENTENCE.to_string(),
        ),
        (
            "a removal",
            "RemoveManagedScalingPolicy",
            json!({ "ClusterId": cluster_id }),
            JOB_FLOW_SENTENCE.to_string(),
        ),
        (
            "a termination",
            "TerminateJobFlows",
            json!({ "JobFlowIds": [cluster_id] }),
            JOB_FLOW_SENTENCE.to_string(),
        ),
        (
            "a termination-protection write",
            "SetTerminationProtection",
            json!({ "JobFlowIds": [cluster_id], "TerminationProtected": true }),
            JOB_FLOW_SENTENCE.to_string(),
        ),
        (
            "a step submission",
            "AddJobFlowSteps",
            json!({
                "JobFlowId": cluster_id,
                "Steps": [{
                    "Name": "a step",
                    "HadoopJarStep": { "Jar": "s3://jars/s.jar" },
                }],
            }),
            JOB_FLOW_SENTENCE.to_string(),
        ),
        (
            // Reporting, not mutating: this one reports the id it was given.
            "a policy read",
            "GetManagedScalingPolicy",
            json!({ "ClusterId": cluster_id }),
            cluster_sentence.clone(),
        ),
        (
            // Takes `JobFlowIds`, yet words the refusal as a cluster id: which surface
            // an operation belongs to is not read off its input member.
            "an unhealthy-node-replacement write",
            "SetUnhealthyNodeReplacement",
            json!({ "JobFlowIds": [cluster_id], "UnhealthyNodeReplacement": true }),
            cluster_sentence,
        ),
    ]
}

/// THE RULE: an operation that resolves no cluster for the id it was given answers the
/// sentence ITS surface words — the legacy job-flow surface's constant, which names no
/// id, or the cluster-id sentence, which quotes the id the request sent — so two
/// operations never share a sentence the service words differently.
#[tokio::test]
async fn each_operation_words_its_own_unknown_cluster_refusal() {
    let emr = Emr::new();
    // A live cluster exists throughout, so every refusal below is a lookup.
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    for unknown in UNKNOWN_CLUSTER_IDS {
        for (label, operation, body, expected) in job_flow_surface_cases(unknown) {
            let case = format!("{operation} ({label}) with cluster id {unknown:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(&case, status, &payload, "ValidationException");
            let said = message(&case, &payload);
            assert_eq!(
                said, expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
            if expected == JOB_FLOW_SENTENCE {
                assert!(
                    unknown.is_empty() || !said.contains(unknown),
                    "case {case:?}: the job-flow sentence names no id, got {said:?}"
                );
            }
        }
    }

    // The live cluster took none of it: it still reads back, and still carries neither a
    // managed scaling policy nor the unhealthy-node replacement the refused calls asked
    // for.
    let cluster = emr.describe_cluster(&live).await;
    assert_eq!(
        cluster["Id"].as_str(),
        Some(live.as_str()),
        "the live cluster must still read back, got {cluster}"
    );
    let policy = emr
        .ok("GetManagedScalingPolicy", json!({ "ClusterId": &live }))
        .await;
    assert!(
        policy["ManagedScalingPolicy"].is_null(),
        "a refused put must attach no policy, got {policy}"
    );
}
