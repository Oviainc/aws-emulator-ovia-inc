// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what the CLOCK may still do to the records a domain
//! CONTAINS once that domain's deletion has been recorded.
//!
//! The RULE under test, asserted over a table rather than one recorded request: a status
//! transition the clock owes a contained record is applied only while the resource that
//! contains it is still live, so from the instant a `DeleteDomain` is recorded the
//! domain's patient insights jobs — like its subscriptions, whose lifecycle that same
//! call ends — take no further transition, whatever operation afterwards materialises the
//! work the clock has made due. The only owed work left on such a record is the purge of
//! the domain that holds it.
//!
//! The contrast every row carries with it: a job of a domain that is still live, left
//! equally overdue in the same store, IS moved on by that same call — so the rows pin the
//! liveness of the containing domain as what decides it, not the absence of due work.
//!
//! The rows differ along what the rule generalises over: the status the job was left in
//! when the deletion was recorded (`SUBMITTED`, so one transition is owed, and
//! `IN_PROGRESS`, so the owed one is the terminal transition that would also derive and
//! write the output location), how far past due the clock is (just due, and overdue by
//! several steps, so without the rule the whole remaining lifecycle would be driven in
//! one call), how many jobs the domain holds, which operation drives the materialisation
//! (a start, the job reader, a domain reader, a listing, a repeated delete) and whether
//! that operation resolves what it addresses or misses it, and the region it is signed
//! for.
//!
//! Sources for everything written by hand here:
//!
//! * `smithy_query(query="shape",shape="DomainStatus,JobStatus",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `ACTIVE | DELETING | DELETED`, so a domain whose deletion is recorded is no longer
//!   `ACTIVE`, and `SUBMITTED | IN_PROGRESS | FAILED | SUCCEEDED` as the job lifecycle.
//! * `smithy_query(query="operation",operation="StartPatientInsightsJob,GetPatientInsightsJob,GetDomain,ListDomains,DeleteDomain",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `POST /domain/{domainId}/patient-insights-job` and `GET
//!   /domain/{domainId}/patient-insights-job/{jobId}` answering 200, `GET
//!   /domain/{domainId}` and `GET /domain` answering 200, `DELETE /domain/{domainId}`
//!   answering 200 and `@idempotent`, each with the modelled `ResourceNotFoundException`
//!   (404).
//!
//! The harness (`Emulator`, `string_at`), the two job routes, the request body and the
//! web-app setup that keeps a domain observable in `DELETING` are the sibling suites':
//! one way of driving this service, not two.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, JobStatus, SubscriptionStatus, PATIENT_INSIGHTS_JOB_STEP_SPAN,
};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{job_path, minimal_body, start_path};

/// A `JobId` that names no job: the modelled shape is `@length 1..36` with no pattern, so
/// a value of the right size naming nothing is a well-formed request whose only possible
/// answer is the modelled 404.
const ABSENT_JOB_ID: &str = "00000000000000000000000000000000";

/// How far in the past a due instant is put so that it is already due when the probe runs,
/// without a second transition (one [`PATIENT_INSIGHTS_JOB_STEP_SPAN`] later) being owed
/// as well.
const JUST_DUE: Duration = Duration::from_millis(1);

/// One emulator holding a domain whose deletion is recorded and still in flight, with
/// jobs under it, beside a live domain with a job of its own.
struct Fixture {
    emulator: Emulator,
    /// The domain that is still `ACTIVE`, whose job is the contrast every row asserts.
    live_domain: String,
    live_job: String,
    /// The domain whose deletion has been recorded and whose purge is not due yet.
    dying_domain: String,
    dying_jobs: Vec<String>,
}

/// Everything about one job record that a clock transition WRITES.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JobRecord {
    status: JobStatus,
    updated_at: SystemTime,
    /// The output location the terminal success transition derives, `None` until then.
    output_uri: Option<String>,
}

/// Everything about one subscription record that a status step writes — the sibling
/// contained record kind, whose lifecycle the deleting call already ended.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SubscriptionRecord {
    status: SubscriptionStatus,
    last_updated_at: SystemTime,
}

