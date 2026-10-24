// SPDX-License-Identifier: CC0-1.0

//! `GetJobUnlockCode`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE rather than over a single job: a `GetJobUnlockCode`
//! call answers the unlock code this service minted for THAT job when the job was created
//! — the documented 29-character form, the same code on every read, distinct from every
//! other job's — and only while the job holds a device of its own, is in the one state the
//! device is in the caller's hands (`GetJobUnlockCode`: "The only valid status for calling
//! this API is WithCustomer"), and is inside the window the code is readable for ("A
//! particular UnlockCode value can be accessed for up to 360 days after the associated job
//! has been created"). Every other call is refused: a job no record answers to and the
//! main job of an export order, which "aren't associated with any Snowballs"
//! (`JobListEntry$IsMaster`), as `InvalidResourceException`, every other `JobState` as
//! `InvalidJobStateException`, and a job past its window as `InvalidResourceException`.
//!
//! The rows differ along what the rule generalises over: the job type and device the order
//! was placed for, whether the job is a plain job or one job part of an export order, the
//! job's state (the WHOLE modelled `JobState` catalog), how old the job is relative to the
//! published window (both sides of the bound and one well past it), and how many times the
//! same job is asked.
//!
//! The shipment states this API moves a job through are not served by this emulator yet,
//! so a row that needs a job in a state other than the one `CreateJob` mints sets that
//! state in the service's own store before calling — the store the handler reads, reached
//! the way the service reaches it. Everything the rule is about still travels over the
//! wire.
//!
//! The call harness (`call_in_region`, `ok`, `refused`, `string`, `create_address`,
//! `deliverable_address`) is the one `job_lifecycle` already drives the service with.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
// The modelled `JobState` catalog and the published window and code form, read from the
// crate rather than restated, so a case cannot drift from the rule it asserts.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::JOB_STATE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    service_state, JobState, UNLOCK_CODE_GROUPS, UNLOCK_CODE_GROUP_LEN, UNLOCK_CODE_LEN,
    UNLOCK_CODE_VALIDITY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, string,
};

/// The one state a job's unlock code is released from.
const RELEASING_STATE: &str = "WithCustomer";

/// A router with just this service mounted, plus the engine it was mounted on, so a row
/// can put a job into a state this emulator's own operations do not reach yet.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still routes by the SigV4
    // credential scope the harness signs with.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Put `job_id` into the state `wire` names, which must be one the model declares.
fn set_job_state(engine: &Arc<engine::InMemoryEngine>, job_id: &str, wire: &str) {
    let state = JobState::from_wire(wire)
        .unwrap_or_else(|| panic!("{wire:?} is not a JobState this service models"));
    let store = service_state(&**engine).expect("the service's store is on this engine");
    let mut store = store.lock();
    let job = store
        .jobs
        .get_mut(job_id)
        .unwrap_or_else(|| panic!("the store holds the job {job_id}"));
    job.job_state = state;
}

/// Make `job_id` `age` old, so a row can stand on either side of the window the unlock
/// code is readable for without waiting for it.
fn age_job(engine: &Arc<engine::InMemoryEngine>, job_id: &str, age: Duration) {
    let store = service_state(&**engine).expect("the service's store is on this engine");
    let mut store = store.lock();
    let job = store
        .jobs
        .get_mut(job_id)
        .unwrap_or_else(|| panic!("the store holds the job {job_id}"));
    job.creation_date = SystemTime::now() - age;
}

/// Assert `code` has the form `GetJobUnlockCode` documents: "a 29-character code with 25
/// alphanumeric characters and 4 hyphens".
fn assert_unlock_code_form(code: &str, case: &str) {
    assert_eq!(
        code.len(),
        UNLOCK_CODE_LEN,
        "case {case:?}: expected a {UNLOCK_CODE_LEN}-character unlock code, got {code:?}"
    );
    let groups: Vec<&str> = code.split('-').collect();
    assert_eq!(
        groups.len(),
        UNLOCK_CODE_GROUPS,
        "case {case:?}: expected {UNLOCK_CODE_GROUPS} hyphen-separated groups, got {code:?}"
    );
    for group in &groups {
        assert!(
            group.len() == UNLOCK_CODE_GROUP_LEN
                && group.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "case {case:?}: expected groups of {UNLOCK_CODE_GROUP_LEN} alphanumeric \
             characters, got {code:?}"
        );
    }
}

