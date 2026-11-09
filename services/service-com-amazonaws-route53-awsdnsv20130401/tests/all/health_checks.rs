// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Route 53 health-check slice.
//!
//! The rule under test for `GetHealthCheckCount`: the operation reports the
//! number of health checks the account holds at the moment of the call, counted
//! from the service's state, so it moves with every create that persisted a
//! health check and with nothing else. The tables below therefore drive the
//! count over several populations (none, one, several) rather than one, and
//! check it against the health checks that are readable through
//! `GetHealthCheck` and against the usage `GetAccountLimit` reports for the same
//! entity kind.
//!
//! The creates those populations are built from span the whole
//! `HealthCheckType` catalog (taken from the model's own value list), because
//! what a health check IS varies by type: an endpoint probe, a CloudWatch alarm
//! reader, a combination of child checks or a routing control follower. Each row
//! is read back through `GetHealthCheck`, so a create is proved by a read rather
//! than by its own answer.
//!
//! Every test drives the service through the real router, using the same
//! harness the hosted-zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::HEALTH_CHECK_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    ACCOUNT_LIMIT_MAX_HEALTH_CHECKS, DEFAULT_FAILURE_THRESHOLD, DEFAULT_REQUEST_INTERVAL,
    FIRST_HEALTH_CHECK_VERSION,
};

use crate::hosted_zone_records::{call, router, Reply};

/// Send one `CreateHealthCheck` request, with `config` as the body of its
/// `HealthCheckConfig`.
///
/// `pub(crate)` so a sibling suite builds its health checks through the SAME
/// request rather than growing a second copy of it.
pub(crate) async fn create_health_check(
    router: &MultiServiceRouter,
    caller_reference: &str,
    config: &str,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/healthcheck",
        &format!(
            "<CreateHealthCheckRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <CallerReference>{caller_reference}</CallerReference>\
             <HealthCheckConfig>{config}</HealthCheckConfig>\
             </CreateHealthCheckRequest>"
        ),
    )
    .await
}

/// Read one health check back.
///
/// `pub(crate)` so a sibling suite proves its reads through the SAME reader.
pub(crate) async fn get_health_check(router: &MultiServiceRouter, id: &str) -> Reply {
    call(router, "GET", &format!("/2013-04-01/healthcheck/{id}"), "").await
}

/// The number of health checks `GetHealthCheckCount` reports.
///
/// `pub(crate)` so a sibling suite counts the same population through the SAME
/// operation.
pub(crate) async fn health_check_count(router: &MultiServiceRouter, case: &str) -> i64 {
    let reply = call(router, "GET", "/2013-04-01/healthcheckcount", "").await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHealthCheckCount, got {} with body {}",
        reply.status, reply.body
    );
    let reported = reply.first("HealthCheckCount").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the reply must carry HealthCheckCount; body {}",
            reply.body
        )
    });
    reported.parse::<i64>().unwrap_or_else(|_| {
        panic!(
            "case {case:?}: HealthCheckCount must be a number, got {reported:?}; body {}",
            reply.body
        )
    })
}

/// The health-check usage `GetAccountLimit` reports, which is the same
/// population counted through another operation.
async fn health_check_usage(router: &MultiServiceRouter, case: &str) -> i64 {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/accountlimit/{ACCOUNT_LIMIT_MAX_HEALTH_CHECKS}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetAccountLimit, got {} with body {}",
        reply.status, reply.body
    );
    let reported = reply.first("Count").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the reply must carry Count; body {}",
            reply.body
        )
    });
    reported.parse::<i64>().unwrap_or_else(|_| {
        panic!("case {case:?}: Count must be a number, got {reported:?}; body {}", reply.body)
    })
}

/// The `Id` of a reply carrying a `HealthCheck`.
///
/// `pub(crate)` so a sibling suite reads an id out of a reply the same way.
pub(crate) fn health_check_id(reply: &Reply, case: &str) -> String {
    assert!(
        reply.status == 201 || reply.status == 200,
        "case {case:?}: expected a successful health-check reply, got {} with body {}",
        reply.status,
        reply.body
    );
    reply
        .first("HealthCheck/Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry HealthCheck/Id; body {}",
                reply.body
            )
        })
        .to_string()
}

