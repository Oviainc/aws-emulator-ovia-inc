// SPDX-License-Identifier: CC0-1.0

//! `CreateDeployment` at the store's ceiling, checked and performed, driven over HTTP
//! through the same router the production emulator serves with.
//!
//! The rule under test, from the `dryRun` member's own modelled documentation
//! (`smithy_query(query="doc",shape="CreateDeploymentInput",member="dryRun")`): a
//! request that sets `dryRun` "checks whether you have the required permissions for the
//! action, without actually making the request", so it is answered the outcome the
//! PERFORMED request would have had — `DryRunOperation` only when that request would
//! have been served, and otherwise the refusal it would have been refused with — and it
//! changes nothing, including the records the performed request would have evicted to
//! fit.
//!
//! `tests/all/dry_run_preflight.rs` drives that rule over the refusals a single request
//! can carry on its own (an unpublished workload or pattern, settings the pattern's
//! schema warns about, a name a live deployment holds). This suite drives the one
//! refusal that is a property of the STORE rather than of the request — the modelled
//! `ResourceLimitException` of a create that does not fit — because only the store's
//! occupancy decides it, and the slot a create takes is made by a WRITE.
//!
//! The rows span that occupancy, which is what the rule generalises over: a store with
//! room, a store at [`MAX_DEPLOYMENTS`] with every record live (nothing to evict), and a
//! store at the ceiling holding a deleted record a create may take the slot of. Each row
//! is asserted over one deployment pattern per workload the catalogue publishes, read out
//! of the service rather than written by hand.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    deployment_limit_message, DRY_RUN_OPERATION_CODE, DRY_RUN_OPERATION_STATUS, MAX_DEPLOYMENTS,
};

use crate::deployment_lifecycle::{create, create_body, get};
use crate::deployment_update::{error_message, one_pattern_per_workload, with_member};
use crate::harness::{walk_pages, Harness};

/// Long enough for the two `state::TRANSITION_SPAN`s a DELETE walks
/// (`DELETE_INITIATING` -> `DELETE_IN_PROGRESS` -> `DELETED`) to come due, plus slack: a
/// record is only evictable once it has reached `DELETED`. The emulator never sleeps;
/// this is the test waiting for the clock, and it yields while it waits so the router's
/// own tasks keep running.
const DELETE_SETTLE: Duration = Duration::from_millis(1_200);

/// How many deployments the service currently stores, as `ListDeployments` reports them
/// — the occupancy each row below is stated against, and the state a checked request
/// must leave exactly as it was.
async fn stored_count(harness: &Harness, case: &str) -> usize {
    walk_pages(
        harness,
        case,
        "/listDeployments",
        &json!({}),
        "deployments",
        100,
    )
    .await
    .len()
}

