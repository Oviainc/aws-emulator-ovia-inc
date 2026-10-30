// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `HealthCheckConfig` members the service decides
//! for a request that leaves them out.
//!
//! The rule under test: when Route 53 stores a health check it MATERIALISES
//! every `HealthCheckConfig` member whose behaviour the service decides on the
//! caller's behalf — with the value that behaviour amounts to for the check's
//! `Type` — and reports those members on every read of the check, while a member
//! the type has no behaviour for stays absent. So a `Type` that probes an
//! endpoint carries `MeasureLatency` and `EnableSNI` (SNI being what a probe
//! that negotiates TLS does unless it is turned off), every type carries
//! `Inverted` and `Disabled`, and the `CALCULATED` type — whose health is a
//! function of its child list — carries the `ChildHealthChecks` container even
//! when it names no child. A value the request DID spell out is never replaced.
//!
//! The tables below therefore drive the rule over the whole `HealthCheckType`
//! catalog (checked against the model's own value list, so a type the table
//! forgets fails rather than going unasserted) rather than over the one type the
//! defect was noticed on, and over both forms of each boolean — left out and
//! spelled out. Every row is proved by a READ through `GetHealthCheck` as well
//! as by the create's own answer, so a member the create reports but does not
//! store fails the test.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::HEALTH_CHECK_TYPE_VALUES;

use crate::health_check_updates::update_health_check;
use crate::health_checks::{create_health_check, get_health_check, health_check_id};
use crate::hosted_zone_records::{router, Reply};

/// Where a reply reports one child of a `CALCULATED` health check.
const CHILD_PATH: &str = "HealthCheckConfig/ChildHealthChecks/ChildHealthCheck";

/// A health check of the account's, for a row that needs one to name as a child.
async fn child_health_check(router: &MultiServiceRouter, caller_reference: &str) -> String {
    let created = create_health_check(
        router,
        caller_reference,
        "<Type>TCP</Type><IPAddress>192.0.2.200</IPAddress><Port>443</Port>",
    )
    .await;
    health_check_id(&created, caller_reference)
}

/// Whether `body` carries a `ChildHealthChecks` container holding no child.
///
/// An empty element reaches the wire in either of its two spellings, and both
/// say the same thing: the container is there and it names nothing.
fn carries_an_empty_child_container(body: &str) -> bool {
    body.contains("<ChildHealthChecks/>")
        || body.contains("<ChildHealthChecks></ChildHealthChecks>")
}

/// One `HealthCheckType`, and what the service decides for a request of that
/// type that spells out none of the members below.
struct TypeRow {
    case: &'static str,
    /// The `HealthCheckType` this row is about.
    kind: &'static str,
    /// The rest of the `HealthCheckConfig`, as raw XML — the members this type
    /// needs and nothing else. `{child}` stands in for a health check of the
    /// account's.
    members: &'static str,
    /// The `(member path, value)` pairs the stored health check must report,
    /// none of which the request spells out.
    materialised: &'static [(&'static str, &'static str)],
    /// The member paths this type has no behaviour for, which stay absent.
    absent: &'static [&'static str],
    /// Whether the stored configuration carries a `ChildHealthChecks` container
    /// holding no child.
    empty_child_container: bool,
}

