// SPDX-License-Identifier: CC0-1.0

//! `DescribeManagedEndpoint`, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client. The create and list helpers are the ones
//! [`crate::managed_endpoint_listing`] drives, so the two suites cannot drift apart.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A describe resolves its endpoint by the PAIR (`virtualClusterId`, `id`) and answers
//!    with the record the create persisted under that pair — the same copy the listing
//!    reports. A pair the store does not hold is refused with the modelled
//!    `ResourceNotFoundException` (HTTP 400): a stored id under another cluster is NOT
//!    that cluster's endpoint.
//! 2. An `id` outside the constraints the model puts on it is refused with the modelled
//!    `ValidationException` (HTTP 400) before the handler runs.
//! 3. A describe is a READ: reading an endpoint any number of times changes nothing a
//!    later caller sees, and never brings the endpoint's `CREATING` -> `ACTIVE` step due
//!    early. The step answers to the clock alone, and when it is materialised it moves
//!    `state` and nothing else in the record.
//! 4. A refused describe changes nothing: every stored endpoint still describes
//!    afterwards.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{
    create_endpoint, described, describe_endpoint, endpoint_body, list_endpoints, listed,
    stored_bodies, wait_for_the_clock,
};
use crate::virtual_cluster_job_run_loop::{field, text, Emr};

/// The record the listing of `cluster` reports for `id`, as the describe's answer should
/// be: both readers answer from one stored copy of the endpoint.
async fn listed_record(emr: &Emr, cluster: &str, id: &str, case: &str) -> Value {
    let answer = list_endpoints(emr, cluster, "").await;
    let body = answer.ok(case).clone();
    listed(&body, case)
        .into_iter()
        .find(|candidate| text(candidate, "/id", case) == id)
        .unwrap_or_else(|| panic!("case {case:?}: the listing does not report {id}: {body}"))
}

/// The record without its `state`, so two reads taken in different phases of the
/// `CREATING` -> `ACTIVE` step can be compared on everything else.
fn without_state(endpoint: &Value, case: &str) -> Value {
    let mut copy = endpoint.clone();
    copy.as_object_mut()
        .unwrap_or_else(|| panic!("case {case:?}: the endpoint is not an object: {endpoint}"))
        .remove("state");
    copy
}