#[tokio::test]
async fn a_checked_create_reports_the_store_ceiling_refusal_the_performed_create_gets() {
    let harness = Harness::new();
    let probes = one_pattern_per_workload(&harness).await;
    let (fill_workload, fill_pattern) = probes
        .first()
        .cloned()
        .expect("the catalogue publishes a pattern to fill the store with");

    // ROW 1 — room in the store: the performed create would be served, so the checked
    // one is answered the pre-flight error response.
    for (index, (workload, pattern)) in probes.iter().enumerate() {
        let case = format!("{workload}/{pattern}: room in the store");
        let body = create_body(
            &harness,
            workload,
            pattern,
            &format!("ceiling-room-{index}"),
            /* dry_run */ true,
        )
        .await;
        harness
            .refused(
                &case,
                "/createDeployment",
                body,
                DRY_RUN_OPERATION_STATUS,
                DRY_RUN_OPERATION_CODE,
            )
            .await;
    }
    assert_eq!(
        stored_count(&harness, "room in the store").await,
        0,
        "case \"room in the store\": a checked create stores nothing, so the store is \
         still empty"
    );

    // Fill the store to its ceiling with records that are all LIVE, so there is nothing
    // a create could evict to fit another one.
    let mut ids = Vec::new();
    for index in 0..MAX_DEPLOYMENTS {
        ids.push(
            create(
                &harness,
                "filling the store",
                &fill_workload,
                &fill_pattern,
                &format!("ceiling-{index:03}"),
            )
            .await,
        );
    }
    let filled = stored_count(&harness, "a filled store").await;
    assert_eq!(
        filled, MAX_DEPLOYMENTS,
        "case \"a filled store\": the store must hold its ceiling of records before the \
         ceiling rows, got {filled}"
    );

    // ROW 2 — at the ceiling with every record live: the performed create is refused
    // with the modelled resource-limit error, so the checked one reports THAT refusal,
    // down to the same status, code and message, and neither stores anything.
    for (index, (workload, pattern)) in probes.iter().enumerate() {
        let case = format!("{workload}/{pattern}: store at its ceiling");
        let checked_body = create_body(
            &harness,
            workload,
            pattern,
            &format!("ceiling-full-{index}"),
            /* dry_run */ true,
        )
        .await;
        let performed_body = with_member(&checked_body, "dryRun", json!(false));
        let checked = harness
            .refused(
                &format!("{case} (checked)"),
                "/createDeployment",
                checked_body,
                400,
                "ResourceLimitException",
            )
            .await;
        let performed = harness
            .refused(
                &format!("{case} (performed)"),
                "/createDeployment",
                performed_body,
                400,
                "ResourceLimitException",
            )
            .await;
        assert_eq!(
            error_message(&checked),
            error_message(&performed),
            "case {case:?}: the checked request must report the message the performed \
             one is refused with, got {:?}",
            error_message(&checked)
        );
        // And both carry the service's own resource-limit wording, so the two paths are
        // pinned to the message rather than only to each other.
        assert_eq!(
            error_message(&checked),
            deployment_limit_message(),
            "case {case:?}: the refusal must name the ceiling and what frees a slot, got \
             {:?}",
            error_message(&checked)
        );
        let after = stored_count(&harness, &case).await;
        assert_eq!(
            after, MAX_DEPLOYMENTS,
            "case {case:?}: a refused create stores nothing, got {after} records"
        );
    }

    // ROW 3 — at the ceiling, but holding a record that has reached DELETED: the
    // performed create fits by evicting it, so the checked one is answered the
    // pre-flight error again AND evicts nothing, because a check writes nothing.
    let evictable = ids
        .first()
        .cloned()
        .expect("the store was filled with deployments");
    harness
        .ok(
            "free a slot",
            "/deleteDeployment",
            json!({"deploymentId": evictable}),
        )
        .await;
    tokio::time::sleep(DELETE_SETTLE).await;
    for (index, (workload, pattern)) in probes.iter().enumerate() {
        let case = format!("{workload}/{pattern}: one deleted record to evict");
        let body = create_body(
            &harness,
            workload,
            pattern,
            &format!("ceiling-evictable-{index}"),
            /* dry_run */ true,
        )
        .await;
        harness
            .refused(
                &case,
                "/createDeployment",
                body,
                DRY_RUN_OPERATION_STATUS,
                DRY_RUN_OPERATION_CODE,
            )
            .await;
        let still_stored = get(&harness, &case, &evictable).await;
        assert_eq!(
            still_stored["id"],
            json!(evictable),
            "case {case:?}: a checked create must not evict the terminal record whose \
             slot the performed one would take, got {}",
            still_stored["id"]
        );
        let after = stored_count(&harness, &case).await;
        assert_eq!(
            after, MAX_DEPLOYMENTS,
            "case {case:?}: a checked create changes no record, got {after} records"
        );
    }

    // The room the check reported is real: the performed create takes that slot, which
    // is what makes the pre-flight error above the right answer.
    let replacement = create(
        &harness,
        "after the check",
        &fill_workload,
        &fill_pattern,
        "ceiling-performed",
    )
    .await;
    assert!(
        !ids.contains(&replacement),
        "case \"after the check\": the performed create must store a new record, got \
         {replacement:?}"
    );
    let evicted: Value = harness
        .refused(
            "the evicted record",
            "/getDeployment",
            json!({"deploymentId": evictable}),
            404,
            "ResourceNotFoundException",
        )
        .await;
    assert!(
        !evicted.to_string().contains(&evictable),
        "case \"the evicted record\": the refusal must not echo the deployment id asked \
         about, got {evicted}"
    );
}
