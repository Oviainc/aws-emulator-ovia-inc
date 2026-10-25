// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CreateHealthCheck`'s create-or-serve rule.
//!
//! The rule under test: `CreateHealthCheck` persists the health check its
//! `HealthCheckConfig` describes — with the members the service fills in already
//! applied — and answers out of that persisted record, so the `HealthCheck` the
//! create reports is the one `GetHealthCheck` serves afterwards. Because
//! `CallerReference` is the idempotency token, a request whose reference already
//! names a health check creates nothing: it is served from the stored record
//! when the configuration the service would store is the SAME one, and refused
//! with `HealthCheckAlreadyExists` when it differs — and the comparison is on
//! the configuration AS STORED, not on the members the request happened to
//! spell out, so a request that leaves out a member the service fills in and one
//! that sends that member's default value are the same request.
//!
//! The tables below therefore drive the create over a SET of inputs rather than
//! one request: pairs of requests that differ in which members they spell out
//! rather than in what they ask for, pairs that differ in what they ask for,
//! health-check types the service fills members in for and types it does not,
//! and `CallerReference` at both published bounds of its length and one past
//! each. Every row is proved by a READ through `GetHealthCheck` (and, for the
//! `Location` the create answers with, by following that URL), so a create is
//! never proved by its own answer alone.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::{
    DEFAULT_FAILURE_THRESHOLD, DEFAULT_REQUEST_INTERVAL, FIRST_HEALTH_CHECK_VERSION,
};

use crate::health_checks::{
    create_health_check, get_health_check, health_check_count, health_check_id,
};
use crate::hosted_zone_records::{call, router, Reply};

/// The error code a caller-reference collision carries (`HealthCheckAlreadyExists`,
/// HTTP 409 — the model's own classification for this operation).
const ALREADY_EXISTS: &str = "HealthCheckAlreadyExists";

/// The one input error this service's model declares (`InvalidInput`, HTTP 400).
const INVALID_INPUT: &str = "InvalidInput";

/// Every member of a `HealthCheck` and its `HealthCheckConfig` these tests
/// compare two answers by.
///
/// The member list is the `HealthCheckConfig` structure's own
/// (`smithy_query(query="shape",shape="HealthCheckConfig")`) plus the three
/// members `HealthCheck` itself carries; a member no row sets simply reads back
/// empty in both snapshots, which is still the comparison this rule needs: the
/// stored record must not move.
const COMPARED_MEMBERS: &[&str] = &[
    "HealthCheck/Id",
    "HealthCheck/CallerReference",
    "HealthCheck/HealthCheckVersion",
    "HealthCheckConfig/Type",
    "HealthCheckConfig/IPAddress",
    "HealthCheckConfig/Port",
    "HealthCheckConfig/FullyQualifiedDomainName",
    "HealthCheckConfig/ResourcePath",
    "HealthCheckConfig/SearchString",
    "HealthCheckConfig/RequestInterval",
    "HealthCheckConfig/FailureThreshold",
    "HealthCheckConfig/Inverted",
    "HealthCheckConfig/Disabled",
    "HealthCheckConfig/MeasureLatency",
    "HealthCheckConfig/EnableSNI",
    "HealthCheckConfig/HealthThreshold",
    "HealthCheckConfig/ChildHealthChecks/ChildHealthCheck",
    "HealthCheckConfig/AlarmIdentifier/Region",
    "HealthCheckConfig/AlarmIdentifier/Name",
    "HealthCheckConfig/InsufficientDataHealthStatus",
    "HealthCheckConfig/RoutingControlArn",
];

/// What a reply says about the health check it reports, as
/// `(member, every value it carries)` pairs — the comparable form of one answer.
fn snapshot(reply: &Reply) -> Vec<(&'static str, Vec<String>)> {
    COMPARED_MEMBERS
        .iter()
        .map(|member| {
            (
                *member,
                reply
                    .all(member)
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<String>>(),
            )
        })
        .collect()
}

