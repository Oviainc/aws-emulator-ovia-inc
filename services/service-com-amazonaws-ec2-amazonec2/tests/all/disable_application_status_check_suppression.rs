// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisableApplicationStatusCheckSuppression`, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=DisableApplicationStatusCheckSuppression`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. each instance the request names is judged on its OWN suppression: one in force is
//!    lifted — "after suppression is disabled, health check results resume affecting the
//!    instance-level application status" — and reported in the successful set with the
//!    instant suppression started and the instant reporting resumed, while an instance
//!    whose reporting is not suppressed as of the call has none to disable and is reported
//!    in the unsuccessful set with the reason;
//! 2. the lift is visible to a reader: `DescribeApplicationStatus` stops reporting
//!    `suppressed` for a lifted instance and derives its status from the check results
//!    again, so one call's write is what a later call reads;
//! 3. a retry leaves the store exactly as the first call left it and answers no error, and
//!    an id the request repeats is answered once;
//! 4. an instance id this service cannot resolve refuses the WHOLE call — malformed before
//!    unknown — and a request naming one lifts nothing, as do a request naming no instance
//!    at all and a batch above the documented ceiling of 100 ids; a `DryRun` is answered
//!    `DryRunOperation` before anything the request names is read, the permission question
//!    being about the caller rather than about the instances.
//!
//! So the table below spans the class rule 1 generalises over: each row is a different
//! suppression STATE of one instance (none recorded, recorded but not suppressing, in force
//! with no end, in force with an end still ahead, ended) beside a different set of check
//! results, and the recorded request the issue behind this suite cited is one row of the
//! refusal table.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ec2::ops::types::ApplicationStatusDetail;
use ec2::state::{
    AGGREGATION_INCLUDED, APPLICATION_STATUS_CHECK_FAILED, APPLICATION_STATUS_CHECK_PASSED,
    APPLICATION_STATUS_IMPAIRED, APPLICATION_STATUS_NOT_APPLICABLE, APPLICATION_STATUS_OK,
    APPLICATION_STATUS_SUPPRESSED, ApplicationStatusSuppression,
    ComAmazonawsEc2Amazonec2State as Ec2State, InstanceApplicationStatusRecord,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names. Which AMI it is does not matter to a suppression, so
/// one is used throughout.
const AMI: &str = "ami-0123456789abcdef0";

/// The operation under test, as an ec2Query `Action`.
const ACTION: &str = "DisableApplicationStatusCheckSuppression";

/// A fixed instant the seeded suppressions and check results are dated from:
/// 2026-01-01T00:00:00Z as epoch seconds. Fixed rather than relative to
/// `SystemTime::now()`, so what a row expects is the same whenever the suite runs.
const BASE_SECS: i64 = 1_767_225_600;

/// The idempotency token the batch calls carry: "a unique, case-sensitive identifier that
/// you provide to ensure that the operation completes no more than one time"
/// (`DisableApplicationStatusCheckSuppressionRequest$ClientToken`).
const CLIENT_TOKEN: &str = "f0d7a0ba-c0ff-4f9c-9c1b-3a0f5e1d2c3b";

/// The documented ceiling on one request's id list: "You can specify a maximum of 100
/// instance IDs for each request".
const MAX_INSTANCE_IDS: usize = 100;

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The `InstanceId.N` form fields for a list of ids, as ec2Query flattens a list.
fn id_params(ids: &[String]) -> Vec<(String, String)> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| p(&format!("InstanceId.{}", index + 1), id.clone()))
        .collect()
}

/// An ISO 8601 timestamp as epoch seconds — which also asserts that what the service wrote
/// parses as an instant at all.
fn secs_of(case: &str, text: &str) -> i64 {
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {text:?} must be an ISO 8601 timestamp: {error}")
        })
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

/// Launch one instance and report its id.
async fn launch_one(router: &MultiServiceRouter) -> String {
    let params = vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")];
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

/// One entry of either result set: the instance it names and the members that entry
/// carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    instance_id: String,
    suppress_at: Option<String>,
    resume_at: Option<String>,
    reason: Option<String>,
}

