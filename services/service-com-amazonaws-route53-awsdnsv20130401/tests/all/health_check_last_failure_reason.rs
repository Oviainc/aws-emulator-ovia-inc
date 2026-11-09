// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetHealthCheckLastFailureReason`.
//!
//! The rule under test: the operation reports "one Observation element for each
//! Amazon Route 53 health checker that is reporting a last failure reason" for
//! the health check the account holds — so it reports exactly those checkers of
//! that health check whose latest observation is a FAILURE, and nothing for a
//! checker with no failure to report, for a health check no checker is checking
//! (one whose `Type` probes no endpoint, or whose checks are disabled) and for
//! one whose checkers have observed no failure at all. An id that names no
//! health check of the account is refused rather than answered.
//!
//! `GetHealthCheckStatus` answers the SAME modelled observation list for the same
//! health check, so the two readers are asserted against each other: what this
//! operation reports is the failing subset of what that one reports, observation
//! for observation. That is the invariant the tables below drive over a SET of
//! health checks — every `HealthCheckType` in the model's own value list, several
//! Region sets, and the `Disabled` setting both as created and as an update
//! leaves it. The operation is a report, so repeated reads must say the same
//! thing and leave the account as it was.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::HEALTH_CHECK_TYPE_CALCULATED;

use crate::health_check_status::{get_status, observations_in, Observation};
use crate::health_check_updates::update_health_check;
use crate::health_checks::{child_for, create_health_check, health_check_count, health_check_id, CATALOG};
use crate::hosted_zone_records::{call, router, Reply};

/// The prefix a checker's `StatusReport$Status` carries when the observation it
/// describes is a failure, as opposed to the `Success: ` of one that is not.
/// Route 53 words a checker's description as a sentence opening with one or the
/// other.
const FAILURE: &str = "Failure: ";

/// Send one `GetHealthCheckLastFailureReason` request.
async fn get_last_failure_reason(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/healthcheck/{id}/lastfailurereason"),
        "",
    )
    .await
}

