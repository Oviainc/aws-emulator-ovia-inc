// SPDX-License-Identifier: CC0-1.0

//! `DescribeDeviceEc2Instances`: the cloud-side cache of a managed device's Amazon EC2
//! instances, end to end over HTTP.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted store and serialization — the same path a client takes.
//!
//! The rules under test:
//!
//! 1. the answer is the projection of (the instances the addressed device holds) ∩ (the
//!    requested `instanceIds`), in the cache's own order — so the order, the repetition
//!    and the count of the requested ids do not change which instances come back, and an
//!    instance of one device is never visible through another;
//! 2. every reported instance's `state.code` is the low byte the model documents for its
//!    `state.name`, and its `rootDeviceName` is one of its block device mappings;
//! 3. a `managedDeviceId` that names no device of the account in the signed Region is
//!    refused with the modelled HTTP 404 `ResourceNotFoundException` — the same answer
//!    the sibling id-addressed operations give for the same condition — whatever was
//!    asked of it;
//! 4. the operation is a report: two identical calls answer identically.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over — the
//! device, the requested id set, its order, its size, and the presence of ids the cache
//! does not hold.
//!
//! The tiny HTTP harness below is local to this suite on purpose: a sibling suite's
//! helpers are private to its module, and this file must keep compiling while that one
//! changes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    device_instance_ids, instance_id, managed_device_id, service_state,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request in this suite signs for, which is also the Region its
/// devices are activated in.
const SIGNED_REGION: &str = "eu-west-1";

/// How many devices this suite activates Snow Device Management on. The fixture puts
/// a different number of instances on each, including none on the third.
const ACTIVATED_DEVICES: u64 = 3;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/snow-device-management/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `InstanceStateName`'s values, each with the `InstanceState$code` the model documents
/// for it — `smithy_query(query="shape",shape="InstanceStateName")` for the names and
/// `smithy_query(query="shape",shape="InstanceState")` for the codes ("the valid values
/// for the instance state code": `0` pending, `16` running, `32` shutting-down, `48`
/// terminated, `64` stopping, `80` stopped). Written out here rather than imported, so a
/// wrong mapping in the implementation cannot make this test pass.
const INSTANCE_STATES: &[(&str, i64)] = &[
    ("PENDING", 0),
    ("RUNNING", 16),
    ("SHUTTING_DOWN", 32),
    ("TERMINATED", 48),
    ("STOPPING", 64),
    ("STOPPED", 80),
];

/// `AttachmentStatus`'s values, from `smithy_query(query="shape",shape="AttachmentStatus")`.
const ATTACHMENT_STATUSES: &[&str] = &["ATTACHING", "ATTACHED", "DETACHING", "DETACHED"];

/// Managed-device ids no account inventory holds, spanning the shapes a client can send:
/// the modelled `@length(1..64)` floor and ceiling, an id shaped like an activated one but
/// absent, and ids in another service's shape.
fn unmanaged_device_ids() -> Vec<String> {
    vec![
        "x".to_string(),
        "f".repeat(64),
        "smd-ffffffffffffffff".to_string(),
        "mdev-sim2real-ghost-70d421d7da1a".to_string(),
        managed_device_id(u64::MAX),
    ]
}

/// One emulator: a router carrying only this service, over one shared engine, with
/// Snow Device Management activated on [`ACTIVATED_DEVICES`] ordered devices in
/// [`SIGNED_REGION`].
///
/// The API models no device-creation operation, so the activation goes through the one
/// state seam that puts a device in the inventory; without it the account's inventory
/// is empty and every id-addressed operation answers the modelled
/// `ResourceNotFoundException` (see `empty_inventory_refusals.rs`).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    service_state(&*engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(SIGNED_REGION, ACTIVATED_DEVICES, SystemTime::now());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, or the empty string on a success.
    fn error_message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "snow-device-management.eu-west-1.amazonaws.com")
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

/// `DescribeDeviceEc2Instances`: `POST /managed-device/{managedDeviceId}/resources/ec2/describe`
/// with `instanceIds` in the body.
async fn describe_instances(
    router: &MultiServiceRouter,
    managed_device_id: &str,
    instance_ids: &[String],
) -> Answer {
    call(
        router,
        "POST",
        &format!("/managed-device/{managed_device_id}/resources/ec2/describe"),
        Some(json!({ "instanceIds": instance_ids })),
    )
    .await
}