/// The entries of the `set` set of a response, in the order the response reported them.
fn results(case: &str, body: &str, set: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), set)
        .into_iter()
        .map(|item| Reported {
            instance_id: child_text(item, "instanceId").unwrap_or_default(),
            suppress_at: child_text(item, "suppressAt"),
            resume_at: child_text(item, "resumeAt"),
            reason: child_text(item, "reason"),
        })
        .collect()
}

/// The entries of `successfulResultSet`.
fn successful(case: &str, body: &str) -> Vec<Reported> {
    results(case, body, "successfulResultSet")
}

/// The entries of `unsuccessfulResultSet`.
fn unsuccessful(case: &str, body: &str) -> Vec<Reported> {
    results(case, body, "unsuccessfulResultSet")
}

/// What `DescribeApplicationStatus` reports for one instance: its instance-level status and
/// the instant reporting resumes, if it carries one.
async fn described(router: &MultiServiceRouter, instance_id: &str) -> (String, Option<String>) {
    let (status, body) = ec2_query::call(
        router,
        REGION,
        "DescribeApplicationStatus",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeApplicationStatus for {instance_id} must succeed, got {status}: {body}"
    );
    let parsed = document(instance_id, &body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "applicationStatus")
        .unwrap_or_else(|| {
            panic!("DescribeApplicationStatus must report {instance_id}'s status: {body}")
        });
    (
        child_text(node, "status").unwrap_or_default(),
        child_text(node, "resumeAt"),
    )
}

// --- the cases --------------------------------------------------------------

/// The suppression state one instance is seeded in: the class rule 1 generalises over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seeded {
    /// Nothing has ever been recorded for this instance, so it has no record at all.
    NoRecord,
    /// A record of check results, with no suppression beside them.
    NoSuppression,
    /// A suppression with no end: it "suppresses health check results indefinitely until
    /// you call DisableApplicationStatusCheckSuppression".
    InForceWithoutEnd,
    /// A suppression whose end is still ahead of the call.
    InForceUntilLater,
    /// A suppression whose end the clock has already passed, so reporting resumed on its
    /// own duration.
    AlreadyEnded,
}

/// One row: a seeded suppression state and set of check results for one instance, and what
/// the call and a later describe must say about it.
struct Case {
    label: &'static str,
    seeded: Seeded,
    /// `(check id, aggregation, check status, the second its result is dated)`.
    checks: Vec<(&'static str, &'static str, &'static str, i64)>,
    /// Whether the call reports this instance in `SuccessfulResults`.
    lifted: bool,
    /// The instance-level status a describe reports once the call has been made.
    status_after: &'static str,
}

/// When every seeded suppression took effect.
fn suppressed_since() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(BASE_SECS).unwrap_or(0))
}

/// The end a [`Seeded::InForceUntilLater`] suppression carries: an hour ahead of the call.
fn ends_later() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3_600)
}

/// The end a [`Seeded::AlreadyEnded`] suppression carries: a minute behind the call.
fn ended_already() -> SystemTime {
    SystemTime::now() - Duration::from_secs(60)
}

/// The suppression a row is seeded with, if any.
fn seeded_suppression(seeded: Seeded) -> Option<ApplicationStatusSuppression> {
    match seeded {
        Seeded::NoRecord | Seeded::NoSuppression => None,
        Seeded::InForceWithoutEnd => Some(ApplicationStatusSuppression {
            since: suppressed_since(),
            resume_at: None,
        }),
        Seeded::InForceUntilLater => Some(ApplicationStatusSuppression {
            since: suppressed_since(),
            resume_at: Some(ends_later()),
        }),
        Seeded::AlreadyEnded => Some(ApplicationStatusSuppression {
            since: suppressed_since(),
            resume_at: Some(ended_already()),
        }),
    }
}