/// What `GetHealthCheck` says about `id`, as a comparable snapshot.
async fn stored(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> Vec<(&'static str, Vec<String>)> {
    let read = get_health_check(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from GetHealthCheck, got {} with body {}",
        read.status, read.body
    );
    snapshot(&read)
}

/// Whether `id` has the form Route 53 assigns a health check: five
/// hyphen-separated groups of 8, 4, 4, 4 and 12 lowercase hexadecimal digits.
fn is_health_check_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8usize, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
        })
}

/// Two `CreateHealthCheck` requests sent under ONE `CallerReference`, and
/// whether the configuration the service stores for them is the same.
struct RepeatRow {
    case: &'static str,
    /// The `HealthCheckConfig` body of the first request.
    first: &'static str,
    /// The `HealthCheckConfig` body of the second request, sent under the same
    /// reference.
    repeat: &'static str,
    /// Whether the two describe the same stored configuration, which is what
    /// decides whether the repeat is served or refused.
    same_configuration: bool,
}

/// An HTTP endpoint check, as the rows below spell it out.
const HTTP_ENDPOINT: &str = "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>";

/// A CloudWatch-alarm check: a type the service fills no member in for, so two
/// requests for it are the same exactly when they spell out the same members.
const ALARM_CHECK: &str = "<Type>CLOUDWATCH_METRIC</Type>\
                           <AlarmIdentifier><Region>eu-west-1</Region>\
                           <Name>endpoint-unhealthy</Name></AlarmIdentifier>";

