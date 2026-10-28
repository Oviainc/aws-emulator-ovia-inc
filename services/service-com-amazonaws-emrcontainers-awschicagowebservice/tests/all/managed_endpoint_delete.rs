// SPDX-License-Identifier: CC0-1.0

//! `DeleteManagedEndpoint`, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client. The create, describe and list helpers are the ones
//! [`crate::managed_endpoint_listing`] drives, so the suites cannot drift apart.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. Deleting a resource that is de-provisioned starts its teardown rather than dropping
//!    it: the endpoint of the named (`virtualClusterId`, `id`) pair moves to `TERMINATING`,
//!    the clock owes it `TERMINATED`, the response reports the pair it acted on, and both
//!    readers answer from that one record — the teardown moves `state` and nothing else.
//! 2. The teardown reaches ONLY the endpoint the request named: a sibling endpoint, another
//!    cluster's endpoint and the virtual cluster the endpoint gateways into are all left as
//!    they were.
//! 3. A teardown is a transition only a resource that has not ended has left to take, so a
//!    retried delete while the teardown is in flight answers the same pair and leaves the
//!    record exactly as it is, and a delete of an endpoint that has already reached a
//!    terminal state is refused.
//! 4. This operation models no not-found error (smithy_query, operation
//!    `DeleteManagedEndpoint`: `InternalServerException`, `ValidationException`), so a pair
//!    the store does not hold — and an `id` outside the constraints the model puts on it —
//!    is refused with the modelled `ValidationException` (HTTP 400), never with a silent
//!    success and never with 501, and changes nothing.

use serde_json::Value;
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::ENDPOINT_STATE_VALUES;
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{
    create_endpoint, endpoint_body, endpoint_record, endpoint_state, list_endpoints, listed_ids,
    listed_record, stored_bodies, wait_for_the_clock, without_state,
};
use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};

/// `DELETE /virtualclusters/{cluster}/endpoints/{id}` — `DeleteManagedEndpoint`.
async fn delete_endpoint(emr: &Emr, cluster: &str, id: &str) -> Answer {
    emr.send(
        "DELETE",
        &format!("/virtualclusters/{cluster}/endpoints/{id}"),
        None,
    )
    .await
}

/// RULE 1: a delete starts the endpoint's teardown, and both readers follow that record
/// through its two steps.
///
/// The rows span the forms an endpoint takes (which optional members the create carried,
/// its `type`, how deep its `configurationOverrides` goes) crossed with the live state the
/// endpoint is in when the delete arrives: still `CREATING`, or settled to `ACTIVE`.
#[tokio::test]
async fn deleting_a_managed_endpoint_tears_down_the_named_pair() {
    for (index, (label, body)) in stored_bodies().into_iter().enumerate() {
        for settled in [false, true] {
            let emr = Emr::new();
            let phase = if settled { "ACTIVE" } else { "CREATING" };
            let case = format!("{label}, deleted while {phase}");
            let cluster = emr
                .cluster(
                    &format!("endpoint-delete-{index}-{settled}"),
                    &format!("vc-tok-ep-delete-{index}-{settled}"),
                )
                .await;
            let created = create_endpoint(&emr, &cluster, body.clone()).await;
            let id = text(created.ok(&case), "/id", &case);
            if settled {
                wait_for_the_clock();
            }

            // The live state the row means to delete from.
            let before = endpoint_record(&emr, &cluster, &id, &case).await;
            let state_before = text(&before, "/state", &case);
            let expected_before = if settled {
                state::ENDPOINT_ACTIVE
            } else {
                state::ENDPOINT_CREATING
            };
            assert_eq!(
                state_before, expected_before,
                "case {case:?}: the fixture must be {expected_before} before the delete, got \
                 {state_before:?}"
            );

            let answer = delete_endpoint(&emr, &cluster, &id).await;
            let body_out = answer.ok(&case).clone();
            assert_eq!(
                text(&body_out, "/id", &case),
                id,
                "case {case:?}: the delete reported another endpoint: {body_out}"
            );
            assert_eq!(
                text(&body_out, "/virtualClusterId", &case),
                cluster,
                "case {case:?}: the delete reported another virtual cluster: {body_out}"
            );

            // The teardown is in flight, and it moved the state and nothing else.
            let during = endpoint_record(&emr, &cluster, &id, &case).await;
            let state_during = text(&during, "/state", &case);
            assert_eq!(
                state_during,
                state::ENDPOINT_TERMINATING,
                "case {case:?}: the delete must leave the endpoint TERMINATING, got \
                 {state_during:?}"
            );
            assert_eq!(
                without_state(&during, &case),
                without_state(&before, &case),
                "case {case:?}: the teardown must move state and nothing else"
            );
            assert_eq!(
                listed_record(&emr, &cluster, &id, &case).await,
                during,
                "case {case:?}: the listing disagrees with the describe while the endpoint \
                 tears down"
            );

            wait_for_the_clock();

            // The clock owed the endpoint its terminal state.
            let after = endpoint_record(&emr, &cluster, &id, &case).await;
            let state_after = text(&after, "/state", &case);
            assert_eq!(
                state_after,
                state::ENDPOINT_TERMINATED,
                "case {case:?}: the clock owed this endpoint TERMINATED, got {state_after:?}"
            );
            assert_eq!(
                without_state(&after, &case),
                without_state(&before, &case),
                "case {case:?}: settling the teardown must move state and nothing else"
            );
            assert_eq!(
                listed_record(&emr, &cluster, &id, &case).await,
                after,
                "case {case:?}: the listing disagrees with the describe after the teardown"
            );

            // The delete ENDED the endpoint, it did not forget it: the cluster still lists
            // exactly the endpoint it held.
            let answer = list_endpoints(&emr, &cluster, "").await;
            let listing = answer.ok(&case).clone();
            assert_eq!(
                listed_ids(&listing, &case),
                vec![id.clone()],
                "case {case:?}: a torn-down endpoint must still be listed: {listing}"
            );
            // And a listing filtered on the terminal state reports it, so the state really
            // is the stored one rather than one assembled for the describe.
            let answer = list_endpoints(
                &emr,
                &cluster,
                &format!("?states={}", state::ENDPOINT_TERMINATED),
            )
            .await;
            let filtered = answer.ok(&case).clone();
            assert_eq!(
                listed_ids(&filtered, &case),
                vec![id.clone()],
                "case {case:?}: the stored state must satisfy a states filter: {filtered}"
            );
        }
    }
}

