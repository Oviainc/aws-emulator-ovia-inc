// SPDX-License-Identifier: CC0-1.0

//! The members MediaConvert fills in itself, on every projection of the resource.
//!
//! The rule under test: a member the service applies a value to when the request omits it
//! is PERSISTED on the record and reported by every projection of that resource — the
//! create response, the reader, the listing and the update response alike — and a member
//! that references another resource is projected as that resource's ARN, never as the bare
//! reference the request sent.
//!
//! The members, with where their value comes from:
//!
//! * `Queue$ConcurrentJobs` — the account's default concurrency for an on-demand queue;
//!   for a reserved queue the slots the reservation plan bought, because `ReservedSlots`'
//!   doc says "The number of RTS determines how many jobs the queue can process in
//!   parallel; each RTS can process one job at a time."
//! * `Queue$MaximumConcurrentFeeds` — none, the bottom of the member's `__integerMin0`
//!   range.
//! * `Job$Priority` / `JobTemplate$Priority` — zero: "If you don't specify a priority, the
//!   service uses the default value 0."
//! * `Job$BillingTagsSource` — `JOB`: "Leave blank to use the default value, Job."
//! * `Job$StatusUpdateInterval` / `JobTemplate$StatusUpdateInterval` — `SECONDS_60`, the
//!   service's default status-update cadence.
//! * `Job$AccelerationSettings` / `JobTemplate$AccelerationSettings` — `{mode: DISABLED}`,
//!   the state `AccelerationStatus`'s doc names ("For jobs that have Acceleration
//!   (AccelerationMode) set to DISABLED, AccelerationStatus is always NOT_APPLICABLE").
//! * `Job$Messages` — the container the service reports its own messages in; this service
//!   has produced none, so both of its lists are empty.
//! * `Job$ShareStatus` — `NOT_SHARED`, the status of a job no resource share names.
//! * `Job$Queue` — the ARN of the queue the job was submitted to.
//!
//! Every case drives the real restJson1 wire path through the router, so what is asserted
//! is the bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::{arn, settings as job_settings, Emulator, Reply, ROLE};

/// The queue every job case that names a queue is submitted to.
const FIXTURE_QUEUE: &str = "projection-queue";

/// The account default an on-demand queue created without `ConcurrentJobs` carries. The
/// model declares the member but no default; this is the value the recorded wire corpus
/// shows for such a queue.
const ACCOUNT_DEFAULT_CONCURRENT_JOBS: i64 = 200;

/// Every `StatusUpdateInterval` value, from
/// `smithy_query(query="shape",shape="StatusUpdateInterval")`. The whole catalogue is
/// round-tripped, not only the `SECONDS_60` default and not only the values the issue
/// named.
const STATUS_UPDATE_INTERVALS: [&str; 15] = [
    "SECONDS_10",
    "SECONDS_12",
    "SECONDS_15",
    "SECONDS_20",
    "SECONDS_30",
    "SECONDS_60",
    "SECONDS_120",
    "SECONDS_180",
    "SECONDS_240",
    "SECONDS_300",
    "SECONDS_360",
    "SECONDS_420",
    "SECONDS_480",
    "SECONDS_540",
    "SECONDS_600",
];

/// Every `AccelerationMode` value, from
/// `smithy_query(query="shape",shape="AccelerationMode")`.
const ACCELERATION_MODES: [&str; 3] = ["DISABLED", "ENABLED", "PREFERRED"];

/// Every `BillingTagsSource` value, from
/// `smithy_query(query="shape",shape="BillingTagsSource")`.
const BILLING_TAGS_SOURCES: [&str; 4] = ["QUEUE", "PRESET", "JOB_TEMPLATE", "JOB"];

/// A minimal `JobTemplateSettings`: one output group with one output.
fn template_settings() -> Value {
    json!({
        "outputGroups": [{
            "name": "File Group",
            "outputs": [{ "nameModifier": "_0" }],
            "outputGroupSettings": {
                "type": "FILE_GROUP_SETTINGS",
                "fileGroupSettings": { "destination": "s3://example-bucket/out/" }
            }
        }]
    })
}

/// `GET /2017-08-29/jobs` — `ListJobs`, which `job_lifecycle`'s harness keeps private.
async fn list_jobs(emulator: &Emulator) -> Reply {
    emulator.send("GET", "/2017-08-29/jobs", None).await
}

