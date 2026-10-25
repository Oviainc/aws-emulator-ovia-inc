// SPDX-License-Identifier: CC0-1.0

//! The cloud-side Amazon EC2 instance cache of a managed device as STATE THAT
//! EVOLVES, end to end over HTTP.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted store and serialization — the same path a client takes.
//!
//! The rules under test:
//!
//! 1. an instance state that names work ALREADY UNDER WAY is owed the state that
//!    work ends in, and the cache reports that outcome once the owed instant has
//!    passed — a state an instance RESTS in is owed nothing and is reported as it
//!    stands;
//! 2. the freshness members (`InstanceSummary$lastUpdatedAt`, `Instance$updatedAt`)
//!    report WHEN the cache entry last changed: they advance to the instant a change
//!    happened, and they do not advance while nothing changed;
//! 3. a device that answered a `reboot` reports its whole instance set afresh: the
//!    freshness members of every entry advance, work that was under way has ended,
//!    and an instance that was up comes back up — while a device the task did not
//!    target is untouched;
//! 4. nothing but the clock is needed: passing time alone carries a starting
//!    instance to running.
//!
//! Each case is a TABLE whose rows differ along what the rules generalise over — the
//! state the entry is in, whether its owed instant has passed, the device, and how
//! many instances that device runs.
//!
//! The tiny HTTP harness below is local to this suite on purpose: a sibling suite's
//! helpers are private to its module, and this file must keep compiling while that
//! one changes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    device_instance_ids, managed_device_id, service_state, EXECUTION_FINISH_SPAN,
    EXECUTION_START_SPAN, INSTANCE_TRANSITION_SPAN,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request in this suite signs for, which is also the Region its
/// devices are activated in.
const SIGNED_REGION: &str = "us-west-2";

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/snow-device-management/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// How many devices this suite activates Snow Device Management on. The fixture puts
/// a different number of instances on each, including none on the third.
const ACTIVATED_DEVICES: u64 = 3;

/// How much slack a comparison of two reported instants allows, in seconds. The
/// instants under test are hundreds of milliseconds apart, so this is well inside
/// the differences the cases assert.
const TOLERANCE: f64 = 1e-2;

/// `InstanceStateName`'s values, each with the `InstanceState$code` the model
/// documents for it and the state the work that value names ends in — `None` for a
/// value naming a state an instance RESTS in, which owes nothing.
///
/// Names from `smithy_query(query="shape",shape="InstanceStateName")`, codes from
/// `smithy_query(query="shape",shape="InstanceState")` ("the valid values for the
/// instance state code": `0` pending, `16` running, `32` shutting-down, `48`
/// terminated, `64` stopping, `80` stopped). The outcomes are the EC2 state
/// machine's: an instance that is starting ends up running, one that is shutting
/// down ends up terminated, one that is stopping ends up stopped. Written out here
/// rather than imported, so a wrong catalog in the implementation cannot make this
/// suite pass.
const INSTANCE_STATES: &[(&str, i64, Option<&str>)] = &[
    ("PENDING", 0, Some("RUNNING")),
    ("RUNNING", 16, None),
    ("SHUTTING_DOWN", 32, Some("TERMINATED")),
    ("TERMINATED", 48, None),
    ("STOPPING", 64, Some("STOPPED")),
    ("STOPPED", 80, None),
];

/// The state one instance of this suite's inventory is reported in after the device
/// it runs on restarted, by the state it was in before.
///
/// Restarting a device restarts the compute environment its instances run in: work
/// that was under way ends with the restart, and an instance that would then be up
/// comes back up — which an instance reports as starting. An instance that was
/// deliberately stopped, or that no longer exists, stays that way.
fn state_after_a_restart(before: &str) -> &'static str {
    let ended = INSTANCE_STATES
        .iter()
        .find(|(name, _, _)| *name == before)
        .and_then(|(_, _, outcome)| *outcome)
        .unwrap_or(before);
    if ended == "RUNNING" {
        "PENDING"
    } else {
        INSTANCE_STATES
            .iter()
            .find(|(name, _, _)| *name == ended)
            .expect("a documented state ends in a documented state")
            .0
    }
}