#[tokio::test]
async fn a_repeat_is_decided_on_the_configuration_the_service_would_store() {
    // The rows differ along what the rule generalises over: which members the
    // two requests spell out (the same ask, written two ways), what they ask for
    // (a different ask), and whether the type is one the service fills members
    // in for at all.
    let rows = [
        RepeatRow {
            case: "a repeat that leaves out the interval the first request sent as its default",
            first: "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                    <RequestInterval>30</RequestInterval>",
            repeat: HTTP_ENDPOINT,
            same_configuration: true,
        },
        RepeatRow {
            case: "a repeat that spells out the interval the first request left out",
            first: HTTP_ENDPOINT,
            repeat: "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                     <RequestInterval>30</RequestInterval>",
            same_configuration: true,
        },
        RepeatRow {
            case: "a repeat that spells out the failure threshold the first request left out",
            first: HTTP_ENDPOINT,
            repeat: "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                     <FailureThreshold>3</FailureThreshold>",
            same_configuration: true,
        },
        RepeatRow {
            case: "a repeat that spells out both members the service fills in",
            first: HTTP_ENDPOINT,
            repeat: "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                     <RequestInterval>30</RequestInterval>\
                     <FailureThreshold>3</FailureThreshold>",
            same_configuration: true,
        },
        RepeatRow {
            case: "a repeat of a type the service fills nothing in for",
            first: ALARM_CHECK,
            repeat: ALARM_CHECK,
            same_configuration: true,
        },
        RepeatRow {
            case: "a repeat that asks for an interval of its own",
            first: HTTP_ENDPOINT,
            repeat: "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                     <RequestInterval>10</RequestInterval>",
            same_configuration: false,
        },
        RepeatRow {
            case: "a repeat that leaves out an interval the first request chose",
            first: "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                    <RequestInterval>10</RequestInterval>",
            repeat: HTTP_ENDPOINT,
            same_configuration: false,
        },
        RepeatRow {
            case: "a repeat that moves the check to another endpoint",
            first: HTTP_ENDPOINT,
            repeat: "<Type>HTTP</Type><IPAddress>192.0.2.11</IPAddress><Port>80</Port>",
            same_configuration: false,
        },
        RepeatRow {
            case: "a repeat that changes the check's type and port",
            first: "<Type>HTTP</Type>\
                    <FullyQualifiedDomainName>probe.example.com</FullyQualifiedDomainName>\
                    <Port>80</Port>",
            repeat: "<Type>HTTPS</Type>\
                     <FullyQualifiedDomainName>probe.example.com</FullyQualifiedDomainName>\
                     <Port>443</Port>",
            same_configuration: false,
        },
        RepeatRow {
            case: "a repeat that adds a resource path the first request had none of",
            first: HTTP_ENDPOINT,
            repeat: "<Type>HTTP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>\
                     <ResourcePath>/healthz</ResourcePath>",
            same_configuration: false,
        },
        RepeatRow {
            case: "a repeat that reads another CloudWatch alarm",
            first: ALARM_CHECK,
            repeat: "<Type>CLOUDWATCH_METRIC</Type>\
                     <AlarmIdentifier><Region>eu-west-1</Region>\
                     <Name>endpoint-latency</Name></AlarmIdentifier>",
            same_configuration: false,
        },
    ];

    for row in &rows {
        // A router of its own, so one row's reference cannot be another's.
        let router = router();
        let created = create_health_check(&router, "one-reference", row.first).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from the first CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = health_check_id(&created, row.case);
        let before = stored(&router, &id, row.case).await;

        let repeated = create_health_check(&router, "one-reference", row.repeat).await;
        if row.same_configuration {
            // The operation's success status is the model's own (201), and the
            // served repeat answers down the same path, so it is asserted
            // exactly rather than as a set of acceptable statuses.
            assert_eq!(
                repeated.status, 201,
                "case {:?}: the same reference and settings must be served, got {} with body {}",
                row.case, repeated.status, repeated.body
            );
            let repeated_id = health_check_id(&repeated, row.case);
            assert_eq!(
                repeated_id, id,
                "case {:?}: expected the health check that reference already names ({id}), \
                 got {repeated_id:?}; body {}",
                row.case, repeated.body
            );
            // The answer is the STORED record, so the repeat reports exactly
            // what a read of it reports.
            let repeat_answer = snapshot(&repeated);
            let after = stored(&router, &id, row.case).await;
            assert_eq!(
                repeat_answer, after,
                "case {:?}: the repeat must report the stored health check, got \
                 {repeat_answer:?} against {after:?}",
                row.case
            );
        } else {
            assert_eq!(
                repeated.status, 409,
                "case {:?}: expected 409, got {} with body {}",
                row.case, repeated.status, repeated.body
            );
            assert_eq!(
                repeated.error_code(),
                ALREADY_EXISTS,
                "case {:?}: expected {ALREADY_EXISTS}, got {:?}; body {}",
                row.case,
                repeated.error_code(),
                repeated.body
            );
        }

        // Either way the account still holds exactly the one health check, and
        // the stored record — its configuration AND its version — is untouched.
        let count = health_check_count(&router, row.case).await;
        assert_eq!(
            count, 1,
            "case {:?}: a repeated reference must not grow the account, got \
             HealthCheckCount {count}",
            row.case
        );
        let after = stored(&router, &id, row.case).await;
        assert_eq!(
            after, before,
            "case {:?}: the stored health check must be exactly as it was, got {after:?} \
             against {before:?}",
            row.case
        );
    }
}

/// One create, and the member of its configuration that makes it its own.
struct CreateRow {
    case: &'static str,
    config: &'static str,
    /// A `(member, value)` pair the stored health check must report, which no
    /// other row asks for.
    distinguishing: (&'static str, &'static str),
}

