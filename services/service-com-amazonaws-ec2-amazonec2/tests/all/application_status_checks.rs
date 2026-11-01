// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeApplicationStatusChecks`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeApplicationStatusChecks`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REPORTS the application status check
//! configurations the store holds — "including configuration details such as protocol,
//! port, path, thresholds, and associations" — narrowed by the ids it was given ("If you do
//! not specify any application status check IDs, all checks in your account are returned"),
//! by the documented `aggregation` filter, and by `IncludeAll` ("If you omit this parameter
//! or set it to false, the response includes only active checks"), cut into
//! `MaxResults`-sized pages in `ApplicationStatusCheckId` order, and writing nothing.
//!
//! So the table below spans that class: each row is a different CONFIGURATION — which
//! aggregation, which protocol, which port (both published bounds of `PortNumber` and a
//! value between them), which thresholds, which grace period (both published bounds of
//! `InitializationGracePeriodSeconds`), how many health check paths with how many
//! destinations, which tags, which target tag associations, and whether the check is active
//! or inside its deletion grace period. The filter and the enum-valued members are driven
//! by the model's own catalogs (`AGGREGATION_STATUS_ENUM_VALUES`,
//! `NETWORK_PROTOCOL_ENUM_VALUES`, `IP_VERSION_ENUM_VALUES`, `IP_SCOPE_ENUM_VALUES`) rather
//! than by a value this emulator happens to produce.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ec2::ops::types::{
    AGGREGATION_STATUS_ENUM_VALUES, ApplicationStatusCheckResponseObject,
    CustomTagKeyValueResponsePair, HealthCheckPathDestinationResponseObject,
    HealthCheckPathResponseObject, HealthCheckPathSourceResponseObject, IP_SCOPE_ENUM_VALUES,
    IP_VERSION_ENUM_VALUES, NETWORK_PROTOCOL_ENUM_VALUES, Tag,
};
use ec2::state::{APPLICATION_STATUS_CHECK_ID_PREFIX, ComAmazonawsEc2Amazonec2State as Ec2State};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
// The XML tree readers every suite of this crate shares — "the child element named X", its
// text, and the entries of the set named X — rather than a fourth copy of them here.
use super::wire::{child, child_text, set_items};

/// The Region every call here is signed for. A check's configuration is a fact of the
/// check rather than of the Region, so one Region is used throughout.
const REGION: &str = "us-west-2";

/// A fixed instant the seeded checks are dated from: 2026-01-01T00:00:00Z as epoch seconds.
/// Fixed rather than relative to `SystemTime::now()`, so what a row expects is the same
/// whenever the suite runs.
const BASE_SECS: i64 = 1_767_225_600;

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `DescribeApplicationStatusChecks` call carrying `params`, as its HTTP status and
/// body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    ec2_query::call(router, REGION, "DescribeApplicationStatusChecks", params).await
}

/// One `DescribeApplicationStatusChecks` call that must succeed, as the rows it reported.
async fn described(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Answer {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    Answer {
        rows: rows(&body),
        next_token: ec2_query::element(&body, "nextToken"),
        body,
    }
}

// --- reading the response ---------------------------------------------------

/// A successful response: the checks it reported, its page token, and the body kept for
/// assertion messages.
struct Answer {
    rows: Vec<Row>,
    next_token: Option<String>,
    body: String,
}

impl Answer {
    /// The check ids reported, in the order the response reported them.
    fn ids(&self) -> Vec<String> {
        self.rows.iter().map(|row| row.id.clone()).collect()
    }

    /// The one row for `id`.
    fn row(&self, id: &str) -> &Row {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| {
                panic!(
                    "the response must report {id}, got {:?}: {}",
                    self.ids(),
                    self.body
                )
            })
    }
}

/// One reported `ApplicationStatusCheckResponseObject`, read out of the XML.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    aggregation: Option<String>,
    protocol: Option<String>,
    port: Option<String>,
    path: Option<String>,
    device_index: Option<String>,
    ip_version: Option<String>,
    ip_scope: Option<String>,
    interval: Option<String>,
    timeout: Option<String>,
    failure_threshold: Option<String>,
    success_threshold: Option<String>,
    status_code_matcher: Option<String>,
    grace: Option<String>,
    /// `(the source's subnet id, the destination subnet ids)` per health check path.
    paths: Vec<(Option<String>, Vec<String>)>,
    tags: Vec<(String, String)>,
    target_tags: Vec<(String, String)>,
    last_updated_secs: Option<i64>,
    creation_secs: Option<i64>,
    modify_secs: Option<i64>,
    deletion_secs: Option<i64>,
}

