// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeApplicationStatus`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeApplicationStatus`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation DERIVES each instance's reported
//! application status from the per-check results the store holds for it, exactly as
//! `ApplicationStatus$Status` documents the aggregation — `impaired` for at least one
//! failed included check, `initializing` and then `insufficient-data` next, `ok` when
//! every included check passed, `not-applicable` when no included check applies, and
//! `suppressed` while suppression is in force — narrowed by `InstanceIds` and the two
//! documented filters and cut into `MaxResults`-sized pages.
//!
//! So the tables below span that class: each row is a different SET of seeded check
//! results (which aggregation, which check status, how many, in which order of time),
//! and the filter table is driven by the model's own `ApplicationStatusEnum` catalog
//! rather than by a value this emulator happens to produce.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ec2::ops::types::{
    AGGREGATION_STATUS_ENUM_VALUES, APPLICATION_STATUS_CHECK_ENUM_VALUES,
    APPLICATION_STATUS_ENUM_VALUES, ApplicationStatusDetail,
};
use ec2::state::{
    AGGREGATION_EXCLUDED, AGGREGATION_INCLUDED, APPLICATION_STATUS_CHECK_FAILED,
    APPLICATION_STATUS_CHECK_INITIALIZING, APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA,
    APPLICATION_STATUS_CHECK_NOT_APPLICABLE, APPLICATION_STATUS_CHECK_PASSED,
    APPLICATION_STATUS_IMPAIRED, APPLICATION_STATUS_INITIALIZING,
    APPLICATION_STATUS_INSUFFICIENT_DATA, APPLICATION_STATUS_NOT_APPLICABLE, APPLICATION_STATUS_OK,
    APPLICATION_STATUS_SUPPRESSED, ApplicationStatusSuppression,
    ComAmazonawsEc2Amazonec2State as Ec2State, InstanceApplicationStatusRecord,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;

/// The Region every call here is signed for; the zone names the launches land in are
/// derived from it.
const REGION: &str = "us-west-2";

/// The image every launch here names. Which AMI it is does not matter to an application
/// status, so one is used throughout.
const AMI: &str = "ami-0123456789abcdef0";

/// A fixed instant the seeded check results are dated from: 2026-01-01T00:00:00Z as epoch
/// seconds. Fixed rather than relative to `SystemTime::now()`, so what a row expects is
/// the same whenever the suite runs.
const BASE_SECS: i64 = 1_767_225_600;

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `DescribeApplicationStatus` call carrying `params`, as its HTTP status and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    ec2_query::call(router, REGION, "DescribeApplicationStatus", params).await
}

/// One `DescribeApplicationStatus` call that must succeed, as the rows it reported.
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

/// Launch one instance and report its id.
async fn launch_one(router: &MultiServiceRouter, extra: &[(String, String)]) -> String {
    let mut params = vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")];
    params.extend_from_slice(extra);
    let (status, body) = ec2_query::call(router, REGION, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    ec2_query::elements(&body, "instanceId")
        .pop()
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

// --- reading the response ---------------------------------------------------

/// A successful response: its rows and its page token, with the body kept for assertion
/// messages.
struct Answer {
    rows: Vec<Row>,
    next_token: Option<String>,
    body: String,
}

impl Answer {
    /// The instance ids reported, in the order the response reported them.
    fn instance_ids(&self) -> Vec<String> {
        self.rows.iter().map(|row| row.instance_id.clone()).collect()
    }

    /// The one row for `instance_id`.
    fn row(&self, instance_id: &str) -> &Row {
        self.rows
            .iter()
            .find(|row| row.instance_id == instance_id)
            .unwrap_or_else(|| {
                panic!(
                    "the response must report {instance_id}, got {:?}: {}",
                    self.instance_ids(),
                    self.body
                )
            })
    }
}

/// One `instanceSet/item` of a response: the members the assertions below read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    instance_id: String,
    availability_zone: Option<String>,
    availability_zone_id: Option<String>,
    status: Option<String>,
    status_time_stamp: Option<i64>,
    status_since: Option<i64>,
    resume_at: Option<i64>,
    /// The `ApplicationStatusCheckId` of every reported detail, in document order.
    detail_ids: Vec<String>,
    tags: BTreeMap<String, String>,
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|inner| inner.is_element() && inner.tag_name().name() == name)
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name).map(|inner| inner.text().unwrap_or_default().to_string())
}