/// The table. Rows differ along what the rule generalises over: whether a suppression is
/// recorded at all, whether it is in force, whether it carries an end, and how many check
/// results of which status decide the instance's status once reporting resumes.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a suppression with no end, over a failed check",
            seeded: Seeded::InForceWithoutEnd,
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_FAILED,
                BASE_SECS + 30,
            )],
            lifted: true,
            // "impaired – At least one included check failed", which decides the status
            // again once the suppression is lifted.
            status_after: APPLICATION_STATUS_IMPAIRED,
        },
        Case {
            label: "a suppression ending later, over two passing checks",
            seeded: Seeded::InForceUntilLater,
            checks: vec![
                (
                    "asc-1",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_PASSED,
                    BASE_SECS + 10,
                ),
                (
                    "asc-2",
                    AGGREGATION_INCLUDED,
                    APPLICATION_STATUS_CHECK_PASSED,
                    BASE_SECS + 80,
                ),
            ],
            lifted: true,
            status_after: APPLICATION_STATUS_OK,
        },
        Case {
            label: "a suppression ending later, with no check at all",
            seeded: Seeded::InForceUntilLater,
            checks: Vec::new(),
            lifted: true,
            // "not-applicable – No checks with Aggregation set to included apply to the
            // instance".
            status_after: APPLICATION_STATUS_NOT_APPLICABLE,
        },
        Case {
            label: "a suppression whose end the clock has passed",
            seeded: Seeded::AlreadyEnded,
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_FAILED,
                BASE_SECS + 45,
            )],
            // Reporting resumed on the suppression's own duration, so this call finds
            // nothing in force to disable.
            lifted: false,
            status_after: APPLICATION_STATUS_IMPAIRED,
        },
        Case {
            label: "check results with no suppression beside them",
            seeded: Seeded::NoSuppression,
            checks: vec![(
                "asc-1",
                AGGREGATION_INCLUDED,
                APPLICATION_STATUS_CHECK_PASSED,
                BASE_SECS + 15,
            )],
            lifted: false,
            status_after: APPLICATION_STATUS_OK,
        },
        Case {
            label: "an instance nothing has ever reported on",
            seeded: Seeded::NoRecord,
            checks: Vec::new(),
            lifted: false,
            status_after: APPLICATION_STATUS_NOT_APPLICABLE,
        },
    ]
}

/// One seeded check result, as the modelled detail the store holds.
fn detail(check_id: &str, aggregation: &str, status: &str, secs: i64) -> ApplicationStatusDetail {
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

/// One instance per row, launched through `RunInstances` and seeded with that row's
/// suppression and check results the way the application status operations write them.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    cases: Vec<Case>,
    /// The instance launched for each row, in row order.
    ids: Vec<String>,
}

impl Fixture {
    async fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        let cases = cases();
        let mut ids = Vec::with_capacity(cases.len());
        for _ in &cases {
            ids.push(launch_one(&router).await);
        }

        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for (case, instance_id) in cases.iter().zip(&ids) {
            if case.seeded == Seeded::NoRecord {
                continue;
            }
            let details: BTreeMap<String, ApplicationStatusDetail> = case
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
                    suppression: seeded_suppression(case.seeded),
                },
            );
        }
        drop(guard);

        Self {
            router,
            engine,
            cases,
            ids,
        }
    }

    /// Every seeded instance id, in row order — the order the batch calls name them in.
    fn all_ids(&self) -> Vec<String> {
        self.ids.clone()
    }

    /// The ids of the rows the call must report in `SuccessfulResults`.
    fn lifted_ids(&self) -> Vec<String> {
        self.cases
            .iter()
            .zip(&self.ids)
            .filter(|(case, _)| case.lifted)
            .map(|(_, instance_id)| instance_id.clone())
            .collect()
    }

    /// The ids of the rows the call must report in `UnsuccessfulResults`.
    fn unlifted_ids(&self) -> Vec<String> {
        self.cases
            .iter()
            .zip(&self.ids)
            .filter(|(case, _)| !case.lifted)
            .map(|(_, instance_id)| instance_id.clone())
            .collect()
    }

    /// One call naming every seeded instance, as its status and body.
    async fn disable_all(&self) -> (u16, String) {
        let mut params = id_params(&self.all_ids());
        params.push(p("ClientToken", CLIENT_TOKEN));
        ec2_query::call(&self.router, REGION, ACTION, &params).await
    }

    /// Whether the store still holds a suppression in force for `instance_id`.
    fn suppressed_in_store(&self, instance_id: &str) -> bool {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .application_status
            .get(instance_id)
            .and_then(|record| record.suppression.as_ref())
            .is_some_and(|suppression| suppression.in_force_at(SystemTime::now()))
    }
}