/// `StartPatientInsightsJob` under `domain_id`, asserting the modelled success, and the
/// `jobId` it minted.
async fn start_job(emulator: &Emulator, case: &str, domain_id: &str) -> String {
    let (status, started) = emulator
        .send("POST", &start_path(domain_id), Some(&minimal_body()))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPatientInsightsJob must answer 200, got {status} with {started}"
    );
    string_at(&started, "jobId")
}

/// Put the next owed transition of each named job `overdue` in the PAST, so the clock has
/// already made it due.
///
/// Only the due instant is moved, so the state is one the clock reaches by itself after a
/// wait rather than one hand-built beside the handlers; the domain is reached whatever its
/// status, which is what lets a row leave work due on a domain already being deleted.
fn make_due(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_ids: &[String],
    overdue: Duration,
) {
    let region = emulator.region();
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let mut store = store.lock();
    let domain = store
        .held_domain_mut(region, domain_id)
        .unwrap_or_else(|| panic!("case {case:?}: domain {domain_id} must still be held"));
    let due = SystemTime::now() - overdue;
    for job_id in job_ids {
        let job = domain
            .patient_insights_jobs
            .get_mut(job_id)
            .unwrap_or_else(|| panic!("case {case:?}: job {job_id} must be held under its domain"));
        job.next_transition_at = Some(due);
    }
}

/// The job records `domain_id` holds under `job_ids`, read straight out of the shared
/// store — the only way to observe a job whose domain is no longer live, since its own
/// reader resolves through that domain.
fn job_records(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_ids: &[String],
) -> Vec<JobRecord> {
    let region = emulator.region();
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let store = store.lock();
    let domain = store
        .held_domain(region, domain_id)
        .unwrap_or_else(|| panic!("case {case:?}: domain {domain_id} must still be held"));
    job_ids
        .iter()
        .map(|job_id| {
            let job = domain
                .patient_insights_jobs
                .get(job_id)
                .unwrap_or_else(|| panic!("case {case:?}: job {job_id} must be held"));
            JobRecord {
                status: job.status,
                updated_at: job.updated_at,
                output_uri: job.insights_output.as_ref().map(|output| output.uri.clone()),
            }
        })
        .collect()
}

/// The subscription records `domain_id` holds, in id order.
fn subscription_records(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
) -> Vec<SubscriptionRecord> {
    let region = emulator.region();
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let store = store.lock();
    let domain = store
        .held_domain(region, domain_id)
        .unwrap_or_else(|| panic!("case {case:?}: domain {domain_id} must still be held"));
    domain
        .subscriptions
        .values()
        .map(|subscription| SubscriptionRecord {
            status: subscription.status,
            last_updated_at: subscription.last_updated_at,
        })
        .collect()
}

/// ONE operation, addressed and driven the way a client drives it.
struct Probe {
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The status this row expects: the operation's modelled success, or the modelled 404
    /// for a row that addresses something nothing holds — the preamble runs before the
    /// resolution, so a miss materialises the owed work too.
    expected: u16,
}

/// How a row addresses its operation at the fixture it runs against.
type Address = fn(&Fixture) -> Probe;

/// One row of the table: the state the contained jobs are left in, and the call that
/// afterwards materialises what the clock has made due.
struct Row {
    case: &'static str,
    region: &'static str,
    /// The status the jobs carry when the deletion is recorded.
    left_in: JobStatus,
    /// How far past due the next owed transition is put.
    overdue: Duration,
    /// How many jobs the dying domain holds.
    job_count: usize,
    probe: Address,
}