/// One row of the health-check type catalog: a value of `HealthCheckType` and a
/// configuration of that type the service accepts.
///
/// `pub(crate)` so a sibling suite populates the account from the SAME catalog
/// rather than growing a second one that could drift from the model.
pub(crate) struct TypeRow {
    /// The `Type` member, which every other member is conditional on.
    pub(crate) kind: &'static str,
    /// The rest of the `HealthCheckConfig`, as raw XML. `{child}` is replaced
    /// with the id of a health check created first, for a type whose
    /// configuration names child health checks.
    pub(crate) members: &'static str,
    /// Whether the service fills in `RequestInterval` and `FailureThreshold`
    /// for a request of this type that leaves them out: it does so for the
    /// types that probe an endpoint, and `RequestInterval` / `FailureThreshold`
    /// are documented as unsupported for `RECOVERY_CONTROL`, while the alarm
    /// reader and the combination of child checks probe no endpoint at all.
    pub(crate) endpoint_probe: bool,
}

/// The whole `HealthCheckType` catalog. Checked against the model's own value
/// list below, so a type this table forgets fails the test rather than going
/// unasserted.
pub(crate) const CATALOG: &[TypeRow] = &[
    TypeRow {
        kind: "HTTP",
        members: "<IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <ResourcePath>/docs/route53-health-check.html</ResourcePath>",
        endpoint_probe: true,
    },
    TypeRow {
        kind: "HTTPS",
        members: "<FullyQualifiedDomainName>secure.example.com</FullyQualifiedDomainName>\
                  <Port>443</Port><EnableSNI>true</EnableSNI>",
        endpoint_probe: true,
    },
    TypeRow {
        kind: "HTTP_STR_MATCH",
        members: "<IPAddress>192.0.2.45</IPAddress><Port>8080</Port>\
                  <SearchString>healthy</SearchString>",
        endpoint_probe: true,
    },
    TypeRow {
        kind: "HTTPS_STR_MATCH",
        members: "<FullyQualifiedDomainName>match.example.org</FullyQualifiedDomainName>\
                  <SearchString>SERVING</SearchString>",
        endpoint_probe: true,
    },
    TypeRow {
        kind: "TCP",
        members: "<IPAddress>2001:db8:85a3::abcd:1:2345</IPAddress><Port>22</Port>",
        endpoint_probe: true,
    },
    TypeRow {
        kind: "CALCULATED",
        members: "<HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        endpoint_probe: false,
    },
    TypeRow {
        kind: "CLOUDWATCH_METRIC",
        members: "<AlarmIdentifier><Region>us-east-1</Region>\
                  <Name>endpoint-unhealthy</Name></AlarmIdentifier>\
                  <InsufficientDataHealthStatus>LastKnownStatus\
                  </InsufficientDataHealthStatus>",
        endpoint_probe: false,
    },
    TypeRow {
        kind: "RECOVERY_CONTROL",
        members: "<RoutingControlArn>arn:aws:route53-recovery-control::111122223333:\
                  controlpanel/0123456789abcdef/routingcontrol/abcdef0123456789\
                  </RoutingControlArn>",
        endpoint_probe: false,
    },
];

impl TypeRow {
    /// This row's `HealthCheckConfig` body, with `child` standing in for any
    /// child health check it names.
    pub(crate) fn config(&self, child: &str) -> String {
        format!(
            "<Type>{}</Type>{}",
            self.kind,
            self.members.replace("{child}", child)
        )
    }
}

/// Create the health check a row that names child health checks needs, and
/// return its id. A row that names none gets an empty id, which it does not use.
///
/// `pub(crate)` so a sibling suite builds the same catalog rows.
pub(crate) async fn child_for(router: &MultiServiceRouter, row: &TypeRow) -> String {
    if !row.members.contains("{child}") {
        return String::new();
    }
    let reply = create_health_check(
        router,
        &format!("child-of-{}", row.kind),
        "<Type>TCP</Type><IPAddress>192.0.2.60</IPAddress><Port>443</Port>",
    )
    .await;
    health_check_id(&reply, "the child of a CALCULATED health check")
}