// --- the tests --------------------------------------------------------------

/// Rule 1 and rule 2: each named instance lands in the set its own suppression state puts
/// it in, the successful entries carry the instant suppression started and the instant
/// reporting resumed, and a later describe reports every instance's status derived from its
/// check results rather than `suppressed`.
#[tokio::test]
async fn each_named_instance_is_judged_on_its_own_suppression() {
    let fixture = Fixture::new().await;

    // Before the call, the rows whose suppression is in force report `suppressed` and the
    // rest already report their derived status — so what the call changes is visible.
    for (case, instance_id) in fixture.cases.iter().zip(&fixture.ids) {
        let (status, _) = described(&fixture.router, instance_id).await;
        let expected = if case.lifted {
            APPLICATION_STATUS_SUPPRESSED
        } else {
            case.status_after
        };
        assert_eq!(
            status, expected,
            "case {:?}: expected the instance to report {expected:?} before the call, \
             got {status:?}",
            case.label
        );
    }

    let before = epoch_secs(SystemTime::now());
    let (status, body) = fixture.disable_all().await;
    let after = epoch_secs(SystemTime::now()) + 1;
    assert_eq!(
        status, 200,
        "the batch call must succeed, got {status} with body {body}"
    );

    let reported_successful = successful("the batch", &body);
    assert_eq!(
        reported_successful
            .iter()
            .map(|entry| entry.instance_id.clone())
            .collect::<Vec<String>>(),
        fixture.lifted_ids(),
        "case {:?}: expected the suppressed instances reported successful, got {body}",
        "the batch"
    );
    let reported_unsuccessful = unsuccessful("the batch", &body);
    assert_eq!(
        reported_unsuccessful
            .iter()
            .map(|entry| entry.instance_id.clone())
            .collect::<Vec<String>>(),
        fixture.unlifted_ids(),
        "case {:?}: expected the instances with nothing in force reported unsuccessful, \
         got {body}",
        "the batch"
    );

    // Every successful entry: "the date and time when suppression started" is the instant
    // the suppression was put in force, and "the date and time when suppression ends and
    // health checks resume" is the instant this call was served.
    for entry in &reported_successful {
        let case = entry.instance_id.as_str();
        let suppress_at = entry
            .suppress_at
            .as_deref()
            .unwrap_or_else(|| panic!("case {case:?}: a successful entry must carry suppressAt"));
        assert_eq!(
            secs_of(case, suppress_at),
            epoch_secs(suppressed_since()),
            "case {case:?}: expected suppressAt {}, got {suppress_at:?}",
            epoch_secs(suppressed_since())
        );
        let resume_at = entry
            .resume_at
            .as_deref()
            .unwrap_or_else(|| panic!("case {case:?}: a successful entry must carry resumeAt"));
        let resumed = secs_of(case, resume_at);
        assert!(
            (before..=after).contains(&resumed),
            "case {case:?}: expected resumeAt inside {before}..={after}, got {resumed}"
        );
        assert!(
            entry.reason.is_none(),
            "case {case:?}: a successful entry carries no reason, got {:?}",
            entry.reason
        );
    }

    // Every unsuccessful entry carries "the reason the suppression failed", and the two
    // instants only when a suppression of that instance was recorded at all.
    for entry in &reported_unsuccessful {
        let case = entry.instance_id.as_str();
        let reason = entry
            .reason
            .as_deref()
            .unwrap_or_else(|| panic!("case {case:?}: an unsuccessful entry must carry a reason"));
        assert!(
            !reason.is_empty(),
            "case {case:?}: an unsuccessful entry's reason must say why, got {reason:?}"
        );
        let seeded = fixture
            .cases
            .iter()
            .zip(&fixture.ids)
            .find(|(_, instance_id)| *instance_id == &entry.instance_id)
            .map(|(case, _)| case.seeded)
            .unwrap_or(Seeded::NoRecord);
        match seeded {
            Seeded::AlreadyEnded => {
                let suppress_at = entry.suppress_at.as_deref().unwrap_or_else(|| {
                    panic!("case {case:?}: an ended suppression's entry must carry suppressAt")
                });
                assert_eq!(
                    secs_of(case, suppress_at),
                    epoch_secs(suppressed_since()),
                    "case {case:?}: expected suppressAt {}, got {suppress_at:?}",
                    epoch_secs(suppressed_since())
                );
                let resume_at = entry.resume_at.as_deref().unwrap_or_else(|| {
                    panic!("case {case:?}: an ended suppression's entry must carry resumeAt")
                });
                assert!(
                    secs_of(case, resume_at) < before,
                    "case {case:?}: expected the suppression's own end, before {before}, \
                     got {resume_at:?}"
                );
            }
            _ => {
                assert_eq!(
                    (entry.suppress_at.clone(), entry.resume_at.clone()),
                    (None, None),
                    "case {case:?}: an instance with no suppression recorded carries neither \
                     instant, got {entry:?}"
                );
            }
        }
    }

    // Rule 2: the write is what a later read reports — no instance is `suppressed` any
    // more, each reports the status its check results derive, and none carries a resumption
    // instant.
    for (case, instance_id) in fixture.cases.iter().zip(&fixture.ids) {
        let (status, resume_at) = described(&fixture.router, instance_id).await;
        assert_eq!(
            status, case.status_after,
            "case {:?}: expected {:?} after the call, got {status:?}",
            case.label, case.status_after
        );
        assert!(
            resume_at.is_none(),
            "case {:?}: nothing is waiting to resume once reporting has resumed, got {resume_at:?}",
            case.label
        );
        assert!(
            !fixture.suppressed_in_store(instance_id),
            "case {:?}: the store must hold no suppression in force any more",
            case.label
        );
    }
}

