// SPDX-License-Identifier: CC0-1.0

//! WHICH envelope an Amazon EMR refusal of an unusable input value arrives in, and what
//! that envelope carries, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends.
//!
//! THE RULES the tables below assert. Amazon EMR has TWO client-error envelopes, and a
//! refusal arrives in the one belonging to the layer that reached the offending value:
//!
//! 1. A value whose CHARACTERS are outside the class an EMR identifier is drawn from
//!    names no resource the service could ever hold, so it is a bad REQUEST rather than
//!    an unresolvable target: the request-validation envelope, `__type`
//!    `ValidationException` with a lowercase `message` naming the input MEMBER in the
//!    casing the wire uses for it and the value exactly as sent, and no error code at
//!    all. Nothing is looked up, so the refusal does not depend on what the service
//!    holds.
//! 2. A value of that class the service cannot USE — one that resolves no resource, a
//!    continuation token no listing minted, a release label the catalog does not offer,
//!    or a member the request omitted altogether — is the SERVICE's own refusal:
//!    `__type` `InvalidRequestException` carrying `ErrorCode`, the discriminator for
//!    that class, beside `Message`, the sentence for it. An omitted member is stated by
//!    the same sentence a value it cannot use gets, with `null` where the value would
//!    be.
//! 3. Values outside a modelled enum on an operation's own input member are the
//!    service's own refusal too, and its `ErrorCode` names the member whose values were
//!    rejected.
//! 4. The cluster a cluster-scoped request names is resolved BEFORE the request's other
//!    members are checked, so a request that gets both wrong is refused for the cluster.
//!
//! The rows span what the rules generalise over: which character puts a value outside
//! the identifier class and where it sits, which operation and which member the value
//! came in on, which surface that operation reaches a cluster through, and which listing
//! minted the token — plus, for each, the admitted neighbour that shows the refusal is
//! about the value and not about the operation.

use serde_json::{json, Value};

use crate::input_refusals::error_message;
use crate::job_flow_lifecycle::{assert_refused, at, kept_alive_request, Emr};
use crate::refusal_messages::{message, studio_request};
use crate::resource_tags::error_code;

/// The request-validation envelope's `__type`: the layer that reaches a value whose
/// characters are outside an identifier's class.
const REQUEST_VALIDATION: &str = "ValidationException";

/// The service's own client error, which carries `ErrorCode` beside `Message`.
const SERVICE_ERROR: &str = "InvalidRequestException";

/// The discriminator the cluster surface answers a cluster id it cannot use with.
const MALFORMED_CLUSTER_ID: &str = "MalformedClusterId";

/// The discriminator the legacy job-flow surface answers the same class with.
const INVALID_JOB_ID_MALFORMED: &str = "INVALID_JOB_ID_MALFORMED";

/// The discriminator every listing answers a `Marker` it did not mint with.
const MALFORMED_MARKER: &str = "MalformedMarker";

/// The discriminator an operation answers a `ReleaseLabel` it cannot use with.
const INVALID_RELEASE_LABEL: &str = "INVALID_RELEASE_LABEL";

/// A release the service's catalog offers, so a row probing another member of
/// `ListSupportedInstanceTypes` gets past the release check.
const CATALOGUED_RELEASE: &str = "emr-7.5.0";

/// Every value `com.amazonaws.emr#ClusterState` declares
/// (`smithy_query(query="shape", shape="ClusterState")`), so a listing filtered on the
/// whole set reports a cluster whichever state the clock has moved it to.
const CLUSTER_STATES: [&str; 7] = [
    "STARTING",
    "BOOTSTRAPPING",
    "RUNNING",
    "WAITING",
    "TERMINATING",
    "TERMINATED",
    "TERMINATED_WITH_ERRORS",
];