/// RULES 1 and 4: every stored (cluster, id) pair describes to the record stored under
/// that pair, and a pair the store does not hold is refused — including a stored endpoint
/// id offered under the wrong virtual cluster.
///
/// The rows span the forms an endpoint takes (which optional members it carries, its
/// `type`, how deep its `configurationOverrides` goes) across SEVERAL clusters, including
/// a cluster holding no endpoint at all, and the forms a `ResourceIdString` takes for an
/// id nothing was minted for.
#[tokio::test]
async fn describing_a_managed_endpoint_resolves_the_cluster_and_id_pair() {
    let emr = Emr::new();
    let first = emr.cluster("describe-pair-a", "vc-desc-a").await;
    let second = emr.cluster("describe-pair-b", "vc-desc-b").await;
    let empty = emr.cluster("describe-pair-c", "vc-desc-c").await;

    // The same bodies are stored in two clusters, so the only thing telling the two
    // endpoints apart is the pair they were stored under.
    let mut stored: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();
    for cluster in [&first, &second] {
        for (label, body) in stored_bodies() {
            let case = format!("{label} in {cluster}");
            let created = create_endpoint(&emr, cluster, body.clone()).await;
            let id = text(created.ok(&case), "/id", &case);
            stored
                .entry((*cluster).clone())
                .or_default()
                .push((id, body));
        }
    }

    for (cluster, endpoints) in &stored {
        for (id, body) in endpoints {
            let case = format!("endpoint {id} of cluster {cluster}");
            let answer = describe_endpoint(&emr, cluster, id).await;
            let endpoint = described(answer.ok(&case), &case);

            assert_eq!(
                text(&endpoint, "/id", &case),
                *id,
                "case {case:?}: the describe answered with another endpoint: {endpoint}"
            );
            assert_eq!(
                text(&endpoint, "/virtualClusterId", &case),
                *cluster,
                "case {case:?}: the describe reported another virtual cluster: {endpoint}"
            );
            // The members the request carried come back out of the record, and the ARN
            // names this pair.
            for member in ["name", "type", "releaseLabel", "executionRoleArn"] {
                assert_eq!(
                    field(&endpoint, &format!("/{member}"), &case),
                    &body[member],
                    "case {case:?}: {member} is not what the create persisted: {endpoint}"
                );
            }
            let arn = text(&endpoint, "/arn", &case);
            assert!(
                arn.ends_with(&format!("/virtualclusters/{cluster}/endpoints/{id}")),
                "case {case:?}: arn {arn:?} does not name this cluster-and-id pair"
            );
            // The describe answers the SAME copy the listing reports.
            assert_eq!(
                listed_record(&emr, cluster, id, &case).await,
                endpoint,
                "case {case:?}: the listed record differs from the described one"
            );
        }
    }

    // Pairs the store does not hold. A stored id under the other cluster, and under a
    // cluster holding nothing, is not found rather than that cluster's endpoint; so is a
    // well-formed id nothing was minted for, at either `ResourceIdString` length bound
    // (len 1..64, `^[0-9a-z]+$`: smithy_query, operation `DescribeManagedEndpoint`,
    // input `id`), and so is another resource type's id.
    let in_first: Vec<String> = stored[&first].iter().map(|(id, _)| id.clone()).collect();
    let in_second: Vec<String> = stored[&second].iter().map(|(id, _)| id.clone()).collect();
    let mut unheld: Vec<(String, String, String)> = vec![
        (
            "the shortest id the model allows".to_string(),
            first.clone(),
            "a".to_string(),
        ),
        (
            "the longest id the model allows".to_string(),
            first.clone(),
            "z".repeat(64),
        ),
        (
            "an all-digit id".to_string(),
            second.clone(),
            "0123456789".to_string(),
        ),
        (
            "a plausible minted id".to_string(),
            second.clone(),
            "doesnotexist1".to_string(),
        ),
        (
            "a virtual cluster's own id as an endpoint id".to_string(),
            first.clone(),
            first.clone(),
        ),
        (
            "an endpoint of the first cluster under an unknown cluster".to_string(),
            "nosuchcluster".to_string(),
            in_first[0].clone(),
        ),
    ];
    for (index, id) in in_first.iter().enumerate() {
        unheld.push((
            format!("endpoint {index} of the first cluster under the second"),
            second.clone(),
            id.clone(),
        ));
        unheld.push((
            format!("endpoint {index} of the first cluster under a cluster holding none"),
            empty.clone(),
            id.clone(),
        ));
    }
    for (index, id) in in_second.iter().enumerate() {
        unheld.push((
            format!("endpoint {index} of the second cluster under the first"),
            first.clone(),
            id.clone(),
        ));
    }

    for (label, cluster, id) in &unheld {
        describe_endpoint(&emr, cluster, id)
            .await
            .refused(label, 400, "ResourceNotFoundException");
    }

    // RULE 4: the refusals changed nothing — every stored pair still describes.
    for (cluster, endpoints) in &stored {
        for (id, _) in endpoints {
            let case = format!("endpoint {id} of cluster {cluster} after the refusals");
            let answer = describe_endpoint(&emr, cluster, id).await;
            assert_eq!(
                text(&described(answer.ok(&case), &case), "/id", &case),
                *id,
                "case {case:?}: a refused describe must leave every stored endpoint readable"
            );
        }
    }
}

/// RULE 2: an `id` the model's constraints refuse never reaches the handler, so it is
/// refused with the modelled `ValidationException` rather than reported as not found.
///
/// The rows walk `ResourceIdString`'s published bounds and its pattern: one character
/// past the length maximum, and the character classes the pattern excludes.
#[tokio::test]
async fn describing_an_endpoint_id_the_model_refuses_is_a_validation_error() {
    let emr = Emr::new();
    let cluster = emr.cluster("describe-validation", "vc-desc-validation").await;
    let created = create_endpoint(
        &emr,
        &cluster,
        endpoint_body("validation", "ep-tok-validation", "JUPYTER_SERVER"),
    )
    .await;
    let stored = text(created.ok("the stored endpoint"), "/id", "the stored endpoint");

    // `ResourceIdString` is `@length 1..64` with pattern `^[0-9a-z]+$` (smithy_query:
    // operation `DescribeManagedEndpoint`, input `id`).
    for (label, id) in [
        ("one character past the length maximum", "z".repeat(65)),
        ("an id twice the length maximum", "0".repeat(128)),
        ("an upper-case id", stored.to_uppercase()),
        ("an id carrying a hyphen", format!("{stored}-1")),
        ("an id carrying an underscore", format!("{stored}_1")),
        ("an id carrying a dot", format!("{stored}.1")),
    ] {
        describe_endpoint(&emr, &cluster, &id)
            .await
            .refused(label, 400, "ValidationException");
    }

    // The refusals left the stored endpoint exactly as it was.
    let case = "the stored endpoint after the refusals";
    let answer = describe_endpoint(&emr, &cluster, &stored).await;
    assert_eq!(
        text(&described(answer.ok(case), case), "/id", case),
        stored,
        "case {case:?}: a refused describe must leave the stored endpoint readable"
    );
}