/// The whole `HealthCheckType` catalog, with the `CALCULATED` type at both forms
/// of its child list. Checked against the model's own value list below.
const ROWS: &[TypeRow] = &[
    TypeRow {
        case: "an HTTP probe",
        kind: "HTTP",
        members: "<IPAddress>192.0.2.100</IPAddress><Port>80</Port>",
        materialised: &[
            ("HealthCheckConfig/MeasureLatency", "false"),
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
            // The probe negotiates no TLS, so it sends no `client_hello` to put
            // a server name in.
            ("HealthCheckConfig/EnableSNI", "false"),
        ],
        absent: &[CHILD_PATH],
        empty_child_container: false,
    },
    TypeRow {
        case: "an HTTPS probe, which negotiates TLS",
        kind: "HTTPS",
        members: "<FullyQualifiedDomainName>tls.example.com</FullyQualifiedDomainName>\
                  <Port>443</Port>",
        materialised: &[
            ("HealthCheckConfig/MeasureLatency", "false"),
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
            ("HealthCheckConfig/EnableSNI", "true"),
        ],
        absent: &[CHILD_PATH],
        empty_child_container: false,
    },
    TypeRow {
        case: "an HTTP string-matching probe",
        kind: "HTTP_STR_MATCH",
        members: "<IPAddress>192.0.2.101</IPAddress><Port>8080</Port>\
                  <SearchString>healthy</SearchString>",
        materialised: &[
            ("HealthCheckConfig/MeasureLatency", "false"),
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
            ("HealthCheckConfig/EnableSNI", "false"),
        ],
        absent: &[CHILD_PATH],
        empty_child_container: false,
    },
    TypeRow {
        case: "an HTTPS string-matching probe, which negotiates TLS",
        kind: "HTTPS_STR_MATCH",
        members: "<FullyQualifiedDomainName>match.example.org</FullyQualifiedDomainName>\
                  <SearchString>SERVING</SearchString>",
        materialised: &[
            ("HealthCheckConfig/MeasureLatency", "false"),
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
            ("HealthCheckConfig/EnableSNI", "true"),
        ],
        absent: &[CHILD_PATH],
        empty_child_container: false,
    },
    TypeRow {
        case: "a TCP probe",
        kind: "TCP",
        members: "<IPAddress>2001:db8:85a3::abcd:1:2345</IPAddress><Port>22</Port>",
        materialised: &[
            ("HealthCheckConfig/MeasureLatency", "false"),
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
            ("HealthCheckConfig/EnableSNI", "false"),
        ],
        absent: &[CHILD_PATH],
        empty_child_container: false,
    },
    TypeRow {
        case: "a calculated check naming no child",
        kind: "CALCULATED",
        members: "<HealthThreshold>0</HealthThreshold>",
        materialised: &[
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
        ],
        // The check probes no endpoint of its own, so neither latency
        // measurement nor TLS negotiation is anything it does.
        absent: &[
            "HealthCheckConfig/MeasureLatency",
            "HealthCheckConfig/EnableSNI",
            CHILD_PATH,
        ],
        empty_child_container: true,
    },
    TypeRow {
        case: "a calculated check naming one child",
        kind: "CALCULATED",
        members: "<HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        materialised: &[
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
        ],
        absent: &[
            "HealthCheckConfig/MeasureLatency",
            "HealthCheckConfig/EnableSNI",
        ],
        empty_child_container: false,
    },
    TypeRow {
        case: "an alarm-reading check",
        kind: "CLOUDWATCH_METRIC",
        members: "<AlarmIdentifier><Region>eu-west-1</Region>\
                  <Name>endpoint-unhealthy</Name></AlarmIdentifier>",
        materialised: &[
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
        ],
        absent: &[
            "HealthCheckConfig/MeasureLatency",
            "HealthCheckConfig/EnableSNI",
            CHILD_PATH,
        ],
        empty_child_container: false,
    },
    TypeRow {
        case: "a routing-control follower",
        kind: "RECOVERY_CONTROL",
        members: "<RoutingControlArn>arn:aws:route53-recovery-control::111122223333:\
                  controlpanel/0123456789abcdef/routingcontrol/abcdef0123456789\
                  </RoutingControlArn>",
        materialised: &[
            ("HealthCheckConfig/Inverted", "false"),
            ("HealthCheckConfig/Disabled", "false"),
        ],
        absent: &[
            "HealthCheckConfig/MeasureLatency",
            "HealthCheckConfig/EnableSNI",
            CHILD_PATH,
        ],
        empty_child_container: false,
    },
];

#[tokio::test]
async fn the_table_below_covers_every_modelled_health_check_type() {
    // The catalog comes from the model's own `HealthCheckType` value list, so a
    // type added to the model is a row this suite is missing rather than a type
    // whose defaults go unasserted.
    for kind in HEALTH_CHECK_TYPE_VALUES {
        assert!(
            ROWS.iter().any(|row| row.kind == *kind),
            "case {kind:?}: every modelled HealthCheckType needs a row; the table covers {:?}",
            ROWS.iter().map(|row| row.kind).collect::<Vec<_>>()
        );
    }
    for row in ROWS {
        assert!(
            HEALTH_CHECK_TYPE_VALUES.contains(&row.kind),
            "case {:?}: {:?} is not a modelled HealthCheckType; the model carries {:?}",
            row.case,
            row.kind,
            HEALTH_CHECK_TYPE_VALUES
        );
    }
}

/// What `reply` says about one member path, as a comparable value.
fn reported(reply: &Reply, path: &str) -> Vec<String> {
    reply
        .all(path)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<String>>()
}

