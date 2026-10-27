// SPDX-License-Identifier: CC0-1.0

//! A job's consumable-resource requirement is RELEASED when the job goes terminal, end to
//! end over the wire.
//!
//! The RULE under test: `ListJobsByConsumableResource` reports "Batch jobs that require a
//! specific consumable resource" (`smithy_query(query="operation",
//! operation="ListJobsByConsumableResource")`), and a job requires one only while it can
//! still consume it — so from the instant a job reaches a terminal `JobStatus` it is
//! reported by no listing of that resource, under NO filter, whatever route took it there.
//! The job itself is untouched: `DescribeJobs` still replays its terminal status and the
//! `consumableResourceProperties` it was submitted with.
//!
//! Each case asserts that rule over a SET of inputs rather than one request: the rows
//! differ in the ROUTE the job took to its terminal status (a `TerminateJob`, a
//! `CancelJob`, and a dependency failing — which is the clock's own route, with no call
//! naming the job at all), in the status the job was in when it got there, in the quantity
//! its requirement asks for (the documented floor of 0, a small count and a large one) and
//! in whether that requirement names the resource by NAME or by ARN. Every row carries a
//! WITNESS job on the same resource that stays live, so a handler that simply answered an
//! empty listing would fail every case.
//!
//! Both the subject and the witness are OBSERVED at rest before a row acts. Each job's
//! clock-owed `SUBMITTED` -> `RUNNABLE` falls due one span after THAT job's own submit, so
//! the subject reaching `RUNNABLE` says nothing about the witness, which was submitted
//! after it; a case that asserted the witness's status without observing it first would be
//! asserting a transition the clock does not owe yet (`job_phase_is_per_job.rs` pins that
//! rule).

use serde_json::{json, Value};

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, job_queue, until_status, Batch,
};
use crate::jobs_by_consumable_resource::{job_names, listed, resource_arn};

/// Every value the `JOB_STATUS` filter documents. Source: the
/// `ListJobsByConsumableResourceRequest$filters` doc (`smithy_query(query="doc",
/// shape="ListJobsByConsumableResourceRequest", member="filters")`): "name: JOB_STATUS
/// values: SUBMITTED | PENDING | RUNNABLE | STARTING | RUNNING | SUCCEEDED | FAILED".
const JOB_STATUS_FILTER_VALUES: [&str; 7] = [
    "SUBMITTED",
    "PENDING",
    "RUNNABLE",
    "STARTING",
    "RUNNING",
    "SUCCEEDED",
    "FAILED",
];

/// How a row drives its subject job to a terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// `TerminateJob` names the job.
    Terminate,
    /// `CancelJob` names the job, which ends a job that has not progressed past
    /// `RUNNABLE`.
    Cancel,
    /// Nothing names the job: it depends on another job, and the clock ends it once that
    /// dependency fails.
    DependencyFailed,
}

/// One row of the table: a consumable resource, a job requiring it that is driven
/// terminal by [`Route`], and a witness job requiring the same resource that stays live.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// What the subject's and the witness's requirement asks of the row's resource.
    quantity: i64,
    /// Whether the requirement names the resource by its ARN rather than by its name.
    by_arn: bool,
    route: Route,
}

/// The table. The rows span the route to the terminal status, the status the job was in
/// before it, the quantity required and the form the requirement names the resource in.
const ROWS: [Row; 4] = [
    Row {
        case: "terminated-by-name",
        quantity: 1,
        by_arn: false,
        route: Route::Terminate,
    },
    Row {
        case: "terminated-by-arn",
        quantity: 1_000_000,
        by_arn: true,
        route: Route::Terminate,
    },
    Row {
        case: "cancelled-zero",
        quantity: 0,
        by_arn: true,
        route: Route::Cancel,
    },
    Row {
        case: "dependency-failed",
        quantity: 3,
        by_arn: false,
        route: Route::DependencyFailed,
    },
];

/// `ListJobsByConsumableResource`, requiring the 200, and the `jobName`s it answered,
/// sorted so a case compares SETS — the paging order is
/// `jobs_by_consumable_resource.rs`'s own rule. Both helpers it is built from are that
/// suite's, so the two cannot drift on the route or the ARN form.
async fn listed_names(batch: &Batch, case: &str, body: Value) -> Vec<String> {
    let mut names = job_names(&listed(batch, case, body).await);
    names.sort();
    names
}

/// What one row of [`ROWS`] left in the store.
struct Built {
    case: &'static str,
    /// The job whose requirement the row releases.
    subject: String,
    /// A job requiring the same resource that stays live throughout.
    witness: String,
    /// The consumable resource both of them require.
    resource: String,
    /// The job the subject depends on, for the row that releases through the clock.
    dependency: Option<String>,
}

/// One emulator carrying, per row, a consumable resource and the jobs that require it.
struct Fixture {
    batch: Batch,
    jobs: Vec<Built>,
}