#[tokio::test]
async fn each_create_mints_its_own_health_check_and_is_answered_from_the_store() {
    // Rows differing by type, by the form the endpoint is given in, by port and
    // by which optional members they carry — so what is asserted is that each
    // create persisted ITS request rather than the previous one's.
    let rows = [
        CreateRow {
            case: "an HTTP check on an IPv4 address",
            config: "<Type>HTTP</Type><IPAddress>192.0.2.20</IPAddress><Port>80</Port>\
                     <ResourcePath>/status</ResourcePath>",
            distinguishing: ("HealthCheckConfig/ResourcePath", "/status"),
        },
        CreateRow {
            case: "an HTTPS check on a domain name",
            config: "<Type>HTTPS</Type>\
                     <FullyQualifiedDomainName>secure.example.net</FullyQualifiedDomainName>\
                     <Port>8443</Port><EnableSNI>true</EnableSNI>",
            distinguishing: ("HealthCheckConfig/Port", "8443"),
        },
        CreateRow {
            case: "a string-matching check on an IPv6 address",
            config: "<Type>HTTPS_STR_MATCH</Type>\
                     <IPAddress>2001:db8::1</IPAddress><Port>443</Port>\
                     <SearchString>SERVING</SearchString>",
            distinguishing: ("HealthCheckConfig/SearchString", "SERVING"),
        },
        CreateRow {
            case: "a TCP check with an interval and threshold of its own",
            config: "<Type>TCP</Type><IPAddress>192.0.2.21</IPAddress><Port>22</Port>\
                     <RequestInterval>10</RequestInterval>\
                     <FailureThreshold>7</FailureThreshold>",
            distinguishing: ("HealthCheckConfig/FailureThreshold", "7"),
        },
        CreateRow {
            case: "a routing-control follower",
            config: "<Type>RECOVERY_CONTROL</Type>\
                     <RoutingControlArn>arn:aws:route53-recovery-control::111122223333:\
                     controlpanel/0123456789abcdef/routingcontrol/fedcba9876543210\
                     </RoutingControlArn>",
            distinguishing: ("HealthCheckConfig/Type", "RECOVERY_CONTROL"),
        },
    ];

    let router = router();
    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        let caller_reference = format!("own-{}", ids.len());
        let created = create_health_check(&router, &caller_reference, row.config).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = health_check_id(&created, row.case);
        assert!(
            is_health_check_id(&id),
            "case {:?}: expected the id form Route 53 assigns a health check, got {id:?}",
            row.case
        );
        assert!(
            !ids.contains(&id),
            "case {:?}: every create must mint an id of its own, got {id:?} again",
            row.case
        );

        // The create's answer and the read's answer are the same record.
        let read = get_health_check(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHealthCheck, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            snapshot(&created),
            snapshot(&read),
            "case {:?}: the create must report the health check it persisted; create body {} \
             read body {}",
            row.case,
            created.body,
            read.body
        );
        let (member, expected) = row.distinguishing;
        let actual = read.first(member).unwrap_or("");
        assert_eq!(
            actual, expected,
            "case {:?}: expected {member} {expected:?}, got {actual:?}; body {}",
            row.case, read.body
        );
        assert_eq!(
            read.first("HealthCheck/CallerReference").unwrap_or(""),
            caller_reference,
            "case {:?}: the stored health check must keep the reference it was created \
             with; body {}",
            row.case,
            read.body
        );
        assert_eq!(
            read.first("HealthCheck/HealthCheckVersion").unwrap_or(""),
            FIRST_HEALTH_CHECK_VERSION.to_string(),
            "case {:?}: a health check nothing has updated carries the first version; body {}",
            row.case,
            read.body
        );

        // The `Location` the create answers with is a URL the caller can
        // follow: it addresses the health check this create made.
        let location = created.location.clone().unwrap_or_else(|| {
            panic!(
                "case {:?}: a create must answer a Location header; body {}",
                row.case, created.body
            )
        });
        let path = location
            .split_once("//")
            .and_then(|(_, rest)| rest.split_once('/'))
            .map(|(_, path)| format!("/{path}"))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: Location must be an absolute URL, got {location:?}",
                    row.case
                )
            });
        let followed = call(&router, "GET", &path, "").await;
        assert_eq!(
            followed.status, 200,
            "case {:?}: following Location {location:?} must reach the health check, got {} \
             with body {}",
            row.case, followed.status, followed.body
        );
        assert_eq!(
            followed.first("HealthCheck/Id").unwrap_or(""),
            id,
            "case {:?}: Location {location:?} must address the new health check; body {}",
            row.case,
            followed.body
        );

        ids.push(id);
    }

    // Every row is still readable, and reports its own distinguishing member:
    // a later create did not overwrite an earlier one's record.
    for (row, id) in rows.iter().zip(&ids) {
        let read = get_health_check(&router, id).await;
        let (member, expected) = row.distinguishing;
        let actual = read.first(member).unwrap_or("");
        assert_eq!(
            actual, expected,
            "case {:?}: after every create, expected {member} {expected:?}, got {actual:?}; \
             body {}",
            row.case, read.body
        );
    }
    let count = health_check_count(&router, "every row created").await;
    let expected = i64::try_from(ids.len()).expect("the population fits an i64");
    assert_eq!(
        count, expected,
        "every create must be counted: expected HealthCheckCount {expected}, got {count}"
    );
}