/// The states this operation writes are members of the model's own `EndpointState` enum, so
/// no response carries a state a client would not accept.
///
/// Source: the generated `ENDPOINT_STATE_VALUES` catalog in `ops::types::shared_types`.
#[test]
fn the_endpoint_states_a_delete_writes_are_modelled_ones() {
    for value in [state::ENDPOINT_TERMINATING, state::ENDPOINT_TERMINATED] {
        assert!(
            ENDPOINT_STATE_VALUES.contains(&value),
            "case {value:?}: not an EndpointState; the model allows {ENDPOINT_STATE_VALUES:?}"
        );
    }
}

/// RULE 2: the teardown reaches only the endpoint the request named.
///
/// The rows differ in WHICH of a cluster's endpoints is deleted — the first, a middle one
/// and the last — so a teardown that reached a neighbour by position is caught.
#[tokio::test]
async fn deleting_one_endpoint_leaves_every_other_resource_alone() {
    for target in 0..3_usize {
        let emr = Emr::new();
        let case = format!("deleting endpoint {target} of three");
        let cluster = emr
            .cluster(
                &format!("endpoint-delete-scope-{target}"),
                &format!("vc-tok-ep-scope-{target}"),
            )
            .await;
        let other = emr
            .cluster(
                &format!("endpoint-delete-other-{target}"),
                &format!("vc-tok-ep-other-{target}"),
            )
            .await;

        let mut ids: Vec<String> = Vec::new();
        for slot in 0..3 {
            let created = create_endpoint(
                &emr,
                &cluster,
                endpoint_body(
                    &format!("scope-{target}-{slot}"),
                    &format!("ep-tok-scope-{target}-{slot}"),
                    "JUPYTER_ENTERPRISE_GATEWAY",
                ),
            )
            .await;
            ids.push(text(created.ok(&case), "/id", &case));
        }
        // The same endpoint id cannot exist in two clusters, so the neighbour under test in
        // the other cluster is its own endpoint, created from the same body.
        let created = create_endpoint(
            &emr,
            &other,
            endpoint_body(
                &format!("scope-other-{target}"),
                &format!("ep-tok-scope-other-{target}"),
                "JUPYTER_SERVER",
            ),
        )
        .await;
        let elsewhere = text(created.ok(&case), "/id", &case);
        wait_for_the_clock();

        let siblings: Vec<Value> = {
            let mut records = Vec::new();
            for (slot, id) in ids.iter().enumerate() {
                if slot != target {
                    records.push(endpoint_record(&emr, &cluster, id, &case).await);
                }
            }
            records
        };
        let elsewhere_before = endpoint_record(&emr, &other, &elsewhere, &case).await;

        delete_endpoint(&emr, &cluster, &ids[target]).await.ok(&case);

        let moved = endpoint_state(&emr, &cluster, &ids[target], &case).await;
        assert_eq!(
            moved,
            state::ENDPOINT_TERMINATING,
            "case {case:?}: the named endpoint must tear down, got {moved:?}"
        );
        let mut index = 0;
        for (slot, id) in ids.iter().enumerate() {
            if slot == target {
                continue;
            }
            let now = endpoint_record(&emr, &cluster, id, &case).await;
            assert_eq!(
                now, siblings[index],
                "case {case:?}: sibling endpoint {id} must be untouched by the delete"
            );
            index += 1;
        }
        assert_eq!(
            endpoint_record(&emr, &other, &elsewhere, &case).await,
            elsewhere_before,
            "case {case:?}: another cluster's endpoint must be untouched by the delete"
        );

        // A child's teardown does not touch the parent: the cluster the endpoint gatewayed
        // into is still RUNNING and still hosts creates.
        let answer = emr.describe_cluster(&cluster).await;
        let described_cluster = field(answer.ok(&case), "/virtualCluster", &case).clone();
        let cluster_state = text(&described_cluster, "/state", &case);
        assert_eq!(
            cluster_state,
            state::CLUSTER_RUNNING,
            "case {case:?}: deleting an endpoint must leave its cluster RUNNING, got \
             {cluster_state:?}"
        );
        let created = create_endpoint(
            &emr,
            &cluster,
            endpoint_body(
                &format!("scope-after-{target}"),
                &format!("ep-tok-scope-after-{target}"),
                "JUPYTER_ENTERPRISE_GATEWAY",
            ),
        )
        .await;
        let fresh = text(created.ok(&case), "/id", &case);
        let fresh_state = endpoint_state(&emr, &cluster, &fresh, &case).await;
        assert_eq!(
            fresh_state,
            state::ENDPOINT_CREATING,
            "case {case:?}: the cluster must still host a fresh endpoint, got {fresh_state:?}"
        );
    }
}