impl Fixture {
    /// What the row named `case` left in the store.
    fn row(&self, case: &str) -> &Built {
        self.jobs
            .iter()
            .find(|built| built.case == case)
            .unwrap_or_else(|| panic!("the fixture built a row called {case:?}"))
    }
}

/// Submit one job through `definition`, optionally depending on `depends_on`; answer its
/// id.
async fn submit(
    batch: &Batch,
    case: &str,
    queue_arn: &str,
    definition: &str,
    job_name: &str,
    depends_on: Option<&str>,
) -> String {
    let mut body = json!({
        "jobName": job_name,
        "jobQueue": queue_arn,
        "jobDefinition": definition,
    });
    if let Some(dependency) = depends_on {
        body["dependsOn"] = json!([{"jobId": dependency}]);
    }
    let answered = batch.ok("/v1/submitjob", body).await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a jobId, got {answered}"))
        .to_string()
}

/// An emulator with a queue, and per row a consumable resource, a job definition carrying
/// that row's requirement, a subject job and a witness job.
async fn fixture() -> Fixture {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, "ce-release").await;
    let queue_arn = job_queue(&batch, "q-release", &environment_arn).await;

    let mut jobs = Vec::new();
    for row in ROWS {
        let resource = format!("cr-{}", row.case);
        batch
            .ok(
                "/v1/createconsumableresource",
                json!({
                    "consumableResourceName": &resource,
                    "resourceType": "REPLENISHABLE",
                    "totalQuantity": 2_000_000,
                }),
            )
            .await;
        let reference = if row.by_arn {
            resource_arn(&resource)
        } else {
            resource.clone()
        };
        let registered = batch
            .ok(
                "/v1/registerjobdefinition",
                json!({
                    "jobDefinitionName": format!("jd-{}", row.case),
                    "type": "container",
                    "containerProperties": container_properties(),
                    "consumableResourceProperties": {
                        "consumableResourceList": [
                            {"consumableResource": reference, "quantity": row.quantity},
                        ],
                    },
                }),
            )
            .await;
        let definition = registered["jobDefinitionArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected a jobDefinitionArn, got {registered}",
                    row.case
                )
            })
            .to_string();

        // The dependency row's subject waits on a job that requires NONE of the resource,
        // so what the listing reports is decided by the subject and the witness alone.
        let dependency = if row.route == Route::DependencyFailed {
            let plain = batch
                .ok(
                    "/v1/registerjobdefinition",
                    json!({
                        "jobDefinitionName": format!("jd-{}-blocker", row.case),
                        "type": "container",
                        "containerProperties": container_properties(),
                    }),
                )
                .await;
            let plain_arn = plain["jobDefinitionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            Some(
                submit(
                    &batch,
                    row.case,
                    &queue_arn,
                    &plain_arn,
                    &format!("{}-blocker", row.case),
                    None,
                )
                .await,
            )
        } else {
            None
        };

        let subject = submit(
            &batch,
            row.case,
            &queue_arn,
            &definition,
            &format!("{}-subject", row.case),
            dependency.as_deref(),
        )
        .await;
        let witness = submit(
            &batch,
            row.case,
            &queue_arn,
            &definition,
            &format!("{}-witness", row.case),
            None,
        )
        .await;
        jobs.push(Built {
            case: row.case,
            subject,
            witness,
            resource,
            dependency,
        });
    }

    Fixture { batch, jobs }
}