/// Rule 3: a retry of the same request with the same `ClientToken` changes nothing and
/// answers no error — every instance is reported unsuccessful the second time, and the
/// statuses a reader sees are the ones the first call left.
#[tokio::test]
async fn a_retry_of_the_same_request_changes_nothing() {
    let fixture = Fixture::new().await;

    let (status, body) = fixture.disable_all().await;
    assert_eq!(
        status, 200,
        "the first call must succeed, got {status} with body {body}"
    );

    let (status, retried) = fixture.disable_all().await;
    assert_eq!(
        status, 200,
        "a retry must not answer an error, got {status} with body {retried}"
    );
    assert!(
        successful("the retry", &retried).is_empty(),
        "case {:?}: a retry lifts nothing, got {retried}",
        "the retry"
    );
    assert_eq!(
        unsuccessful("the retry", &retried)
            .iter()
            .map(|entry| entry.instance_id.clone())
            .collect::<Vec<String>>(),
        fixture.all_ids(),
        "case {:?}: a retry reports every named instance as having nothing in force, \
         got {retried}",
        "the retry"
    );

    for (case, instance_id) in fixture.cases.iter().zip(&fixture.ids) {
        let (status, _) = described(&fixture.router, instance_id).await;
        assert_eq!(
            status, case.status_after,
            "case {:?}: expected {:?} after the retry, got {status:?}",
            case.label, case.status_after
        );
    }
}