/// Values that are NOT cluster ids at all: each carries at least one character outside
/// the class an EMR-minted id is drawn from (ASCII letters and digits, the hyphen and
/// the underscore), and they differ in WHICH character that is and where it sits.
const MALFORMED_CLUSTER_IDS: &[&str] = &[
    // The recorded mistake: the cluster's ARN passed where its id belongs.
    "arn:aws:elasticmapreduce:us-west-2:000000000000:cluster/j-3SD91U2P1PANK",
    // Another resource's ARN, so the refusal is not about the cluster path.
    "arn:aws:elasticmapreduce:eu-west-1:000000000000:editor/e-ABCDEFGHIJKLM",
    // A console URL, a path and a plain sentence.
    "https://console.aws.amazon.com/emr/home#cluster-details:j-1",
    "clusters/j-ABCDEFGHIJKLM",
    "the cluster called j-ABCDEFGHIJKLM",
    // One offending character on an otherwise well-formed id, at the front, in the
    // middle and at the end.
    " j-ABCDEFGHIJKLM",
    "j-ABCDEF.HIJKLM",
    "j-ABCDEFGHIJKLM!",
    // A character outside ASCII entirely.
    "j-ÅBCDEFGHIJKLM",
];

/// Cluster ids of that class which the service nevertheless holds no cluster under: the
/// shape a real id has, a shorter and a longer body, a prefix with nothing after it, the
/// empty string (`ClusterId` is `len 0..256`, so the boundary passes it) and a value of
/// another form built only from characters the class admits.
const UNRESOLVABLE_CLUSTER_IDS: &[&str] = &[
    "j-ZZZZZZZZZZZZZ",
    "j-1",
    "j-000000000000000000000000",
    "j-",
    "",
    "not_a_cluster_id",
];

/// One request per operation that resolves a `ClusterId` through the cluster surface,
/// naming `cluster_id`. `None` omits the member altogether.
///
/// Rows span the family rather than only the operations the mistake was recorded on: the
/// cluster read, the three listings, the step read, the step cancellation, the cluster
/// modification and the session read.
fn cluster_id_requests(cluster_id: Option<&str>) -> Vec<(&'static str, Value)> {
    let with_id = |mut body: Value| {
        if let Some(cluster_id) = cluster_id {
            body["ClusterId"] = json!(cluster_id);
        }
        body
    };
    vec![
        ("DescribeCluster", with_id(json!({}))),
        ("ListSteps", with_id(json!({}))),
        ("ListInstanceGroups", with_id(json!({}))),
        ("ListInstanceFleets", with_id(json!({}))),
        (
            "DescribeStep",
            with_id(json!({ "StepId": "s-ZZZZZZZZZZZZZ" })),
        ),
        (
            "CancelSteps",
            with_id(json!({ "StepIds": ["s-ZZZZZZZZZZZZZ"] })),
        ),
        (
            "ModifyCluster",
            with_id(json!({ "StepConcurrencyLevel": 2 })),
        ),
        (
            "GetSession",
            with_id(json!({ "SessionId": "s-ABCDEFGHIJKLM" })),
        ),
    ]
}

/// One request per operation that resolves a `ClusterId` but models NO client error, so
/// both its refusals travel the request-validation envelope and only the SENTENCE tells
/// them apart: the policy family. Each names `cluster_id`.
fn no_client_error_requests(cluster_id: &str) -> Vec<(&'static str, Value)> {
    let group = "ig-ABCDEFGHIJKLM";
    vec![
        (
            "GetManagedScalingPolicy",
            json!({ "ClusterId": cluster_id }),
        ),
        (
            "PutAutoScalingPolicy",
            json!({
                "ClusterId": cluster_id,
                "InstanceGroupId": group,
                "AutoScalingPolicy": {
                    "Constraints": { "MinCapacity": 1, "MaxCapacity": 2 },
                    "Rules": [],
                },
            }),
        ),
        (
            "RemoveAutoScalingPolicy",
            json!({ "ClusterId": cluster_id, "InstanceGroupId": group }),
        ),
        (
            "ModifyInstanceGroups",
            json!({
                "ClusterId": cluster_id,
                "InstanceGroups": [{ "InstanceGroupId": group, "InstanceCount": 2 }],
            }),
        ),
    ]
}