/// Stand up one emulator, leave `job_count` jobs under a domain whose deletion is recorded
/// (and one under a live domain) in `left_in`, each owing its next transition `overdue`
/// ago.
async fn fixture(row: &Row) -> Fixture {
    let case = row.case;
    let emulator = Emulator::new(row.region);

    let live_domain = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-live" }))
            .await,
        "domainId",
    );
    // A domain owning a web application is the one whose deletion cannot finish inside the
    // call, so its record is observable between the recorded deletion and the purge.
    let dying_domain = string_at(
        &emulator
            .create_domain(&json!({
                "name": "clinic-dying",
                "webAppSetupConfiguration": web_app_setup(),
            }))
            .await,
        "domainId",
    );
    // The sibling contained record kind, so each row asserts both kinds at once.
    emulator.create_subscription(&dying_domain).await;

    let live_job = start_job(&emulator, case, &live_domain).await;
    let mut dying_jobs = Vec::new();
    for _ in 0..row.job_count {
        dying_jobs.push(start_job(&emulator, case, &dying_domain).await);
    }

    // A row whose jobs must be further along is driven there by the clock, while both
    // domains are still live: one transition made due, then any call to materialise it.
    if row.left_in != JobStatus::Submitted {
        make_due(
            &emulator,
            case,
            &live_domain,
            std::slice::from_ref(&live_job),
            JUST_DUE,
        );
        make_due(&emulator, case, &dying_domain, &dying_jobs, JUST_DUE);
        let (status, listed) = emulator.send("GET", "/domain", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListDomains must answer 200, got {status} with {listed}"
        );
    }

    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{dying_domain}"), None)
        .await;
    assert_eq!(
        (status, string_at(&deleted, "status").as_str()),
        (200, "DELETING"),
        "case {case:?}: deleting a web-app domain must report the non-terminal status, \
         got {status} with {deleted}"
    );

    // The owed work is left standing LAST, so nothing in this setup materialises what the
    // probe is supposed to: the jobs of the deleted domain and the live domain's job are
    // equally overdue.
    make_due(&emulator, case, &dying_domain, &dying_jobs, row.overdue);
    make_due(
        &emulator,
        case,
        &live_domain,
        std::slice::from_ref(&live_job),
        row.overdue,
    );

    Fixture {
        emulator,
        live_domain,
        live_job,
        dying_domain,
        dying_jobs,
    }
}

/// Every row of the table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a submitted job, just due, materialised by a start in the live domain",
            region: "us-east-1",
            left_in: JobStatus::Submitted,
            overdue: JUST_DUE,
            job_count: 1,
            probe: |fixture| Probe {
                method: "POST",
                path: start_path(&fixture.live_domain),
                body: Some(minimal_body()),
                expected: 200,
            },
        },
        Row {
            case: "a submitted job, overdue by three steps, materialised by the job reader",
            region: "eu-west-1",
            left_in: JobStatus::Submitted,
            overdue: PATIENT_INSIGHTS_JOB_STEP_SPAN * 3 + JUST_DUE,
            job_count: 3,
            probe: |fixture| Probe {
                method: "GET",
                path: job_path(&fixture.live_domain, &fixture.live_job),
                body: None,
                expected: 200,
            },
        },
        Row {
            case: "an in-progress job, just due, materialised by the dying domain's own reader",
            region: "ap-south-1",
            left_in: JobStatus::InProgress,
            overdue: JUST_DUE,
            job_count: 2,
            probe: |fixture| Probe {
                method: "GET",
                path: format!("/domain/{}", fixture.dying_domain),
                body: None,
                expected: 200,
            },
        },
        Row {
            case: "an in-progress job, overdue by two steps, materialised by a listing",
            region: "us-west-2",
            left_in: JobStatus::InProgress,
            overdue: PATIENT_INSIGHTS_JOB_STEP_SPAN * 2 + JUST_DUE,
            job_count: 1,
            probe: |_| Probe {
                method: "GET",
                path: "/domain".to_string(),
                body: None,
                expected: 200,
            },
        },
        Row {
            case: "a submitted job, materialised by a repeated delete of its own domain",
            region: "eu-central-1",
            left_in: JobStatus::Submitted,
            overdue: PATIENT_INSIGHTS_JOB_STEP_SPAN + JUST_DUE,
            job_count: 2,
            probe: |fixture| Probe {
                method: "DELETE",
                path: format!("/domain/{}", fixture.dying_domain),
                body: None,
                expected: 200,
            },
        },
        Row {
            case: "an in-progress job, materialised by a read that resolves no job at all",
            region: "ca-central-1",
            left_in: JobStatus::InProgress,
            overdue: JUST_DUE,
            job_count: 1,
            probe: |fixture| Probe {
                method: "GET",
                path: job_path(&fixture.live_domain, ABSENT_JOB_ID),
                body: None,
                expected: 404,
            },
        },
    ]
}