/// RULE 3: a delete retried while the teardown is in flight is answered without changing
/// the record, and an endpoint that has already ended has no teardown left to take.
///
/// The rows differ in HOW MANY retries arrive before the clock settles the teardown, and
/// the last block deletes an endpoint its cluster's teardown already ended, so an endpoint
/// that reached its terminal state another way is refused on the same terms.
#[tokio::test]
async fn deleting_an_endpoint_whose_teardown_already_happened() {
    for retries in [1_usize, 3] {
        let emr = Emr::new();
        let case = format!("{retries} retried delete(s) while the teardown is in flight");
        let cluster = emr
            .cluster(
                &format!("endpoint-delete-retry-{retries}"),
                &format!("vc-tok-ep-retry-{retries}"),
            )
            .await;
        let created = create_endpoint(
            &emr,
            &cluster,
            endpoint_body(
                &format!("retry-{retries}"),
                &format!("ep-tok-retry-{retries}"),
                "JUPYTER_ENTERPRISE_GATEWAY",
            ),
        )
        .await;
        let id = text(created.ok(&case), "/id", &case);
        wait_for_the_clock();

        delete_endpoint(&emr, &cluster, &id).await.ok(&case);
        let during = endpoint_record(&emr, &cluster, &id, &case).await;

        for retry in 0..retries {
            let answer = delete_endpoint(&emr, &cluster, &id).await;
            let body = answer.ok(&case).clone();
            assert_eq!(
                text(&body, "/id", &case),
                id,
                "case {case:?}: retry {retry} reported another endpoint: {body}"
            );
            assert_eq!(
                text(&body, "/virtualClusterId", &case),
                cluster,
                "case {case:?}: retry {retry} reported another virtual cluster: {body}"
            );
            assert_eq!(
                endpoint_record(&emr, &cluster, &id, &case).await,
                during,
                "case {case:?}: retry {retry} must leave the tearing-down record exactly as \
                 it was"
            );
        }

        wait_for_the_clock();
        let settled = endpoint_state(&emr, &cluster, &id, &case).await;
        assert_eq!(
            settled,
            state::ENDPOINT_TERMINATED,
            "case {case:?}: the retries must not have kept the teardown from settling, got \
             {settled:?}"
        );

        // Once the endpoint has ended there is no teardown left to start.
        delete_endpoint(&emr, &cluster, &id)
            .await
            .refused(&case, 400, "ValidationException");
        let after = endpoint_state(&emr, &cluster, &id, &case).await;
        assert_eq!(
            after,
            state::ENDPOINT_TERMINATED,
            "case {case:?}: the refused delete must leave the endpoint as it was, got \
             {after:?}"
        );
    }

    // An endpoint that reached its terminal state through its CLUSTER's teardown is refused
    // on the same terms, and one still being carried down by that teardown is left to it.
    let emr = Emr::new();
    let case = "an endpoint taken down by its cluster's teardown";
    let cluster = emr.cluster("endpoint-delete-cascade", "vc-tok-ep-cascade").await;
    let created = create_endpoint(
        &emr,
        &cluster,
        endpoint_body("cascade", "ep-tok-cascade", "JUPYTER_SERVER"),
    )
    .await;
    let id = text(created.ok(case), "/id", case);
    wait_for_the_clock();
    emr.delete_cluster(&cluster).await.ok(case);

    let during = endpoint_record(&emr, &cluster, &id, case).await;
    let state_during = text(&during, "/state", case);
    assert_eq!(
        state_during,
        state::ENDPOINT_TERMINATING,
        "case {case:?}: the cluster's teardown must have taken the endpoint with it, got \
         {state_during:?}"
    );
    delete_endpoint(&emr, &cluster, &id).await.ok(case);
    assert_eq!(
        endpoint_record(&emr, &cluster, &id, case).await,
        during,
        "case {case:?}: a delete during the cluster's teardown must change nothing"
    );

    wait_for_the_clock();
    let settled = endpoint_state(&emr, &cluster, &id, case).await;
    assert_eq!(
        settled,
        state::ENDPOINT_TERMINATED,
        "case {case:?}: the cascade owed this endpoint TERMINATED, got {settled:?}"
    );
    delete_endpoint(&emr, &cluster, &id)
        .await
        .refused(case, 400, "ValidationException");
}