/// Ask `GetJobUnlockCode` for `job_id` and assert the code it answers has the documented
/// form. Returns the code.
async fn unlock_code(router: &MultiServiceRouter, job_id: &str, case: &str) -> String {
    let answered = ok(router, "GetJobUnlockCode", json!({ "JobId": job_id }), case).await;
    let code = string(&answered, "UnlockCode", case);
    assert_unlock_code_form(&code, case);
    code
}

/// Create one job of `submitted`'s shape against a deliverable address and return its
/// `JobId`. The row's members are merged over the harness's order defaults
/// (`job_lifecycle::complete_order`), so a row states only the axis it varies.
async fn create_job(router: &MultiServiceRouter, submitted: Value, case: &str) -> String {
    let address_id = create_address(
        router,
        deliverable_address(case, "123 Main Street", json!({})),
        case,
    )
    .await;
    let mut body = complete_order(&submitted);
    body["AddressId"] = json!(address_id);
    let created = ok(router, "CreateJob", body, case).await;
    string(&created, "JobId", case)
}

/// The job ids `DescribeJob` reports as `job_id`'s job parts — empty for a job with none.
async fn job_parts(router: &MultiServiceRouter, job_id: &str, case: &str) -> Vec<String> {
    let described = ok(router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    described
        .get("SubJobMetadata")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().map(|part| string(part, "JobId", case)).collect())
        .unwrap_or_default()
}

/// Every order row: one per job type and device the unlock code is asked for, so the rule
/// is asserted over the class of persisted jobs rather than over one of them.
fn order_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "import job on an EDGE device",
            json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::unlock-bucket" }] },
            }),
        ),
        (
            "local-use job on a Snowcone SSD",
            json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "SNC1_SSD",
                "ShippingOption": "NEXT_DAY",
                "RemoteManagement": "INSTALLED_AUTOSTART",
            }),
        ),
        (
            "an order naming only its job type",
            json!({ "JobType": "IMPORT" }),
        ),
        (
            "export order, whose job parts each carry a device",
            json!({
                "JobType": "EXPORT",
                "SnowballType": "EDGE_S",
                "ShippingOption": "EXPRESS",
                "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }] },
            }),
        ),
    ]
}

#[tokio::test]
async fn every_device_bearing_job_answers_its_own_minted_unlock_code() {
    let (router, engine) = router_with_engine();
    let mut codes: BTreeSet<String> = BTreeSet::new();

    for (case, submitted) in order_rows() {
        let job_id = create_job(&router, submitted, case).await;
        // An export create mints a MAIN job plus one job part per device; it is the parts
        // that carry a device, so they are what the unlock code is asked for. Every other
        // create mints one plain job, which carries its own device.
        let parts = job_parts(&router, &job_id, case).await;
        let with_a_device: Vec<String> = if parts.is_empty() {
            vec![job_id.clone()]
        } else {
            parts
        };

        for job in &with_a_device {
            set_job_state(&engine, job, RELEASING_STATE);
            let first = unlock_code(&router, job, case).await;
            // The code is the job's own persisted credential, so asking twice answers the
            // same code rather than minting a second one.
            let second = unlock_code(&router, job, case).await;
            assert_eq!(
                second, first,
                "case {case:?}: expected the same unlock code on a second read of {job}, \
                 got {second:?} after {first:?}"
            );
            assert!(
                codes.insert(first.clone()),
                "case {case:?}: job {job} answered {first:?}, which another job already \
                 answered"
            );
        }
    }

    assert!(
        codes.len() >= order_rows().len(),
        "expected one distinct unlock code per device-bearing job, got {}",
        codes.len()
    );
}