/// An ISO 8601 timestamp as epoch seconds — which also asserts that what the service wrote
/// parses as an instant at all.
fn secs_of(text: &str) -> i64 {
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{text:?} must be an ISO 8601 timestamp: {error}"))
        .secs()
}

/// The `(key, value)` pairs of one `key`/`value` item set.
fn pairs(node: roxmltree::Node<'_, '_>, set: &str) -> Vec<(String, String)> {
    set_items(node, set)
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every check the response reported, in document order.
fn rows(body: &str) -> Vec<Row> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response must be XML: {error}: {body}"));
    let root = document.root_element();
    set_items(root, "applicationStatusCheckSet")
        .into_iter()
        .map(|item| Row {
            id: child_text(item, "applicationStatusCheckId").unwrap_or_default(),
            aggregation: child_text(item, "aggregation"),
            protocol: child_text(item, "protocol"),
            port: child_text(item, "port"),
            path: child_text(item, "path"),
            device_index: child_text(item, "deviceIndex"),
            ip_version: child_text(item, "ipVersion"),
            ip_scope: child_text(item, "ipScope"),
            interval: child_text(item, "interval"),
            timeout: child_text(item, "timeout"),
            failure_threshold: child_text(item, "failureThreshold"),
            success_threshold: child_text(item, "successThreshold"),
            status_code_matcher: child_text(item, "statusCodeMatcher"),
            grace: child_text(item, "initializationGracePeriodSeconds"),
            paths: set_items(item, "healthCheckPathSet")
                .into_iter()
                .map(|path| {
                    (
                        child(path, "source").and_then(|source| child_text(source, "subnetId")),
                        set_items(path, "destinationSet")
                            .into_iter()
                            .map(|destination| {
                                child_text(destination, "subnetId").unwrap_or_default()
                            })
                            .collect(),
                    )
                })
                .collect(),
            tags: pairs(item, "tagSet"),
            target_tags: pairs(item, "targetTagAssociationSet"),
            last_updated_secs: child_text(item, "lastUpdatedAt").map(|at| secs_of(&at)),
            creation_secs: child_text(item, "creationTime").map(|at| secs_of(&at)),
            modify_secs: child_text(item, "modifyTime").map(|at| secs_of(&at)),
            deletion_secs: child_text(item, "deletionTime").map(|at| secs_of(&at)),
        })
        .collect()
}

// --- the cases --------------------------------------------------------------

/// One row of the configuration table: an application status check as it is stored, and so
/// as the describe must report it.
struct Case {
    label: &'static str,
    aggregation: &'static str,
    protocol: &'static str,
    port: i32,
    path: &'static str,
    device_index: i32,
    ip_version: &'static str,
    ip_scope: &'static str,
    interval: i32,
    timeout: i32,
    failure_threshold: i32,
    success_threshold: i32,
    status_code_matcher: &'static str,
    /// "Valid values: 1 to 600" (`InitializationGracePeriodSeconds`), whose modelled
    /// `@range` is -1..600 — so both published bounds appear below.
    grace: i32,
    /// `(the source's subnet id, the destination subnet ids)` per health check path.
    paths: Vec<(&'static str, Vec<&'static str>)>,
    tags: Vec<(&'static str, &'static str)>,
    target_tags: Vec<(&'static str, &'static str)>,
    /// `Some(the second it was deleted)` for a check inside its deletion grace period,
    /// which is the one `IncludeAll` decides about.
    deleted_secs: Option<i64>,
}

