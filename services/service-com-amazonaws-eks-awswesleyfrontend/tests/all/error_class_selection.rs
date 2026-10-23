// SPDX-License-Identifier: CC0-1.0

//! WHICH modelled error a refused request earns, and in which ORDER the handlers
//! decide it — the two rules an SDK's error branching reads:
//!
//! 1. A child-resource create resolves its PARENT first: a `clusterName` that
//!    resolves to no cluster is absence, so it is the service's 404
//!    `ResourceNotFoundException` naming that cluster, whatever else the request
//!    carries — not a 400 about the body. (The payload's shape across every
//!    operation that can raise one is `tests/all/not_found_payload.rs`; this suite
//!    pins the CLASS and the precedence.)
//! 2. An operation whose input binds a caller-chosen `resourceArn` PARSES it before
//!    it looks anything up: a value that is not a resource ARN of this service's own
//!    ARN namespace (`aws.api#service$arnNamespace` is `eks`, `smithy_query
//!    query="service_info"`) addresses nothing this API can name, so it is a 400
//!    `BadRequestException` ("thrown if the request contains a semantic error");
//!    only a WELL-FORMED ARN of that namespace which no stored resource owns is the
//!    404 `NotFoundException`.
//!
//! Each table spans the class rather than one recorded request: rows differ along
//! HOW the value fails the ARN grammar, along the vendor and partition fields, along
//! which body rule a create breaks, and along the operation (a POST carrying a body
//! and a path-only GET). Every assertion names its case and prints what came back.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`; the cluster and node group request builders are the ones
//! `cluster_lifecycle` and `nodegroup_lifecycle` already own.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, INVALID_RESOURCE_ARN_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, delete, minimal_request as cluster_request, REGION};
use crate::nodegroup_lifecycle::{create_cluster, create_nodegroup_in, minimal_request};
use crate::wire::{call, emulator, encode_label, Answer};

// ---------------------------------------------------------------------------
// RULE 1: `CreateNodegroup` resolves `clusterName` before it reports anything about
// the rest of the request, and an unresolvable cluster is the service's 404
// `ResourceNotFoundException` naming only that cluster.
// ---------------------------------------------------------------------------

/// The node role every `CreateNodegroup` row below passes to its nodes comes from
/// `nodegroup_lifecycle::minimal_request`, the smallest request that operation
/// accepts; a row only adds or overrides the members its own case is about.
fn nodegroup_body(name: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut body = minimal_request(name);
    let (Some(object), Some(extra)) = (body.as_object_mut(), extra.as_object()) else {
        return body;
    };
    for (member, value) in extra {
        object.insert(member.clone(), value.clone());
    }
    body
}