/// Assert `reply` reports exactly what `row` says the stored configuration
/// carries. `source` names which answer is being checked.
fn assert_row_is_reported(reply: &Reply, row: &TypeRow, source: &str) {
    for (path, expected) in row.materialised {
        let actual = reported(reply, path);
        assert_eq!(
            actual,
            vec![(*expected).to_string()],
            "case {:?}: expected {source} to report {path} {expected:?}, got {actual:?}; \
             body {}",
            row.case,
            reply.body
        );
    }
    for path in row.absent {
        let actual = reported(reply, path);
        assert!(
            actual.is_empty(),
            "case {:?}: expected {source} to report no {path}, got {actual:?}; body {}",
            row.case,
            reply.body
        );
    }
    assert_eq!(
        carries_an_empty_child_container(&reply.body),
        row.empty_child_container,
        "case {:?}: expected {source} to carry an empty ChildHealthChecks container: {}; \
         body {}",
        row.case,
        row.empty_child_container,
        reply.body
    );
}

#[tokio::test]
async fn a_create_stores_and_reports_the_members_its_type_decides() {
    for (index, row) in ROWS.iter().enumerate() {
        // A router of its own, so one row's children and references cannot be
        // another's.
        let router = router();
        let child = child_health_check(&router, &format!("child-{index}")).await;
        let config = format!("<Type>{}</Type>{}", row.kind, row.members).replace("{child}", &child);
        let created = create_health_check(&router, &format!("defaults-{index}"), &config).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        assert_row_is_reported(&created, row, "the create");

        // The members are STORED, not merely reported: the read answers the same
        // thing the create did.
        let id = health_check_id(&created, row.case);
        let read = get_health_check(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHealthCheck, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_row_is_reported(&read, row, "the read");

        // A row that names a child must report that child, so the empty
        // container above is the absence of children rather than the member
        // being dropped.
        if row.members.contains("{child}") {
            let listed = reported(&read, CHILD_PATH);
            assert_eq!(
                listed,
                vec![child.clone()],
                "case {:?}: expected the read to report the child the request named, got \
                 {listed:?}; body {}",
                row.case,
                read.body
            );
        }
    }
}

/// One member the caller spelled out, and the value it must keep.
struct GivenRow {
    case: &'static str,
    /// The whole `HealthCheckConfig`, carrying the member this row sets.
    config: &'static str,
    /// Where the reply reports it, and the value the request asked for.
    member: (&'static str, &'static str),
}

/// A request that names a member the service would otherwise decide keeps the
/// value it sent — including the value that happens to be the default, and the
/// value opposite to it.
const GIVEN: &[GivenRow] = &[
    GivenRow {
        case: "an inverted probe",
        config: "<Type>HTTP</Type><IPAddress>192.0.2.110</IPAddress><Port>80</Port>\
                 <Inverted>true</Inverted>",
        member: ("HealthCheckConfig/Inverted", "true"),
    },
    GivenRow {
        case: "a probe created stopped",
        config: "<Type>HTTP</Type><IPAddress>192.0.2.111</IPAddress><Port>80</Port>\
                 <Disabled>true</Disabled>",
        member: ("HealthCheckConfig/Disabled", "true"),
    },
    GivenRow {
        case: "a probe measuring latency",
        config: "<Type>TCP</Type><IPAddress>192.0.2.112</IPAddress><Port>22</Port>\
                 <MeasureLatency>true</MeasureLatency>",
        member: ("HealthCheckConfig/MeasureLatency", "true"),
    },
    GivenRow {
        case: "a TLS probe with SNI turned off",
        config: "<Type>HTTPS</Type>\
                 <FullyQualifiedDomainName>no-sni.example.com</FullyQualifiedDomainName>\
                 <Port>443</Port><EnableSNI>false</EnableSNI>",
        member: ("HealthCheckConfig/EnableSNI", "false"),
    },
    GivenRow {
        case: "a TLS string-matching probe with SNI turned off",
        config: "<Type>HTTPS_STR_MATCH</Type>\
                 <FullyQualifiedDomainName>no-sni.example.org</FullyQualifiedDomainName>\
                 <SearchString>SERVING</SearchString><EnableSNI>false</EnableSNI>",
        member: ("HealthCheckConfig/EnableSNI", "false"),
    },
    GivenRow {
        case: "a non-TLS probe with SNI asked for",
        config: "<Type>HTTP</Type><IPAddress>192.0.2.113</IPAddress><Port>80</Port>\
                 <EnableSNI>true</EnableSNI>",
        member: ("HealthCheckConfig/EnableSNI", "true"),
    },
    GivenRow {
        case: "a probe that spells out the value the service would have decided",
        config: "<Type>HTTP</Type><IPAddress>192.0.2.114</IPAddress><Port>80</Port>\
                 <Inverted>false</Inverted>",
        member: ("HealthCheckConfig/Inverted", "false"),
    },
    GivenRow {
        case: "an inverted alarm reader",
        config: "<Type>CLOUDWATCH_METRIC</Type>\
                 <AlarmIdentifier><Region>us-east-1</Region>\
                 <Name>endpoint-unhealthy</Name></AlarmIdentifier>\
                 <Inverted>true</Inverted>",
        member: ("HealthCheckConfig/Inverted", "true"),
    },
    GivenRow {
        case: "a calculated check created stopped",
        config: "<Type>CALCULATED</Type><HealthThreshold>0</HealthThreshold>\
                 <Disabled>true</Disabled>",
        member: ("HealthCheckConfig/Disabled", "true"),
    },
    GivenRow {
        case: "an inverted routing-control follower",
        config: "<Type>RECOVERY_CONTROL</Type>\
                 <RoutingControlArn>arn:aws:route53-recovery-control::111122223333:\
                 controlpanel/0123456789abcdef/routingcontrol/fedcba9876543210\
                 </RoutingControlArn><Inverted>true</Inverted>",
        member: ("HealthCheckConfig/Inverted", "true"),
    },
];

/// Assert `reply` reports `path` as exactly `expected`. `source` names which
/// answer is being checked.
fn assert_member(reply: &Reply, case: &str, source: &str, path: &str, expected: &str) {
    let actual = reported(reply, path);
    assert_eq!(
        actual,
        vec![expected.to_string()],
        "case {case:?}: expected {source} to report {path} {expected:?}, got {actual:?}; \
         body {}",
        reply.body
    );
}

#[tokio::test]
async fn a_member_the_request_spells_out_keeps_the_value_it_sent() {    for (index, row) in GIVEN.iter().enumerate() {
        let router = router();
        let created = create_health_check(&router, &format!("given-{index}"), row.config).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = health_check_id(&created, row.case);
        let (path, expected) = row.member;
        assert_member(&created, row.case, "the create", path, expected);
        let read = get_health_check(&router, &id).await;
        assert_member(&read, row.case, "the read", path, expected);
    }
}

#[tokio::test]
async fn the_child_container_of_a_calculated_check_follows_its_child_list() {
    // The container is a function of the check's TYPE, not of the request that
    // last touched it: it is there when the check names no child, holds the
    // children it is given, and is back to empty once the list is reset.
    let router = router();
    let child = child_health_check(&router, "container-child").await;
    let created = create_health_check(
        &router,
        "container",
        "<Type>CALCULATED</Type><HealthThreshold>0</HealthThreshold>",
    )
    .await;
    let id = health_check_id(&created, "a calculated check naming no child");
    assert!(
        carries_an_empty_child_container(&created.body),
        "case {:?}: the create must carry the empty container; body {}",
        "a calculated check naming no child",
        created.body
    );

    let with_child = update_health_check(
        &router,
        &id,
        &format!(
            "<HealthThreshold>1</HealthThreshold><ChildHealthChecks>\
             <ChildHealthCheck>{child}</ChildHealthCheck></ChildHealthChecks>"
        ),
    )
    .await;
    assert_eq!(
        with_child.status, 200,
        "case {:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
        "a child added to the list", with_child.status, with_child.body
    );
    let read = get_health_check(&router, &id).await;
    let listed = reported(&read, CHILD_PATH);
    assert_eq!(
        listed,
        vec![child.clone()],
        "case {:?}: expected the container to hold the child the update named, got {listed:?}; \
         body {}",
        "a child added to the list",
        read.body
    );
    assert!(
        !carries_an_empty_child_container(&read.body),
        "case {:?}: a container holding a child is not empty; body {}",
        "a child added to the list",
        read.body
    );

    let reset = update_health_check(
        &router,
        &id,
        "<ResetElements><ResettableElementName>ChildHealthChecks</ResettableElementName>\
         </ResetElements>",
    )
    .await;
    assert_eq!(
        reset.status, 200,
        "case {:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
        "the list reset", reset.status, reset.body
    );
    let read = get_health_check(&router, &id).await;
    let listed = reported(&read, CHILD_PATH);
    assert!(
        listed.is_empty(),
        "case {:?}: expected no child after the reset, got {listed:?}; body {}",
        "the list reset",
        read.body
    );
    assert!(
        carries_an_empty_child_container(&read.body),
        "case {:?}: the container is still reported after the reset; body {}",
        "the list reset",
        read.body
    );
}
