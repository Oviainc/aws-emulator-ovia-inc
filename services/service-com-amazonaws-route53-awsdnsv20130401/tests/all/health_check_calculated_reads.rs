// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of ONE rule: the two readers of a health check's checker
//! observations refuse a health check that has no checkers.
//!
//! The rule under test: a `CALCULATED` health check is "for health checks that
//! monitor the status of other health checks", so Route 53 "adds up the number
//! of health checks that Route 53 health checkers consider to be healthy and
//! compares that number with the value of HealthThreshold"
//! (smithy_query(query="doc",shape="HealthCheckConfig",member="Type")). Its
//! health is a function of its children, published as a CloudWatch series, and
//! not an observation any checker made — so an operation that reports per-checker
//! observations has nothing to report for it and refuses the read with the
//! modelled `InvalidInput` (HTTP 400) rather than answering with a list.
//!
//! One read of one calculated check is ONE row of that class, so the tables
//! below span it: both readers (`GetHealthCheckStatus` and
//! `GetHealthCheckLastFailureReason`), calculated checks that differ along the
//! children they name and the settings they carry, and the two neighbouring
//! classes that must NOT be refused — a health check that probes an endpoint,
//! and the other two types that probe none (the alarm reader and the
//! routing-control follower), which answer an empty observation list.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::CALCULATED_HEALTH_CHECK_OBSERVATIONS_MESSAGE;

use crate::health_check_status::get_status;
use crate::health_checks::{create_health_check, health_check_count, health_check_id};
use crate::hosted_zone_records::{call, router, Reply};

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

/// The two readers of a health check's checker observations, by name.
const READERS: &[&str] = &["GetHealthCheckStatus", "GetHealthCheckLastFailureReason"];

/// Read `id` through the reader `reader` names.
async fn read_through(router: &MultiServiceRouter, reader: &str, id: &str) -> Reply {
    if reader == "GetHealthCheckStatus" {
        get_status(router, id).await
    } else {
        get_last_failure_reason(router, id).await
    }
}

/// Create an endpoint health check whose child a calculated check can name, and
/// return its id.
async fn child_check(router: &MultiServiceRouter, reference: &str) -> String {
    let created = create_health_check(
        router,
        reference,
        "<Type>HTTP</Type><IPAddress>192.0.2.64</IPAddress><Port>80</Port>",
    )
    .await;
    health_check_id(&created, reference)
}