/// One request per operation that resolves a `ClusterId` through a surface that words an
/// unusable id its OWN way — the legacy job-flow surface, and the session-credentials
/// read that names no id — so the member-named shape refusal is shown to be the
/// request-validation layer's whatever the operation does with a well-formed id.
fn other_surface_requests(cluster_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "AddInstanceFleet",
            json!({
                "ClusterId": cluster_id,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
        ),
        (
            "GetClusterSessionCredentials",
            json!({
                "ClusterId": cluster_id,
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime",
            }),
        ),
    ]
}

/// RULE 1: a `ClusterId` whose characters are outside the identifier class is refused by
/// the request-validation layer — naming the member and the value, with no error code —
/// on every operation that resolves one, and while a cluster EXISTS, so the refusal is
/// not a lookup.
#[tokio::test]
async fn a_cluster_id_outside_the_identifier_class_is_refused_on_its_shape() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    for malformed in MALFORMED_CLUSTER_IDS {
        let requests = cluster_id_requests(Some(malformed))
            .into_iter()
            .chain(no_client_error_requests(malformed))
            .chain(other_surface_requests(malformed));
        for (operation, body) in requests {
            let case = format!("{operation} with cluster id {malformed:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(&case, status, &payload, REQUEST_VALIDATION);
            assert_eq!(
                message(&case, &payload),
                format!("clusterId '{malformed}' is not valid."),
                "case {case:?}: expected the member-named shape refusal, got {payload}"
            );
            assert_eq!(
                error_code(&payload),
                None,
                "case {case:?}: the request-validation envelope carries no ErrorCode, got \
                 {:?} in {payload}",
                error_code(&payload)
            );
        }
    }

    // The same operations answer the OTHER sentence for an id of that class they resolve
    // no cluster for, so the shape refusal is not the only thing either family says.
    for (operation, body) in no_client_error_requests(UNRESOLVABLE_CLUSTER_IDS[0]) {
        let case = format!("{operation} with an unresolvable cluster id");
        let (status, payload) = emr.call(operation, body).await;
        assert_refused(&case, status, &payload, REQUEST_VALIDATION);
        assert_eq!(
            message(&case, &payload),
            format!("Cluster id '{}' is not valid.", UNRESOLVABLE_CLUSTER_IDS[0]),
            "case {case:?}: expected the cluster-id sentence, got {payload}"
        );
    }

    // The live cluster is untouched and still resolves, so none of the refusals above
    // was a refusal of the operation itself.
    let cluster = emr.describe_cluster(&live).await;
    assert_eq!(
        cluster["Id"].as_str(),
        Some(live.as_str()),
        "the live cluster must still read back, got {cluster}"
    );
}

