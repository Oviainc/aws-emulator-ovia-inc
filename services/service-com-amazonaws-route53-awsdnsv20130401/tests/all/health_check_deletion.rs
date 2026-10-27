// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteHealthCheck`.
//!
//! The rule under test: a delete takes the health check the id names out of the
//! account, so every later read of it is `NoSuchHealthCheck` (HTTP 404), the
//! population every count and listing reports shrinks by exactly that one check,
//! and nothing the store holds can refuse the call — `HealthCheckInUse`'s own
//! documentation says the error "is not in use", and the operation's
//! documentation says "Amazon Route 53 does not prevent you from deleting a
//! health check even if the health check is associated with one or more resource
//! record sets". What OUTLIVES the health check is its `CallerReference`: a
//! create presenting the reference of a health check "that you created and later
//! deleted, regardless of the other settings in the request" is
//! `HealthCheckAlreadyExists` (HTTP 409).
//!
//! The tables therefore vary the KIND of health check deleted (an endpoint
//! probe on an address and on a name, a string-match probe, a CloudWatch-alarm
//! reader and a calculated check over a child), the population it is deleted
//! from, and — for the retained reference — whether the create that follows the
//! delete repeats the deleted check's configuration or sends another one.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use.

use smithy_server_runtime::MultiServiceRouter;

use crate::health_checks::{
    create_health_check, get_health_check, health_check_count, health_check_id,
};
use crate::hosted_zone_records::{call, router, Reply};

/// Send one `DeleteHealthCheck` request.
///
/// `pub(crate)` so a sibling suite deletes through the SAME request rather than
/// growing a second copy of it.
pub(crate) async fn delete_health_check(router: &MultiServiceRouter, id: &str) -> Reply {
    call(router, "DELETE", &format!("/2013-04-01/healthcheck/{id}"), "").await
}

/// One health check to create and then delete.
struct DeleteRow {
    case: &'static str,
    /// The `HealthCheckConfig` the health check is created with. A `CALCULATED`
    /// row names its child by the `{child}` placeholder, filled in with the id
    /// of a check created for it.
    config: &'static str,
}

/// The health checks this suite deletes: one per kind of thing a health check
/// can assess, because what a health check IS varies by type — an endpoint
/// probe, a string match in a response body, a CloudWatch alarm reader, or a
/// combination of child checks.
const DELETABLE: &[DeleteRow] = &[
    DeleteRow {
        case: "an HTTP check on an address",
        config: "<Type>HTTP</Type><IPAddress>192.0.2.20</IPAddress><Port>80</Port>",
    },
    DeleteRow {
        case: "an HTTPS check on a domain name",
        config: "<Type>HTTPS</Type><FullyQualifiedDomainName>delete.example.com\
                 </FullyQualifiedDomainName><Port>443</Port>",
    },
    DeleteRow {
        case: "a TCP check",
        config: "<Type>TCP</Type><IPAddress>192.0.2.21</IPAddress><Port>8080</Port>",
    },
    DeleteRow {
        case: "a string-match check",
        config: "<Type>HTTP_STR_MATCH</Type><IPAddress>192.0.2.22</IPAddress><Port>80</Port>\
                 <SearchString>healthy</SearchString>",
    },
    DeleteRow {
        case: "a CloudWatch-alarm check",
        config: "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
                 <Name>endpoint-unhealthy</Name></AlarmIdentifier>",
    },
    DeleteRow {
        case: "a CALCULATED check over one child",
        config: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                 <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                 </ChildHealthChecks>",
    },
];