fn set_items<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Vec<roxmltree::Node<'a, 'i>> {
    child(node, name)
        .map(|set| set.children().filter(roxmltree::Node::is_element).collect())
        .unwrap_or_default()
}

/// An ISO 8601 timestamp as epoch seconds — which also asserts that what the service
/// wrote parses as an instant at all.
fn secs_of(text: &str) -> i64 {
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{text:?} must be an ISO 8601 timestamp: {error}"))
        .secs()
}

/// `at` as epoch seconds.
fn epoch_secs(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or_default()
}

/// Every instance status in a response body, as rows.
fn rows(body: &str) -> Vec<Row> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| node.is_element() && child(*node, "applicationStatus").is_some())
        .map(|node| {
            let status = child(node, "applicationStatus").expect("filtered just above");
            Row {
                instance_id: child_text(node, "instanceId").unwrap_or_default(),
                availability_zone: child_text(node, "availabilityZone"),
                availability_zone_id: child_text(node, "availabilityZoneId"),
                status: child_text(status, "status"),
                status_time_stamp: child_text(status, "statusTimeStamp")
                    .map(|text| secs_of(&text)),
                status_since: child_text(status, "statusSince").map(|text| secs_of(&text)),
                resume_at: child_text(status, "resumeAt").map(|text| secs_of(&text)),
                detail_ids: set_items(status, "detailSet")
                    .into_iter()
                    .filter_map(|item| child_text(item, "applicationStatusCheckId"))
                    .collect(),
                tags: set_items(node, "tagSet")
                    .into_iter()
                    .filter_map(|item| Some((child_text(item, "key")?, child_text(item, "value")?)))
                    .collect(),
            }
        })
        .collect()
}

// --- the cases --------------------------------------------------------------

/// Which instant a row expects, since one of them is a fact of the instance rather than
/// of a seeded check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    /// The instance's own launch time: what an instance no check has reported anything
    /// for has had its status since.
    LaunchTime,
    /// A fixed epoch second one of the seeded results carries.
    Secs(i64),
}

impl When {
    fn resolve(self, launch_secs: i64) -> i64 {
        match self {
            Self::LaunchTime => launch_secs,
            Self::Secs(secs) => secs,
        }
    }
}