/// The configuration table. Rows differ along what the rule generalises over: the
/// aggregation setting, the protocol, the port (both published bounds of `PortNumber` and a
/// value between them), the path, the device index, the IP version and scope, the interval,
/// the timeout, the two thresholds, the status code matcher, the grace period (both
/// published bounds), how many health check paths carry how many destinations, which tags
/// and target tag associations the check holds, and whether it is active or deleted.
///
/// Every enum-valued member names a member of the model's own catalog for it, and between
/// them the rows cover every member of all four catalogs.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "included http on the lowest port, no paths, one tag",
            aggregation: AGGREGATION_STATUS_ENUM_VALUES[1],
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[0],
            // The published minimum of `PortNumber` (`range 1..65535`).
            port: 1,
            path: "/",
            // "The value is greater than or equal to 0" (`DeviceIndex`).
            device_index: 0,
            ip_version: IP_VERSION_ENUM_VALUES[0],
            ip_scope: IP_SCOPE_ENUM_VALUES[0],
            // "Valid value: 60" (`Interval`).
            interval: 60,
            // "Valid values: 1 to 30" (`Timeout`).
            timeout: 1,
            failure_threshold: 1,
            success_threshold: 1,
            status_code_matcher: "200",
            // The published minimum of `InitializationGracePeriodSeconds`.
            grace: -1,
            paths: Vec::new(),
            tags: vec![("Name", "checkout")],
            target_tags: Vec::new(),
            deleted_secs: None,
        },
        Case {
            label: "included https on the highest port, two paths, two target tags",
            aggregation: AGGREGATION_STATUS_ENUM_VALUES[1],
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[1],
            // The published maximum of `PortNumber`.
            port: 65_535,
            path: "/healthz",
            device_index: 1,
            ip_version: IP_VERSION_ENUM_VALUES[1],
            ip_scope: IP_SCOPE_ENUM_VALUES[0],
            interval: 60,
            // The published maximum of `Timeout`.
            timeout: 30,
            failure_threshold: 10,
            success_threshold: 5,
            status_code_matcher: "200-299,301",
            // The published maximum of `InitializationGracePeriodSeconds`.
            grace: 600,
            paths: vec![
                ("subnet-0000000000000aaa1", vec!["subnet-0000000000000bbb1"]),
                (
                    "subnet-0000000000000aaa2",
                    vec!["subnet-0000000000000bbb2", "subnet-0000000000000bbb3"],
                ),
            ],
            tags: Vec::new(),
            target_tags: vec![("Stage", "prod"), ("Tier", "web")],
            deleted_secs: None,
        },
        Case {
            label: "excluded http on a well-known port",
            aggregation: AGGREGATION_STATUS_ENUM_VALUES[0],
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[0],
            port: 80,
            path: "/status",
            device_index: 2,
            ip_version: IP_VERSION_ENUM_VALUES[0],
            ip_scope: IP_SCOPE_ENUM_VALUES[0],
            interval: 60,
            timeout: 5,
            failure_threshold: 3,
            success_threshold: 2,
            status_code_matcher: "200,204",
            grace: 0,
            paths: vec![("subnet-0000000000000aaa3", Vec::new())],
            tags: vec![("Name", "api"), ("Owner", "platform")],
            target_tags: Vec::new(),
            deleted_secs: None,
        },
        Case {
            label: "excluded https with a single status code",
            aggregation: AGGREGATION_STATUS_ENUM_VALUES[0],
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[1],
            port: 8_443,
            path: "/ping",
            device_index: 0,
            ip_version: IP_VERSION_ENUM_VALUES[1],
            ip_scope: IP_SCOPE_ENUM_VALUES[0],
            interval: 60,
            timeout: 15,
            failure_threshold: 2,
            success_threshold: 3,
            status_code_matcher: "204",
            grace: 30,
            paths: Vec::new(),
            tags: Vec::new(),
            target_tags: vec![("Name", "ping")],
            deleted_secs: None,
        },
        Case {
            label: "included http with many destinations on one path",
            aggregation: AGGREGATION_STATUS_ENUM_VALUES[1],
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[0],
            port: 8_080,
            path: "/alive",
            device_index: 3,
            ip_version: IP_VERSION_ENUM_VALUES[0],
            ip_scope: IP_SCOPE_ENUM_VALUES[0],
            interval: 60,
            timeout: 10,
            failure_threshold: 4,
            success_threshold: 4,
            status_code_matcher: "200-399",
            grace: 120,
            paths: vec![(
                "subnet-0000000000000aaa4",
                vec![
                    "subnet-0000000000000bbb4",
                    "subnet-0000000000000bbb5",
                    "subnet-0000000000000bbb6",
                ],
            )],
            tags: vec![("Name", "alive")],
            target_tags: Vec::new(),
            deleted_secs: None,
        },
        Case {
            label: "included https, deleted and inside its grace period",
            aggregation: AGGREGATION_STATUS_ENUM_VALUES[1],
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[1],
            port: 443,
            path: "/retired",
            device_index: 0,
            ip_version: IP_VERSION_ENUM_VALUES[0],
            ip_scope: IP_SCOPE_ENUM_VALUES[0],
            interval: 60,
            timeout: 20,
            failure_threshold: 6,
            success_threshold: 1,
            status_code_matcher: "200",
            grace: 60,
            paths: Vec::new(),
            tags: vec![("Name", "retired")],
            target_tags: Vec::new(),
            deleted_secs: Some(BASE_SECS + 10_000),
        },
        Case {
            label: "excluded http, deleted and inside its grace period",
            aggregation: AGGREGATION_STATUS_ENUM_VALUES[0],
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[0],
            port: 3_000,
            path: "/gone",
            device_index: 1,
            ip_version: IP_VERSION_ENUM_VALUES[1],
            ip_scope: IP_SCOPE_ENUM_VALUES[0],
            interval: 60,
            timeout: 25,
            failure_threshold: 2,
            success_threshold: 2,
            status_code_matcher: "200-204",
            grace: 90,
            paths: vec![("subnet-0000000000000aaa5", vec!["subnet-0000000000000bbb7"])],
            tags: Vec::new(),
            target_tags: Vec::new(),
            deleted_secs: Some(BASE_SECS + 20_000),
        },
    ]
}