/// RULE 4: a pair the store does not hold is refused with the modelled
/// `ValidationException`, and the refusals change nothing.
///
/// The rows walk the forms an unheld pair takes — a stored id under another cluster, under
/// a cluster holding no endpoint and under a cluster that was never created, a well-formed
/// id nothing was minted for at each `ResourceIdString` length bound, and a cluster's own
/// id as an endpoint id — and then the forms the model itself refuses: one character past
/// the length maximum and the character classes the pattern excludes.
#[tokio::test]
async fn deleting_a_pair_the_store_does_not_hold_is_a_validation_error() {
    let emr = Emr::new();
    let case = "the fixture";
    let first = emr.cluster("endpoint-delete-unheld-a", "vc-tok-unheld-a").await;
    let second = emr.cluster("endpoint-delete-unheld-b", "vc-tok-unheld-b").await;
    let empty = emr.cluster("endpoint-delete-unheld-c", "vc-tok-unheld-c").await;

    let mut stored: Vec<(String, String)> = Vec::new();
    for cluster in [&first, &second] {
        let created = create_endpoint(
            &emr,
            cluster,
            endpoint_body(
                "unheld",
                &format!("ep-tok-unheld-{cluster}"),
                "JUPYTER_ENTERPRISE_GATEWAY",
            ),
        )
        .await;
        stored.push(((*cluster).clone(), text(created.ok(case), "/id", case)));
    }
    wait_for_the_clock();

    // `ResourceIdString` is len 1..64 with pattern `^[0-9a-z]+$` (smithy_query: operation
    // `DeleteManagedEndpoint`, input `id`).
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
            "a stored endpoint under a cluster that was never created".to_string(),
            "nosuchcluster".to_string(),
            stored[0].1.clone(),
        ),
        (
            "the first cluster's endpoint under the second".to_string(),
            second.clone(),
            stored[0].1.clone(),
        ),
        (
            "the second cluster's endpoint under the first".to_string(),
            first.clone(),
            stored[1].1.clone(),
        ),
        (
            "a stored endpoint under a cluster holding none".to_string(),
            empty.clone(),
            stored[0].1.clone(),
        ),
    ];
    for (label, id) in [
        ("one character past the length maximum", "z".repeat(65)),
        ("an id twice the length maximum", "0".repeat(128)),
        ("an upper-case id", stored[0].1.to_uppercase()),
        ("an id carrying a hyphen", format!("{}-1", stored[0].1)),
        ("an id carrying an underscore", format!("{}_1", stored[0].1)),
        ("an id carrying a dot", format!("{}.1", stored[0].1)),
    ] {
        unheld.push((label.to_string(), first.clone(), id));
    }

    for (label, cluster, id) in &unheld {
        delete_endpoint(&emr, cluster, id)
            .await
            .refused(label, 400, "ValidationException");
    }

    // The refusals changed nothing: every stored endpoint is still ACTIVE, so none of them
    // was torn down by a request that named another pair.
    for (cluster, id) in &stored {
        let case = format!("endpoint {id} of cluster {cluster} after the refusals");
        let reported = endpoint_state(&emr, cluster, id, &case).await;
        assert_eq!(
            reported,
            state::ENDPOINT_ACTIVE,
            "case {case:?}: a refused delete must leave every stored endpoint as it was, got \
             {reported:?}"
        );
    }
}