#[tokio::test]
async fn the_table_below_covers_every_modelled_health_check_type() {
    // `HEALTH_CHECK_TYPE_VALUES` is the model's own `HealthCheckType` value
    // list, so this is what keeps the catalog above from drifting from it.
    let mut covered: Vec<&str> = CATALOG.iter().map(|row| row.kind).collect();
    covered.sort_unstable();
    let mut modelled: Vec<&str> = HEALTH_CHECK_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        covered, modelled,
        "every HealthCheckType must be covered: expected {modelled:?}, got {covered:?}"
    );
}

#[tokio::test]
async fn a_created_health_check_reads_back_as_it_was_created() {
    for row in CATALOG {
        let router = router();
        let child = child_for(&router, row).await;
        let caller_reference = format!("roundtrip-{}", row.kind);
        let created = create_health_check(&router, &caller_reference, &row.config(&child)).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.kind, created.status, created.body
        );
        let id = health_check_id(&created, row.kind);
        let location = created.location.clone().unwrap_or_else(|| {
            panic!(
                "case {:?}: a create must answer a Location header; body {}",
                row.kind, created.body
            )
        });
        assert!(
            location.ends_with(&format!("/2013-04-01/healthcheck/{id}")),
            "case {:?}: Location must address the new health check, got {location:?}",
            row.kind
        );

        // The create is proved by a READ: what the caller was told has to be
        // what a later GetHealthCheck tells it.
        let read = get_health_check(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHealthCheck, got {} with body {}",
            row.kind, read.status, read.body
        );
        for (path, expected) in [
            ("HealthCheck/Id", id.clone()),
            ("HealthCheck/CallerReference", caller_reference.clone()),
            ("HealthCheckConfig/Type", row.kind.to_string()),
            (
                "HealthCheck/HealthCheckVersion",
                FIRST_HEALTH_CHECK_VERSION.to_string(),
            ),
        ] {
            let actual = read.first(path).unwrap_or("");
            assert_eq!(
                actual, expected,
                "case {:?}: expected {path} {expected:?}, got {actual:?}; body {}",
                row.kind, read.body
            );
        }

        // The two members the service fills in for a request that leaves them
        // out, and only for the types that probe an endpoint.
        let interval = read.first("HealthCheckConfig/RequestInterval");
        let threshold = read.first("HealthCheckConfig/FailureThreshold");
        if row.endpoint_probe {
            assert_eq!(
                interval,
                Some(DEFAULT_REQUEST_INTERVAL.to_string()).as_deref(),
                "case {:?}: expected the default RequestInterval, got {interval:?}; body {}",
                row.kind,
                read.body
            );
            assert_eq!(
                threshold,
                Some(DEFAULT_FAILURE_THRESHOLD.to_string()).as_deref(),
                "case {:?}: expected the default FailureThreshold, got {threshold:?}; body {}",
                row.kind,
                read.body
            );
        } else {
            assert_eq!(
                (interval, threshold),
                (None, None),
                "case {:?}: a health check that probes no endpoint takes neither \
                 RequestInterval nor FailureThreshold; body {}",
                row.kind,
                read.body
            );
        }

        // A value the request DID carry is kept rather than defaulted away.
        if let Some(port) = read.first("HealthCheckConfig/Port") {
            assert!(
                row.members.contains(&format!("<Port>{port}</Port>")),
                "case {:?}: Port {port:?} is not the one the request sent; body {}",
                row.kind,
                read.body
            );
        }
    }
}