/// One seeded check, as the modelled shape the store holds.
///
/// `index` spaces the three instants a check carries apart, so a response that reported one
/// of them in another's place would fail rather than coincide.
fn stored(id: &str, case: &Case, index: usize) -> ApplicationStatusCheckResponseObject {
    let offset = i64::try_from(index).unwrap_or_default() * 60;
    ApplicationStatusCheckResponseObject {
        application_status_check_id: Some(id.to_string()),
        aggregation: Some(case.aggregation.to_string()),
        health_check_paths: Some(
            case.paths
                .iter()
                .map(|(source, destinations)| HealthCheckPathResponseObject {
                    source: Some(HealthCheckPathSourceResponseObject {
                        subnet_id: Some((*source).to_string()),
                        security_group_id: Some("sg-0000000000000ccc1".to_string()),
                    }),
                    destinations: Some(
                        destinations
                            .iter()
                            .map(|destination| HealthCheckPathDestinationResponseObject {
                                subnet_id: Some((*destination).to_string()),
                                security_group_id: Some("sg-0000000000000ccc2".to_string()),
                            })
                            .collect(),
                    ),
                })
                .collect(),
        ),
        protocol: Some(case.protocol.to_string()),
        port: Some(case.port),
        path: Some(case.path.to_string()),
        device_index: Some(case.device_index),
        ip_version: Some(case.ip_version.to_string()),
        ip_scope: Some(case.ip_scope.to_string()),
        interval: Some(case.interval),
        timeout: Some(case.timeout),
        failure_threshold: Some(case.failure_threshold),
        success_threshold: Some(case.success_threshold),
        status_code_matcher: Some(case.status_code_matcher.to_string()),
        initialization_grace_period_seconds: Some(case.grace),
        last_updated_at: Some(DateTime::from_secs(BASE_SECS + 7_200 + offset)),
        target_tag_associations: Some(
            case.target_tags
                .iter()
                .map(|(key, value)| CustomTagKeyValueResponsePair {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect(),
        ),
        tags: Some(
            case.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect(),
        ),
        creation_time: Some(DateTime::from_secs(BASE_SECS + offset)),
        modify_time: Some(DateTime::from_secs(BASE_SECS + 3_600 + offset)),
        deletion_time: case.deleted_secs.map(DateTime::from_secs),
    }
}

/// The row a response must carry for a seeded check.
fn expected(id: &str, case: &Case, index: usize) -> Row {
    let offset = i64::try_from(index).unwrap_or_default() * 60;
    Row {
        id: id.to_string(),
        aggregation: Some(case.aggregation.to_string()),
        protocol: Some(case.protocol.to_string()),
        port: Some(case.port.to_string()),
        path: Some(case.path.to_string()),
        device_index: Some(case.device_index.to_string()),
        ip_version: Some(case.ip_version.to_string()),
        ip_scope: Some(case.ip_scope.to_string()),
        interval: Some(case.interval.to_string()),
        timeout: Some(case.timeout.to_string()),
        failure_threshold: Some(case.failure_threshold.to_string()),
        success_threshold: Some(case.success_threshold.to_string()),
        status_code_matcher: Some(case.status_code_matcher.to_string()),
        grace: Some(case.grace.to_string()),
        paths: case
            .paths
            .iter()
            .map(|(source, destinations)| {
                (
                    Some((*source).to_string()),
                    destinations
                        .iter()
                        .map(|destination| (*destination).to_string())
                        .collect(),
                )
            })
            .collect(),
        tags: case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
        target_tags: case
            .target_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
        last_updated_secs: Some(BASE_SECS + 7_200 + offset),
        creation_secs: Some(BASE_SECS + offset),
        modify_secs: Some(BASE_SECS + 3_600 + offset),
        deletion_secs: case.deleted_secs,
    }
}

/// The emulator with the table seeded into the store, and what was seeded: the id of each
/// case in table order, and the id of the one check that carries nothing but an id.
struct Fixture {
    router: MultiServiceRouter,
    cases: Vec<Case>,
    ids: Vec<String>,
    bare: String,
}

impl Fixture {
    fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        let cases = cases();
        let store = engine.state::<Ec2State>();
        let mut ids = Vec::with_capacity(cases.len());
        {
            let mut guard = store.lock();
            for (index, case) in cases.iter().enumerate() {
                // Minted the way every id of this service is, so the describe is exercised
                // against the id form it will actually hold.
                let id = guard.next_id(APPLICATION_STATUS_CHECK_ID_PREFIX);
                let check = stored(&id, case, index);
                guard.application_status_checks.insert(id.clone(), check);
                ids.push(id);
            }
        }
        // A check that carries nothing but its id: every optional member is absent, which is
        // what a describe must report as absent rather than invent a value for — and, having
        // no aggregation, it is a check the `aggregation` filter can never select.
        let bare = {
            let mut guard = store.lock();
            let id = guard.next_id(APPLICATION_STATUS_CHECK_ID_PREFIX);
            guard.application_status_checks.insert(
                id.clone(),
                ApplicationStatusCheckResponseObject {
                    application_status_check_id: Some(id.clone()),
                    ..ApplicationStatusCheckResponseObject::default()
                },
            );
            id
        };
        Self {
            router,
            cases,
            ids,
            bare,
        }
    }

    /// The ids of the ACTIVE seeded checks, in the id order the describe reports.
    fn active_ids(&self) -> Vec<String> {
        let mut active: Vec<String> = self
            .cases
            .iter()
            .zip(&self.ids)
            .filter(|(case, _)| case.deleted_secs.is_none())
            .map(|(_, id)| id.clone())
            .collect();
        active.push(self.bare.clone());
        active.sort();
        active
    }

    /// The ids of EVERY seeded check, in that same order.
    fn every_id(&self) -> Vec<String> {
        let mut every = self.ids.clone();
        every.push(self.bare.clone());
        every.sort();
        every
    }

    /// The ids of the seeded checks inside their deletion grace period.
    fn deleted_ids(&self) -> Vec<String> {
        let mut deleted: Vec<String> = self
            .cases
            .iter()
            .zip(&self.ids)
            .filter(|(case, _)| case.deleted_secs.is_some())
            .map(|(_, id)| id.clone())
            .collect();
        deleted.sort();
        deleted
    }

    /// The ids of the seeded ACTIVE checks whose `aggregation` is `aggregation`.
    fn active_ids_with_aggregation(&self, aggregation: &str) -> Vec<String> {
        let mut selected: Vec<String> = self
            .cases
            .iter()
            .zip(&self.ids)
            .filter(|(case, _)| case.deleted_secs.is_none() && case.aggregation == aggregation)
            .map(|(_, id)| id.clone())
            .collect();
        selected.sort();
        selected
    }
}

// --- the tests --------------------------------------------------------------

/// One row of the `IncludeAll` table: the call it makes, and the check ids that call must
/// report.
struct Selection {
    label: &'static str,
    params: Vec<(String, String)>,
    wanted: Vec<String>,
}

/// An account that holds no application status check is reported as holding none: HTTP 200
/// with an empty `applicationStatusCheckSet` and no page token — not an error, and not a
/// row invented for it.
#[tokio::test]
async fn an_account_with_no_checks_reports_an_empty_set() {
    let (router, _engine) = ec2_query::emulator();
    let answer = described(&router, "a fresh account", &[]).await;
    assert!(
        answer.rows.is_empty(),
        "case \"a fresh account\": expected no checks, got {:?} in body {}",
        answer.ids(),
        answer.body
    );
    assert_eq!(
        answer.next_token, None,
        "case \"a fresh account\": expected no page token, got body {}",
        answer.body
    );
}

/// Every member of every stored configuration round-trips: the describe reports what the
/// store holds, member for member, for each row of the table.
#[tokio::test]
async fn every_stored_member_is_reported() {
    let fixture = Fixture::new();
    // `IncludeAll` so the deleted rows of the table are covered here too; which rows
    // `IncludeAll` selects is asserted by its own test below.
    let answer = described(
        &fixture.router,
        "every member",
        &[p("IncludeAll", "true")],
    )
    .await;
    for (index, (case, id)) in fixture.cases.iter().zip(&fixture.ids).enumerate() {
        let label = case.label;
        let reported = answer.row(id);
        let wanted = expected(id, case, index);
        assert_eq!(
            *reported, wanted,
            "case {label:?}: expected {wanted:?}, got {reported:?} in body {}",
            answer.body
        );
    }

    // A check that holds nothing but an id reports nothing but an id: an absent member is
    // reported absent rather than defaulted to a value the store never held.
    let bare = answer.row(&fixture.bare);
    assert_eq!(
        *bare,
        Row {
            id: fixture.bare.clone(),
            aggregation: None,
            protocol: None,
            port: None,
            path: None,
            device_index: None,
            ip_version: None,
            ip_scope: None,
            interval: None,
            timeout: None,
            failure_threshold: None,
            success_threshold: None,
            status_code_matcher: None,
            grace: None,
            paths: Vec::new(),
            tags: Vec::new(),
            target_tags: Vec::new(),
            last_updated_secs: None,
            creation_secs: None,
            modify_secs: None,
            deletion_secs: None,
        },
        "case \"a check with nothing but an id\": got {bare:?} in body {}",
        answer.body
    );
}

/// "If you do not specify any application status check IDs, all checks in your account are
/// returned", and `IncludeAll` decides whether a deleted check inside its grace period is
/// one of them.
#[tokio::test]
async fn include_all_decides_whether_deleted_checks_are_reported() {
    let fixture = Fixture::new();
    let cases = vec![
        Selection {
            label: "IncludeAll omitted",
            params: Vec::new(),
            wanted: fixture.active_ids(),
        },
        Selection {
            label: "IncludeAll false",
            params: vec![p("IncludeAll", "false")],
            wanted: fixture.active_ids(),
        },
        Selection {
            label: "IncludeAll true",
            params: vec![p("IncludeAll", "true")],
            wanted: fixture.every_id(),
        },
    ];
    for Selection {
        label,
        params,
        wanted,
    } in cases
    {
        let answer = described(&fixture.router, label, &params).await;
        assert_eq!(
            answer.ids(),
            wanted,
            "case {label:?}: expected {wanted:?}, got {:?} in body {}",
            answer.ids(),
            answer.body
        );
    }

    // A deleted check named by id is still only reported when deleted checks are asked for:
    // `IncludeAll` selects which checks a response carries, whether or not the request also
    // narrowed by id.
    let deleted = fixture
        .deleted_ids()
        .first()
        .cloned()
        .expect("the table seeds a deleted check");
    let answer = described(
        &fixture.router,
        "a deleted id without IncludeAll",
        &[p("ApplicationStatusCheckId.1", deleted.clone())],
    )
    .await;
    assert!(
        answer.rows.is_empty(),
        "case \"a deleted id without IncludeAll\": expected no checks, got {:?} in body {}",
        answer.ids(),
        answer.body
    );
    let answer = described(
        &fixture.router,
        "a deleted id with IncludeAll",
        &[
            p("ApplicationStatusCheckId.1", deleted.clone()),
            p("IncludeAll", "true"),
        ],
    )
    .await;
    assert_eq!(
        answer.ids(),
        vec![deleted.clone()],
        "case \"a deleted id with IncludeAll\": expected only {deleted}, got {:?} in body {}",
        answer.ids(),
        answer.body
    );
}

/// "The IDs of the application status checks to describe": the ids narrow the account's
/// checks to those named, in id order whichever order they were named in — and an id the
/// account holds no check for is an error rather than an absent row.
#[tokio::test]
async fn ids_narrow_the_account_to_the_checks_named() {
    let fixture = Fixture::new();
    let active = fixture.active_ids();
    let first = active.first().cloned().expect("the table seeds a check");
    let last = active.last().cloned().expect("the table seeds a check");
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one id", vec![first.clone()], vec![first.clone()]),
        (
            "two ids, named in reverse",
            vec![last.clone(), first.clone()],
            vec![first.clone(), last.clone()],
        ),
        ("every active id", active.clone(), active.clone()),
        (
            "one id named twice",
            vec![first.clone(), first.clone()],
            vec![first.clone()],
        ),
    ];
    for (label, named, wanted) in cases {
        let params: Vec<(String, String)> = named
            .iter()
            .enumerate()
            .map(|(index, id)| p(&format!("ApplicationStatusCheckId.{}", index + 1), id.clone()))
            .collect();
        let answer = described(&fixture.router, label, &params).await;
        assert_eq!(
            answer.ids(),
            wanted,
            "case {label:?}: expected {wanted:?}, got {:?} in body {}",
            answer.ids(),
            answer.body
        );
    }

    // An id this service cannot resolve is refused, and WHICH refusal says why: an id of the
    // right form that names no check is `NotFound`, while a value that is no application
    // status check id at all is `Malformed`, answered before any check is looked up.
    for (unknown, code) in [
        ("asc-0123456789abcdef0", "InvalidApplicationStatusCheckId.NotFound"),
        ("asc-12345678", "InvalidApplicationStatusCheckId.NotFound"),
        (
            "not-an-application-status-check-id",
            "InvalidApplicationStatusCheckId.Malformed",
        ),
        ("asc-0123456789abcdefg", "InvalidApplicationStatusCheckId.Malformed"),
        ("appstatuscheck-0123456789abcdef0", "InvalidApplicationStatusCheckId.Malformed"),
    ] {
        let (status, body) = describe(
            &fixture.router,
            &[p("ApplicationStatusCheckId.1", unknown)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            code,
            "case {unknown:?}: expected the code {code:?}, got body {body}"
        );
        let message = ec2_query::error_message(&body);
        assert!(
            message.contains(unknown),
            "case {unknown:?}: expected the refusal to name the id, got {message:?}"
        );
    }
}

/// "`aggregation` – The aggregation setting. Valid values: `included` and `excluded`":
/// driven by the model's own `AggregationStatusEnum` catalog, with EC2's `*` wildcard and
/// the multi-value OR, and nothing else accepted as a filter name.
#[tokio::test]
async fn the_aggregation_filter_selects_on_the_stored_setting() {
    let fixture = Fixture::new();
    // Every member of the catalog, so a value the issue never cited is covered too.
    for aggregation in AGGREGATION_STATUS_ENUM_VALUES.iter().copied() {
        let wanted = fixture.active_ids_with_aggregation(aggregation);
        assert!(
            !wanted.is_empty(),
            "case {aggregation:?}: the table must seed a check with this aggregation"
        );
        let answer = described(
            &fixture.router,
            aggregation,
            &ec2_query::filters(&[("aggregation", &[aggregation])]),
        )
        .await;
        assert_eq!(
            answer.ids(),
            wanted,
            "case {aggregation:?}: expected {wanted:?}, got {:?} in body {}",
            answer.ids(),
            answer.body
        );
    }

    // "If you specify multiple values for a filter, the values are joined with an OR"
    // (`Filter$Values`) — both members of the catalog select every check that carries one,
    // which is every active check but the one with no aggregation at all.
    let both: Vec<&str> = AGGREGATION_STATUS_ENUM_VALUES.to_vec();
    let mut wanted = fixture.active_ids();
    wanted.retain(|id| *id != fixture.bare);
    let answer = described(
        &fixture.router,
        "both values",
        &ec2_query::filters(&[("aggregation", both.as_slice())]),
    )
    .await;
    assert_eq!(
        answer.ids(),
        wanted,
        "case \"both values\": expected {wanted:?}, got {:?} in body {}",
        answer.ids(),
        answer.body
    );

    // EC2's own `*` wildcard, matched against the stored word.
    let wanted = fixture.active_ids_with_aggregation("included");
    let answer = described(
        &fixture.router,
        "a wildcard",
        &ec2_query::filters(&[("aggregation", &["incl*"])]),
    )
    .await;
    assert_eq!(
        answer.ids(),
        wanted,
        "case \"a wildcard\": expected {wanted:?}, got {:?} in body {}",
        answer.ids(),
        answer.body
    );

    // A value no stored check carries selects nothing, rather than everything.
    let answer = described(
        &fixture.router,
        "a value nothing carries",
        &ec2_query::filters(&[("aggregation", &["sometimes"])]),
    )
    .await;
    assert!(
        answer.rows.is_empty(),
        "case \"a value nothing carries\": expected no checks, got {:?} in body {}",
        answer.ids(),
        answer.body
    );

    // A filter name this operation does not document is refused whether or not any check
    // would have matched it, so an unsupported filter can never read as "no results".
    for name in ["protocol", "tag:Name", "tag-key", "Aggregation", ""] {
        let (status, body) = describe(
            &fixture.router,
            &ec2_query::filters(&[(name, &["included"])]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: got body {body}"
        );
    }
}

/// "The maximum number of items to return for this request. To get the next page of items,
/// make another request with the token returned in the output": a page carries a token only
/// while a check follows it, and paging walks every check once, in id order.
#[tokio::test]
async fn pages_walk_every_check_in_id_order() {
    let fixture = Fixture::new();
    let every = fixture.every_id();

    // The published minimum page size (`DescribeApplicationStatusChecksMaxResults`,
    // `range 5..100`), which is below the number of checks seeded, and the published
    // maximum, which is above it.
    for page_size in [5_usize, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string()), p("IncludeAll", "true")];
            if let Some(carried) = token.clone() {
                params.push(p("NextToken", carried));
            }
            let label = format!("page size {page_size}");
            let answer = described(&fixture.router, &label, &params).await;
            assert!(
                answer.rows.len() <= page_size,
                "case {label:?}: expected at most {page_size} checks, got {:?}",
                answer.ids()
            );
            walked.extend(answer.ids());
            token = answer.next_token.clone();
            pages += 1;
            assert!(
                pages <= every.len() + 1,
                "case {label:?}: paging must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, every,
            "case \"page size {page_size}\": expected {every:?}, got {walked:?}"
        );
        let expected_pages = every.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case \"page size {page_size}\": expected {expected_pages} pages, got {pages}"
        );
    }

    // One below the published minimum and one above the published maximum are refused
    // before the handler runs, by the boundary validator the model's `@range` drives.
    for page_size in ["4", "101"] {
        let (status, body) = describe(&fixture.router, &[p("MaxResults", page_size)]).await;
        assert_eq!(
            status, 400,
            "case {page_size:?}: expected HTTP 400, got {status} with body {body}"
        );
    }

    // A token this service did not mint is refused rather than read as an id.
    let (status, body) = describe(&fixture.router, &[p("NextToken", "deadbeef")]).await;
    assert_eq!(
        status, 400,
        "case \"a forged token\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "InvalidPaginationToken",
        "case \"a forged token\": got body {body}"
    );
}