/// The `InstanceState$code` this suite's catalog documents for `state_name`.
fn documented_code(state_name: &str) -> i64 {
    INSTANCE_STATES
        .iter()
        .find(|(name, _, _)| *name == state_name)
        .unwrap_or_else(|| panic!("{state_name:?} is not an InstanceStateName value"))
        .1
}

/// One emulator and the engine its state lives in: a router carrying only this
/// service, with Snow Device Management activated on [`ACTIVATED_DEVICES`] ordered
/// devices in [`SIGNED_REGION`].
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// The emulator every case starts from, with the fixture's OWED instants held past
/// this suite's lifetime.
///
/// The API models no device-creation operation, so the activation goes through the
/// one state seam that puts a device in the inventory. That fixture arms the
/// transitions of the instances it reports as starting, shutting down or stopping a
/// few hundred milliseconds out; each case below stages the entry and the instant it
/// is about, so the fixture's own instants are moved out of the way first and no case
/// depends on how fast the machine runs it.
fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = service_state(&*engine).expect("the in-memory engine carries this service's state");
        let mut store = state.lock();
        store.activate_devices(SIGNED_REGION, ACTIVATED_DEVICES, SystemTime::now());
        let held = SystemTime::now() + Duration::from_secs(3600);
        for device in store.devices.values_mut() {
            for instance in device.ec2_instances.values_mut() {
                if instance.transition_due.is_some() {
                    instance.transition_due = Some(held);
                }
            }
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "snow-device-management.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_SCOPE);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Assert a call succeeded and hand back its body.
fn ok(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body.clone()
}

/// `DescribeDeviceEc2Instances`, as a map from `instanceId` to the reported summary.
async fn cache_of(
    em: &Emulator,
    device: &str,
    instance_ids: &[String],
    case: &str,
) -> BTreeMap<String, Value> {
    let answer = call(
        &em.router,
        "POST",
        &format!("/managed-device/{device}/resources/ec2/describe"),
        Some(json!({ "instanceIds": instance_ids })),
    )
    .await;
    let body = ok(&answer, case);
    body["instances"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an instances array, got {body}"))
        .iter()
        .map(|summary| {
            let id = summary["instance"]["instanceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an instanceId, got {summary}"))
                .to_string();
            (id, summary.clone())
        })
        .collect()
}

/// `CreateTask`: `POST /task`.
async fn create_task(em: &Emulator, request: Value) -> Answer {
    call(&em.router, "POST", "/task", Some(request)).await
}

/// `DescribeTask`: `POST /task/{taskId}`.
async fn describe_task(em: &Emulator, task_id: &str) -> Answer {
    call(&em.router, "POST", &format!("/task/{task_id}"), None).await
}

/// The epoch-seconds the timestamp at `path` carries.
fn epoch(summary: &Value, path: &[&str], case: &str) -> f64 {
    let mut cursor = summary;
    for key in path {
        cursor = &cursor[*key];
    }
    cursor.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected {path:?} to report a timestamp, got {summary}")
    })
}