/// Rule 3: one instance gets one answer however many times the batch names it.
#[tokio::test]
async fn an_id_the_request_repeats_is_answered_once() {
    let (router, engine) = ec2_query::emulator();
    let instance_id = launch_one(&router).await;
    let store = engine.state::<Ec2State>();
    store.lock().application_status.insert(
        instance_id.clone(),
        InstanceApplicationStatusRecord {
            details: BTreeMap::new(),
            suppression: seeded_suppression(Seeded::InForceWithoutEnd),
        },
    );

    let repeated = vec![instance_id.clone(), instance_id.clone(), instance_id.clone()];
    let (status, body) = ec2_query::call(&router, REGION, ACTION, &id_params(&repeated)).await;
    assert_eq!(
        status, 200,
        "the call must succeed, got {status} with body {body}"
    );
    let case = "three copies of one id";
    assert_eq!(
        successful(case, &body)
            .iter()
            .map(|entry| entry.instance_id.clone())
            .collect::<Vec<String>>(),
        vec![instance_id.clone()],
        "case {case:?}: expected the instance reported once, got {body}"
    );
    assert!(
        unsuccessful(case, &body).is_empty(),
        "case {case:?}: a repeat is not a second suppression to lift, got {body}"
    );
}

// --- what the call refuses --------------------------------------------------

/// One refusal row: what the request names, and the answer it must get.
struct Refusal {
    label: &'static str,
    /// The ids the request names; `None` stands for the valid, suppressed instance the row
    /// is run against, so a row can mix it with an id this service cannot resolve.
    ids: Vec<Option<&'static str>>,
    dry_run: bool,
    status: u16,
    code: &'static str,
    /// The message the refusal must carry, when the row pins it.
    message: Option<String>,
}

/// The id the issue behind this suite recorded: a 17-digit id whose leading digit is above
/// zero, which is no id EC2 issues.
const EXAMPLE_ID: &str = "i-1234567890abcdef0";

/// What an answered dry run carries: "if you have the required permissions, the error
/// response is DryRunOperation", and the permission question is about the caller rather
/// than about the instances the request names — so this is the answer whatever a dry run
/// names, which is what this operation's recorded dry run carries.
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// The malformed-id refusal's wording, built from the id the request sent.
fn malformed(instance_id: &str) -> Option<String> {
    Some(format!("The instance ID '{instance_id}' is malformed"))
}

/// The refusal table: every way one call is refused outright, each with the code and the
/// wording this service answers that cause with.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "a documentation-example id, whose leading digit is above zero",
            ids: vec![Some(EXAMPLE_ID)],
            dry_run: false,
            status: 400,
            code: "InvalidInstanceID.Malformed",
            message: malformed(EXAMPLE_ID),
        },
        Refusal {
            label: "the retired short id form",
            ids: vec![Some("i-1a2b3c4d")],
            dry_run: false,
            status: 400,
            code: "InvalidInstanceID.Malformed",
            message: malformed("i-1a2b3c4d"),
        },
        Refusal {
            label: "the all-zero id",
            ids: vec![Some("i-00000000000000000")],
            dry_run: false,
            status: 400,
            code: "InvalidInstanceID.Malformed",
            message: malformed("i-00000000000000000"),
        },
        Refusal {
            label: "a value that is no instance id at all",
            ids: vec![Some("instance-one")],
            dry_run: false,
            status: 400,
            code: "InvalidInstanceID.Malformed",
            message: malformed("instance-one"),
        },
        Refusal {
            label: "a well-formed id the account holds no instance for",
            ids: vec![Some("i-01234567890abcdef")],
            dry_run: false,
            status: 400,
            code: "InvalidInstanceID.NotFound",
            message: Some("The instance ID 'i-01234567890abcdef' does not exist".to_string()),
        },
        Refusal {
            label: "the suppressed instance beside an id this service cannot resolve",
            ids: vec![None, Some(EXAMPLE_ID)],
            dry_run: false,
            status: 400,
            code: "InvalidInstanceID.Malformed",
            message: malformed(EXAMPLE_ID),
        },
        Refusal {
            label: "a dry run naming the suppressed instance",
            ids: vec![None],
            dry_run: true,
            status: 412,
            code: "DryRunOperation",
            message: Some(DRY_RUN_MESSAGE.to_string()),
        },
        Refusal {
            label: "a dry run naming an id this service cannot resolve",
            ids: vec![Some(EXAMPLE_ID)],
            dry_run: true,
            status: 412,
            code: "DryRunOperation",
            message: Some(DRY_RUN_MESSAGE.to_string()),
        },
        Refusal {
            label: "a dry run naming no instance at all",
            ids: Vec::new(),
            dry_run: true,
            status: 412,
            code: "DryRunOperation",
            message: Some(DRY_RUN_MESSAGE.to_string()),
        },
        Refusal {
            label: "a request naming no instance at all",
            ids: Vec::new(),
            dry_run: false,
            status: 400,
            code: "MissingParameter",
            message: Some("The request must contain the parameter InstanceId".to_string()),
        },
    ]
}