/// RULE 2 for the same member, which is what gives rule 1 its meaning: a `ClusterId`
/// the identifier class ADMITS but the service holds no cluster under is the service's
/// own refusal, carrying the cluster surface's discriminator beside its sentence.
#[tokio::test]
async fn a_cluster_id_the_class_admits_is_refused_by_the_service_instead() {
    let emr = Emr::new();
    emr.run_job_flow(kept_alive_request("live")).await;

    for unresolvable in UNRESOLVABLE_CLUSTER_IDS {
        for (operation, body) in cluster_id_requests(Some(unresolvable)) {
            let case = format!("{operation} with cluster id {unresolvable:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(&case, status, &payload, SERVICE_ERROR);
            assert_eq!(
                error_code(&payload).as_deref(),
                Some(MALFORMED_CLUSTER_ID),
                "case {case:?}: expected ErrorCode {MALFORMED_CLUSTER_ID}, got {:?} in \
                 {payload}",
                error_code(&payload)
            );
            assert_eq!(
                message(&case, &payload),
                format!("Cluster id '{unresolvable}' is not valid."),
                "case {case:?}: expected the cluster-id sentence, got {payload}"
            );
        }
    }
}

/// RULE 2 for a member the request OMITS: the service answers the same refusal it
/// answers a value it cannot use, with `null` where the value would be, under the
/// discriminator of the surface the operation reaches a cluster through — the cluster
/// surface for the family above, the legacy job-flow surface for the fleet add, and the
/// session-credentials read's own sentence, which names no id.
#[tokio::test]
async fn an_omitted_cluster_id_is_refused_by_its_operations_surface() {
    let emr = Emr::new();
    emr.run_job_flow(kept_alive_request("live")).await;

    let cluster_surface = (
        MALFORMED_CLUSTER_ID,
        "Cluster id 'null' is not valid.".to_string(),
    );
    let mut cases: Vec<(&'static str, Value, (&'static str, String))> =
        cluster_id_requests(None)
            .into_iter()
            .map(|(operation, body)| (operation, body, cluster_surface.clone()))
            .collect();
    // The fleet add reaches a cluster as a JOB FLOW and words every refusal of that
    // class its own way, so an omitted id answers that surface's code and sentence.
    cases.push((
        "AddInstanceFleet",
        json!({
            "InstanceFleet": {
                "InstanceFleetType": "TASK",
                "TargetOnDemandCapacity": 1,
                "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
            },
        }),
        (
            INVALID_JOB_ID_MALFORMED,
            "Specified job flow ID not valid.".to_string(),
        ),
    ));
    // The session-credentials read states the cluster-id sentence with no id in it, so
    // there is nothing for `null` to fill.
    cases.push((
        "GetClusterSessionCredentials",
        json!({ "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime" }),
        (
            MALFORMED_CLUSTER_ID,
            "Cluster id is not valid.".to_string(),
        ),
    ));

    for (operation, body, (code, sentence)) in cases {
        let case = format!("{operation} with no ClusterId at all");
        let (status, payload) = emr.call(operation, body).await;
        assert_refused(&case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(code),
            "case {case:?}: expected ErrorCode {code}, got {:?} in {payload}",
            error_code(&payload)
        );
        assert_eq!(
            message(&case, &payload),
            sentence,
            "case {case:?}: expected {sentence:?}, got {payload}"
        );
    }

    // An operation that declares no client error cannot answer it, so the generic code
    // stands there — and the SENTENCE is still the cluster surface's, with `null` where
    // the id would be. The group and policy members are supplied, so the `ClusterId` is
    // the only member missing.
    let group = "ig-ABCDEFGHIJKLM";
    let no_client_error: Vec<(&str, Value)> = vec![
        ("GetManagedScalingPolicy", json!({})),
        (
            "PutAutoScalingPolicy",
            json!({
                "InstanceGroupId": group,
                "AutoScalingPolicy": {
                    "Constraints": { "MinCapacity": 1, "MaxCapacity": 2 },
                    "Rules": [],
                },
            }),
        ),
        (
            "RemoveAutoScalingPolicy",
            json!({ "InstanceGroupId": group }),
        ),
    ];
    for (operation, body) in no_client_error {
        let case = format!("{operation} with no ClusterId at all");
        let (status, payload) = emr.call(operation, body).await;
        assert_refused(&case, status, &payload, REQUEST_VALIDATION);
        assert_eq!(
            message(&case, &payload),
            "Cluster id 'null' is not valid.",
            "case {case:?}: expected the cluster-id sentence with no id, got {payload}"
        );
        assert_eq!(
            error_code(&payload),
            None,
            "case {case:?}: an operation that declares no client error carries no \
             ErrorCode, got {:?} in {payload}",
            error_code(&payload)
        );
    }
}