/// A dry run reports the permission outcome and reads nothing, and describing checks twice
/// reports the same thing both times: this operation writes nothing a later caller could
/// see.
#[tokio::test]
async fn a_dry_run_is_refused_and_a_read_changes_nothing() {
    let fixture = Fixture::new();
    let (status, body) = describe(
        &fixture.router,
        &[p("DryRun", "true"), p("IncludeAll", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case \"a dry run\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "DryRunOperation",
        "case \"a dry run\": got body {body}"
    );

    let first = described(&fixture.router, "the first read", &[p("IncludeAll", "true")]).await;
    let second = described(&fixture.router, "the second read", &[p("IncludeAll", "true")]).await;
    assert_eq!(
        first.rows, second.rows,
        "case \"two reads\": a read must not change what a later caller sees, got {:?} then {:?}",
        first.ids(),
        second.ids()
    );
}

/// Every refusal this operation answers closes its envelope with the id of the request that
/// failed — `<Response><Errors><Error/></Errors><RequestID/></Response>`, EC2's own error
/// envelope — and each call carries its own, so two failures are distinguishable.
#[tokio::test]
async fn every_refusal_carries_its_own_request_id() {
    let fixture = Fixture::new();
    let refusals: Vec<(&str, Vec<(String, String)>)> = vec![
        ("a dry run", vec![p("DryRun", "true")]),
        (
            "an unknown id",
            vec![p("ApplicationStatusCheckId.1", "asc-0123456789abcdef0")],
        ),
        (
            "an undocumented filter",
            ec2_query::filters(&[("protocol", &["http"])]),
        ),
        ("a forged page token", vec![p("NextToken", "deadbeef")]),
    ];
    let mut seen: Vec<String> = Vec::new();
    for (label, params) in refusals {
        let (status, body) = describe(&fixture.router, &params).await;
        assert_ne!(
            status, 200,
            "case {label:?}: expected a refusal, got {status} with body {body}"
        );
        let request_id = ec2_query::element(&body, "RequestID").unwrap_or_else(|| {
            panic!("case {label:?}: the error envelope must carry a RequestID: {body}")
        });
        // The 8-4-4-4-12 lowercase-hex form an AWS request id takes, asserted the way the
        // suite that owns that element asserts it (`tests/all/public_ipv4_pools.rs`).
        let groups: Vec<usize> = request_id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "case {label:?}: expected the 8-4-4-4-12 grouping, got {request_id:?} in \
             body {body}"
        );
        assert!(
            request_id
                .chars()
                .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "case {label:?}: expected a lowercase hexadecimal request id, got \
             {request_id:?} in body {body}"
        );
        assert!(
            !seen.contains(&request_id),
            "case {label:?}: each response carries its own request id, got {request_id:?} \
             again after {seen:?}"
        );
        seen.push(request_id);
    }
}
