// SPDX-License-Identifier: CC0-1.0

//! Where the discovered-resource inventory comes from: the resources the account holds
//! in the OTHER services of this emulator, not only the ones a client handed Config.
//!
//! The rule pinned here: while a configuration recorder is recording and its recording
//! group covers a resource type, the resources of that type the account holds are
//! admitted to Config's discovered-resource inventory as the owning service reports
//! them — including the DELETION of one that has gone away — and a discovered-resource
//! read projects that inventory however the request narrows it (by resource ids, by
//! resource name, by whether deleted resources are asked for), with no Config write ever
//! naming the resource. A recorder whose group does not cover the type, and one that is
//! not recording, admit nothing.
//!
//! So no row below calls `PutResourceConfig`: every identifier a read reports here got
//! into the inventory because a recorder was recording while the resource existed in its
//! own service. The reader's own paging, filtering and refusals are pinned over a seeded
//! inventory in `discovered_resources`; what this suite adds is where that inventory
//! comes from.
//!
//! Config and S3 are mounted on ONE engine, reusing the harness
//! `config_recorded_resource_compliance` builds: the recorder reads the resources it
//! records from the owning service's own API, so without that service mounted there is
//! nothing to discover.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::{Emulator, ACCOUNT_ID};

/// The resource type Config records an Amazon S3 general purpose bucket under — an
/// entry of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// Another entry of that same enum, for the rows that ask about a type no resource was
/// created for and for the recording groups that name a type other than the bucket one.
const EC2_VOLUME_RESOURCE_TYPE: &str = "AWS::EC2::Volume";

/// The buckets the narrowing table is read over. Sorted as their resource ids sort,
/// which is the order a listing reports them in.
const ALPHA: &str = "guardrail-alpha";
/// See [`ALPHA`].
const BETA: &str = "guardrail-beta";
/// See [`ALPHA`]. This is the one deleted through S3 part way through, so the inventory
/// records its deletion.
const GAMMA: &str = "guardrail-gamma";
/// A bucket name nothing creates, for the rows that narrow to a resource the account
/// does not hold.
const ABSENT: &str = "guardrail-absent";

/// The recorder every row writes.
const RECORDER_NAME: &str = "the-recorder";

/// How long a poll waits for the discovery pass the clock owes. A pass falls due a
/// fraction of a second after the call that makes it due, so this is many passes' worth
/// of slack and not a span the behaviour depends on.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(15);

/// How long a row waits while asserting that nothing is admitted — several passes' worth
/// of time, so an admission that was going to happen would have happened.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One discovered-resource read.
async fn listing(emulator: &Emulator, request: &Value) -> Value {
    emulator.config("ListDiscoveredResources", request).await
}