#[tokio::test]
async fn a_recorded_domain_deletion_stops_the_transitions_of_what_it_contains() {
    for row in rows() {
        let case = row.case;
        let fixture = fixture(&row).await;
        let emulator = &fixture.emulator;

        // What the deleting call left behind, before any further operation runs.
        let before = job_records(emulator, case, &fixture.dying_domain, &fixture.dying_jobs);
        let subscriptions_before = subscription_records(emulator, case, &fixture.dying_domain);
        let live_before = job_records(
            emulator,
            case,
            &fixture.live_domain,
            std::slice::from_ref(&fixture.live_job),
        );
        for (index, record) in before.iter().enumerate() {
            assert_eq!(
                (record.status, record.output_uri.as_deref()),
                (row.left_in, None),
                "case {case:?} job #{index}: the deleted domain's job must still carry the \
                 status it was left in and no output, got {record:?}"
            );
        }

        let probe = (row.probe)(&fixture);
        let (status, response) = emulator
            .send(probe.method, &probe.path, probe.body.as_ref())
            .await;
        assert_eq!(
            status, probe.expected,
            "case {case:?}: {} {} must answer {}, got {status} with {response}",
            probe.method, probe.path, probe.expected
        );

        // The rule: nothing the clock owed was applied to a job the recorded deletion has
        // already retired — not its status, not its last-changed instant, and not the
        // output location the terminal transition would derive.
        let after = job_records(emulator, case, &fixture.dying_domain, &fixture.dying_jobs);
        assert_eq!(
            after, before,
            "case {case:?}: a job of a domain whose deletion is recorded takes no further \
             transition, expected {before:?}, got {after:?}"
        );
        // The sibling contained kind, whose lifecycle the deleting call itself ended, is
        // equally untouched.
        let subscriptions_after = subscription_records(emulator, case, &fixture.dying_domain);
        assert_eq!(
            subscriptions_after, subscriptions_before,
            "case {case:?}: a subscription of a domain whose deletion is recorded takes no \
             further transition, expected {subscriptions_before:?}, got \
             {subscriptions_after:?}"
        );
        for record in &subscriptions_after {
            assert_eq!(
                record.status,
                SubscriptionStatus::Deleted,
                "case {case:?}: the deleting call ends every subscription's lifecycle, got \
                 {record:?}"
            );
        }

        // The contrast: the SAME call applied the equally overdue work of the live
        // domain's job, so what the rows above pin is the liveness of the containing
        // domain and not an absence of due work.
        let live_after = job_records(
            emulator,
            case,
            &fixture.live_domain,
            std::slice::from_ref(&fixture.live_job),
        );
        assert_ne!(
            live_after, live_before,
            "case {case:?}: a job of a LIVE domain must be moved on by the same call, \
             got {live_after:?}"
        );

        // The same two facts on the wire: the deleted domain's record is still held and
        // still reports the non-terminal status, and the job under it is readable through
        // no route.
        let (status, domain) = emulator
            .send("GET", &format!("/domain/{}", fixture.dying_domain), None)
            .await;
        assert_eq!(
            (status, string_at(&domain, "status").as_str()),
            (200, "DELETING"),
            "case {case:?}: the deleted domain's record is still held in the non-terminal \
             status, got {status} with {domain}"
        );
        for job_id in &fixture.dying_jobs {
            let (status, missing) = emulator
                .send("GET", &job_path(&fixture.dying_domain, job_id), None)
                .await;
            assert_eq!(
                (status, error_code(&missing).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a job under a domain that is no longer live resolves \
                 nowhere, got {status} with {missing}"
            );
        }
    }
}