/// One row of the aggregation table: a SET of seeded check results (and possibly a
/// suppression) for one instance, and the instance-level status the model's rule derives
/// from it.
struct Case {
    label: &'static str,
    /// `(check id, aggregation, check status, the second its result is dated)`.
    checks: Vec<(&'static str, &'static str, &'static str, i64)>,
    suppression: Option<ApplicationStatusSuppression>,
    status: &'static str,
    /// "The date and time when the current status started".
    since: When,
    /// "The date and time of the last status update".
    last_update: When,
    /// "The date and time when application status reporting resumes after suppression".
    resume_at: Option<i64>,
}

/// One seeded check result, as the modelled detail the store holds.
fn detail(
    check_id: &str,
    aggregation: &str,
    status: &str,
    secs: i64,
) -> ApplicationStatusDetail {
    ApplicationStatusDetail {
        application_status_check_id: Some(check_id.to_string()),
        check_update_time: Some(DateTime::from_secs(secs)),
        aggregation: Some(aggregation.to_string()),
        status: Some(status.to_string()),
        status_time_stamp: Some(DateTime::from_secs(secs)),
        status_since: Some(DateTime::from_secs(secs)),
        reason: None,
    }
}

/// The aggregation table. Rows differ along what the rule generalises over: how many
/// included checks there are, which statuses they report, which aggregation they carry,
/// the order in time of their results, and whether a suppression is in force, has no end,
/// or has already ended.
fn cases() -> Vec<Case> {
    let in_force_until = SystemTime::now() + Duration::from_secs(3_600);
    let ended_a_minute_ago = SystemTime::now() - Duration::from_secs(60);
    let suppressed_since = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    // A suppression that took effect AFTER the instance's last check update, so the two
    // sides of "the last status update" are covered in both orders.
    let suppressed_after_the_last_check =
        SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(BASE_SECS + 500).unwrap_or(0));
    vec![
        Case {
            label: "no check has reported anything",
            checks: Vec::new(),
            suppression: None,
            status: APPLICATION_STATUS_NOT_APPLICABLE,
            since: When::LaunchTime,
            last_update: When::LaunchTime,
            resume_at: None,
        },
        Case {
            label: "one included check passed",
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_PASSED,
                BASE_SECS,
            )],
            suppression: None,
            status: APPLICATION_STATUS_OK,
            since: When::Secs(BASE_SECS),
            last_update: When::Secs(BASE_SECS),
            resume_at: None,
        },
        Case {
            label: "two included checks passed, the later one last",
            checks: vec![
                (
                    "asc-1",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_PASSED,
                    BASE_SECS,
                ),
                (
                    "asc-2",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_PASSED,
                    BASE_SECS + 60,
                ),
            ],
            suppression: None,
            // "ok – All included checks passed", which it has been only since the last
            // of them turned passed.
            status: APPLICATION_STATUS_OK,
            since: When::Secs(BASE_SECS + 60),
            last_update: When::Secs(BASE_SECS + 60),
            resume_at: None,
        },
        Case {
            label: "two failed checks among a passing one",
            checks: vec![
                (
                    "asc-1",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_PASSED,
                    BASE_SECS,
                ),
                (
                    "asc-2",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_FAILED,
                    BASE_SECS + 30,
                ),
                (
                    "asc-3",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_FAILED,
                    BASE_SECS + 90,
                ),
            ],
            suppression: None,
            // "impaired – At least one included check failed", since the earlier failure.
            status: APPLICATION_STATUS_IMPAIRED,
            since: When::Secs(BASE_SECS + 30),
            last_update: When::Secs(BASE_SECS + 90),
            resume_at: None,
        },
        Case {
            label: "a failed check outranks an initializing one",
            checks: vec![
                (
                    "asc-1",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_FAILED,
                    BASE_SECS + 70,
                ),
                (
                    "asc-2",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_INITIALIZING,
                    BASE_SECS + 15,
                ),
            ],
            suppression: None,
            status: APPLICATION_STATUS_IMPAIRED,
            since: When::Secs(BASE_SECS + 70),
            last_update: When::Secs(BASE_SECS + 70),
            resume_at: None,
        },
        Case {
            label: "an initializing check outranks one with insufficient data",
            checks: vec![
                (
                    "asc-1",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_INITIALIZING,
                    BASE_SECS + 10,
                ),
                (
                    "asc-2",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA,
                    BASE_SECS + 5,
                ),
            ],
            suppression: None,
            status: APPLICATION_STATUS_INITIALIZING,
            since: When::Secs(BASE_SECS + 10),
            last_update: When::Secs(BASE_SECS + 10),
            resume_at: None,
        },
        Case {
            label: "insufficient data beside a passing check",
            checks: vec![
                (
                    "asc-1",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA,
                    BASE_SECS + 20,
                ),
                (
                    "asc-2",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_PASSED,
                    BASE_SECS + 40,
                ),
            ],
            suppression: None,
            status: APPLICATION_STATUS_INSUFFICIENT_DATA,
            since: When::Secs(BASE_SECS + 20),
            last_update: When::Secs(BASE_SECS + 40),
            resume_at: None,
        },
        Case {
            label: "every included check reports not-applicable",
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_NOT_APPLICABLE,
                BASE_SECS + 5,
            )],
            suppression: None,
            // "not-applicable – No checks with Aggregation set to included APPLY to the
            // instance": a check reporting not-applicable is not one that applies.
            status: APPLICATION_STATUS_NOT_APPLICABLE,
            since: When::LaunchTime,
            last_update: When::Secs(BASE_SECS + 5),
            resume_at: None,
        },
        Case {
            label: "an excluded failed check on its own",
            checks: vec![(
                "asc-1",
                AGGREGATION_EXCLUDED,
                APPLICATION_STATUS_CHECK_FAILED,
                BASE_SECS + 50,
            )],
            suppression: None,
            // "Checks with Aggregation set to excluded do not affect this value."
            status: APPLICATION_STATUS_NOT_APPLICABLE,
            since: When::LaunchTime,
            last_update: When::Secs(BASE_SECS + 50),
            resume_at: None,
        },
        Case {
            label: "an excluded failed check beside an included passing one",
            checks: vec![
                (
                    "asc-1",
                    AGGREGATION_EXCLUDED,
                    APPLICATION_STATUS_CHECK_FAILED,
                    BASE_SECS + 50,
                ),
                (
                    "asc-2",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_PASSED,
                    BASE_SECS + 25,
                ),
            ],
            suppression: None,
            status: APPLICATION_STATUS_OK,
            since: When::Secs(BASE_SECS + 25),
            last_update: When::Secs(BASE_SECS + 50),
            resume_at: None,
        },
        Case {
            label: "suppression in force with an end",
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_FAILED,
                BASE_SECS + 45,
            )],
            suppression: Some(ApplicationStatusSuppression {
                since: suppressed_since,
                resume_at: Some(in_force_until),
            }),
            // "suppressed – Application status reporting is suppressed for the instance",
            // whatever the checks report.
            status: APPLICATION_STATUS_SUPPRESSED,
            since: When::Secs(epoch_secs(suppressed_since)),
            // The check reported after the suppression took effect, so IT is the last
            // status update.
            last_update: When::Secs(BASE_SECS + 45),
            resume_at: Some(epoch_secs(in_force_until)),
        },
        Case {
            label: "suppression with no end",
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_PASSED,
                BASE_SECS + 11,
            )],
            suppression: Some(ApplicationStatusSuppression {
                since: suppressed_after_the_last_check,
                resume_at: None,
            }),
            status: APPLICATION_STATUS_SUPPRESSED,
            since: When::Secs(epoch_secs(suppressed_after_the_last_check)),
            // Nothing has reported since the suppression took effect, so that is the last
            // status update.
            last_update: When::Secs(epoch_secs(suppressed_after_the_last_check)),
            // Nothing to report: reporting resumes only when the caller lifts it.
            resume_at: None,
        },
        Case {
            label: "suppression whose end the clock has passed",
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_FAILED,
                BASE_SECS + 75,
            )],
            suppression: Some(ApplicationStatusSuppression {
                since: suppressed_since,
                resume_at: Some(ended_a_minute_ago),
            }),
            // The clock already owed the instance its resumption, so the checks decide
            // again.
            status: APPLICATION_STATUS_IMPAIRED,
            since: When::Secs(BASE_SECS + 75),
            last_update: When::Secs(BASE_SECS + 75),
            resume_at: None,
        },
    ]
}