/// The device ids of the account's inventory, in `ListDevices` order.
async fn inventory(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(router, "GET", "/managed-devices", None).await;
    assert_eq!(
        answer.status, 200,
        "case \"inventory\": expected HTTP 200 from ListDevices, got {} with {}",
        answer.status, answer.body
    );
    let devices = answer.body["devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"inventory\": expected a devices array, got {}", answer.body))
        .clone();
    assert!(
        devices.len() >= 2,
        "case \"inventory\": the emulator must manage at least two devices for a \
         per-device cache to be observable, got {}",
        answer.body
    );
    devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            let id = device["managedDeviceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case \"inventory\": expected a managedDeviceId, got {device}"))
                .to_string();
            let expected =
                managed_device_id(u64::try_from(index).expect("a small index fits in u64"));
            assert_eq!(
                id, expected,
                "case \"inventory\": ListDevices must serve the inventory in index order \
                 so a device's instance ids are known, got {id:?} at index {index}"
            );
            id
        })
        .collect()
}

/// Assert a call succeeded and hand back its `instances` array.
fn instances_of(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["instances"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an instances array, got {}", answer.body))
        .clone()
}

/// The `instance.instanceId` of every summary of an answer, in answer order.
fn reported_ids(instances: &[Value], case: &str) -> Vec<String> {
    instances
        .iter()
        .map(|summary| {
            summary["instance"]["instanceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an instanceId, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Every rule a single reported `InstanceSummary` must satisfy, whatever device it came
/// from and whatever was asked for.
fn assert_modelled_summary(summary: &Value, case: &str) {
    let instance = &summary["instance"];
    assert!(
        !summary["lastUpdatedAt"].is_null(),
        "case {case:?}: an InstanceSummary reports when it was last updated, got {summary}"
    );
    for member in ["createdAt", "updatedAt", "imageId", "instanceType", "privateIpAddress"] {
        assert!(
            !instance[member].is_null(),
            "case {case:?}: expected the reported instance to carry {member}, got {summary}"
        );
    }

    // Rule 2: the state code is the low byte the model documents for the state name.
    let name = instance["state"]["name"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a state name, got {summary}"));
    let code = instance["state"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a state code, got {summary}"));
    let expected = INSTANCE_STATES
        .iter()
        .find(|(state, _)| *state == name)
        .unwrap_or_else(|| {
            panic!("case {case:?}: state name {name:?} is not an InstanceStateName value")
        })
        .1;
    assert_eq!(
        code, expected,
        "case {case:?}: expected state code {expected} for {name:?}, got {code}"
    );

    // Rule 2: the root device volume is one of the instance's block device mappings.
    let root = instance["rootDeviceName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a rootDeviceName, got {summary}"));
    let mappings = instance["blockDeviceMappings"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected blockDeviceMappings, got {summary}"))
        .clone();
    let root_mapping = mappings
        .iter()
        .find(|mapping| mapping["deviceName"].as_str() == Some(root))
        .unwrap_or_else(|| {
            panic!("case {case:?}: rootDeviceName {root:?} has no block device mapping in {summary}")
        });
    let status = root_mapping["ebs"]["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ebs status, got {summary}"));
    assert!(
        ATTACHMENT_STATUSES.contains(&status),
        "case {case:?}: ebs status {status:?} is not an AttachmentStatus value"
    );
    assert_eq!(
        status == "ATTACHED",
        name != "TERMINATED",
        "case {case:?}: a volume is attached exactly while its instance exists to hold \
         it, got state {name:?} with status {status:?}"
    );
    assert!(
        !root_mapping["ebs"]["volumeId"].is_null(),
        "case {case:?}: an attached block device names its volume, got {summary}"
    );

    let cores = instance["cpuOptions"]["coreCount"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected cpuOptions.coreCount, got {summary}"));
    let threads = instance["cpuOptions"]["threadsPerCore"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected cpuOptions.threadsPerCore, got {summary}"));
    assert!(
        cores >= 1 && threads >= 1,
        "case {case:?}: expected at least one core and one thread per core, got \
         {cores} cores / {threads} threads"
    );
    assert!(
        instance["securityGroups"]
            .as_array()
            .is_some_and(|groups| !groups.is_empty()),
        "case {case:?}: an instance reports the security groups it is in, got {summary}"
    );
}

#[tokio::test]
async fn the_answer_is_the_requested_ids_intersected_with_the_devices_cache() {
    let router = emulator();
    let devices = inventory(&router).await;
    // Which name/code pairs the whole inventory exercised, so the documented catalog is
    // checked on each of its published values and not just on one.
    let mut seen_states: BTreeSet<String> = BTreeSet::new();
    // Whether any device reported a cache at all, and whether any reported none.
    let mut non_empty_devices = 0usize;
    let mut empty_devices = 0usize;

    for (index, device) in devices.iter().enumerate() {
        let held = device_instance_ids(u64::try_from(index).expect("a small index fits in u64"));
        let ghost = instance_id(u64::MAX);

        // The table rows: how the requested id set is formed. The expected answer is
        // always the held ids the row asks for, in cache order.
        let mut reversed = held.clone();
        reversed.reverse();
        let mut duplicated = held.clone();
        duplicated.extend(held.iter().cloned());
        let mut with_ghost = held.clone();
        with_ghost.push(ghost.clone());
        let mut rows: Vec<(String, Vec<String>, Vec<String>)> = vec![
            ("all held ids".to_string(), held.clone(), held.clone()),
            ("no ids".to_string(), vec![], vec![]),
            ("reversed".to_string(), reversed, held.clone()),
            ("each id twice".to_string(), duplicated, held.clone()),
            ("held ids plus one the cache does not hold".to_string(), with_ghost, held.clone()),
            ("only an id the cache does not hold".to_string(), vec![ghost], vec![]),
        ];
        // One row per held id on its own: the smallest non-empty request there is.
        for id in &held {
            rows.push((format!("only {id}"), vec![id.clone()], vec![id.clone()]));
        }

        for (row, requested, expected) in rows {
            let case = format!("device {device} ({index}), {row}");
            let answer = describe_instances(&router, device, &requested).await;
            let instances = instances_of(&answer, &case);
            assert_eq!(
                reported_ids(&instances, &case),
                expected,
                "case {case:?}: expected exactly the requested ids the cache holds, in \
                 cache order, got {}",
                answer.body
            );
            for summary in &instances {
                assert_modelled_summary(summary, &case);
                if let Some(name) = summary["instance"]["state"]["name"].as_str() {
                    seen_states.insert(name.to_string());
                }
            }
        }

        if held.is_empty() {
            empty_devices += 1;
        } else {
            non_empty_devices += 1;
        }
    }

    assert!(
        non_empty_devices >= 1 && empty_devices >= 1,
        "case \"fixture\": the inventory must span devices that run instances and a \
         device that runs none, got {non_empty_devices} with and {empty_devices} without"
    );
    let catalog: BTreeSet<String> = INSTANCE_STATES
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert_eq!(
        seen_states, catalog,
        "case \"state catalog\": every documented InstanceStateName must appear in some \
         reported instance, so each name/code pair is checked; saw {seen_states:?}"
    );
}

#[tokio::test]
async fn an_instance_of_one_device_is_not_visible_through_another() {
    let router = emulator();
    let devices = inventory(&router).await;
    let held: BTreeMap<&String, Vec<String>> = devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            (
                device,
                device_instance_ids(u64::try_from(index).expect("a small index fits in u64")),
            )
        })
        .collect();

    // The table rows: every (asked device, other device whose ids are requested) pair.
    for asked in held.keys() {
        for (owner, ids) in &held {
            if asked == owner || ids.is_empty() {
                continue;
            }
            let case = format!("device {asked} asked for device {owner}'s instances");
            let answer = describe_instances(&router, asked, ids).await;
            let instances = instances_of(&answer, &case);
            assert!(
                instances.is_empty(),
                "case {case:?}: a device's cache holds only its own instances, got {}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_device_the_account_does_not_manage_is_not_found() {
    let router = emulator();
    let managed = inventory(&router).await;
    let held = device_instance_ids(0);
    let mut codes: BTreeSet<String> = BTreeSet::new();

    // The table rows: the shape of the unknown id × what was asked of it. The id is
    // resolved ahead of the instance ids, so every row is the same refusal.
    for device in unmanaged_device_ids() {
        assert!(
            !managed.contains(&device),
            "case \"fixture\": {device:?} must not be in the managed inventory"
        );
        for requested in [vec![], held.clone(), vec![instance_id(u64::MAX)]] {
            let case = format!("device {device:?} with {} requested id(s)", requested.len());
            let answer = describe_instances(&router, &device, &requested).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {}",
                answer.body
            );
            assert!(
                !answer.error_message().is_empty(),
                "case {case:?}: the refusal carries a message, got {}",
                answer.body
            );
            codes.insert(answer.error_code().to_string());
        }
    }

    assert_eq!(
        codes.len(),
        1,
        "case \"one refusal\": an id that names no device is one outcome, whatever its \
         shape and whatever was asked of it; got {codes:?}"
    );

    // The refusals did not disturb the devices the account does manage.
    let case = "a managed device after the refusals";
    let answer = describe_instances(&router, &managed[0], &held).await;
    assert_eq!(
        reported_ids(&instances_of(&answer, case), case),
        held,
        "case {case:?}: a refused call must not change what a later caller sees, got {}",
        answer.body
    );
}

#[tokio::test]
async fn two_identical_calls_answer_identically() {
    let router = emulator();
    let devices = inventory(&router).await;

    // The table rows: every device, asked for its whole cache.
    for (index, device) in devices.iter().enumerate() {
        let held = device_instance_ids(u64::try_from(index).expect("a small index fits in u64"));
        let case = format!("device {device} ({index})");
        let first = describe_instances(&router, device, &held).await;
        let second = describe_instances(&router, device, &held).await;
        assert_eq!(
            instances_of(&first, &case),
            instances_of(&second, &case),
            "case {case:?}: DescribeDeviceEc2Instances is a report, so two calls must \
             answer identically"
        );
    }
}