#[tokio::test]
async fn an_endpoint_check_that_asks_for_neither_interval_nor_threshold_is_stored_with_both() {
    // The two members the service fills in are stored, not merely reported: a
    // read of the health check carries them, and so does the repeat of the
    // create that spells them out (asserted by the repeat table above).
    let router = router();
    let created = create_health_check(
        &router,
        "filled-in",
        "<Type>HTTP</Type>\
         <FullyQualifiedDomainName>defaults.example.com</FullyQualifiedDomainName>",
    )
    .await;
    let id = health_check_id(&created, "a check that asks for neither member");
    let read = get_health_check(&router, &id).await;
    for (member, expected) in [
        (
            "HealthCheckConfig/RequestInterval",
            DEFAULT_REQUEST_INTERVAL.to_string(),
        ),
        (
            "HealthCheckConfig/FailureThreshold",
            DEFAULT_FAILURE_THRESHOLD.to_string(),
        ),
    ] {
        let actual = read.first(member).unwrap_or("");
        assert_eq!(
            actual, expected,
            "case {:?}: expected {member} {expected:?}, got {actual:?}; body {}",
            "a check that asks for neither member", read.body
        );
    }
}

/// One `CallerReference` length, and whether the service takes it.
struct ReferenceRow {
    case: &'static str,
    length: usize,
    accepted: bool,
}

#[tokio::test]
async fn the_caller_reference_is_taken_inside_its_published_length_and_refused_outside_it() {
    // `CreateHealthCheck$CallerReference` targets `HealthCheckNonce`, length
    // 1..64 (smithy_query(query="operation",operation="CreateHealthCheck")), so
    // the rows are both published bounds and one past each.
    let rows = [
        ReferenceRow {
            case: "a reference of no characters, one below the minimum",
            length: 0,
            accepted: false,
        },
        ReferenceRow {
            case: "a reference of one character, the published minimum",
            length: 1,
            accepted: true,
        },
        ReferenceRow {
            case: "a reference of 63 characters, inside the published range",
            length: 63,
            accepted: true,
        },
        ReferenceRow {
            case: "a reference of 64 characters, the published maximum",
            length: 64,
            accepted: true,
        },
        ReferenceRow {
            case: "a reference of 65 characters, one past the maximum",
            length: 65,
            accepted: false,
        },
    ];

    for row in &rows {
        let router = router();
        let caller_reference = "r".repeat(row.length);
        let created = create_health_check(
            &router,
            &caller_reference,
            "<Type>TCP</Type><IPAddress>192.0.2.30</IPAddress><Port>443</Port>",
        )
        .await;
        if row.accepted {
            let id = health_check_id(&created, row.case);
            let read = get_health_check(&router, &id).await;
            let stored_reference = read.first("HealthCheck/CallerReference").unwrap_or("");
            assert_eq!(
                stored_reference, caller_reference,
                "case {:?}: the health check must keep the reference it was created with, \
                 got {stored_reference:?}; body {}",
                row.case, read.body
            );
            let count = health_check_count(&router, row.case).await;
            assert_eq!(
                count, 1,
                "case {:?}: the accepted create must be counted, got HealthCheckCount {count}",
                row.case
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {:?}: expected 400, got {} with body {}",
                row.case, created.status, created.body
            );
            assert_eq!(
                created.error_code(),
                INVALID_INPUT,
                "case {:?}: expected {INVALID_INPUT}, got {:?}; body {}",
                row.case,
                created.error_code(),
                created.body
            );
            let count = health_check_count(&router, row.case).await;
            assert_eq!(
                count, 0,
                "case {:?}: a refused create must persist nothing, got HealthCheckCount {count}",
                row.case
            );
        }
    }
}
