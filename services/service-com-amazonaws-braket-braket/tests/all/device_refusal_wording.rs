// SPDX-License-Identifier: CC0-1.0

//! How Braket WORDS the refusals it decides from the DEVICE, over HTTP.
//!
//! The rule these suites pin: a refusal a create takes from the device — the KIND of
//! device the `deviceArn` names, or the status the inventory publishes it in — is
//! answered in the service's own FIXED sentence for that class, interpolating nothing
//! from the request. The same refusal therefore reads identically whatever device was
//! asked for, whichever region the request was signed for and however often it is
//! repeated, and the two negative classes stay distinguishable by message as well as by
//! code.
//!
//! Each suite asserts that over a TABLE spanning the class: the rows differ along the
//! device (every published row of the inventory of the kind under test, plus ARNs the
//! inventory does not publish, across partitions and regions), the operation that
//! refused, and the region the request was signed for. The recorded calls behind the
//! rule are one row each.
//!
//! Everything goes through the production router, reusing the harness in
//! `quantum_task_loop.rs`, so what is pinned is what a Braket client sees.

use serde_json::Value;

use crate::hybrid_job_loop::job_body;
use crate::quantum_task_loop::{create_body, member, Braket, Reply};
use crate::spending_limits::{create_limit, limit_body, limits_on};

/// The sentence `CreateSpendingLimit` refuses a simulator `deviceArn` with: a cost
/// control is not sold per simulator at all, so the refusal states the KIND of device
/// and nothing of the request.
const SIMULATOR_NOT_SUPPORTED: &str = "Spending limits can not be created for simulator devices.";

/// The sentence every door refuses a device the inventory publishes as `RETIRED` with:
/// the status that closed the device and the recovery left to the caller, with no ARN
/// and no device name in it.
const DEVICE_RETIRED: &str = "Device is RETIRED. Try another device.";

/// The regions a row is signed for: a fixed sentence is the same sentence wherever the
/// request was signed, which only a table over more than one region can show.
const REGIONS: &[&str] = &["us-west-2", "eu-west-1", "ap-northeast-1"];

/// Simulator device ARNs the inventory does NOT publish: a cost control is about the
/// device an ARN names, so the kind in the ARN decides the refusal whether or not this
/// emulator publishes the device. The forms span the provider, the device name, the
/// region field (the published form carries none) and the partition. `(case, value)`.
const UNPUBLISHED_SIMULATOR_ARNS: &[(&str, &str)] = &[
    (
        "a simulator of a provider nothing is published for",
        "arn:aws:braket:::device/quantum-simulator/acme/Nonesuch",
    ),
    (
        "a simulator ARN carrying a region",
        "arn:aws:braket:us-west-2::device/quantum-simulator/amazon/not-published",
    ),
    (
        "a simulator ARN of another partition",
        "arn:aws-cn:braket:cn-north-1::device/quantum-simulator/amazon/other",
    ),
];

/// The device type and status members of one inventory row.
fn kind(device: &Value) -> (String, String) {
    (
        member(device, "deviceType").to_string(),
        member(device, "deviceStatus").to_string(),
    )
}

/// Assert `reply` carries `status`, the error code `error` and exactly `expected`, and
/// that `expected` really is fixed text: none of `absent` appears in it.
fn assert_refusal(
    reply: &Reply,
    case: &str,
    status: u16,
    error: &str,
    expected: &str,
    absent: &[&str],
) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} ({})",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some(error),
        "case {case:?}: expected the modelled {error}, got {:?} ({})",
        reply.error,
        reply.body
    );
    assert_eq!(
        reply.message(),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        reply.message()
    );
    for leaked in absent {
        assert!(
            !reply.message().contains(leaked),
            "case {case:?}: the sentence interpolates nothing, so it must not carry \
             {leaked:?}, got {:?}",
            reply.message()
        );
    }
}

/// `CreateSpendingLimit` refuses a simulator in the service's fixed sentence for the
/// class — the same sentence for every simulator, published or not, in every region.
#[tokio::test]
async fn a_simulator_is_refused_a_spending_limit_in_one_fixed_sentence() {
    let braket = Braket::new();

    // (case, deviceArn) — every published simulator the inventory does not also publish
    // as RETIRED (a retired device is refused for its STATUS, which the suite below
    // pins), plus the simulator ARNs this emulator publishes nothing for.
    let mut rows: Vec<(String, String)> = Vec::new();
    for device in braket.inventory().await {
        let (device_type, status) = kind(&device);
        if device_type == "SIMULATOR" && status != "RETIRED" {
            let arn = member(&device, "deviceArn").to_string();
            rows.push((format!("the published {status} simulator {arn}"), arn));
        }
    }
    assert!(
        !rows.is_empty(),
        "case the seeded inventory: expected it to publish a simulator that is not \
         retired, got none"
    );
    for (case, value) in UNPUBLISHED_SIMULATOR_ARNS {
        rows.push(((*case).to_string(), (*value).to_string()));
    }

    let mut index = 0;
    for (case, device_arn) in &rows {
        for region in REGIONS {
            index += 1;
            let reply = braket
                .call_in_region(
                    region,
                    "POST",
                    "/spending-limit",
                    Some(limit_body(
                        &format!("tok-simulator-wording-{index}"),
                        device_arn,
                        "7.00",
                    )),
                )
                .await;
            assert_refusal(
                &reply,
                &format!("{case} asked for in {region}"),
                400,
                "ValidationException",
                SIMULATOR_NOT_SUPPORTED,
                // Neither the ARN the request carried nor the device-type vocabulary
                // value is part of the sentence.
                &[device_arn.as_str(), "SIMULATOR"],
            );
        }
    }
}