/// RULE 2 for the paging token: every `Marker`-paged listing refuses a marker it did not
/// mint with the continuation-token discriminator beside the marker sentence. Rows span
/// the listings of this service — cluster-scoped, account-scoped and catalog — and the
/// marker forms a client sends: one shaped like the ids that listing pages on, one that
/// is no id at all, and the empty string.
#[tokio::test]
async fn every_listing_refuses_a_marker_it_did_not_mint_with_the_marker_code() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("paged")).await;
    emr.ok("CreateStudio", studio_request("paged")).await;

    let listings: Vec<(&str, Value)> = vec![
        ("ListClusters", json!({})),
        ("ListSteps", json!({ "ClusterId": &cluster })),
        ("ListInstanceGroups", json!({ "ClusterId": &cluster })),
        ("ListInstanceFleets", json!({ "ClusterId": &cluster })),
        ("ListNotebookExecutions", json!({})),
        ("ListStudios", json!({})),
        ("ListSecurityConfigurations", json!({})),
        ("ListStudioSessionMappings", json!({})),
        (
            "ListSupportedInstanceTypes",
            json!({ "ReleaseLabel": CATALOGUED_RELEASE }),
        ),
    ];

    for (operation, template) in listings {
        for marker in ["j-ZZZZZZZZZZZZZ", "not-a-marker", ""] {
            let mut body = template.clone();
            body["Marker"] = json!(marker);
            let case = format!("{operation} with marker {marker:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(&case, status, &payload, SERVICE_ERROR);
            assert_eq!(
                error_code(&payload).as_deref(),
                Some(MALFORMED_MARKER),
                "case {case:?}: expected ErrorCode {MALFORMED_MARKER}, got {:?} in {payload}",
                error_code(&payload)
            );
            assert_eq!(
                message(&case, &payload),
                format!("Marker '{marker}' is not valid."),
                "case {case:?}: expected the marker sentence, got {payload}"
            );
        }
    }

    // A listing with no marker still answers, so the refusals above are about the token.
    let listed = emr.ok("ListClusters", json!({})).await;
    assert_eq!(
        at(&listed, &["Clusters"])
            .as_array()
            .map(|clusters| clusters.len()),
        Some(1),
        "the unpaged listing must still report the launched cluster, got {listed}"
    );
}

/// RULE 2 for the release label: a `ReleaseLabel` the catalog does not offer — and one
/// the request omits — is refused with the release-label discriminator beside the
/// sentence that echoes the label, `null` where it was absent. Rows differ along WHY the
/// label is unusable: not in the catalog, not in the label form at all, differently
/// cased, empty, and absent.
#[tokio::test]
async fn an_unusable_release_label_is_refused_with_the_release_label_code() {
    let emr = Emr::new();

    let absent: Value = json!({});
    let mut cases: Vec<(String, Value, String)> = vec![(
        "no ReleaseLabel at all".to_string(),
        absent,
        "The supplied release label is invalid: null.".to_string(),
    )];
    for label in ["emr-6.99.0", "6.15.0", "EMR-7.5.0", "emr-7.5", ""] {
        cases.push((
            format!("ReleaseLabel {label:?}"),
            json!({ "ReleaseLabel": label }),
            format!("The supplied release label is invalid: {label}."),
        ));
    }

    for (case, body, sentence) in cases {
        let (status, payload) = emr.call("ListSupportedInstanceTypes", body).await;
        assert_refused(&case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(INVALID_RELEASE_LABEL),
            "case {case:?}: expected ErrorCode {INVALID_RELEASE_LABEL}, got {:?} in {payload}",
            error_code(&payload)
        );
        assert_eq!(
            message(&case, &payload),
            sentence,
            "case {case:?}: expected {sentence:?}, got {payload}"
        );
    }

    // A label the catalog DOES offer is served, so the refusals above are about the
    // label rather than about the operation.
    let served = emr
        .ok(
            "ListSupportedInstanceTypes",
            json!({ "ReleaseLabel": CATALOGUED_RELEASE }),
        )
        .await;
    assert!(
        at(&served, &["SupportedInstanceTypes"])
            .as_array()
            .is_some_and(|types| !types.is_empty()),
        "case {CATALOGUED_RELEASE:?}: a catalogued release must be served, got {served}"
    );
}

