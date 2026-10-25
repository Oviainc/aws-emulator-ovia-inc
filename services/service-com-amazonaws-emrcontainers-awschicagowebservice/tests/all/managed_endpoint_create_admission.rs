// SPDX-License-Identifier: CC0-1.0

//! What `CreateManagedEndpoint` admits, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client. The create, describe and list helpers are the ones
//! [`crate::managed_endpoint_listing`] drives, so no suite here has a request path of its
//! own.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A managed endpoint is a gateway INTO a virtual cluster, so the host cluster is
//!    resolved out of state before anything is minted and only a cluster that is still
//!    `RUNNING` can host one: a cluster this store never held, one whose teardown is in
//!    flight and one whose teardown has finished are each refused with the modelled
//!    `ResourceNotFoundException` (HTTP 400) and nothing is persisted.
//! 2. The constraints the model publishes for the create's members are enforced before
//!    the handler runs: a value outside a declared length bound or pattern is refused with
//!    `ValidationException` (HTTP 400) and stores nothing, while a value AT each published
//!    bound is accepted and reads back out of the record the create wrote.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{
    create_endpoint, describe_endpoint, described, endpoint_body, list_endpoints, listed_ids,
    wait_for_the_clock,
};
use crate::virtual_cluster_job_run_loop::{assert_resource_id, field, text, Emr};

/// The `state` a describe reports for a virtual cluster, through the harness's own
/// `DescribeVirtualCluster` call so this suite adds no second request path for it.
async fn cluster_state(emr: &Emr, id: &str, case: &str) -> String {
    let answer = emr.describe_cluster(id).await;
    let cluster = field(answer.ok(case), "/virtualCluster", case).clone();
    text(&cluster, "/state", case)
}

/// The ids a cluster's endpoint listing reports, as a set.
async fn stored_ids(emr: &Emr, cluster: &str, case: &str) -> BTreeSet<String> {
    let answer = list_endpoints(emr, cluster, "").await;
    let body = answer.ok(case).clone();
    listed_ids(&body, case).into_iter().collect()
}

/// One acceptable `CreateManagedEndpoint` body with `member` replaced by `value`, so each
/// row below differs from an accepted request in exactly the member it is about. The
/// insert happens last, so a row about `clientToken` carries its own token.
fn body_with(token: &str, member: &str, value: Value) -> Value {
    let mut body = endpoint_body("constraint-row", token, "JUPYTER_ENTERPRISE_GATEWAY");
    body.as_object_mut()
        .expect("the fixture body is a JSON object")
        .insert(member.to_string(), value);
    body
}

/// A `TagMap` of `count` entries, which is what the published 0..50 bound counts.
fn tags(count: usize) -> Value {
    let entries: serde_json::Map<String, Value> = (0..count)
        .map(|index| (format!("key-{index}"), json!(format!("value-{index}"))))
        .collect();
    Value::Object(entries)
}