/// Every door that models `DeviceRetiredException` answers the one fixed sentence for
/// that class, over every retired device the inventory publishes — and that sentence is
/// not the one the simulator class draws, so a caller can still tell the two apart.
#[tokio::test]
async fn a_retired_device_is_refused_in_one_fixed_sentence_by_every_door() {
    let braket = Braket::new();

    let retired: Vec<String> = braket
        .inventory()
        .await
        .iter()
        .filter(|device| kind(device).1 == "RETIRED")
        .map(|device| member(device, "deviceArn").to_string())
        .collect();
    assert!(
        !retired.is_empty(),
        "case the seeded inventory: expected it to publish a retired device, got none"
    );

    let mut index = 0;
    for device_arn in &retired {
        // (operation, path, body) — the three creates whose model declares
        // `DeviceRetiredException`
        // (`smithy_query(query="operation",operation="CreateQuantumTask,CreateJob,CreateSpendingLimit")`).
        index += 1;
        let doors: Vec<(&str, &str, Value)> = vec![
            (
                "CreateSpendingLimit",
                "/spending-limit",
                limit_body(&format!("tok-retired-limit-{index}"), device_arn, "7.00"),
            ),
            (
                "CreateQuantumTask",
                "/quantum-task",
                create_body(&format!("tok-retired-task-{index}"), device_arn, 1),
            ),
            (
                "CreateJob",
                "/job",
                job_body(
                    &format!("tok-retired-job-{index}"),
                    &format!("job-retired-{index}"),
                    device_arn,
                ),
            ),
        ];
        for (operation, path, body) in doors {
            let reply = braket.call("POST", path, Some(body)).await;
            assert_refusal(
                &reply,
                &format!("{operation} on the retired device {device_arn}"),
                410,
                "DeviceRetiredException",
                DEVICE_RETIRED,
                &[device_arn.as_str()],
            );
            assert_ne!(
                reply.message(),
                SIMULATOR_NOT_SUPPORTED,
                "case {operation} on the retired device {device_arn}: the retired class \
                 and the simulator class are different refusals, got the same sentence"
            );
        }
    }
}

/// A device the service has retired stays in the published catalog, carrying that
/// status: that is what lets the refusals above be taken from the device's status at
/// all, rather than a caller being told the device does not exist. The catalog publishes
/// retired devices of more than one program family, and `GetDevice` reports each of them.
#[tokio::test]
async fn the_catalog_publishes_the_devices_the_service_has_retired() {
    let braket = Braket::new();

    let mut providers: Vec<String> = Vec::new();
    for device in braket.inventory().await {
        if kind(&device).1 != "RETIRED" {
            continue;
        }
        let arn = member(&device, "deviceArn").to_string();
        // The catalog entry is read back through the operation that reports one device,
        // so what the refusals above decide from is what a caller can see.
        let reply = braket.get_device(&arn).await;
        assert_eq!(
            reply.status, 200,
            "case the retired device {arn}: a retired device is still published, so \
             GetDevice must answer 200, got {} ({})",
            reply.status, reply.body
        );
        assert_eq!(
            member(&reply.body, "deviceStatus"),
            "RETIRED",
            "case the retired device {arn}: expected GetDevice to report it RETIRED, got {}",
            reply.body
        );
        providers.push(member(&device, "providerName").to_string());
    }

    providers.sort();
    providers.dedup();
    assert!(
        providers.len() >= 2,
        "case the published catalog: the devices the service has retired come from more \
         than one provider, got {providers:?}"
    );
}

/// A refusal taken from the device is not a one-off: every caller repeating the SAME
/// `@idempotencyToken` concurrently reads the same code and the same fixed sentence, and
/// none of them leaves a spending limit behind.
#[tokio::test]
async fn every_racer_sharing_a_client_token_reads_the_same_refusal() {
    let braket = Braket::new();

    let device_arn = braket
        .inventory()
        .await
        .iter()
        .find(|device| {
            let (device_type, status) = kind(device);
            device_type == "SIMULATOR" && status != "RETIRED"
        })
        .map(|device| member(device, "deviceArn").to_string())
        .expect("the seeded inventory publishes a simulator that is not retired");

    // One token shared by every racer, as a client retrying a create presents it.
    let token = "tok-simulator-racers";
    let body = || limit_body(token, &device_arn, "7.00");
    let raced = tokio::join!(
        create_limit(&braket, body()),
        create_limit(&braket, body()),
        create_limit(&braket, body()),
        create_limit(&braket, body()),
        create_limit(&braket, body()),
        create_limit(&braket, body()),
        create_limit(&braket, body()),
        create_limit(&braket, body()),
    );
    let replies = [
        raced.0, raced.1, raced.2, raced.3, raced.4, raced.5, raced.6, raced.7,
    ];
    for (racer, reply) in replies.iter().enumerate() {
        assert_refusal(
            reply,
            &format!("racer {racer} of {} sharing one clientToken", replies.len()),
            400,
            "ValidationException",
            SIMULATOR_NOT_SUPPORTED,
            &[device_arn.as_str()],
        );
    }

    assert!(
        limits_on(&braket, &device_arn).await.is_empty(),
        "case eight refused racers: a refused create persists nothing, got {:?}",
        limits_on(&braket, &device_arn).await
    );
}