/// RULE 3: a describe is a read. Reading an endpoint does not move its state, however
/// many times it is read: the `CREATING` -> `ACTIVE` step comes due on the clock alone,
/// and materialising it moves `state` and nothing else.
///
/// The rows differ in HOW MANY reads happen inside the transition span, which is what a
/// read that advanced the state on being polled would be caught by.
#[tokio::test]
async fn describing_a_managed_endpoint_does_not_move_its_state() {
    let emr = Emr::new();
    let cluster = emr.cluster("describe-reads", "vc-desc-reads").await;

    for reads in [1_usize, 3, 10] {
        let case = format!("{reads} read(s) before the transition is due");
        let created = create_endpoint(
            &emr,
            &cluster,
            endpoint_body(
                &format!("reads-{reads}"),
                &format!("ep-tok-reads-{reads}"),
                "JUPYTER_ENTERPRISE_GATEWAY",
            ),
        )
        .await;
        let id = text(created.ok(&case), "/id", &case);

        let mut first: Option<Value> = None;
        for read in 0..reads {
            let answer = describe_endpoint(&emr, &cluster, &id).await;
            let endpoint = described(answer.ok(&case), &case);
            let state_now = text(&endpoint, "/state", &case);
            assert_eq!(
                state_now,
                state::ENDPOINT_CREATING,
                "case {case:?}: read {read} reported {state_now:?} before the transition was due"
            );
            match &first {
                None => first = Some(endpoint),
                Some(before) => assert_eq!(
                    &endpoint, before,
                    "case {case:?}: read {read} answered a different record than the first read"
                ),
            }
        }
        let before = first.expect("every row reads at least once");

        wait_for_the_clock();

        let answer = describe_endpoint(&emr, &cluster, &id).await;
        let after = described(answer.ok(&case), &case);
        let state_then = text(&after, "/state", &case);
        assert_eq!(
            state_then,
            state::ENDPOINT_ACTIVE,
            "case {case:?}: the endpoint reported {state_then:?} once the transition was due"
        );
        assert_eq!(
            without_state(&after, &case),
            without_state(&before, &case),
            "case {case:?}: the transition must move state and nothing else"
        );

        // Reading the settled endpoint again answers the same record: the read that
        // materialised the step owed nothing further.
        let again = describe_endpoint(&emr, &cluster, &id).await;
        assert_eq!(
            described(again.ok(&case), &case),
            after,
            "case {case:?}: describing a settled endpoint twice answered two records"
        );
        assert_eq!(
            listed_record(&emr, &cluster, &id, &case).await,
            after,
            "case {case:?}: the listing disagrees with the describe after the transition"
        );
    }

    // The endpoints every row created are all still there, each in the state its own
    // record holds: no read consumed one.
    let case = "the cluster after every read";
    let answer = list_endpoints(&emr, &cluster, "").await;
    let body = answer.ok(case).clone();
    assert_eq!(
        listed(&body, case).len(),
        3,
        "case {case:?}: the reads must have neither created nor consumed an endpoint: {body}"
    );
    for endpoint in listed(&body, case) {
        let id = text(&endpoint, "/id", case);
        let reported = text(&endpoint, "/state", case);
        assert_eq!(
            reported,
            state::ENDPOINT_ACTIVE,
            "case {case:?}: endpoint {id} reported {reported:?} long after its step came due"
        );
    }

    // A sanity row on the fixture itself: the bodies above really do omit the optional
    // members, so the record compared across the transition is not an empty one.
    let sample = listed(&body, case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the cluster lists no endpoint: {body}"));
    assert_eq!(
        field(&sample, "/tags", case),
        &json!({}),
        "case {case:?}: a create carrying no tags must report an empty tag set: {sample}"
    );
}