/// Rule 4: every refusal row answers its own code and wording, and the suppression the row
/// was run against is still in force afterwards.
#[tokio::test]
async fn a_call_it_cannot_serve_is_refused_and_lifts_nothing() {
    for refusal in refusals() {
        let (router, engine) = ec2_query::emulator();
        let instance_id = launch_one(&router).await;
        let store = engine.state::<Ec2State>();
        store.lock().application_status.insert(
            instance_id.clone(),
            InstanceApplicationStatusRecord {
                details: BTreeMap::new(),
                suppression: seeded_suppression(Seeded::InForceWithoutEnd),
            },
        );

        let ids: Vec<String> = refusal
            .ids
            .iter()
            .map(|id| id.map_or_else(|| instance_id.clone(), str::to_string))
            .collect();
        let mut params = id_params(&ids);
        if refusal.dry_run {
            params.push(p("DryRun", "true"));
        }
        let case = refusal.label;
        let (status, body) = ec2_query::call(&router, REGION, ACTION, &params).await;
        assert_eq!(
            status, refusal.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            refusal.status
        );
        assert_eq!(
            ec2_query::error_code(&body),
            refusal.code,
            "case {case:?}: expected the code {:?}, got {body}",
            refusal.code
        );
        if let Some(expected) = &refusal.message {
            assert_eq!(
                &ec2_query::error_message(&body),
                expected,
                "case {case:?}: expected the message {expected:?}, got {body}"
            );
        }

        let (described_status, _) = described(&router, &instance_id).await;
        assert_eq!(
            described_status, APPLICATION_STATUS_SUPPRESSED,
            "case {case:?}: a refused call lifts nothing, so the instance must still be \
             suppressed, got {described_status:?}"
        );
    }
}

/// Rule 4: the documented ceiling on one request's id list — "you can specify a maximum of
/// 100 instance IDs for each request" — is charged before any id is looked up, so a batch
/// one id past it is refused whether or not those ids name anything.
#[tokio::test]
async fn a_batch_above_the_documented_ceiling_is_refused() {
    let (router, _engine) = ec2_query::emulator();
    for (case, count) in [
        ("the documented ceiling", MAX_INSTANCE_IDS),
        ("one id past it", MAX_INSTANCE_IDS + 1),
    ] {
        let ids: Vec<String> = (1..=count)
            .map(|index| format!("i-{index:017x}"))
            .collect();
        let (status, body) = ec2_query::call(&router, REGION, ACTION, &id_params(&ids)).await;
        if count > MAX_INSTANCE_IDS {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                ec2_query::error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert_eq!(
                ec2_query::error_message(&body),
                format!("InstanceId carries {count} ids, above the maximum of {MAX_INSTANCE_IDS}"),
                "case {case:?}: expected the ceiling named in the message, got {body}"
            );
        } else {
            // At the ceiling the batch is served, so the refusal is the one its ids earn:
            // none of these names an instance this account holds.
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                ec2_query::error_code(&body),
                "InvalidInstanceID.NotFound",
                "case {case:?}: expected the ids looked up rather than the batch refused, \
                 got {body}"
            );
        }
    }
}