#[tokio::test]
async fn the_unlock_code_is_released_only_while_the_customer_holds_the_device() {
    let (router, engine) = router_with_engine();
    let case = "one job walked through every modelled JobState";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;

    // The rows are the WHOLE modelled `JobState` catalog, so the rule is asserted over
    // every state a job can be in rather than over the one `CreateJob` mints.
    assert!(
        JOB_STATE_VALUES.contains(&RELEASING_STATE),
        "expected {RELEASING_STATE} among the modelled JobState values, got {JOB_STATE_VALUES:?}"
    );
    for wire in JOB_STATE_VALUES {
        let case = format!("{case}: in state {wire}");
        set_job_state(&engine, &job_id, wire);
        if *wire == RELEASING_STATE {
            unlock_code(&router, &job_id, &case).await;
        } else {
            refused(
                &router,
                "GetJobUnlockCode",
                json!({ "JobId": job_id }),
                "InvalidJobStateException",
                &case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_job_with_no_unlock_code_of_its_own_is_refused_as_an_invalid_resource() {
    let (router, engine) = router_with_engine();

    // An export order: the main job is associated with no device, its job parts are. It is
    // put into the releasing state so the refusal can only be about the device it lacks.
    let export_case = "export order whose main job has no device";
    let main_job = create_job(
        &router,
        json!({
            "JobType": "EXPORT",
            "SnowballType": "EDGE",
            "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }] },
        }),
        export_case,
    )
    .await;
    assert!(
        !job_parts(&router, &main_job, export_case).await.is_empty(),
        "case {export_case:?}: expected the export create to mint job parts"
    );
    set_job_state(&engine, &main_job, RELEASING_STATE);

    let rows: Vec<(&str, String)> = vec![
        (
            "a JID that was never created",
            "JID00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an MID that was never created",
            "MID00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (export_case, main_job),
    ];

    for (case, job_id) in rows {
        assert_eq!(
            job_id.len(),
            39,
            "case {case:?}: {job_id} is not a 39-character JobId"
        );
        refused(
            &router,
            "GetJobUnlockCode",
            json!({ "JobId": job_id }),
            "InvalidResourceException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_unlock_code_is_readable_only_inside_the_published_window() {
    let (router, engine) = router_with_engine();

    // Both sides of the published bound, and one age well past it: the window runs from
    // the job's creation instant, so a job older than it has no code left to report.
    let hour = Duration::from_secs(60 * 60);
    let rows: Vec<(&str, Duration, bool)> = vec![
        ("a job created just now", Duration::ZERO, true),
        (
            "a job an hour short of the window's end",
            UNLOCK_CODE_VALIDITY - hour,
            true,
        ),
        ("a job exactly at the window's end", UNLOCK_CODE_VALIDITY, false),
        (
            "a job an hour past the window's end",
            UNLOCK_CODE_VALIDITY + hour,
            false,
        ),
        (
            "a job twice the window old",
            UNLOCK_CODE_VALIDITY * 2,
            false,
        ),
    ];

    for (case, age, readable) in rows {
        let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
        set_job_state(&engine, &job_id, RELEASING_STATE);
        age_job(&engine, &job_id, age);
        if readable {
            unlock_code(&router, &job_id, case).await;
        } else {
            refused(
                &router,
                "GetJobUnlockCode",
                json!({ "JobId": job_id }),
                "InvalidResourceException",
                case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn reading_the_unlock_code_changes_nothing_a_later_caller_reads() {
    let (router, engine) = router_with_engine();
    let case = "a job read before and after its unlock code is asked for";
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE" }),
        case,
    )
    .await;
    set_job_state(&engine, &job_id, RELEASING_STATE);

    let before = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_before = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;

    for attempt in 1..=3 {
        let case = format!("{case}: call {attempt}");
        unlock_code(&router, &job_id, &case).await;
    }

    let after = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_after = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    assert_eq!(
        after, before,
        "case {case:?}: GetJobUnlockCode moved the job DescribeJob reports"
    );
    assert_eq!(
        listed_after, listed_before,
        "case {case:?}: GetJobUnlockCode changed what ListJobs reports"
    );
}