/// `at` as the epoch-seconds the wire carries it in.
fn seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// The reported state name of one summary.
fn state_name(summary: &Value, case: &str) -> String {
    summary["instance"]["state"]["name"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a state name, got {summary}"))
        .to_string()
}

/// Every rule a reported summary satisfies whatever the case: its state code is the
/// one its name is documented with, and both freshness members report the same
/// instant, since both say when the entry was last updated.
fn assert_consistent(summary: &Value, case: &str) {
    let name = state_name(summary, case);
    let code = summary["instance"]["state"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a state code, got {summary}"));
    assert_eq!(
        code,
        documented_code(&name),
        "case {case:?}: expected the documented code for {name:?}, got {code}"
    );
    let summary_updated = epoch(summary, &["lastUpdatedAt"], case);
    let instance_updated = epoch(summary, &["instance", "updatedAt"], case);
    assert!(
        (summary_updated - instance_updated).abs() < TOLERANCE,
        "case {case:?}: an InstanceSummary is as fresh as the instance it reports, \
         got {summary_updated} and {instance_updated}"
    );
}

/// Put one instance of one device in the state, freshness and owed instant a row
/// wants, through the state seam the inventory itself is staged through.
///
/// The cache is written by the device and not by any request this API models, so a
/// case that is about an entry the device pushed stages it here, the same way the
/// inventory is staged.
fn stage_instance(
    em: &Emulator,
    device: &str,
    instance: &str,
    state_name: &str,
    created_at: SystemTime,
    updated_at: SystemTime,
    transition_due: Option<SystemTime>,
) {
    let state = service_state(&*em.engine).expect("the engine carries this service's state");
    let mut store = state.lock();
    let entry = store
        .devices
        .get_mut(device)
        .unwrap_or_else(|| panic!("the suite activated {device}"))
        .ec2_instances
        .get_mut(instance)
        .unwrap_or_else(|| panic!("the fixture put {instance} on {device}"));
    entry.state_name = state_name.to_string();
    entry.state_code =
        i32::try_from(documented_code(state_name)).expect("a documented state code fits in i32");
    entry.created_at = created_at;
    entry.updated_at = updated_at;
    entry.transition_due = transition_due;
}

#[tokio::test]
async fn an_in_flight_instance_state_resolves_once_the_instant_it_is_owed_has_passed() {
    let em = emulator();
    let device = managed_device_id(0);
    let held = device_instance_ids(0);
    let instance = held
        .first()
        .expect("the fixture puts instances on the first device")
        .clone();
    let base = SystemTime::now();
    let created_at = base - Duration::from_secs(600);
    let updated_at = base - Duration::from_secs(300);

    // The table: every documented state × whether the instant the clock owes it has
    // passed. A state an instance rests in owes nothing, so for it only the first
    // half of the row applies.
    for (name, _, outcome) in INSTANCE_STATES {
        // Nothing is due yet (and for a resting state nothing ever is): the cache
        // reports the entry as it stands, with the freshness it was pushed with.
        let case = format!("{name}, nothing due yet");
        let not_yet = outcome.map(|_| base + Duration::from_secs(3600));
        stage_instance(&em, &device, &instance, name, created_at, updated_at, not_yet);
        let reported = cache_of(&em, &device, &held, &case).await;
        let summary = reported
            .get(&instance)
            .unwrap_or_else(|| panic!("case {case:?}: the cache reports the staged instance"));
        assert_consistent(summary, &case);
        assert_eq!(
            state_name(summary, &case),
            *name,
            "case {case:?}: a state with nothing due is reported as it stands"
        );
        let reported_updated = epoch(summary, &["lastUpdatedAt"], &case);
        assert!(
            (reported_updated - seconds(updated_at)).abs() < TOLERANCE,
            "case {case:?}: freshness must not advance while nothing changed, \
             expected {} got {reported_updated}",
            seconds(updated_at)
        );
        let reported_created = epoch(summary, &["instance", "createdAt"], &case);
        assert!(
            (reported_created - seconds(created_at)).abs() < TOLERANCE,
            "case {case:?}: expected the stored createdAt {} got {reported_created}",
            seconds(created_at)
        );

        let Some(outcome) = *outcome else {
            continue;
        };

        // The instant has passed: the cache reports the state the work ended in, as
        // of THAT instant and not of the call that happened to look.
        for (row, due) in [
            ("just past", base - Duration::from_secs(5)),
            ("long past", base - Duration::from_secs(3600)),
        ] {
            let case = format!("{name}, due {row}");
            let due = due.max(updated_at);
            stage_instance(
                &em,
                &device,
                &instance,
                name,
                created_at,
                updated_at,
                Some(due),
            );
            let reported = cache_of(&em, &device, &held, &case).await;
            let summary = reported
                .get(&instance)
                .unwrap_or_else(|| panic!("case {case:?}: the cache reports the staged instance"));
            assert_consistent(summary, &case);
            assert_eq!(
                state_name(summary, &case),
                outcome,
                "case {case:?}: work that was under way ends in {outcome:?}"
            );
            let reported_updated = epoch(summary, &["lastUpdatedAt"], &case);
            assert!(
                (reported_updated - seconds(due)).abs() < TOLERANCE,
                "case {case:?}: freshness must advance to the instant the change was \
                 owed, expected {} got {reported_updated}",
                seconds(due)
            );
            let reported_created = epoch(summary, &["instance", "createdAt"], &case);
            assert!(
                (reported_created - seconds(created_at)).abs() < TOLERANCE,
                "case {case:?}: resolving a state does not re-create the instance, \
                 expected createdAt {} got {reported_created}",
                seconds(created_at)
            );
            // A volume is attached exactly while its instance exists to hold it, so
            // the projection follows the state the entry resolved to.
            let attachment = summary["instance"]["blockDeviceMappings"][0]["ebs"]["status"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an ebs status, got {summary}"));
            assert_eq!(
                attachment == "ATTACHED",
                outcome != "TERMINATED",
                "case {case:?}: expected the attachment to follow the resolved state \
                 {outcome:?}, got {attachment:?}"
            );

            // The resolution is a materialised change and not a per-call answer: a
            // second identical call reports exactly the same entry.
            let again = cache_of(&em, &device, &held, &case).await;
            assert_eq!(
                again.get(&instance),
                Some(summary),
                "case {case:?}: a resolved entry is state, so a later identical call \
                 reports it unchanged"
            );
        }
    }
}

#[tokio::test]
async fn time_alone_carries_a_starting_instance_to_running() {
    let em = emulator();
    let device = managed_device_id(0);
    let held = device_instance_ids(0);
    let instance = held
        .first()
        .expect("the fixture puts instances on the first device")
        .clone();
    let case = "a starting instance, with nothing but the clock";
    let started = SystemTime::now();
    stage_instance(
        &em,
        &device,
        &instance,
        "PENDING",
        started - Duration::from_secs(60),
        started,
        Some(started + INSTANCE_TRANSITION_SPAN),
    );

    // The span is never zero, so a caller reading right after the instance started
    // still sees it starting.
    let reported = cache_of(&em, &device, &held, case).await;
    let before = reported
        .get(&instance)
        .unwrap_or_else(|| panic!("case {case:?}: the cache reports the staged instance"));
    assert_eq!(
        state_name(before, case),
        "PENDING",
        "case {case:?}: an instance that has just started is reported starting"
    );

    // Past the span, and with no call of any kind in between, it is up.
    tokio::time::sleep(INSTANCE_TRANSITION_SPAN + Duration::from_millis(250)).await;
    let reported = cache_of(&em, &device, &held, case).await;
    let after = reported
        .get(&instance)
        .unwrap_or_else(|| panic!("case {case:?}: the cache reports the staged instance"));
    assert_consistent(after, case);
    assert_eq!(
        state_name(after, case),
        "RUNNING",
        "case {case:?}: an instance that is starting ends up running, so polling it \
         must not report {:?} for ever",
        state_name(before, case)
    );
    let advanced = epoch(after, &["lastUpdatedAt"], case);
    let owed = seconds(started + INSTANCE_TRANSITION_SPAN);
    assert!(
        (advanced - owed).abs() < TOLERANCE,
        "case {case:?}: freshness must advance to the instant the change was owed, \
         expected {owed} got {advanced}"
    );
}

#[tokio::test]
async fn a_completed_reboot_has_the_device_report_its_instance_set_afresh() {
    let em = emulator();
    let rebooted = managed_device_id(0);
    let untouched = managed_device_id(1);
    let rebooted_ids = device_instance_ids(0);
    let untouched_ids = device_instance_ids(1);
    assert!(
        !rebooted_ids.is_empty() && !untouched_ids.is_empty(),
        "case \"fixture\": the inventory must run instances on two devices for a \
         per-device reboot to be observable, got {} and {}",
        rebooted_ids.len(),
        untouched_ids.len()
    );

    // The table rows: the state each entry of the rebooted device is in before the
    // reboot, spanning the documented catalog, with the resting and the in-flight
    // states spread over the two devices so the untouched device is checked too.
    let staged = SystemTime::now();
    let created_at = staged - Duration::from_secs(600);
    let mut before_states: BTreeMap<String, String> = BTreeMap::new();
    for (slot, instance) in rebooted_ids.iter().chain(untouched_ids.iter()).enumerate() {
        let (name, _, outcome) = INSTANCE_STATES[slot % INSTANCE_STATES.len()];
        let device = if rebooted_ids.contains(instance) {
            &rebooted
        } else {
            &untouched
        };
        stage_instance(
            &em,
            device,
            instance,
            name,
            created_at,
            staged,
            // Held past the case's lifetime: the reboot is the only event under
            // test, so nothing must resolve on its own in the meantime.
            outcome.map(|_| staged + Duration::from_secs(3600)),
        );
        before_states.insert(instance.clone(), name.to_string());
    }

    let before = cache_of(&em, &rebooted, &rebooted_ids, "before the reboot").await;
    let other_before = cache_of(&em, &untouched, &untouched_ids, "before the reboot").await;

    // "Reboots the device." — the task is created and the clock carries it to
    // SUCCEEDED, which is when the command reaches the device.
    let created = ok(
        &create_task(
            &em,
            json!({ "targets": [&rebooted], "command": { "reboot": {} } }),
        )
        .await,
        "reboot create",
    );
    let task_id = created["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"reboot create\": expected a taskId, got {created}"))
        .to_string();
    tokio::time::sleep(EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + Duration::from_millis(400))
        .await;
    let settled = ok(&describe_task(&em, &task_id).await, "reboot completes");
    assert_eq!(
        settled["state"].as_str(),
        Some("COMPLETED"),
        "case \"reboot completes\": the task must have finished for its effect to be \
         observable, got {settled}"
    );

    let after = cache_of(&em, &rebooted, &rebooted_ids, "after the reboot").await;
    assert_eq!(
        after.len(),
        before.len(),
        "case \"after the reboot\": a restart does not add or drop cache entries"
    );
    for instance in &rebooted_ids {
        let was = before_states
            .get(instance)
            .expect("every staged instance has a recorded state")
            .clone();
        let case = format!("{instance} was {was} when its device rebooted");
        let summary = after
            .get(instance)
            .unwrap_or_else(|| panic!("case {case:?}: the cache still reports the instance"));
        assert_consistent(summary, &case);

        // Rule 3: the freshness members advance — the device reported the whole set
        // afresh — while the instance itself is not re-created.
        let refreshed = epoch(summary, &["lastUpdatedAt"], &case);
        let staged_at = seconds(staged);
        assert!(
            refreshed > staged_at + TOLERANCE,
            "case {case:?}: a device that rebooted reported its instances afresh, so \
             freshness must advance past {staged_at}, got {refreshed}"
        );
        let reported_created = epoch(summary, &["instance", "createdAt"], &case);
        assert!(
            (reported_created - seconds(created_at)).abs() < TOLERANCE,
            "case {case:?}: a restart does not re-create an instance, expected \
             createdAt {} got {reported_created}",
            seconds(created_at)
        );

        // Rule 3: work that was under way has ended, and an instance that would be
        // up comes back up — reported starting, or already running if its own owed
        // instant passed in the meantime.
        let expected = state_after_a_restart(&was);
        let reported = state_name(summary, &case);
        if expected == "PENDING" {
            assert!(
                reported == "PENDING" || reported == "RUNNING",
                "case {case:?}: a restarted instance comes back up, so it reports \
                 PENDING or RUNNING, got {reported:?}"
            );
        } else {
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?} after the restart, got {reported:?}"
            );
            let was_under_way = INSTANCE_STATES
                .iter()
                .any(|(name, _, outcome)| *name == was && outcome.is_some());
            if was_under_way {
                assert_ne!(
                    reported, was,
                    "case {case:?}: work that was under way must not still be reported \
                     under way after the restart"
                );
            }
        }
    }

    // Rule 3: a device the task did not target is untouched — not its states and not
    // its freshness.
    let other_after = cache_of(&em, &untouched, &untouched_ids, "a device not rebooted").await;
    assert_eq!(
        other_after, other_before,
        "case \"a device not rebooted\": rebooting one device must not change another \
         device's cache"
    );

    // And every instance the restart brought back up reaches RUNNING on the clock,
    // so a caller polling after a reboot is not left waiting for ever.
    tokio::time::sleep(INSTANCE_TRANSITION_SPAN + Duration::from_millis(250)).await;
    let settled = cache_of(&em, &rebooted, &rebooted_ids, "once the restart finished").await;
    for instance in &rebooted_ids {
        let was = before_states
            .get(instance)
            .expect("every staged instance has a recorded state")
            .clone();
        let case = format!("{instance} was {was}, once the restart finished");
        let summary = settled
            .get(instance)
            .unwrap_or_else(|| panic!("case {case:?}: the cache still reports the instance"));
        assert_consistent(summary, &case);
        let expected = state_after_a_restart(&was);
        let expected = if expected == "PENDING" {
            "RUNNING"
        } else {
            expected
        };
        assert_eq!(
            state_name(summary, &case),
            expected,
            "case {case:?}: expected the restart to have settled on {expected:?}"
        );
    }
}