/// One instance per case, launched through `RunInstances` and seeded with that case's
/// check results the way the application-status-check operations will write them.
struct Fixture {
    router: MultiServiceRouter,
    cases: Vec<Case>,
    /// The instance launched for each case, in case order.
    ids: Vec<String>,
    /// Each of those instances' launch time, in epoch seconds.
    launch_secs: Vec<i64>,
}

impl Fixture {
    async fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        let cases = cases();
        let mut ids = Vec::with_capacity(cases.len());
        for _ in &cases {
            ids.push(launch_one(&router, &[]).await);
        }

        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let mut launch_secs = Vec::with_capacity(cases.len());
        for (case, instance_id) in cases.iter().zip(&ids) {
            let details = case
                .checks
                .iter()
                .map(|(check_id, aggregation, status, secs)| {
                    (
                        (*check_id).to_string(),
                        detail(check_id, aggregation, status, *secs),
                    )
                })
                .collect();
            guard.application_status.insert(
                instance_id.clone(),
                InstanceApplicationStatusRecord {
                    details,
                    suppression: case.suppression.clone(),
                },
            );
            launch_secs.push(epoch_secs(
                guard
                    .instances
                    .get(instance_id)
                    .unwrap_or_else(|| panic!("{instance_id} must be in the store"))
                    .launch_time,
            ));
        }
        drop(guard);