/// RULE 1: only a virtual cluster this store holds in the `RUNNING` state can host a
/// managed endpoint.
///
/// The rows span the ways a host cluster can fail to be one: four forms of an id nothing
/// was minted for (`ResourceIdString` is len 1..64, `^[0-9a-z]+$`: smithy_query, operation
/// `CreateManagedEndpoint`, input `virtualClusterId`), a cluster whose teardown is in
/// flight, and a cluster whose teardown has finished.
#[tokio::test]
async fn an_endpoint_is_minted_only_in_a_virtual_cluster_that_can_host_one() {
    let emr = Emr::new();
    let host = emr.cluster("endpoint-admission", "vc-ep-admit").await;
    let winding_down = emr.cluster("endpoint-admission-down", "vc-ep-admit-down").await;

    // The endpoint the winding-down cluster holds from while it was RUNNING: the listing
    // at the end shows whether any refused create added a second one.
    let case = "an endpoint created while the host cluster was running";
    let created = create_endpoint(
        &emr,
        &winding_down,
        endpoint_body("held", "ep-tok-held", "JUPYTER_ENTERPRISE_GATEWAY"),
    )
    .await;
    let held = text(created.ok(case), "/id", case);

    for (label, cluster) in [
        ("the shortest cluster id the model allows", "a".to_string()),
        ("the longest cluster id the model allows", "z".repeat(64)),
        ("an all-digit cluster id", "0123456789".to_string()),
        ("a plausible minted cluster id", "doesnotexist1".to_string()),
    ] {
        create_endpoint(
            &emr,
            &cluster,
            endpoint_body("orphan", "ep-tok-orphan", "JUPYTER_SERVER"),
        )
        .await
        .refused(label, 400, "ResourceNotFoundException");
    }

    // A teardown in flight: the cluster is still stored, and still cannot host a gateway.
    let case = "the teardown of the winding-down cluster";
    emr.delete_cluster(&winding_down).await.ok(case);
    let reported = cluster_state(&emr, &winding_down, case).await;
    assert_eq!(
        reported,
        state::CLUSTER_TERMINATING,
        "case {case:?}: the delete must leave the cluster TERMINATING, got {reported:?}"
    );
    create_endpoint(
        &emr,
        &winding_down,
        endpoint_body("in-flight", "ep-tok-terminating", "JUPYTER_SERVER"),
    )
    .await
    .refused(
        "a cluster whose teardown is in flight",
        400,
        "ResourceNotFoundException",
    );

    wait_for_the_clock();

    let case = "the winding-down cluster once its teardown came due";
    let reported = cluster_state(&emr, &winding_down, case).await;
    assert_eq!(
        reported,
        state::CLUSTER_TERMINATED,
        "case {case:?}: the cluster must be TERMINATED once the clock made it due, got {reported:?}"
    );
    create_endpoint(
        &emr,
        &winding_down,
        endpoint_body("torn-down", "ep-tok-terminated", "JUPYTER_SERVER"),
    )
    .await
    .refused(
        "a cluster whose teardown has finished",
        400,
        "ResourceNotFoundException",
    );

    // Nothing a refused create named was persisted: the winding-down cluster still holds
    // exactly the one endpoint it had, and the cluster that CAN host one still does.
    let case = "the winding-down cluster after the refusals";
    assert_eq!(
        stored_ids(&emr, &winding_down, case).await,
        [held].into_iter().collect::<BTreeSet<String>>(),
        "case {case:?}: a refused create must store no endpoint"
    );

    let case = "a create in the cluster that can host one";
    let created = create_endpoint(
        &emr,
        &host,
        endpoint_body("admitted", "ep-tok-admitted", "JUPYTER_SERVER"),
    )
    .await;
    let id = text(created.ok(case), "/id", case);
    assert_resource_id(case, "the endpoint id", &id);
    let answer = describe_endpoint(&emr, &host, &id).await;
    let endpoint = described(answer.ok(case), case);
    assert_eq!(
        text(&endpoint, "/virtualClusterId", case),
        host,
        "case {case:?}: the stored endpoint names another host cluster: {endpoint}"
    );
    assert_eq!(
        stored_ids(&emr, &host, case).await,
        [id].into_iter().collect::<BTreeSet<String>>(),
        "case {case:?}: the host cluster must hold exactly the endpoint it admitted"
    );
}

/// RULE 2, the accepted half: a value AT a published bound is inside it, so the create is
/// accepted and the value reads back out of the stored record.
///
/// Every bound comes from the model (smithy_query, operation `CreateManagedEndpoint`):
/// `name` len 1..64 pattern `^[\.\-_/#A-Za-z0-9]+$`, `clientToken` len 1..64 pattern
/// `\S`, `releaseLabel` len 1..64 pattern `^[\.\-_/A-Za-z0-9]+$`, `executionRoleArn` len
/// 20..2048, `certificateArn` len 44..2048 and `tags` len 0..50.
#[tokio::test]
async fn a_create_at_each_published_bound_is_accepted_and_reads_back() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-bounds", "vc-ep-bounds").await;

    // The shortest value the `IAMRoleArn` pattern can produce is also exactly its length
    // minimum of 20, and the shortest the `ACMCertArn` pattern can produce is shorter than
    // its minimum of 44, so the row below sits at the minimum rather than under it.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("a name at the length minimum", "name", json!("n")),
        (
            "a name at the length maximum",
            "name",
            json!("n".repeat(64)),
        ),
        (
            "a name using every character class the pattern allows",
            "name",
            json!("A.b-c_d/e#9"),
        ),
        (
            "a clientToken at the length minimum",
            "clientToken",
            json!("t"),
        ),
        (
            "a clientToken at the length maximum",
            "clientToken",
            json!("t".repeat(64)),
        ),
        (
            "a releaseLabel at the length minimum",
            "releaseLabel",
            json!("r"),
        ),
        (
            "a releaseLabel at the length maximum",
            "releaseLabel",
            json!("r".repeat(64)),
        ),
        (
            "an executionRoleArn at the length minimum",
            "executionRoleArn",
            json!("arn:aws:iam:::role/a"),
        ),
        (
            "an executionRoleArn carrying a path",
            "executionRoleArn",
            json!("arn:aws:iam::000000000000:role/studio/EMRStudioRole"),
        ),
        (
            "a certificateArn at the length minimum",
            "certificateArn",
            json!("arn:aws:acm:a:000000000000:certificate/abcde"),
        ),
        (
            "the gateway endpoint type",
            "type",
            json!("JUPYTER_ENTERPRISE_GATEWAY"),
        ),
        ("the server endpoint type", "type", json!("JUPYTER_SERVER")),
        ("no tags at all, the length minimum", "tags", tags(0)),
        ("tags at the length maximum", "tags", tags(50)),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (index, (label, member, value)) in rows.iter().enumerate() {
        let body = body_with(&format!("ep-tok-bound-{index}"), member, value.clone());
        let created = create_endpoint(&emr, &cluster, body).await;
        let id = text(created.ok(label), "/id", label);
        assert_resource_id(label, "the endpoint id", &id);
        assert!(
            minted.insert(id.clone()),
            "case {label:?}: the create answered an id another row already minted: {id}"
        );

        let answer = describe_endpoint(&emr, &cluster, &id).await;
        let endpoint = described(answer.ok(label), label);
        // `clientToken` is not a member of the `Endpoint` shape, so what a row about it
        // proves is that the value was accepted and the endpoint stored under it.
        if *member != "clientToken" {
            assert_eq!(
                field(&endpoint, &format!("/{member}"), label),
                value,
                "case {label:?}: {member} is not the value the create persisted: {endpoint}"
            );
        }
        assert_eq!(
            text(&endpoint, "/id", label),
            id,
            "case {label:?}: the describe answered another endpoint: {endpoint}"
        );
    }

    let case = "the cluster after every accepted row";
    assert_eq!(
        stored_ids(&emr, &cluster, case).await,
        minted,
        "case {case:?}: the cluster must hold exactly one endpoint per accepted row"
    );
}