/// THE ORDER the checks run in: Amazon EMR resolves the cluster a cluster-scoped request
/// names BEFORE it checks the request's other members, so a request that gets both wrong
/// is refused for the CLUSTER. Rows pair each form of unusable cluster id with another
/// member the same request gets wrong, and the last two rows are the other half: with a
/// cluster that EXISTS the other member's refusal is the answer, and a violation of the
/// `ClusterId` member itself is answered for that member.
#[tokio::test]
async fn the_cluster_a_request_names_is_resolved_before_its_other_members() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;
    let arn = MALFORMED_CLUSTER_IDS[0];
    let unresolvable = UNRESOLVABLE_CLUSTER_IDS[0];

    // A cluster id the service cannot use, beside a filter value outside its enum and a
    // member outside its documented range.
    let cluster_first: Vec<(&str, &str, Value, String)> = vec![
        (
            "an unresolvable cluster id beside an undeclared step state",
            "ListSteps",
            json!({ "ClusterId": unresolvable, "StepStates": ["pending"] }),
            format!("Cluster id '{unresolvable}' is not valid."),
        ),
        (
            "an unresolvable cluster id beside a step concurrency level out of range",
            "ModifyCluster",
            json!({ "ClusterId": unresolvable, "StepConcurrencyLevel": 0 }),
            format!("Cluster id '{unresolvable}' is not valid."),
        ),
        (
            "a cluster ARN beside an undeclared step state",
            "ListSteps",
            json!({ "ClusterId": arn, "StepStates": ["pending"] }),
            format!("clusterId '{arn}' is not valid."),
        ),
    ];
    for (case, operation, body, sentence) in cluster_first {
        let (status, payload) = emr.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            message(case, &payload),
            sentence,
            "case {case:?}: the cluster is resolved first, so expected {sentence:?}, got \
             {payload}"
        );
    }

    // With a cluster that resolves, the other member is what the refusal is about.
    let case = "an undeclared step state on a cluster that exists";
    let (status, payload) = emr
        .call(
            "ListSteps",
            json!({ "ClusterId": &live, "StepStates": ["pending"] }),
        )
        .await;
    assert_refused(case, status, &payload, SERVICE_ERROR);
    assert_eq!(
        message(case, &payload),
        "Step states [pending] are not valid.",
        "case {case:?}: expected the step-states sentence, got {payload}"
    );

    // A violation of the `ClusterId` member ITSELF is answered for that member, by the
    // layer that declared the constraint: `ClusterId` is `@length(0..256)`
    // (`smithy_query(query="operation", operation="DescribeCluster")`).
    let case = "a cluster id one character past its maximum length";
    let over_long = "j".repeat(257);
    let (status, payload) = emr
        .call("DescribeCluster", json!({ "ClusterId": &over_long }))
        .await;
    assert_refused(case, status, &payload, REQUEST_VALIDATION);
    assert!(
        message(case, &payload).contains("clusterId"),
        "case {case:?}: the length refusal names the member, got {payload}"
    );
}

/// RULE 3: values outside a modelled enum on an operation's own input member are the
/// service's own refusal, and its `ErrorCode` names the MEMBER whose values were
/// rejected. Rows span the filter members of three operations, so the code is built from
/// the member rather than written for one of them.
#[tokio::test]
async fn rejected_enum_values_carry_the_code_of_their_member() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("filtered")).await;

    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "ListClusters",
            json!({ "ClusterStates": ["waiting"] }),
            "InvalidClusterStates",
            "Cluster states [waiting] are not valid.",
        ),
        (
            "ListClusters",
            json!({ "ClusterStates": ["RUNNING", "nope"] }),
            "InvalidClusterStates",
            "Cluster states [nope] are not valid.",
        ),
        (
            "ListSteps",
            json!({ "ClusterId": &cluster, "StepStates": ["pending"] }),
            "InvalidStepStates",
            "Step states [pending] are not valid.",
        ),
        (
            "ListInstances",
            json!({ "ClusterId": &cluster, "InstanceStates": ["running", "nope"] }),
            "InvalidInstanceStates",
            "Instance states [running, nope] are not valid.",
        ),
    ];

    for (operation, body, code, sentence) in cases {
        let case = format!("{operation} with {body}");
        let (status, payload) = emr.call(operation, body.clone()).await;
        assert_refused(&case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(code),
            "case {case:?}: expected ErrorCode {code}, got {:?} in {payload}",
            error_code(&payload)
        );
        assert_eq!(
            error_message(&payload),
            sentence,
            "case {case:?}: expected {sentence:?}, got {payload}"
        );
    }

    // A declared value on the same member is served, so the refusals above are about
    // the values. Filtering on the member's WHOLE value set reports the launched
    // cluster whichever state the clock has moved it to.
    let listed = emr
        .ok("ListClusters", json!({ "ClusterStates": CLUSTER_STATES }))
        .await;
    assert!(
        at(&listed, &["Clusters"])
            .as_array()
            .is_some_and(|clusters| !clusters.is_empty()),
        "a declared ClusterState must still filter the listing, got {listed}"
    );
}