        Self {
            router,
            cases,
            ids,
            launch_secs,
        }
    }

    /// The instance ids whose case expects the instance-level status `status`.
    fn ids_with_status(&self, status: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .cases
            .iter()
            .zip(&self.ids)
            .filter(|(case, _)| case.status == status)
            .map(|(_, instance_id)| instance_id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Every seeded instance id, in the order the service reports them.
    fn all_ids(&self) -> Vec<String> {
        let mut ids = self.ids.clone();
        ids.sort();
        ids
    }
}

// --- the tests --------------------------------------------------------------

/// The constants this suite seeds with are members of the model's own enums, so a row
/// cannot be asserting against a value the API does not have.
#[test]
fn the_seeded_values_are_the_modelled_enum_members() {
    for status in [
        APPLICATION_STATUS_OK,
        APPLICATION_STATUS_IMPAIRED,
        APPLICATION_STATUS_INITIALIZING,
        APPLICATION_STATUS_INSUFFICIENT_DATA,
        APPLICATION_STATUS_NOT_APPLICABLE,
        APPLICATION_STATUS_SUPPRESSED,
    ] {
        assert!(
            APPLICATION_STATUS_ENUM_VALUES.contains(&status),
            "case {status:?}: expected a member of ApplicationStatusEnum, \
             got one outside {APPLICATION_STATUS_ENUM_VALUES:?}"
        );
    }
    for status in [
        APPLICATION_STATUS_CHECK_PASSED,
        APPLICATION_STATUS_CHECK_FAILED,
        APPLICATION_STATUS_CHECK_INITIALIZING,
        APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA,
        APPLICATION_STATUS_CHECK_NOT_APPLICABLE,
    ] {
        assert!(
            APPLICATION_STATUS_CHECK_ENUM_VALUES.contains(&status),
            "case {status:?}: expected a member of ApplicationStatusCheckEnum, \
             got one outside {APPLICATION_STATUS_CHECK_ENUM_VALUES:?}"
        );
    }
    for aggregation in [AGGREGATION_INCLUDED, AGGREGATION_EXCLUDED] {
        assert!(
            AGGREGATION_STATUS_ENUM_VALUES.contains(&aggregation),
            "case {aggregation:?}: expected a member of AggregationStatusEnum, \
             got one outside {AGGREGATION_STATUS_ENUM_VALUES:?}"
        );
    }
}

/// The aggregation table: every row's instance-level status, the instant it started, the
/// last update and the resumption instant come out of the seeded check results.
#[tokio::test]
async fn the_instance_level_status_is_derived_from_the_stored_check_results() {
    let fixture = Fixture::new().await;
    let answer = described(&fixture.router, "the whole store", &[]).await;

    assert_eq!(
        answer.instance_ids(),
        fixture.all_ids(),
        "the response must report every instance the account holds, in instance-id order"
    );

    for ((case, instance_id), launch_secs) in fixture
        .cases
        .iter()
        .zip(&fixture.ids)
        .zip(&fixture.launch_secs)
    {
        let label = case.label;
        let row = answer.row(instance_id);
        assert_eq!(
            row.status.as_deref(),
            Some(case.status),
            "case {label:?}: expected status {:?}, got {row:?}",
            case.status
        );
        assert_eq!(
            row.status_since,
            Some(case.since.resolve(*launch_secs)),
            "case {label:?}: expected statusSince {:?}, got {row:?}",
            case.since.resolve(*launch_secs)
        );
        assert_eq!(
            row.status_time_stamp,
            Some(case.last_update.resolve(*launch_secs)),
            "case {label:?}: expected statusTimeStamp {:?}, got {row:?}",
            case.last_update.resolve(*launch_secs)
        );
        assert_eq!(
            row.resume_at, case.resume_at,
            "case {label:?}: expected resumeAt {:?}, got {row:?}",
            case.resume_at
        );
        // "Details about the application status checks for the instance": every stored
        // result is reported, whether or not it is one the status is aggregated from.
        let mut expected_details: Vec<String> = case
            .checks
            .iter()
            .map(|(check_id, ..)| (*check_id).to_string())
            .collect();
        expected_details.sort();
        assert_eq!(
            row.detail_ids, expected_details,
            "case {label:?}: expected the detail set {expected_details:?}, got {row:?}"
        );
    }
}

/// A read does not change what a later caller sees: two identical calls report exactly
/// the same rows, so nothing about the status was minted by reading it.
#[tokio::test]
async fn describing_the_status_twice_reports_the_same_rows() {
    let fixture = Fixture::new().await;
    let first = described(&fixture.router, "first read", &[]).await;
    let second = described(&fixture.router, "second read", &[]).await;
    assert_eq!(
        first.rows, second.rows,
        "a second read must report the same rows as the first"
    );
}

/// The `status` filter, driven by the model's own `ApplicationStatusEnum` catalog: each
/// value selects exactly the instances whose derived status is that value — including the
/// values no row of the aggregation table produces, which must select nothing.
#[tokio::test]
async fn the_status_filter_selects_on_the_derived_status() {
    let fixture = Fixture::new().await;
    for status in APPLICATION_STATUS_ENUM_VALUES {
        let answer = described(
            &fixture.router,
            status,
            &ec2_query::filters(&[("status", &[status])]),
        )
        .await;
        let mut reported = answer.instance_ids();
        reported.sort();
        assert_eq!(
            reported,
            fixture.ids_with_status(status),
            "case {status:?}: expected the instances whose derived status is {status:?}, \
             got {reported:?}"
        );
    }

    // EC2's filter values take its own wildcards, so one filter can span two statuses.
    let answer = described(
        &fixture.router,
        "the in* wildcard",
        &ec2_query::filters(&[("status", &["in*"])]),
    )
    .await;
    let mut reported = answer.instance_ids();
    reported.sort();
    let mut expected = fixture.ids_with_status(APPLICATION_STATUS_INITIALIZING);
    expected.extend(fixture.ids_with_status(APPLICATION_STATUS_INSUFFICIENT_DATA));
    expected.sort();
    assert_eq!(
        reported, expected,
        "case \"the in* wildcard\": expected the initializing and insufficient-data \
         instances, got {reported:?}"
    );

    // Two values of one filter are joined with OR.
    let answer = described(
        &fixture.router,
        "two values",
        &ec2_query::filters(&[(
            "status",
            &[APPLICATION_STATUS_OK, APPLICATION_STATUS_SUPPRESSED],
        )]),
    )
    .await;
    let mut reported = answer.instance_ids();
    reported.sort();
    let mut expected = fixture.ids_with_status(APPLICATION_STATUS_OK);
    expected.extend(fixture.ids_with_status(APPLICATION_STATUS_SUPPRESSED));
    expected.sort();
    assert_eq!(
        reported, expected,
        "case \"two values\": expected the ok and suppressed instances, got {reported:?}"
    );

    // A filter carrying no value at all matches nothing: there is no value for the
    // attribute to match.
    let answer = described(
        &fixture.router,
        "no values",
        &[p("Filter.1.Name", "status")],
    )
    .await;
    assert!(
        answer.rows.is_empty(),
        "case \"no values\": expected no instance to match a valueless filter, \
         got {:?}",
        answer.instance_ids()
    );
}

/// The `availability-zone-id` filter selects on the zone id of the instance's zone — the
/// same id `DescribeAvailabilityZones` reports for that zone name, which is also what the
/// row carries.
#[tokio::test]
async fn the_zone_filter_selects_on_the_instances_zone() {
    let (router, _engine) = ec2_query::emulator();

    // The Region's own zones, from the operation that reports them, so this table is not
    // a copy of the zone-naming rule.
    let (status, body) = ec2_query::call(&router, REGION, "DescribeAvailabilityZones", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeAvailabilityZones must succeed, got {status}: {body}"
    );
    let zone_names = ec2_query::elements(&body, "zoneName");
    let zone_ids = ec2_query::elements(&body, "zoneId");
    assert!(
        zone_names.len() >= 2,
        "the Region must report at least two zones to tell apart, got {zone_names:?}"
    );

    // One instance in each of the Region's zones, so each zone id selects exactly its own.
    let mut launched: Vec<(String, String, String)> = Vec::new();
    for (zone_name, zone_id) in zone_names.iter().zip(&zone_ids) {
        let instance_id =
            launch_one(&router, &[p("Placement.AvailabilityZone", zone_name.clone())]).await;
        launched.push((instance_id, zone_name.clone(), zone_id.clone()));
    }

    for (instance_id, zone_name, zone_id) in &launched {
        let answer = described(
            &router,
            zone_id,
            &ec2_query::filters(&[("availability-zone-id", &[zone_id.as_str()])]),
        )
        .await;
        assert_eq!(
            answer.instance_ids(),
            vec![instance_id.clone()],
            "case {zone_id:?}: expected only the instance launched in {zone_name:?}, \
             got {:?}",
            answer.instance_ids()
        );
        let row = answer.row(instance_id);
        assert_eq!(
            row.availability_zone.as_deref(),
            Some(zone_name.as_str()),
            "case {zone_id:?}: expected the zone {zone_name:?}, got {row:?}"
        );
        assert_eq!(
            row.availability_zone_id.as_deref(),
            Some(zone_id.as_str()),
            "case {zone_id:?}: expected the zone id DescribeAvailabilityZones reports, \
             got {row:?}"
        );
    }
}

/// `InstanceIds` narrows the answer to the instances it names, and the tags the launch
/// set round-trip onto the row.
#[tokio::test]
async fn instance_ids_narrow_the_answer_and_tags_round_trip() {
    let (router, _engine) = ec2_query::emulator();
    let tagged = launch_one(
        &router,
        &[
            p("TagSpecification.1.ResourceType", "instance"),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "checkout"),
            p("TagSpecification.1.Tag.2.Key", "Stage"),
            p("TagSpecification.1.Tag.2.Value", "prod"),
        ],
    )
    .await;
    let untagged = launch_one(&router, &[]).await;

    let answer = described(&router, "one id", &[p("InstanceId.1", tagged.clone())]).await;
    assert_eq!(
        answer.instance_ids(),
        vec![tagged.clone()],
        "case \"one id\": expected only the instance the request named, got {:?}",
        answer.instance_ids()
    );
    let row = answer.row(&tagged);
    assert_eq!(
        row.tags,
        BTreeMap::from([
            ("Name".to_string(), "checkout".to_string()),
            ("Stage".to_string(), "prod".to_string()),
        ]),
        "case \"one id\": expected the tags the launch set, got {row:?}"
    );
    // No check has reported anything for this instance, so the model's own answer for
    // "no checks with Aggregation set to included apply" is what it reports.
    assert_eq!(
        row.status.as_deref(),
        Some(APPLICATION_STATUS_NOT_APPLICABLE),
        "case \"one id\": expected not-applicable for an instance with no check, \
         got {row:?}"
    );
    assert!(
        row.detail_ids.is_empty(),
        "case \"one id\": expected no details for an instance with no check, got {row:?}"
    );

    let mut both = vec![tagged.clone(), untagged.clone()];
    both.sort();
    let answer = described(
        &router,
        "both ids",
        &ec2_query::indexed("InstanceId", &[&tagged, &untagged]),
    )
    .await;
    assert_eq!(
        answer.instance_ids(),
        both,
        "case \"both ids\": expected both instances, got {:?}",
        answer.instance_ids()
    );
    assert!(
        answer.row(&untagged).tags.is_empty(),
        "case \"both ids\": expected no tags on the untagged instance, got {:?}",
        answer.row(&untagged)
    );
}