#[tokio::test]
async fn both_readers_refuse_a_calculated_health_check() {
    // The rows differ along what the calculated check is made of: how many
    // children it adds up (none, one, several), the threshold it compares them
    // with, and whether its own checks are disabled — none of which gives it a
    // checker, so every row is refused by both readers with the same sentence.
    struct Row {
        case: &'static str,
        /// `{child}` stands for the id of a health check created first.
        members: &'static str,
    }
    let rows = [
        Row {
            case: "a calculated check over one child",
            members: "<HealthThreshold>1</HealthThreshold><ChildHealthChecks>\
                      <ChildHealthCheck>{child}</ChildHealthCheck></ChildHealthChecks>",
        },
        Row {
            case: "a calculated check naming no children at all",
            members: "<HealthThreshold>0</HealthThreshold>",
        },
        Row {
            case: "a calculated check over the same child twice, at threshold two",
            members: "<HealthThreshold>2</HealthThreshold><ChildHealthChecks>\
                      <ChildHealthCheck>{child}</ChildHealthCheck>\
                      <ChildHealthCheck>{child}</ChildHealthCheck></ChildHealthChecks>",
        },
        Row {
            case: "a calculated check whose own checks are disabled",
            members: "<HealthThreshold>1</HealthThreshold><Disabled>true</Disabled>\
                      <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                      </ChildHealthChecks>",
        },
        Row {
            case: "an inverted calculated check",
            members: "<HealthThreshold>1</HealthThreshold><Inverted>true</Inverted>\
                      <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                      </ChildHealthChecks>",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = router();
        let child = child_check(&router, &format!("calculated-child-{index}")).await;
        let created = create_health_check(
            &router,
            &format!("calculated-read-{index}"),
            &format!("<Type>CALCULATED</Type>{}", row.members.replace("{child}", &child)),
        )
        .await;
        let id = health_check_id(&created, row.case);
        let population = health_check_count(&router, row.case).await;

        for reader in READERS {
            let case = format!("{} through {reader}", row.case);
            let reply = read_through(&router, reader, &id).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidInput",
                "case {case:?}: expected the InvalidInput code, got body {}",
                reply.body
            );
            let got = reply.first("Error/Message").unwrap_or("");
            assert_eq!(
                got, CALCULATED_HEALTH_CHECK_OBSERVATIONS_MESSAGE,
                "case {case:?}: expected the refusal to state why a calculated check has no \
                 observations, got {got:?}; body {}",
                reply.body
            );
            // A refusal reports no observation at all.
            assert!(
                reply.all("HealthCheckObservation/Region").is_empty(),
                "case {case:?}: a refusal carries no observation, got {:?}; body {}",
                reply.all("HealthCheckObservation/Region"),
                reply.body
            );
        }

        // The refusals changed nothing: the health check is still there, and the
        // child it names is still readable through both readers.
        let counted = health_check_count(&router, row.case).await;
        assert_eq!(
            counted, population,
            "case {:?}: a refused read must leave the account's health checks as they were, \
             GetHealthCheckCount reports {counted}",
            row.case
        );
        for reader in READERS {
            let reply = read_through(&router, reader, &child).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: the child endpoint check must still be readable through {reader}, \
                 got {} with body {}",
                row.case, reply.status, reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_health_check_with_checkers_or_of_another_checkerless_type_is_served() {
    // The neighbouring classes: a type that probes an endpoint has checkers and
    // is answered with their observations, and the other two types that probe no
    // endpoint are answered with an EMPTY list — the refusal belongs to the
    // calculated type alone, so a reader that refused by "has no observations"
    // would fail these rows.
    struct Row {
        case: &'static str,
        config: &'static str,
        /// Whether the answer carries at least one observation.
        observed: bool,
    }
    let rows = [
        Row {
            case: "an HTTP check, which checkers probe",
            config: "<Type>HTTP</Type><IPAddress>192.0.2.65</IPAddress><Port>80</Port>",
            observed: true,
        },
        Row {
            case: "a TCP check, which checkers probe",
            config: "<Type>TCP</Type><IPAddress>192.0.2.66</IPAddress><Port>443</Port>",
            observed: true,
        },
        Row {
            case: "a CLOUDWATCH_METRIC check, which reads an alarm",
            config: "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
                     <Name>endpoint-unhealthy</Name></AlarmIdentifier>",
            observed: false,
        },
        Row {
            case: "a RECOVERY_CONTROL check, which follows a routing control",
            config: "<Type>RECOVERY_CONTROL</Type><RoutingControlArn>\
                     arn:aws:route53-recovery-control::111122223333:controlpanel/\
                     0123456789abcdef/routingcontrol/abcdef0123456789</RoutingControlArn>",
            observed: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = router();
        let created =
            create_health_check(&router, &format!("served-read-{index}"), row.config).await;
        let id = health_check_id(&created, row.case);
        for reader in READERS {
            let case = format!("{} through {reader}", row.case);
            let reply = read_through(&router, reader, &id).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            if row.observed && *reader == "GetHealthCheckStatus" {
                assert!(
                    !reply.all("HealthCheckObservation/Region").is_empty(),
                    "case {case:?}: a checked endpoint has checkers reporting on it; body {}",
                    reply.body
                );
            }
            if !row.observed {
                assert!(
                    reply.all("HealthCheckObservation/Region").is_empty(),
                    "case {case:?}: a check nothing probes has no observation, got {:?}; body {}",
                    reply.all("HealthCheckObservation/Region"),
                    reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn an_id_that_names_no_health_check_is_still_the_not_found_refusal() {
    // The type gate reads the STORED configuration, so it can only apply to a
    // health check the account holds: an id that names none is the 404 it was
    // before, through both readers.
    let router = router();
    let child = child_check(&router, "calculated-404-child").await;
    let created = create_health_check(
        &router,
        "calculated-404",
        &format!(
            "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold><ChildHealthChecks>\
             <ChildHealthCheck>{child}</ChildHealthCheck></ChildHealthChecks>"
        ),
    )
    .await;
    let held = health_check_id(&created, "calculated-404");

    for reader in READERS {
        for id in ["00000000-1111-2222-3333-444444444444", "Z1D633PJN98FT9", "x"] {
            let case = format!("{id:?} through {reader}");
            let reply = read_through(&router, reader, id).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchHealthCheck",
                "case {case:?}: expected the NoSuchHealthCheck code, got body {}",
                reply.body
            );
        }
        // And the calculated check the store DOES hold is still the 400.
        let reply = read_through(&router, reader, &held).await;
        assert_eq!(
            reply.status, 400,
            "case {reader:?}: the calculated check the store holds is the input refusal, got \
             {} with body {}",
            reply.status, reply.body
        );
    }
}