/// RULE 2, the refused half: a value one step outside a published bound, or outside a
/// member's pattern, never reaches the handler — it is refused with `ValidationException`
/// (HTTP 400) and stores nothing.
///
/// The rows walk each constrained member of the create (`name`, `clientToken`,
/// `releaseLabel`, `executionRoleArn`, `certificateArn`, `type`, `tags` and a tag's own
/// value), one character past a length bound and outside the pattern.
#[tokio::test]
async fn a_create_outside_a_published_bound_is_refused_and_stores_nothing() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-refused", "vc-ep-refused").await;

    // The endpoint the cluster holds before any refusal, so the listing at the end shows
    // whether a refused create stored anything.
    let case = "the endpoint stored before the refusals";
    let created = create_endpoint(
        &emr,
        &cluster,
        endpoint_body("stored", "ep-tok-stored", "JUPYTER_SERVER"),
    )
    .await;
    let stored = text(created.ok(case), "/id", case);

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a name one character past the length maximum",
            "name",
            json!("n".repeat(65)),
        ),
        ("an empty name", "name", json!("")),
        ("a name carrying a space", "name", json!("endpoint name")),
        (
            "a name carrying a character the pattern excludes",
            "name",
            json!("endpoint!"),
        ),
        (
            "a clientToken one character past the length maximum",
            "clientToken",
            json!("t".repeat(65)),
        ),
        ("an empty clientToken", "clientToken", json!("")),
        (
            "a clientToken of whitespace alone",
            "clientToken",
            json!("   "),
        ),
        (
            "a releaseLabel one character past the length maximum",
            "releaseLabel",
            json!("r".repeat(65)),
        ),
        (
            "a releaseLabel carrying a space",
            "releaseLabel",
            json!("emr 6.10.0-latest"),
        ),
        ("an empty releaseLabel", "releaseLabel", json!("")),
        (
            "an executionRoleArn that names no role",
            "executionRoleArn",
            json!("arn:aws:iam::000000000000:user/not-a-role"),
        ),
        (
            "an executionRoleArn whose account is not twelve digits",
            "executionRoleArn",
            json!("arn:aws:iam::0:role/EMRStudioRole"),
        ),
        (
            "an executionRoleArn below the length minimum",
            "executionRoleArn",
            json!("arn:aws:iam:::role"),
        ),
        (
            "a certificateArn one character below the length minimum",
            "certificateArn",
            json!("arn:aws:acm:a:000000000000:certificate/abcd"),
        ),
        (
            "a certificateArn that names no certificate",
            "certificateArn",
            json!("arn:aws:acm:us-west-2:000000000000:key/abcdefghijklmnopqrst"),
        ),
        ("an empty type", "type", json!("")),
        ("a type of whitespace alone", "type", json!(" ")),
        (
            "a type one character past the length maximum",
            "type",
            json!("t".repeat(65)),
        ),
        ("tags one past the length maximum", "tags", tags(51)),
        (
            "a tag value one character past its length maximum",
            "tags",
            json!({"key": "v".repeat(257)}),
        ),
    ];

    for (index, (label, member, value)) in rows.iter().enumerate() {
        let body = body_with(&format!("ep-tok-refused-{index}"), member, value.clone());
        create_endpoint(&emr, &cluster, body)
            .await
            .refused(label, 400, "ValidationException");
    }

    let case = "the cluster after every refusal";
    assert_eq!(
        stored_ids(&emr, &cluster, case).await,
        [stored.clone()].into_iter().collect::<BTreeSet<String>>(),
        "case {case:?}: a refused create must store no endpoint"
    );
    // The refusals left the endpoint the cluster did hold exactly as it was.
    let answer = describe_endpoint(&emr, &cluster, &stored).await;
    let endpoint = described(answer.ok(case), case);
    assert_eq!(
        text(&endpoint, "/id", case),
        stored,
        "case {case:?}: a refused create must leave the stored endpoint readable: {endpoint}"
    );
}