/// RULE: a job that reached a terminal status requires the resource no longer, so no
/// listing of that resource reports it — not unfiltered, and not under any value the
/// `JOB_STATUS` filter documents, including its own terminal status.
#[tokio::test]
async fn a_terminal_job_drops_out_of_every_listing_of_its_resource() {
    let fixture = fixture().await;

    for row in ROWS {
        let Built {
            subject,
            witness,
            resource,
            dependency,
            ..
        } = fixture.row(row.case);
        let subject_name = format!("{}-subject", row.case);
        let witness_name = format!("{}-witness", row.case);

        // While it is live it IS listed: the rule is about the release, so the "before"
        // must hold or the case proves nothing.
        let live = if row.route == Route::DependencyFailed {
            // A job whose dependency has not succeeded waits in PENDING.
            "PENDING"
        } else {
            "RUNNABLE"
        };
        let detail = until_status(&fixture.batch, row.case, subject, live).await;
        assert_eq!(
            detail["status"].as_str(),
            Some(live),
            "case {}: expected the subject to be live before the release, got {detail}",
            row.case
        );
        // The witness's own span runs from ITS submit, which is a later instant than the
        // subject's: it is observed coming to rest HERE, so that what the case asserts
        // about it after the release is about the release and nothing else.
        until_status(&fixture.batch, row.case, witness, "RUNNABLE").await;
        let before = listed_names(
            &fixture.batch,
            row.case,
            json!({"consumableResource": resource}),
        )
        .await;
        assert!(
            before.contains(&subject_name) && before.contains(&witness_name),
            "case {}: expected both jobs requiring {resource} to be listed while live, got \
             {before:?}",
            row.case
        );

        // The route to the terminal status.
        match row.route {
            Route::Terminate => {
                fixture
                    .batch
                    .ok(
                        "/v1/terminatejob",
                        json!({"jobId": subject, "reason": "released by the suite"}),
                    )
                    .await;
            }
            Route::Cancel => {
                fixture
                    .batch
                    .ok(
                        "/v1/canceljob",
                        json!({"jobId": subject, "reason": "released by the suite"}),
                    )
                    .await;
            }
            // NOTHING names the subject here: ending the job it depends on is what makes
            // the clock fail it.
            Route::DependencyFailed => {
                let dependency = dependency.as_deref().unwrap_or_else(|| {
                    panic!("case {}: the row submitted a dependency", row.case)
                });
                fixture
                    .batch
                    .ok(
                        "/v1/terminatejob",
                        json!({"jobId": dependency, "reason": "the blocker is ended"}),
                    )
                    .await;
            }
        }
        let detail = until_status(&fixture.batch, row.case, subject, "FAILED").await;

        // The release: gone from the unfiltered listing.
        let after = listed_names(
            &fixture.batch,
            row.case,
            json!({"consumableResource": resource}),
        )
        .await;
        assert_eq!(
            after,
            vec![witness_name.clone()],
            "case {}: expected only the job still requiring {resource}, got {after:?}",
            row.case
        );

        // ... and gone under every documented status filter, its own included.
        for status in JOB_STATUS_FILTER_VALUES {
            let filtered = listed_names(
                &fixture.batch,
                row.case,
                json!({
                    "consumableResource": resource,
                    "filters": [{"name": "JOB_STATUS", "values": [status]}],
                }),
            )
            .await;
            assert!(
                !filtered.contains(&subject_name),
                "case {}: a released job must not be listed under JOB_STATUS={status}, got \
                 {filtered:?}",
                row.case
            );
        }

        // ... and gone when the request asks for it by name.
        let by_name = listed_names(
            &fixture.batch,
            row.case,
            json!({
                "consumableResource": resource,
                "filters": [{"name": "JOB_NAME", "values": [&subject_name]}],
            }),
        )
        .await;
        assert!(
            by_name.is_empty(),
            "case {}: a released job must not be listed under JOB_NAME={subject_name}, got \
             {by_name:?}",
            row.case
        );

        // The RECORD is untouched: only the requirement ended.
        assert_eq!(
            detail["status"].as_str(),
            Some("FAILED"),
            "case {}: expected the job's terminal status to stand, got {detail}",
            row.case
        );
        let required = detail["consumableResourceProperties"]["consumableResourceList"][0].clone();
        assert_eq!(
            required["quantity"].as_i64(),
            Some(row.quantity),
            "case {}: expected DescribeJobs to still replay the submitted requirement, got \
             {detail}",
            row.case
        );
        // The witness is where it was, so nothing but the subject was released.
        let witness_detail = describe_job(&fixture.batch, witness).await;
        assert_eq!(
            witness_detail["status"].as_str(),
            Some("RUNNABLE"),
            "case {}: the release moved no other job, got {witness_detail}",
            row.case
        );
    }
}

/// RULE: the release CONVERGES and the listing stays a read — once a job is terminal every
/// later listing of its resource answers the same thing, and listing moves no status.
#[tokio::test]
async fn the_listing_converges_once_the_job_is_terminal() {
    let fixture = fixture().await;
    let row = ROWS[0];
    let built = fixture.row(row.case);
    let (subject, witness, resource) = (&built.subject, &built.witness, &built.resource);

    until_status(&fixture.batch, row.case, subject, "RUNNABLE").await;
    // The witness comes to rest on its OWN span, one span after its own submit, so it is
    // observed there before the terminate: the probes below are then the only thing that
    // could move it.
    until_status(&fixture.batch, row.case, witness, "RUNNABLE").await;
    fixture
        .batch
        .ok(
            "/v1/terminatejob",
            json!({"jobId": subject, "reason": "released by the suite"}),
        )
        .await;
    until_status(&fixture.batch, row.case, subject, "FAILED").await;

    let witness_name = format!("{}-witness", row.case);
    for probe in 0..3 {
        let names = listed_names(
            &fixture.batch,
            row.case,
            json!({"consumableResource": resource}),
        )
        .await;
        assert_eq!(
            names,
            vec![witness_name.clone()],
            "case {} probe {probe}: the listing must not change from one read to the next, \
             got {names:?}",
            row.case
        );
    }
    // A listing is a read: the jobs are exactly where the writes left them.
    let subject_detail = describe_job(&fixture.batch, subject).await;
    assert_eq!(
        subject_detail["status"].as_str(),
        Some("FAILED"),
        "case {}: listing moved the terminated job's status, got {subject_detail}",
        row.case
    );
    let witness_detail = describe_job(&fixture.batch, witness).await;
    assert_eq!(
        witness_detail["status"].as_str(),
        Some("RUNNABLE"),
        "case {}: listing moved the live job's status, got {witness_detail}",
        row.case
    );
}