#[tokio::test]
async fn a_request_that_asks_for_its_own_interval_and_threshold_keeps_them() {
    // The defaults are what the service fills in for a member the request left
    // out; a request that names one keeps its own value. `FailureThreshold` is
    // `range 1..10`, so the rows below carry both of its published bounds and
    // one inside them, crossed with the two `RequestInterval` values the
    // service's request schema accepts (an interval outside that enumeration is
    // refused before any handler runs, which
    // `health_check_request_interval_enumeration` asserts).
    let router = router();
    for (index, (interval, threshold)) in [(10, 1), (30, 10), (10, 4), (30, 7)].iter().enumerate() {
        let case = format!("RequestInterval {interval}, FailureThreshold {threshold}");
        let created = create_health_check(
            &router,
            &format!("explicit-{index}"),
            &format!(
                "<Type>HTTP</Type><IPAddress>192.0.2.50</IPAddress><Port>80</Port>\
                 <RequestInterval>{interval}</RequestInterval>\
                 <FailureThreshold>{threshold}</FailureThreshold>"
            ),
        )
        .await;
        let id = health_check_id(&created, &case);
        let read = get_health_check(&router, &id).await;
        let reported_interval = read.first("HealthCheckConfig/RequestInterval").unwrap_or("");
        let reported_threshold = read.first("HealthCheckConfig/FailureThreshold").unwrap_or("");
        assert_eq!(
            (reported_interval, reported_threshold),
            (interval.to_string().as_str(), threshold.to_string().as_str()),
            "case {case:?}: the health check must keep the values it was created with, \
             got ({reported_interval:?}, {reported_threshold:?}); body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_count_follows_the_health_checks_the_account_holds() {
    let router = router();
    let mut created_ids: Vec<String> = Vec::new();

    // Several populations, not one: the count is asserted with no health
    // checks and after each create, over rows that differ by health-check type
    // rather than merely by name.
    for row in CATALOG {
        let expected = i64::try_from(created_ids.len()).expect("the population fits an i64");
        let case = format!("{expected} health check(s), before creating a {}", row.kind);
        let count = health_check_count(&router, &case).await;
        assert_eq!(
            count, expected,
            "case {case:?}: expected HealthCheckCount {expected}, got {count}"
        );
        // The same population, counted through the account's usage report.
        let usage = health_check_usage(&router, &case).await;
        assert_eq!(
            usage, expected,
            "case {case:?}: GetAccountLimit must report the same population, got {usage}"
        );
        // And every health check counted is one a caller can still read.
        for id in &created_ids {
            let read = get_health_check(&router, id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the counted health check {id} must still be readable, got {} \
                 with body {}",
                read.status, read.body
            );
        }

        let child = child_for(&router, row).await;
        if !child.is_empty() {
            created_ids.push(child.clone());
        }
        let created =
            create_health_check(&router, &format!("count-{}", row.kind), &row.config(&child)).await;
        created_ids.push(health_check_id(&created, &case));
    }

    let total = i64::try_from(created_ids.len()).expect("the population fits an i64");
    let count = health_check_count(&router, "every row created").await;
    assert_eq!(
        count, total,
        "case {:?}: expected HealthCheckCount {total}, got {count}",
        "every row created"
    );

    // A hosted zone is not a health check, so creating one does not move the
    // count.
    let zone = call(
        &router,
        "POST",
        "/2013-04-01/hostedzone",
        "<CreateHostedZoneRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <Name>health-check-count.example.com</Name>\
         <CallerReference>health-check-count-zone</CallerReference>\
         </CreateHostedZoneRequest>",
    )
    .await;
    assert_eq!(
        zone.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        zone.status, zone.body
    );
    let after_zone = health_check_count(&router, "after creating a hosted zone").await;
    assert_eq!(
        after_zone, total,
        "case {:?}: a hosted zone must not be counted as a health check, got {after_zone}",
        "after creating a hosted zone"
    );
}

#[tokio::test]
async fn reading_the_count_leaves_the_account_exactly_as_it_was() {
    // The rule is a report, so repeating it answers the same thing and creates
    // nothing: the count after any number of reads is still the population.
    let router = router();
    let created = create_health_check(
        &router,
        "count-is-a-report",
        "<Type>HTTP</Type><FullyQualifiedDomainName>report.example.com</FullyQualifiedDomainName>",
    )
    .await;
    let id = health_check_id(&created, "the only health check");

    let first = health_check_count(&router, "the first read").await;
    assert_eq!(first, 1, "one health check was created, got {first}");
    for attempt in 1..=3 {
        let case = format!("read {attempt}");
        let again = health_check_count(&router, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: a read must report what the first read did ({first}), got {again}"
        );
    }
    let read = get_health_check(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "reading the count must not destroy the health check, got {} with body {}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_repeated_caller_reference_is_served_or_refused_by_its_settings() {
    // `CallerReference` is the idempotency token: the same reference with the
    // same settings names the health check that reference already names, and
    // with different settings it is refused. Either way the population does not
    // grow, and a reference of its own creates a second health check even when
    // the settings are identical.
    let router = router();
    let config = "<Type>HTTP</Type><IPAddress>192.0.2.70</IPAddress><Port>80</Port>";
    let first = create_health_check(&router, "idempotent-ref", config).await;
    let id = health_check_id(&first, "the first create");
    assert_eq!(
        health_check_count(&router, "after the first create").await,
        1,
        "the first create must be counted"
    );

    let repeated = create_health_check(&router, "idempotent-ref", config).await;
    let repeated_id = health_check_id(&repeated, "the repeated create");
    assert_eq!(
        repeated_id, id,
        "case {:?}: the same reference and settings must name the health check that already \
         exists, got {repeated_id:?}; body {}",
        "repeated identically", repeated.body
    );
    assert_eq!(
        health_check_count(&router, "after the repeated create").await,
        1,
        "case {:?}: a repeat must not create a second health check",
        "repeated identically"
    );

    let differing = create_health_check(
        &router,
        "idempotent-ref",
        "<Type>HTTP</Type><IPAddress>192.0.2.71</IPAddress><Port>80</Port>",
    )
    .await;
    assert_eq!(
        differing.status, 409,
        "case {:?}: expected 409, got {} with body {}",
        "repeated with different settings", differing.status, differing.body
    );
    assert_eq!(
        differing.error_code(),
        "HealthCheckAlreadyExists",
        "case {:?}: expected HealthCheckAlreadyExists, got {:?}; body {}",
        "repeated with different settings",
        differing.error_code(),
        differing.body
    );
    assert_eq!(
        health_check_count(&router, "after the refused create").await,
        1,
        "case {:?}: a refused create must not be counted",
        "repeated with different settings"
    );

    let fresh = create_health_check(&router, "another-ref", config).await;
    let fresh_id = health_check_id(&fresh, "a fresh reference, identical settings");
    assert_ne!(
        fresh_id, id,
        "case {:?}: a unique reference must create a health check of its own",
        "a fresh reference, identical settings"
    );
    assert_eq!(
        health_check_count(&router, "after the second reference").await,
        2,
        "case {:?}: the second health check must be counted",
        "a fresh reference, identical settings"
    );
}

/// One configuration that does not hold together, and why.
struct RefusalRow {
    case: &'static str,
    config: &'static str,
}

#[tokio::test]
async fn a_configuration_that_does_not_hold_together_is_refused() {
    // Each row leaves out a member its `Type` needs, or carries one that type
    // does not take; none of them is a health check the service can run, so
    // none of them may be counted.
    let rows = [
        RefusalRow {
            case: "an HTTP check with neither IPAddress nor FullyQualifiedDomainName",
            config: "<Type>HTTP</Type><Port>80</Port>",
        },
        RefusalRow {
            case: "a TCP check with neither IPAddress nor FullyQualifiedDomainName",
            config: "<Type>TCP</Type><Port>443</Port>",
        },
        RefusalRow {
            case: "an HTTP_STR_MATCH check with no SearchString",
            config: "<Type>HTTP_STR_MATCH</Type><IPAddress>192.0.2.80</IPAddress>",
        },
        RefusalRow {
            case: "an HTTPS_STR_MATCH check with no SearchString",
            config: "<Type>HTTPS_STR_MATCH</Type>\
                     <FullyQualifiedDomainName>no-string.example.com</FullyQualifiedDomainName>",
        },
        RefusalRow {
            case: "a CLOUDWATCH_METRIC check with no AlarmIdentifier",
            config: "<Type>CLOUDWATCH_METRIC</Type>\
                     <InsufficientDataHealthStatus>Healthy</InsufficientDataHealthStatus>",
        },
        RefusalRow {
            case: "a RECOVERY_CONTROL check with no RoutingControlArn",
            config: "<Type>RECOVERY_CONTROL</Type><Inverted>false</Inverted>",
        },
        RefusalRow {
            case: "a CALCULATED check carrying a Port",
            config: "<Type>CALCULATED</Type><Port>80</Port>\
                     <HealthThreshold>0</HealthThreshold>",
        },
        RefusalRow {
            case: "a CALCULATED check naming a child the account does not hold",
            config: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                     <ChildHealthChecks>\
                     <ChildHealthCheck>8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c</ChildHealthCheck>\
                     </ChildHealthChecks>",
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let reply =
            create_health_check(&router, &format!("refused-{index}"), row.config).await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected HTTP 400, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {:?}: expected InvalidInput, got {:?}; body {}",
            row.case,
            reply.error_code(),
            reply.body
        );
        let count = health_check_count(&router, row.case).await;
        assert_eq!(
            count, 0,
            "case {:?}: a refused create must persist nothing, got HealthCheckCount {count}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_health_check_is_refused() {
    // A health check is addressed by the id the service assigned it, so no
    // other id resolves — including the id of another kind of resource and the
    // caller reference the health check was created with.
    let router = router();
    let created = create_health_check(
        &router,
        "lookup-reference",
        "<Type>HTTP</Type><IPAddress>192.0.2.90</IPAddress><Port>80</Port>",
    )
    .await;
    let id = health_check_id(&created, "the created health check");

    for case in [
        "8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c",
        "lookup-reference",
        "Z1D633PJN98FT9",
        &id.to_uppercase(),
    ] {
        if case == id {
            continue;
        }
        let reply = get_health_check(&router, case).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchHealthCheck",
            "case {case:?}: expected NoSuchHealthCheck, got {:?}; body {}",
            reply.error_code(),
            reply.body
        );
    }
}

#[tokio::test]
async fn tags_follow_the_health_check_they_were_put_on() {
    // A health check is a taggable resource (`TagResourceType` has
    // `healthcheck`), so once one exists its tags resolve against it rather
    // than reporting that no health check exists.
    let router = router();
    let created = create_health_check(
        &router,
        "tagged-health-check",
        "<Type>HTTPS</Type>\
         <FullyQualifiedDomainName>tagged.example.com</FullyQualifiedDomainName><Port>443</Port>",
    )
    .await;
    let id = health_check_id(&created, "the tagged health check");

    let listed = call(
        &router,
        "GET",
        &format!("/2013-04-01/tags/healthcheck/{id}"),
        "",
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "an existing health check must report its (empty) tag set, got {} with body {}",
        listed.status, listed.body
    );
    assert!(
        listed.all("Tag/Key").is_empty(),
        "a health check created without tags must carry none; body {}",
        listed.body
    );

    let changed = call(
        &router,
        "POST",
        &format!("/2013-04-01/tags/healthcheck/{id}"),
        "<ChangeTagsForResourceRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <AddTags><Tag><Key>Owner</Key><Value>dns-team</Value></Tag></AddTags>\
         </ChangeTagsForResourceRequest>",
    )
    .await;
    assert_eq!(
        changed.status, 200,
        "expected 200 from ChangeTagsForResource, got {} with body {}",
        changed.status, changed.body
    );

    let again = call(
        &router,
        "GET",
        &format!("/2013-04-01/tags/healthcheck/{id}"),
        "",
    )
    .await;
    assert_eq!(
        (
            again.first("ResourceId").unwrap_or(""),
            again.first("Tag/Key").unwrap_or(""),
            again.first("Tag/Value").unwrap_or("")
        ),
        (id.as_str(), "Owner", "dns-team"),
        "the tag must be readable off the health check it was put on; body {}",
        again.body
    );
}