/// The `resourceId` of every identifier a read reported, in the order reported.
fn reported_ids(case: &str, body: &Value) -> Vec<String> {
    body["resourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected resourceIdentifiers, got {body}"))
        .iter()
        .map(|identifier| {
            identifier["resourceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every identifier reports a resourceId, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// Read `request` until it reports exactly `wanted`, and answer the response that did.
///
/// What is being waited for is the pass the recorder owes, never a value: the assertion
/// that fails at the deadline names the ids the read did report.
async fn await_ids(case: &str, emulator: &Emulator, request: &Value, wanted: &[&str]) -> Value {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let body = listing(emulator, request).await;
        let ids = reported_ids(case, &body);
        if ids.iter().map(String::as_str).collect::<Vec<&str>>() == wanted {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the discovered resource ids {wanted:?}, got {ids:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert that `request` reports nothing for as long as [`QUIET_BUDGET`].
async fn stays_empty(case: &str, emulator: &Emulator, request: &Value) {
    let deadline = Instant::now() + QUIET_BUDGET;
    loop {
        let body = listing(emulator, request).await;
        let ids = reported_ids(case, &body);
        assert!(
            ids.is_empty(),
            "case {case:?}: nothing must be admitted to the inventory, got {ids:?}"
        );
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Put a recorder recording with `group` as its recording group, and the delivery
/// channel a recorder needs, starting it only when `start` says so.
async fn recorder_with_group(emulator: &Emulator, group: &Value, start: bool) {
    emulator
        .config(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": RECORDER_NAME,
                    "roleARN": format!("arn:aws:iam::{ACCOUNT_ID}:role/config-role"),
                    "recordingGroup": group,
                }
            }),
        )
        .await;
    emulator
        .config(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-delivery" } }),
        )
        .await;
    if start {
        emulator
            .config(
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": RECORDER_NAME }),
            )
            .await;
    }
}

#[tokio::test]
async fn a_resource_created_in_another_service_is_reported_however_the_read_narrows_to_it() {
    let emulator = Emulator::new();
    emulator.recording().await;

    // Every resource below is created through the OWNING service's API. Config is told
    // nothing about any of them.
    emulator.create_bucket(ALPHA, &[("stage", "prod")]).await;
    emulator.create_bucket(BETA, &[]).await;
    emulator.create_bucket(GAMMA, &[]).await;
    await_ids(
        "every bucket the account holds",
        &emulator,
        &json!({ "resourceType": BUCKET_RESOURCE_TYPE }),
        &[ALPHA, BETA, GAMMA],
    )
    .await;

    // A resource that goes away in its own service goes away from the live inventory,
    // and the inventory keeps it as deleted.
    emulator
        .s3()
        .delete_bucket()
        .bucket(GAMMA)
        .send()
        .await
        .unwrap_or_else(|error| panic!("deleting {GAMMA} must succeed, got {error:?}"));
    await_ids(
        "a bucket deleted in its own service",
        &emulator,
        &json!({ "resourceType": BUCKET_RESOURCE_TYPE }),
        &[ALPHA, BETA],
    )
    .await;

    // The rows differ along what the rule generalises over: whether the read narrows by
    // resource ids (one, several, one the account never held, the deleted one), by
    // resource name, by nothing at all, whether deleted resources are asked for, and
    // which resource type is asked about.
    let cases: [(&str, Value, &[&str]); 10] = [
        (
            "no narrowing at all",
            json!({ "resourceType": BUCKET_RESOURCE_TYPE }),
            &[ALPHA, BETA],
        ),
        (
            "one resource id",
            json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceIds": [BETA] }),
            &[BETA],
        ),
        (
            "every live resource id",
            json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceIds": [BETA, ALPHA] }),
            &[ALPHA, BETA],
        ),
        (
            "several resource ids, one the account never held",
            json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceIds": [ALPHA, ABSENT] }),
            &[ALPHA],
        ),
        (
            "the deleted resource's id, deleted resources excluded",
            json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceIds": [GAMMA] }),
            &[],
        ),
        (
            "the deleted resource's id, deleted resources included",
            json!({
                "resourceType": BUCKET_RESOURCE_TYPE,
                "resourceIds": [GAMMA],
                "includeDeletedResources": true,
            }),
            &[GAMMA],
        ),
        (
            "deleted resources included, with no other narrowing",
            json!({
                "resourceType": BUCKET_RESOURCE_TYPE,
                "includeDeletedResources": true,
            }),
            &[ALPHA, BETA, GAMMA],
        ),
        (
            "the resource name one of them carries",
            json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceName": ALPHA }),
            &[ALPHA],
        ),
        (
            "a resource name none of them carries",
            json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceName": ABSENT }),
            &[],
        ),
        (
            "a resource type the account holds nothing of",
            json!({ "resourceType": EC2_VOLUME_RESOURCE_TYPE }),
            &[],
        ),
    ];

    for (case, request, expected) in cases {
        let body = await_ids(case, &emulator, &request, expected).await;

        // Each identifier reports the resource as its own service names it: the type it
        // is recorded under, the bucket's name as both its id and its custom name, and a
        // deletion time for the one that has gone away and only for that one.
        for identifier in body["resourceIdentifiers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected resourceIdentifiers, got {body}"))
        {
            let resource_id = identifier["resourceId"].as_str().unwrap_or_default();
            assert_eq!(
                identifier["resourceType"],
                json!(BUCKET_RESOURCE_TYPE),
                "case {case:?}: expected the type the resource is recorded under, got \
                 {identifier}"
            );
            assert_eq!(
                identifier["resourceName"].as_str(),
                Some(resource_id),
                "case {case:?}: a bucket's name is its resource name, got {identifier}"
            );
            assert_eq!(
                identifier["resourceDeletionTime"].is_null(),
                resource_id != GAMMA,
                "case {case:?}: a deletion time is reported for the deleted resource and \
                 only for it, got {identifier}"
            );
        }

        // The read is a report: reading again reports the same resources.
        let again = listing(&emulator, &request).await;
        assert_eq!(
            reported_ids(case, &again),
            reported_ids(case, &body),
            "case {case:?}: a second read must report the same resources, got {again}"
        );
    }
}

#[tokio::test]
async fn the_recording_group_and_the_recorder_decide_what_is_admitted() {
    // The rows differ along which resource types the recorder's group covers — all of
    // them, an inclusion group naming this type or another, an exclusion group excluding
    // this type or another — and along whether the recorder is recording at all.
    let cases: [(&str, Value, bool, bool); 6] = [
        (
            "a group of all supported types",
            json!({ "allSupported": true }),
            true,
            true,
        ),
        (
            "an inclusion group naming this type",
            json!({ "resourceTypes": [BUCKET_RESOURCE_TYPE] }),
            true,
            true,
        ),
        (
            "an inclusion group naming another type only",
            json!({ "resourceTypes": [EC2_VOLUME_RESOURCE_TYPE] }),
            true,
            false,
        ),
        (
            "an exclusion group excluding this type",
            json!({
                "exclusionByResourceTypes": { "resourceTypes": [BUCKET_RESOURCE_TYPE] },
            }),
            true,
            false,
        ),
        (
            "an exclusion group excluding another type",
            json!({
                "exclusionByResourceTypes": { "resourceTypes": [EC2_VOLUME_RESOURCE_TYPE] },
            }),
            true,
            true,
        ),
        (
            "a recorder that was never started",
            json!({ "allSupported": true }),
            false,
            false,
        ),
    ];

    for (case, group, recording, admitted) in cases {
        let emulator = Emulator::new();
        recorder_with_group(&emulator, &group, recording).await;
        let bucket = "grouped-bucket";
        emulator.create_bucket(bucket, &[]).await;

        let request = json!({
            "resourceType": BUCKET_RESOURCE_TYPE,
            "resourceIds": [bucket],
        });
        if admitted {
            await_ids(case, &emulator, &request, &[bucket]).await;
        } else {
            stays_empty(case, &emulator, &request).await;
        }
    }
}