/// Assert one member of a projection.
fn assert_member(case: &str, where_from: &str, view: &Value, member: &str, want: &Value) {
    let got = view.pointer(&format!("/{member}")).unwrap_or(&Value::Null);
    assert_eq!(
        got, want,
        "case {case:?}: expected {member} to be {want} in the {where_from} projection, \
         got {got} in {view}"
    );
}

/// One row of the queue table: a `CreateQueue` request and the concurrency the stored
/// queue must carry afterwards.
struct QueueRow {
    case: &'static str,
    request: Value,
    concurrent_jobs: i64,
    maximum_concurrent_feeds: i64,
}

/// A queue always reports both concurrency members, whether the request set them or not,
/// and `GetQueue` reports the same values `CreateQueue` did.
///
/// The table spans the class: neither member given, each one alone, both, the published
/// minimum of `MaximumConcurrentFeeds`, a paused queue, and a reserved queue whose
/// concurrency comes from its reservation plan instead of the account default.
#[tokio::test]
async fn a_queue_always_reports_its_concurrency_members() {
    let emulator = Emulator::new();
    let rows = vec![
        QueueRow {
            case: "neither concurrency member given",
            request: json!({ "name": "q-neither" }),
            concurrent_jobs: ACCOUNT_DEFAULT_CONCURRENT_JOBS,
            maximum_concurrent_feeds: 0,
        },
        QueueRow {
            case: "only concurrentJobs given",
            request: json!({ "name": "q-jobs-only", "concurrentJobs": 1 }),
            concurrent_jobs: 1,
            maximum_concurrent_feeds: 0,
        },
        QueueRow {
            case: "only maximumConcurrentFeeds given",
            request: json!({ "name": "q-feeds-only", "maximumConcurrentFeeds": 7 }),
            concurrent_jobs: ACCOUNT_DEFAULT_CONCURRENT_JOBS,
            maximum_concurrent_feeds: 7,
        },
        QueueRow {
            case: "both given",
            request: json!({
                "name": "q-both",
                "concurrentJobs": 150,
                "maximumConcurrentFeeds": 3,
            }),
            concurrent_jobs: 150,
            maximum_concurrent_feeds: 3,
        },
        QueueRow {
            // `__integerMin0` publishes 0 as the bottom of the range.
            case: "maximumConcurrentFeeds at its published minimum",
            request: json!({ "name": "q-feeds-zero", "maximumConcurrentFeeds": 0 }),
            concurrent_jobs: ACCOUNT_DEFAULT_CONCURRENT_JOBS,
            maximum_concurrent_feeds: 0,
        },
        QueueRow {
            case: "a paused queue defaults exactly as an active one does",
            request: json!({ "name": "q-paused", "status": "PAUSED" }),
            concurrent_jobs: ACCOUNT_DEFAULT_CONCURRENT_JOBS,
            maximum_concurrent_feeds: 0,
        },
        QueueRow {
            // "For reserved queues, specify the number of jobs you can process
            // concurrently in your reservation plan instead."
            case: "a reserved queue takes its concurrency from its reservation plan",
            request: json!({
                "name": "q-reserved",
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": "EXPIRE",
                    "reservedSlots": 8,
                },
            }),
            concurrent_jobs: 8,
            maximum_concurrent_feeds: 0,
        },
    ];

    for row in &rows {
        let case = row.case;
        let created = emulator.create_queue(row.request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let name = row
            .request
            .pointer("/name")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let fetched = emulator.get_queue(name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the queue to read back with HTTP 200, got {} with \
             body {}",
            fetched.status, fetched.body
        );

        for (where_from, view) in [("create", created.at("/queue")), ("get", fetched.at("/queue"))]
        {
            assert_member(
                case,
                where_from,
                view,
                "concurrentJobs",
                &json!(row.concurrent_jobs),
            );
            assert_member(
                case,
                where_from,
                view,
                "maximumConcurrentFeeds",
                &json!(row.maximum_concurrent_feeds),
            );
        }
        assert_eq!(
            fetched.at("/queue"),
            created.at("/queue"),
            "case {case:?}: GetQueue must project the record CreateQueue persisted, got {} \
             after {}",
            fetched.body,
            created.body
        );
    }

    // The built-in queue nobody created carries both members too: a reader of it never
    // sees a missing concurrency either.
    let built_in = emulator.get_queue("Default").await;
    assert_eq!(
        built_in.status, 200,
        "case built-in queue: expected HTTP 200, got {} with body {}",
        built_in.status, built_in.body
    );
    assert_member(
        "built-in queue",
        "get",
        built_in.at("/queue"),
        "concurrentJobs",
        &json!(ACCOUNT_DEFAULT_CONCURRENT_JOBS),
    );
    assert_member(
        "built-in queue",
        "get",
        built_in.at("/queue"),
        "maximumConcurrentFeeds",
        &json!(0),
    );

    // One past the published bound of `MaximumConcurrentFeeds` is refused, and the refusal
    // stores nothing.
    let refused = emulator
        .create_queue(json!({ "name": "q-below-zero", "maximumConcurrentFeeds": -1 }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case one below the published minimum: expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
    let absent = emulator.get_queue("q-below-zero").await;
    assert_eq!(
        absent.status, 404,
        "case one below the published minimum: a refused create must store nothing, got {} \
         with body {}",
        absent.status, absent.body
    );
}

/// One row of the job table: the members a `CreateJob` request carries beyond `Role` and
/// `Settings`, and the members the stored job must carry afterwards.
struct JobRow {
    case: &'static str,
    /// Members merged into the request on top of `Role` and `Settings`.
    extras: Value,
    /// The queue NAME the job must be attributed to; the projection carries its ARN.
    queue: &'static str,
    /// The seven members asserted on every projection.
    expected: Value,
}

/// The members the service applies to a job, with the values a request that sets none of
/// them gets.
fn job_defaults() -> Value {
    json!({
        "priority": 0,
        "billingTagsSource": "JOB",
        "statusUpdateInterval": "SECONDS_60",
        "accelerationSettings": { "mode": "DISABLED" },
        "messages": { "info": [], "warning": [] },
        "shareStatus": "NOT_SHARED",
    })
}

/// `job_defaults` with `overrides` applied, for a row that supplies some of the members.
fn job_expectation(overrides: Value) -> Value {
    let mut expected = job_defaults();
    if let (Some(target), Some(overrides)) = (expected.as_object_mut(), overrides.as_object()) {
        for (member, value) in overrides {
            target.insert(member.clone(), value.clone());
        }
    }
    expected
}

/// A job always reports the six members the service applies, and its `Queue` is the ARN of
/// the queue it was submitted to — on the create response, on `GetJob` and on every
/// element of `ListJobs`.
///
/// The table spans the class: no optional member at all, the queue named by name and by
/// ARN, each defaulted member supplied on its own, both ends of the `Priority` range and
/// every member supplied at once.
#[tokio::test]
async fn a_job_always_reports_its_service_applied_members_and_a_queue_arn() {
    let emulator = Emulator::new();
    let queue = emulator
        .create_queue(json!({ "name": FIXTURE_QUEUE }))
        .await;
    assert_eq!(
        queue.status, 201,
        "the fixture queue must be created: got {} with body {}",
        queue.status, queue.body
    );

    let rows = vec![
        JobRow {
            case: "no optional member at all, so the built-in queue",
            extras: json!({}),
            queue: "Default",
            expected: job_defaults(),
        },
        JobRow {
            case: "the queue named by name",
            extras: json!({ "queue": FIXTURE_QUEUE }),
            queue: FIXTURE_QUEUE,
            expected: job_defaults(),
        },
        JobRow {
            case: "the same queue named by ARN",
            extras: json!({ "queue": arn(&format!("queues/{FIXTURE_QUEUE}")) }),
            queue: FIXTURE_QUEUE,
            expected: job_defaults(),
        },
        JobRow {
            case: "priority at the lower bound",
            extras: json!({ "priority": -50 }),
            queue: "Default",
            expected: job_expectation(json!({ "priority": -50 })),
        },
        JobRow {
            case: "priority at the upper bound",
            extras: json!({ "priority": 50 }),
            queue: "Default",
            expected: job_expectation(json!({ "priority": 50 })),
        },
        JobRow {
            case: "a billing tags source the request chose",
            extras: json!({ "queue": FIXTURE_QUEUE, "billingTagsSource": "PRESET" }),
            queue: FIXTURE_QUEUE,
            expected: job_expectation(json!({ "billingTagsSource": "PRESET" })),
        },
        JobRow {
            case: "an acceleration mode the request chose",
            extras: json!({ "accelerationSettings": { "mode": "PREFERRED" } }),
            queue: "Default",
            expected: job_expectation(json!({
                "accelerationSettings": { "mode": "PREFERRED" },
            })),
        },
        JobRow {
            case: "a status-update cadence the request chose",
            extras: json!({ "statusUpdateInterval": "SECONDS_480" }),
            queue: "Default",
            expected: job_expectation(json!({ "statusUpdateInterval": "SECONDS_480" })),
        },
        JobRow {
            case: "every defaulted member supplied at once",
            extras: json!({
                "queue": FIXTURE_QUEUE,
                "priority": 7,
                "billingTagsSource": "QUEUE",
                "statusUpdateInterval": "SECONDS_10",
                "accelerationSettings": { "mode": "ENABLED" },
            }),
            queue: FIXTURE_QUEUE,
            expected: job_expectation(json!({
                "priority": 7,
                "billingTagsSource": "QUEUE",
                "statusUpdateInterval": "SECONDS_10",
                "accelerationSettings": { "mode": "ENABLED" },
            })),
        },
    ];

    let mut created_jobs = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let mut request = json!({
            "role": ROLE,
            "settings": job_settings(&format!("s3://example-bucket/in/{index}.mp4"), "_0"),
        });
        if let (Some(target), Some(extras)) = (request.as_object_mut(), row.extras.as_object()) {
            for (member, value) in extras {
                target.insert(member.clone(), value.clone());
            }
        }

        let created = emulator.create_job(request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("/job/id");
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetJob HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );

        let listed = list_jobs(&emulator).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListJobs HTTP 200, got {} with body {}",
            listed.status, listed.body
        );
        let element = listed
            .at("/jobs")
            .as_array()
            .and_then(|jobs| {
                jobs.iter().find(|job| {
                    job.pointer("/id").and_then(Value::as_str) == Some(id.as_str())
                })
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            element.is_object(),
            "case {case:?}: the created job must be listed, got {}",
            listed.body
        );

        let want_queue = json!(arn(&format!("queues/{}", row.queue)));
        for (where_from, view) in [
            ("create", created.at("/job").clone()),
            ("get", fetched.at("/job").clone()),
            ("list", element.clone()),
        ] {
            assert_member(case, where_from, &view, "queue", &want_queue);
            for (member, want) in row.expected.as_object().into_iter().flatten() {
                assert_member(case, where_from, &view, member, want);
            }
        }
        created_jobs.push((case, created.at("/job").clone()));
    }

    // Reading never changes what a later caller sees: every job still projects exactly
    // what its create returned, after the whole table has been read and listed.
    for (case, created) in &created_jobs {
        let id = created
            .pointer("/id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let again = emulator.get_job(id).await;
        assert_eq!(
            again.at("/job"),
            created,
            "case {case:?}: a repeated read must answer the same job, got {}",
            again.body
        );
    }
}

/// One row of the job-template table: the members a `CreateJobTemplate` request carries
/// beyond `Name` and `Settings`, and the members the stored template must carry.
struct TemplateRow {
    case: &'static str,
    name: &'static str,
    extras: Value,
    expected: Value,
}

/// The members the service applies to a job template.
fn template_defaults() -> Value {
    json!({
        "priority": 0,
        "statusUpdateInterval": "SECONDS_60",
        "accelerationSettings": { "mode": "DISABLED" },
    })
}

/// `template_defaults` with `overrides` applied.
fn template_expectation(overrides: Value) -> Value {
    let mut expected = template_defaults();
    if let (Some(target), Some(overrides)) = (expected.as_object_mut(), overrides.as_object()) {
        for (member, value) in overrides {
            target.insert(member.clone(), value.clone());
        }
    }
    expected
}

/// A job template always reports the members the service applies — on the create response,
/// on `GetJobTemplate`, on every element of `ListJobTemplates` and on the template an
/// update answers with.
#[tokio::test]
async fn a_job_template_always_reports_its_service_applied_members() {
    let emulator = Emulator::new();
    let rows = vec![
        TemplateRow {
            case: "none of the defaulted members given",
            name: "t-neither",
            extras: json!({}),
            expected: template_defaults(),
        },
        TemplateRow {
            case: "a status-update cadence the request chose",
            name: "t-cadence",
            extras: json!({ "statusUpdateInterval": "SECONDS_540" }),
            expected: template_expectation(json!({ "statusUpdateInterval": "SECONDS_540" })),
        },
        TemplateRow {
            case: "an acceleration mode the request chose",
            name: "t-accelerated",
            extras: json!({ "accelerationSettings": { "mode": "ENABLED" } }),
            expected: template_expectation(json!({
                "accelerationSettings": { "mode": "ENABLED" },
            })),
        },
        TemplateRow {
            case: "a priority the request chose",
            name: "t-priority",
            extras: json!({ "priority": -50 }),
            expected: template_expectation(json!({ "priority": -50 })),
        },
        TemplateRow {
            case: "all three given",
            name: "t-all",
            extras: json!({
                "priority": 50,
                "statusUpdateInterval": "SECONDS_12",
                "accelerationSettings": { "mode": "PREFERRED" },
            }),
            expected: template_expectation(json!({
                "priority": 50,
                "statusUpdateInterval": "SECONDS_12",
                "accelerationSettings": { "mode": "PREFERRED" },
            })),
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut request = json!({ "name": row.name, "settings": template_settings() });
        if let (Some(target), Some(extras)) = (request.as_object_mut(), row.extras.as_object()) {
            for (member, value) in extras {
                target.insert(member.clone(), value.clone());
            }
        }

        let created = emulator.create_job_template(request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let fetched = emulator.get_job_template(row.name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetJobTemplate HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        let listed = emulator.list_job_templates("").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListJobTemplates HTTP 200, got {} with body {}",
            listed.status, listed.body
        );
        let element = listed
            .at("/jobTemplates")
            .as_array()
            .and_then(|templates| {
                templates.iter().find(|template| {
                    template.pointer("/name").and_then(Value::as_str) == Some(row.name)
                })
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            element.is_object(),
            "case {case:?}: the created template must be listed, got {}",
            listed.body
        );

        // An update that names NONE of these members keeps every one of them: the update
        // response is the same projection the create and the read answer with.
        let updated = emulator
            .update_job_template(row.name, json!({ "description": "updated" }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateJobTemplate HTTP 200, got {} with body {}",
            updated.status, updated.body
        );

        for (where_from, view) in [
            ("create", created.template().clone()),
            ("get", fetched.template().clone()),
            ("list", element),
            ("update", updated.template().clone()),
        ] {
            for (member, want) in row.expected.as_object().into_iter().flatten() {
                assert_member(case, where_from, &view, member, want);
            }
        }
    }

    // A built-in template nobody created reports them too.
    let built_in = emulator.list_job_templates("listBy=SYSTEM").await;
    let first = built_in
        .at("/jobTemplates/0")
        .clone();
    for (member, want) in template_defaults().as_object().into_iter().flatten() {
        assert_member("built-in template", "list", &first, member, want);
    }
}

/// Every value of the three enumerated members round-trips: a value the request sets is
/// the value every projection reports, and only an absent member takes the service's own.
///
/// The catalogues come from the model (see the `const`s at the top of this file), so the
/// case set is the whole published class rather than the values the recordings happened to
/// carry.
#[tokio::test]
async fn every_published_enum_value_round_trips_through_the_projections() {
    let emulator = Emulator::new();

    for (index, interval) in STATUS_UPDATE_INTERVALS.iter().enumerate() {
        let name = format!("interval-{index}");
        let created = emulator
            .create_job_template(json!({
                "name": &name,
                "statusUpdateInterval": interval,
                "settings": template_settings(),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case interval {interval:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let fetched = emulator.get_job_template(&name).await;
        for (where_from, view) in [
            ("create", created.template().clone()),
            ("get", fetched.template().clone()),
        ] {
            assert_member(
                interval,
                where_from,
                &view,
                "statusUpdateInterval",
                &json!(interval),
            );
        }
    }

    for (index, mode) in ACCELERATION_MODES.iter().enumerate() {
        for (billing_index, source) in BILLING_TAGS_SOURCES.iter().enumerate() {
            let case = format!("mode {mode} with billing tags source {source}");
            let created = emulator
                .create_job(json!({
                    "role": ROLE,
                    "accelerationSettings": { "mode": mode },
                    "billingTagsSource": source,
                    "settings": job_settings(
                        &format!("s3://example-bucket/in/{index}-{billing_index}.mp4"),
                        "_0",
                    ),
                }))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            let fetched = emulator.get_job(&created.string("/job/id")).await;
            for (where_from, view) in [
                ("create", created.at("/job").clone()),
                ("get", fetched.at("/job").clone()),
            ] {
                assert_member(
                    &case,
                    where_from,
                    &view,
                    "accelerationSettings",
                    &json!({ "mode": mode }),
                );
                assert_member(&case, where_from, &view, "billingTagsSource", &json!(source));
                // The members the request said nothing about still carry the service's
                // own values.
                assert_member(&case, where_from, &view, "priority", &json!(0));
                assert_member(
                    &case,
                    where_from,
                    &view,
                    "statusUpdateInterval",
                    &json!("SECONDS_60"),
                );
                assert_member(&case, where_from, &view, "shareStatus", &json!("NOT_SHARED"));
                assert_member(
                    &case,
                    where_from,
                    &view,
                    "messages",
                    &json!({ "info": [], "warning": [] }),
                );
            }
        }
    }
}