#[tokio::test]
async fn a_deleted_health_check_is_gone_from_every_read() {
    let router = router();
    // The child a CALCULATED row names, and the check that is NEVER deleted, so
    // every row also shows the delete reached one health check only.
    let kept = create_health_check(
        &router,
        "delete-kept",
        "<Type>TCP</Type><IPAddress>192.0.2.60</IPAddress><Port>443</Port>",
    )
    .await;
    let kept_id = health_check_id(&kept, "the kept health check");

    for row in DELETABLE {
        let config = row.config.replace("{child}", &kept_id);
        let created = create_health_check(&router, row.case, &config).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = health_check_id(&created, row.case);
        let before = health_check_count(&router, row.case).await;

        let deleted = delete_health_check(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected HTTP 200 from DeleteHealthCheck, got {} with body {}",
            row.case, deleted.status, deleted.body
        );

        // The read is the proof: the health check is no longer there, and the
        // population shrank by exactly one.
        let read_back = get_health_check(&router, &id).await;
        assert_eq!(
            read_back.status, 404,
            "case {:?}: expected HTTP 404 from GetHealthCheck after the delete, got {} with \
             body {}",
            row.case, read_back.status, read_back.body
        );
        assert_eq!(
            read_back.error_code(),
            "NoSuchHealthCheck",
            "case {:?}: expected NoSuchHealthCheck, got {:?}; body {}",
            row.case,
            read_back.error_code(),
            read_back.body
        );
        let after = health_check_count(&router, row.case).await;
        assert_eq!(
            after,
            before - 1,
            "case {:?}: expected GetHealthCheckCount {}, got {after}",
            row.case,
            before - 1
        );

        // Deleting it again addresses a health check that is not there, so it
        // reads as missing rather than as a second delete.
        let again = delete_health_check(&router, &id).await;
        assert_eq!(
            again.status, 404,
            "case {:?}: expected HTTP 404 from a repeated DeleteHealthCheck, got {} with body {}",
            row.case, again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "NoSuchHealthCheck",
            "case {:?}: expected NoSuchHealthCheck from a repeated delete, got {:?}; body {}",
            row.case,
            again.error_code(),
            again.body
        );

        // The delete reached ONE health check: the one every row kept is still
        // readable.
        let kept_read = get_health_check(&router, &kept_id).await;
        assert_eq!(
            kept_read.status, 200,
            "case {:?}: the health check no row deleted must still be readable, got {} with \
             body {}",
            row.case, kept_read.status, kept_read.body
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_health_check_is_refused_without_deleting_anything() {
    let router = router();
    let created = create_health_check(
        &router,
        "delete-missing",
        "<Type>HTTP</Type><IPAddress>192.0.2.30</IPAddress><Port>80</Port>",
    )
    .await;
    let id = health_check_id(&created, "the health check that stays");

    // The ids differ along form and length: a well-formed id of the shape this
    // service mints, one in another service's form, and a bare word — none of
    // them names a health check the account holds.
    for missing in [
        "01234567-89ab-cdef-0123-456789abcdef",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "Z1D633PJN98FT9",
        "not-an-id",
    ] {
        let refused = delete_health_check(&router, missing).await;
        assert_eq!(
            refused.status, 404,
            "case {missing:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchHealthCheck",
            "case {missing:?}: expected NoSuchHealthCheck, got {:?}; body {}",
            refused.error_code(),
            refused.body
        );
        // The refusal names the id the request asked about, so a caller
        // deleting several ids reads back which one was missing.
        let message = refused.first("Error/Message").unwrap_or("");
        assert!(
            message.contains(missing),
            "case {missing:?}: the refusal must name the id asked about, got {message:?}"
        );
    }

    let read_back = get_health_check(&router, &id).await;
    assert_eq!(
        read_back.status, 200,
        "the health check no request named must still be readable, got {} with body {}",
        read_back.status, read_back.body
    );
}

#[tokio::test]
async fn the_caller_reference_of_a_deleted_health_check_stays_held() {
    // "If you send a CreateHealthCheck request with the same CallerReference as
    // a deleted health check, regardless of the settings, Route 53 returns a
    // HealthCheckAlreadyExists error." The rows vary what the create that
    // follows the delete sends: the deleted check's own configuration, a
    // configuration differing in one setting, and one of another type
    // altogether — "regardless of the settings" covers all three.
    let original = "<Type>HTTP</Type><IPAddress>192.0.2.40</IPAddress><Port>80</Port>";
    for (case, retried) in [
        ("the configuration the deleted health check carried", original),
        (
            "a configuration differing in one setting",
            "<Type>HTTP</Type><IPAddress>192.0.2.40</IPAddress><Port>8080</Port>",
        ),
        (
            "a configuration of another type altogether",
            "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
             <Name>endpoint-unhealthy</Name></AlarmIdentifier>",
        ),
    ] {
        let router = router();
        let reference = "held-reference";
        let created = create_health_check(&router, reference, original).await;
        let id = health_check_id(&created, case);
        let deleted = delete_health_check(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteHealthCheck, got {} with body {}",
            deleted.status, deleted.body
        );

        let refused = create_health_check(&router, reference, retried).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "HealthCheckAlreadyExists",
            "case {case:?}: expected HealthCheckAlreadyExists, got {:?}; body {}",
            refused.error_code(),
            refused.body
        );
        // The refusal created nothing: the account is back to holding none.
        let count = health_check_count(&router, case).await;
        assert_eq!(
            count, 0,
            "case {case:?}: the refused create must leave the account holding no health check, \
             got {count}"
        );

        // A reference no health check ever used is free, which is what makes the
        // refusals above about the reference the deleted check held.
        let fresh = create_health_check(&router, "another-reference", retried).await;
        assert_eq!(
            fresh.status, 201,
            "case {case:?}: a create under an unused caller reference must succeed, got {} with \
             body {}",
            fresh.status, fresh.body
        );
    }
}