/// The observations `GetHealthCheckLastFailureReason` reports for `id`, in wire
/// order.
///
/// A 501 here means the operation is still a scaffold, which is exactly what the
/// message says, so a regression to the stub reads as one.
async fn last_failure_observations(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> Vec<Observation> {
    let reply = get_last_failure_reason(router, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHealthCheckLastFailureReason (501 = still \
         scaffolded), got {} with body {}",
        reply.status, reply.body
    );
    observations_in(&reply, case)
}

/// The observations `GetHealthCheckStatus` reports for `id`, in wire order.
async fn status_observations(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> Vec<Observation> {
    let reply = get_status(router, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHealthCheckStatus, got {} with body {}",
        reply.status, reply.body
    );
    observations_in(&reply, case)
}

/// The checkers of `id` that are reporting a failure, according to the status
/// reader: the subset this operation has to report, computed in the test from
/// the other operation's answer rather than from the one under test.
async fn failing_checkers(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> Vec<Observation> {
    status_observations(router, id, case)
        .await
        .into_iter()
        .filter(|observation| observation.status.starts_with(FAILURE))
        .collect()
}

/// Assert the two readers of one health check's observations agree: the last
/// failure reason is the failing subset of the current status, observation for
/// observation.
async fn assert_agrees_with_the_status(router: &MultiServiceRouter, id: &str, case: &str) {
    let failing = failing_checkers(router, id, case).await;
    let reported = last_failure_observations(router, id, case).await;
    assert_eq!(
        reported, failing,
        "case {case:?}: expected the checkers reporting a failure {failing:?}, got {reported:?}"
    );
    for observation in &reported {
        assert!(
            observation.status.starts_with(FAILURE),
            "case {case:?}: every reported observation describes a failure, got {:?}",
            observation.status
        );
        assert!(
            !observation.region.is_empty() && !observation.ip_address.is_empty(),
            "case {case:?}: an observation names the checker that made it, got {observation:?}"
        );
    }
}

#[tokio::test]
async fn the_last_failure_reason_is_reported_for_every_checker_that_has_one() {
    // One row per modelled `HealthCheckType` (the catalog the sibling suite
    // keeps pinned to the model's own value list), because what a health check
    // IS varies by type: four of them probe an endpoint for a checker to report
    // about, and the other four read a CloudWatch alarm, combine child health
    // checks or follow a routing control — and a checker only has a failure to
    // report about an endpoint it is checking.
    for row in CATALOG {
        let router = router();
        let child = child_for(&router, row).await;
        let created = create_health_check(
            &router,
            &format!("last-failure-{}", row.kind),
            &row.config(&child),
        )
        .await;
        let id = health_check_id(&created, row.kind);
        // A CALCULATED health check has no endpoint and no checkers, so BOTH
        // readers of its observations refuse it rather than answering with a
        // list; `health_check_calculated_reads` asserts that refusal.
        if row.kind == HEALTH_CHECK_TYPE_CALCULATED {
            let reply = get_last_failure_reason(&router, &id).await;
            assert_eq!(
                reply.status, 400,
                "case {:?}: expected 400 from GetHealthCheckLastFailureReason, got {} with \
                 body {}",
                row.kind, reply.status, reply.body
            );
            continue;
        }
        assert_agrees_with_the_status(&router, &id, row.kind).await;

        if !row.endpoint_probe {
            let reported = last_failure_observations(&router, &id, row.kind).await;
            assert!(
                reported.is_empty(),
                "case {:?}: a health check that probes no endpoint has no checker reporting a \
                 failure reason, got {reported:?}",
                row.kind
            );
        }
    }
}

#[tokio::test]
async fn the_checkers_the_configuration_names_are_the_only_ones_that_can_report_a_failure() {
    // The rows differ along the SIZE and the ORDER of the Region set the
    // configuration names: whatever the request spelled, a checker that is not
    // checking this endpoint cannot be reporting a failure of it. `Regions` is
    // modelled `len 3..64`, so three is the smallest set a request may carry
    // (smithy_query(query="shape",shape="HealthCheckRegionList")).
    let rows: &[(&str, &[&str])] = &[
        (
            "the smallest set the model allows",
            &["us-east-1", "us-west-2", "eu-west-1"],
        ),
        (
            "four Regions, spelled in reverse order",
            &["sa-east-1", "ap-southeast-2", "ap-southeast-1", "ap-northeast-1"],
        ),
        (
            "a Region named twice",
            &["us-west-1", "us-west-1", "eu-west-1", "us-east-1"],
        ),
    ];

    for (case, regions) in rows {
        let router = router();
        let spelled: String = regions
            .iter()
            .map(|region| format!("<Region>{region}</Region>"))
            .collect();
        let created = create_health_check(
            &router,
            &format!("last-failure-regions-{}", regions.len()),
            &format!(
                "<Type>HTTP</Type><IPAddress>192.0.2.83</IPAddress><Port>80</Port>\
                 <Regions>{spelled}</Regions>"
            ),
        )
        .await;
        let id = health_check_id(&created, case);
        assert_agrees_with_the_status(&router, &id, case).await;

        let reported = last_failure_observations(&router, &id, case).await;
        for observation in &reported {
            assert!(
                regions.contains(&observation.region.as_str()),
                "case {case:?}: only a checker the configuration names can report a failure, \
                 got {observation:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_health_check_whose_checks_are_disabled_has_no_checker_reporting_a_failure() {
    // Disabled stops Route 53 submitting requests to the endpoint, so no checker
    // is observing it and none has a failure to report. The rows span the types
    // that probe an endpoint, and each is then re-enabled and disabled again
    // through UpdateHealthCheck, so the setting is asserted both as a create
    // persisted it and as an update left it.
    for row in CATALOG.iter().filter(|row| row.endpoint_probe) {
        let router = router();
        let created = create_health_check(
            &router,
            &format!("last-failure-disabled-{}", row.kind),
            &format!("{}<Disabled>true</Disabled>", row.config("")),
        )
        .await;
        let id = health_check_id(&created, row.kind);
        let disabled = last_failure_observations(&router, &id, row.kind).await;
        assert!(
            disabled.is_empty(),
            "case {:?}: a disabled health check has no checker reporting a failure reason, got \
             {disabled:?}",
            row.kind
        );

        for (attempt, setting) in ["false", "true"].iter().enumerate() {
            let case = format!("{} with Disabled {setting:?}", row.kind);
            let updated = update_health_check(
                &router,
                &id,
                &format!("<Disabled>{setting}</Disabled>"),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected 200 from UpdateHealthCheck {attempt}, got {} with body {}",
                updated.status, updated.body
            );
            assert_agrees_with_the_status(&router, &id, &case).await;
        }
    }
}

#[tokio::test]
async fn reading_the_last_failure_reason_reports_the_same_thing_and_changes_nothing() {
    // The operation is a report: the rows are repeated reads, with other calls
    // in between, and every one of them has to say what the first said and leave
    // the account's health checks — and what the status reader says about them —
    // exactly as they were.
    let router = router();
    let created = create_health_check(
        &router,
        "last-failure-stable",
        "<Type>HTTPS_STR_MATCH</Type><FullyQualifiedDomainName>stable.example.net\
         </FullyQualifiedDomainName><SearchString>SERVING</SearchString>\
         <RequestInterval>30</RequestInterval>",
    )
    .await;
    let id = health_check_id(&created, "last-failure-stable");
    let first = last_failure_observations(&router, &id, "the first read").await;
    let status_before = status_observations(&router, &id, "before the repeated reads").await;
    let population = health_check_count(&router, "after the first read").await;

    for attempt in 1..=3 {
        let case = format!("read {attempt}");
        let again = last_failure_observations(&router, &id, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: a repeated read must report the same observations, got {again:?}"
        );
        let counted = health_check_count(&router, &case).await;
        assert_eq!(
            counted, population,
            "case {case:?}: reading the last failure reason must not create or destroy a health \
             check, GetHealthCheckCount reports {counted}"
        );
        let status_now = status_observations(&router, &id, &case).await;
        assert_eq!(
            status_now, status_before,
            "case {case:?}: reading the last failure reason must not change what the status \
             reader reports, got {status_now:?}"
        );
    }
}