#[tokio::test]
async fn a_nodegroup_create_on_an_unresolvable_cluster_is_resource_not_found() {
    let router = emulator();
    // One real cluster, so every row below can be run twice: against a cluster that
    // exists (where the BODY is what is reported) and against one that does not
    // (where the CLUSTER is).
    let cluster_version = create_cluster(&router, "class-cluster").await;

    // (case, body, the status the row earns against the cluster that EXISTS, the
    // error code it earns there) — the rows differ along which documented body rule
    // the request breaks, including one row that breaks none.
    let rows: Vec<(&str, serde_json::Value, u16, Option<&str>)> = vec![
        (
            "a request that breaks no rule at all",
            nodegroup_body("class-valid", serde_json::Value::Null),
            200,
            None,
        ),
        (
            "an empty subnet list",
            nodegroup_body("class-subnets", serde_json::json!({ "subnets": [] })),
            400,
            Some("InvalidParameterException"),
        ),
        (
            "a diskSize below the one-GiB floor",
            nodegroup_body("class-disk", serde_json::json!({ "diskSize": 0 })),
            400,
            Some("InvalidParameterException"),
        ),
        (
            "a scaling triple that is not ordered",
            nodegroup_body(
                "class-scaling",
                serde_json::json!({
                    "scalingConfig": { "minSize": 3, "maxSize": 2, "desiredSize": 1 },
                }),
            ),
            400,
            Some("InvalidParameterException"),
        ),
        (
            "a launch template beside diskSize",
            nodegroup_body(
                "class-template",
                serde_json::json!({
                    "launchTemplate": { "name": "lt-probe" },
                    "diskSize": 40,
                }),
            ),
            400,
            Some("InvalidParameterException"),
        ),
        (
            "a Kubernetes version the cluster does not run",
            nodegroup_body(
                "class-version",
                serde_json::json!({ "version": format!("{cluster_version}9") }),
            ),
            400,
            Some("InvalidParameterException"),
        ),
    ];

    // The cluster names that resolve to nothing: never created, differing only in
    // case from the real one, and the real one in a region that does not hold it.
    let unresolvable: [(&str, &str, &str); 3] = [
        ("a cluster name nothing holds", REGION, "class-absent"),
        ("a name differing only in case", REGION, "CLASS-CLUSTER"),
        (
            "the real cluster name in another region",
            "eu-west-1",
            "class-cluster",
        ),
    ];

    for (body_case, body, status, code) in &rows {
        // CONTROL: against the cluster that exists, the row is answered on its own
        // merits — which is what makes the 404 below about precedence.
        let on_real = create_nodegroup_in(&router, REGION, "class-cluster", body).await;
        assert_eq!(
            on_real.status, *status,
            "case {body_case:?}: expected HTTP {status} against the real cluster, got {on_real:?}"
        );
        assert_eq!(
            on_real.error_type.as_deref(),
            *code,
            "case {body_case:?}: expected {code:?} against the real cluster, got {on_real:?}"
        );

        for (cluster_case, region, cluster) in &unresolvable {
            let case = format!("{body_case} / {cluster_case}");
            let answer = create_nodegroup_in(&router, region, cluster, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.body["clusterName"].as_str(),
                Some(*cluster),
                "case {case:?}: the payload names the cluster it could not resolve, got {:?}",
                answer.body["clusterName"]
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(cluster_not_found_message(cluster).as_str()),
                "case {case:?}: expected the message that names the cluster, got {:?}",
                answer.body["message"]
            );
            assert_eq!(
                answer.body["nodegroupName"],
                serde_json::Value::Null,
                "case {case:?}: the payload must not echo the requested nodegroupName, got {:?}",
                answer.body["nodegroupName"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a caller-supplied `resourceArn` is PARSED before any lookup — not a
// resource ARN of this service means 400 `BadRequestException`; a well-formed ARN of
// this service that nothing owns means 404 `NotFoundException`.
// ---------------------------------------------------------------------------

/// Values that are NOT a resource ARN of this service.
///
/// Rows differ along how each one fails the ARN grammar
/// (`services/common/src/arn.rs`: six colon-separated fields, the first literally
/// `arn`, with a non-empty partition, vendor and resource) and along the vendor
/// field, whose value must be this service's own ARN namespace.
const MALFORMED_ARNS: &[(&str, &str)] = &[
    ("not an ARN at all", "not-a-valid-eks-resource-arn"),
    ("a bare cluster name", "my-cluster"),
    ("the arn prefix alone", "arn"),
    ("a single separator", ":"),
    (
        "an ARN that stops before its resource",
        "arn:aws:eks:us-west-2:000000000000",
    ),
    (
        "an ARN with an empty resource",
        "arn:aws:eks:us-west-2:000000000000:",
    ),
    (
        "an ARN with an empty vendor",
        "arn:aws::us-west-2:000000000000:cluster/probe",
    ),
    (
        "an ARN with an empty partition",
        "arn::eks:us-west-2:000000000000:cluster/probe",
    ),
    (
        "a first field that is not arn",
        "urn:aws:eks:us-west-2:000000000000:cluster/probe",
    ),
    ("another vendor's ARN", "arn:aws:s3:::some-bucket"),
    (
        "a vendor this one only prefixes",
        "arn:aws:eksauth:us-west-2:000000000000:cluster/probe",
    ),
    (
        "this vendor in upper case",
        "arn:aws:EKS:us-west-2:000000000000:cluster/probe",
    ),
    (
        "an endpoint URL rather than an ARN",
        "https://eks.us-west-2.amazonaws.com/clusters/probe",
    ),
];

async fn tag(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        &serde_json::json!({ "tags": { "k": "v" } }).to_string(),
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        "",
    )
    .await
}

#[tokio::test]
async fn a_resource_arn_that_is_not_one_is_a_bad_request_on_both_tag_operations() {
    let router = emulator();

    for (case, arn) in MALFORMED_ARNS {
        for (operation, answer) in [
            ("TagResource", tag(&router, arn).await),
            ("ListTagsForResource", list_tags(&router, arn).await),
        ] {
            let case = format!("{case} / {operation}");
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {answer:?}"
            );
            // The refusal is about the SYNTAX of the input, so it is one constant
            // sentence: it echoes nothing the request carried.
            assert_eq!(
                answer.body["message"].as_str(),
                Some(INVALID_RESOURCE_ARN_MESSAGE),
                "case {case:?}: expected the invalid-ARN sentence, got {:?}",
                answer.body["message"]
            );
        }
    }
}

#[tokio::test]
async fn a_well_formed_eks_arn_is_resolved_and_only_then_reported_missing() {
    let router = emulator();

    // A real cluster with a tag, so the 200 arm reads a STORED map back, and a
    // cluster that is created and deleted, so one row carries the ARN of a resource
    // that is well formed and genuinely gone.
    let mut request = cluster_request("class-tagged");
    request["tags"] = serde_json::json!({ "owner": "platform" });
    let created = create(&router, &request).await;
    assert_eq!(created.status, 200, "setup create: got {created:?}");
    let real_arn = created.body["cluster"]["arn"]
        .as_str()
        .expect("CreateCluster reports the cluster ARN")
        .to_owned();

    let gone = create(&router, &cluster_request("class-gone")).await;
    assert_eq!(gone.status, 200, "setup create: got {gone:?}");
    let gone_arn = gone.body["cluster"]["arn"]
        .as_str()
        .expect("CreateCluster reports the cluster ARN")
        .to_owned();
    let removed = delete(&router, "class-gone").await;
    assert_eq!(removed.status, 200, "setup delete: got {removed:?}");

    // (case, ARN, the status both tag operations earn) — every row is a WELL-FORMED
    // resource ARN of this service, so none of them is a bad request: the answer is
    // decided by whether the store owns the resource.
    let rows: Vec<(&str, String, u16)> = vec![
        ("the ARN of a cluster the store owns", real_arn.clone(), 200),
        ("the ARN of a deleted cluster", gone_arn, 404),
        (
            "a cluster ARN for a name never created",
            format!("arn:aws:eks:{REGION}:000000000000:cluster/never"),
            404,
        ),
        (
            "a resource kind this slice does not hold",
            format!("arn:aws:eks:{REGION}:000000000000:nodegroup/c/n/uuid"),
            404,
        ),
        (
            "a cluster ARN in another partition",
            "arn:aws-cn:eks:cn-north-1:000000000000:cluster/never".to_string(),
            404,
        ),
        (
            "a partition-global cluster ARN, region and account fields empty",
            "arn:aws:eks:::cluster/never".to_string(),
            404,
        ),
    ];

    for (case, arn, status) in &rows {
        for (operation, answer) in [
            ("TagResource", tag(&router, arn).await),
            ("ListTagsForResource", list_tags(&router, arn).await),
        ] {
            let case = format!("{case} / {operation}");
            assert_eq!(
                answer.status, *status,
                "case {case:?}: expected HTTP {status}, got {answer:?}"
            );
            let expected_code = if *status == 404 {
                Some("NotFoundException")
            } else {
                None
            };
            assert_eq!(
                answer.error_type.as_deref(),
                expected_code,
                "case {case:?}: expected {expected_code:?}, got {answer:?}"
            );
        }
    }

    // The 400 arm above refused on the input, so it wrote nothing: the real
    // cluster's map still holds what was stored for it, plus the key the 200 arm
    // merged in — and nothing from the refused requests.
    let listed = list_tags(&router, &real_arn).await;
    assert_eq!(listed.status, 200, "the real ARN reads back, got {listed:?}");
    assert_eq!(
        listed.body["tags"],
        serde_json::json!({ "owner": "platform", "k": "v" }),
        "expected the stored map, got {:?}",
        listed.body["tags"]
    );
}