/// `MaxResults` cuts the answer into pages and `NextToken` resumes it, with every
/// instance reported exactly once across the pages.
#[tokio::test]
async fn pages_cover_every_instance_exactly_once() {
    let (router, _engine) = ec2_query::emulator();
    let mut expected: Vec<String> = Vec::new();
    for _ in 0..5 {
        expected.push(launch_one(&router, &[]).await);
    }
    expected.sort();

    for page_size in [1_usize, 2, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let answer = described(&router, &format!("page size {page_size}"), &params).await;
            assert!(
                answer.rows.len() <= page_size,
                "case \"page size {page_size}\": expected at most {page_size} rows, \
                 got {:?}",
                answer.instance_ids()
            );
            seen.extend(answer.instance_ids());
            pages += 1;
            assert!(
                pages <= 10,
                "case \"page size {page_size}\": paging must terminate, saw {seen:?}"
            );
            match answer.next_token {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case \"page size {page_size}\": the pages must cover every instance once, \
             in instance-id order"
        );
    }
}

/// The refusals: an id that names no instance, a malformed one, a filter name this
/// operation does not document, a token this service did not mint, a page size outside
/// the modelled range, and `DryRun`.
#[tokio::test]
async fn bad_requests_are_refused_with_the_services_own_codes() {
    let (router, _engine) = ec2_query::emulator();
    let known = launch_one(&router, &[]).await;

    struct Refusal {
        label: &'static str,
        params: Vec<(String, String)>,
        status: u16,
        code: &'static str,
    }

    let refusals = vec![
        Refusal {
            label: "an instance id nothing was launched for",
            params: vec![p("InstanceId.1", "i-0123456789abcdef0")],
            status: 400,
            code: "InvalidInstanceID.NotFound",
        },
        Refusal {
            label: "an instance id that is not one at all",
            params: vec![p("InstanceId.1", "instance-7")],
            status: 400,
            code: "InvalidInstanceID.Malformed",
        },
        Refusal {
            label: "a filter this operation does not document",
            params: ec2_query::filters(&[("instance-id", &["i-0123456789abcdef0"])]),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "a tag filter, which this operation documents no family for",
            params: ec2_query::filters(&[("tag:Name", &["checkout"])]),
            status: 400,
            code: "InvalidParameterValue",
        },
        Refusal {
            label: "a page token this service did not mint",
            params: vec![p("NextToken", "not-a-token")],
            status: 400,
            code: "InvalidPaginationToken",
        },
        Refusal {
            label: "DryRun on a request that would have succeeded",
            params: vec![p("DryRun", "true"), p("InstanceId.1", known.clone())],
            status: 412,
            code: "DryRunOperation",
        },
    ];

    for refusal in refusals {
        let label = refusal.label;
        let (status, body) = describe(&router, &refusal.params).await;
        assert_eq!(
            status, refusal.status,
            "case {label:?}: expected HTTP {}, got {status} with body {body}",
            refusal.status
        );
        assert_eq!(
            ec2_query::error_code(&body),
            refusal.code,
            "case {label:?}: expected the code {:?}, got body {body}",
            refusal.code
        );
    }

    // The modelled `@range` on `MaxResults` (1..100) is refused before the handler runs,
    // so only the status is this operation's to promise.
    for page_size in ["0", "101"] {
        let (status, body) = describe(&router, &[p("MaxResults", page_size)]).await;
        assert_eq!(
            status, 400,
            "case {page_size:?}: a page size outside the modelled range must be refused, \
             got {status} with body {body}"
        );
    }
}
