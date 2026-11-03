// SPDX-License-Identifier: CC0-1.0

//! What a Lightsail refusal SAYS when a resource's state blocks a well-formed
//! call, driven over the wire.
//!
//! Lightsail is `awsJson1_1` and its failure structures
//! (`OperationFailureException`, `InvalidInputException`) model a `message`
//! member, so the sentence a refusal carries is part of the wire contract a
//! client reads — not prose. The rule under test is about that sentence:
//!
//! * a disk call a disk's own `DiskState` blocks is refused with the sentence
//!   keyed on that STATE — `You can't <verb> this disk right now. The state of
//!   this disk is: <state>.` — whose only variable parts are the refused call's
//!   verb and the `DiskState` value the store holds for the disk, so an attach,
//!   a detach and a delete cannot word one condition three different ways;
//! * a disk call blocked because an instance still HOLDS the disk is refused
//!   with the sentence that names the holder — `You can't perform this operation
//!   because the disk is still attached to a Lightsail instance: <instance>.` —
//!   whose only variable part is the instance name read from the disk's own
//!   record, because that name is the one thing the caller needs to clear the
//!   condition;
//! * a refusal carries that sentence ALONE: which conditions a store found is a
//!   diagnostic of this emulator, so a `DeleteBucket` a bucket's contents block
//!   answers the service's own headline — `Bucket is not empty: <bucketName>` —
//!   with no parenthesised inventory of what the bucket holds and no advice
//!   about what to clear appended to it.
//!
//! Every case goes through the same front door a client uses: the Lightsail
//! calls as `POST /` with `X-Amz-Target` and an `awsJson1_1` body, the object
//! writes as the path-style S3 requests a client sends to the bucket's own
//! address, both dispatched by the multi-service router over ONE shared engine —
//! so the bucket's contents are read from the store that actually serves them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_lifecycle::error_code;

/// The `X-Amz-Target` prefix the Lightsail service routes on.
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint every request here reaches the emulator on.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The blueprint and bundle every instance in this suite is created from: the
/// smallest published Linux plan (`src/state.rs`, `BUNDLE_SPECS`). Which plan an
/// instance runs on changes nothing about what a refusal says.
const BLUEPRINT_ID: &str = "amazon_linux_2023";
/// See [`BLUEPRINT_ID`].
const BUNDLE_ID: &str = "nano_3_0";
/// The bucket plan every bucket here is created on — the cheapest published one
/// (`src/state.rs`, `BUCKET_BUNDLE_SPECS`).
const BUCKET_BUNDLE_ID: &str = "small_1_0";

/// Lightsail and Amazon S3 mounted on ONE shared engine, the way the router
/// mounts every service: a Lightsail bucket's objects live in the object store
/// the S3 data plane serves, which is where the control plane reads how many the
/// bucket holds.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: routing is by `X-Amz-Target` for the control
        // plane and by the credential scope's service field for the object
        // calls, and the header below carries only that scope.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Call a Lightsail `operation` in `region` with `body`, answering its HTTP
    /// status and parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization(region, "lightsail"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call a Lightsail `operation` in `region` and require success, answering
    /// the response body.
    async fn ok(&self, region: &str, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} in {region}: expected HTTP 200, got {status} with {parsed:?}"
        );
        parsed
    }

    /// `PUT /<bucket>/<key>` in `region` — S3's `PutObject`, which answers 200.
    async fn put_object(&self, region: &str, bucket: &str, key: &str, body: &[u8]) {
        let path = format!("/{bucket}/{key}");
        let request = ::http::Request::builder()
            .method("PUT")
            .uri(format!("https://{ENDPOINT_HOST}{path}"))
            .header("host", ENDPOINT_HOST)
            .header("authorization", authorization(region, "s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request is well formed");
        let status = self.router.dispatch(request).await.status().as_u16();
        assert_eq!(
            status, 200,
            "case PutObject {bucket}/{key}: expected HTTP 200, got {status}"
        );
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the
/// routing key the router claims a request by — and `region`, which is the
/// Region the call is made in. Nothing here is a credential: the harness turns
/// request authentication off, so nothing is verified.
fn authorization(region: &str, service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The `message` of a failure body.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or("<no message>")
        .to_string()
}

/// The stored disk `name` in `region`, as `GetDisk` reports it.
async fn disk(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator.ok(region, "GetDisk", json!({"diskName": name})).await["disk"].clone()
}

/// Poll `GetDisk` until it reports `want`. The transition is owed by the clock,
/// not by the poll.
async fn wait_for_disk_state(emulator: &Emulator, region: &str, name: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reported = disk(emulator, region, name).await;
        if reported["state"].as_str() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for disk state {want:?} on {name}: expected it within 5s, got {:?}",
            reported["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll `GetInstance` until it reports `want`.
async fn wait_for_instance_state(emulator: &Emulator, region: &str, name: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reported = emulator
            .ok(region, "GetInstance", json!({"instanceName": name}))
            .await["instance"]
            .clone();
        if reported["state"]["name"].as_str() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for instance state {want:?} on {name}: expected it within 5s, got {:?}",
            reported["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create the instance `name` in `zone` and wait until it is serving — the state
/// a disk can be attached to.
async fn create_running_instance(emulator: &Emulator, region: &str, zone: &str, name: &str) {
    emulator
        .ok(
            region,
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": zone,
                "blueprintId": BLUEPRINT_ID,
                "bundleId": BUNDLE_ID,
            }),
        )
        .await;
    wait_for_instance_state(emulator, region, name, "running").await;
}

/// Create the disk `name` in `zone` at `size_in_gb` GB, waiting for the clock to
/// settle it to `available` only when `settled` says so — a disk caught before
/// that settle is `pending`, which is the other `DiskState` a stored disk can be
/// blocked by.
async fn create_disk(
    emulator: &Emulator,
    region: &str,
    zone: &str,
    name: &str,
    size_in_gb: i64,
    settled: bool,
) {
    emulator
        .ok(
            region,
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": zone, "sizeInGb": size_in_gb}),
        )
        .await;
    if settled {
        wait_for_disk_state(emulator, region, name, "available").await;
    }
}

// ---------------------------------------------------------------------------
// The sentence keyed on the disk's state
// ---------------------------------------------------------------------------

/// How the disk a row is about got into the state the row is about.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DiskSetup {
    /// Created and left alone, so the clock still owes it `available`.
    Pending,
    /// Created and settled, so nothing holds it and it is `available`.
    Available,
    /// Created, settled and attached to an instance, so it is `in-use`.
    Attached,
}

/// One call a disk's own state blocks, and the sentence it must be refused with.
#[derive(Debug)]
struct StateBlocked {
    case: &'static str,
    /// The Lightsail operation the row calls.
    operation: &'static str,
    /// The verb that operation names itself by in the refusal.
    verb: &'static str,
    /// The `DiskState` value the disk carries when the call is made, and so the
    /// one the refusal must report.
    state: &'static str,
    setup: DiskSetup,
    region: &'static str,
    availability_zone: &'static str,
    disk_name: &'static str,
    size_in_gb: i64,
    /// The instance the row's `AttachDisk` names, and the one an `Attached` row
    /// attaches the disk to. `None` for a row whose operation names no instance.
    instance_name: Option<&'static str>,
    /// The device path the row's `AttachDisk` asks for.
    disk_path: &'static str,
}

/// The table for the state-keyed sentence. The rows differ along everything the
/// rule generalises over: WHICH call is refused (all three disk calls that have
/// such a guard, and so all three verbs), the `DiskState` value that blocks it
/// (both non-`available` values a stored disk can be found in, plus `available`
/// where that is the blocking state), the Region and Availability Zone, the form
/// of the disk's name, its size, and — for the attach rows — whether the request
/// names the instance already holding the disk or another one, at the path that
/// is taken or a free one.
const STATE_BLOCKED: &[StateBlocked] = &[
    StateBlocked {
        case: "an attach of a disk another instance already holds",
        operation: "AttachDisk",
        verb: "attach",
        state: "in-use",
        setup: DiskSetup::Attached,
        region: "us-east-1",
        availability_zone: "us-east-1a",
        disk_name: "held-disk",
        size_in_gb: 8,
        instance_name: Some("other-holder"),
        disk_path: "/dev/xvdg",
    },
    StateBlocked {
        case: "an attach of a disk to the very instance holding it, at its own path",
        operation: "AttachDisk",
        verb: "attach",
        state: "in-use",
        setup: DiskSetup::Attached,
        region: "eu-west-1",
        availability_zone: "eu-west-1c",
        disk_name: "re_attached_disk_2048",
        size_in_gb: 2048,
        instance_name: None,
        disk_path: "/dev/xvdf",
    },
    StateBlocked {
        case: "an attach of a disk the clock is still provisioning",
        operation: "AttachDisk",
        verb: "attach",
        state: "pending",
        setup: DiskSetup::Pending,
        region: "ap-southeast-2",
        availability_zone: "ap-southeast-2b",
        disk_name: "pending-attach-disk",
        size_in_gb: 128,
        instance_name: Some("pending-attach-host"),
        disk_path: "/dev/xvdf",
    },
    StateBlocked {
        case: "a delete of a disk the clock is still provisioning",
        operation: "DeleteDisk",
        verb: "delete",
        state: "pending",
        setup: DiskSetup::Pending,
        region: "us-west-2",
        availability_zone: "us-west-2a",
        disk_name: "pending_delete_disk",
        size_in_gb: 16384,
        instance_name: None,
        disk_path: "/dev/xvdf",
    },
    StateBlocked {
        case: "a detach of a disk the clock is still provisioning",
        operation: "DetachDisk",
        verb: "detach",
        state: "pending",
        setup: DiskSetup::Pending,
        region: "eu-central-1",
        availability_zone: "eu-central-1b",
        disk_name: "pending-detach-disk",
        size_in_gb: 32,
        instance_name: None,
        disk_path: "/dev/xvdf",
    },
    StateBlocked {
        case: "a detach of a settled disk no instance holds",
        operation: "DetachDisk",
        verb: "detach",
        state: "available",
        setup: DiskSetup::Available,
        region: "ap-northeast-1",
        availability_zone: "ap-northeast-1c",
        disk_name: "free_disk_512",
        size_in_gb: 512,
        instance_name: None,
        disk_path: "/dev/xvdf",
    },
];

/// RULE: a disk call the disk's own `DiskState` blocks is refused with HTTP 400
/// `OperationFailureException` whose message is keyed on that state — the
/// refused call's verb and the state value the store holds, and nothing else —
/// whichever of the disk calls was made, and the refusal leaves the disk exactly
/// as it was.
#[tokio::test]
async fn a_disk_call_the_state_blocks_is_refused_with_the_state_sentence() {
    let emulator = Emulator::new();

    for row in STATE_BLOCKED {
        let case = row.case;
        // The instance an `Attached` row's disk is attached to, which is also
        // the instance an attach row that names none re-attaches to.
        let holder = format!("{}-holder", row.disk_name.replace('_', "-"));
        if row.setup == DiskSetup::Attached {
            create_running_instance(&emulator, row.region, row.availability_zone, &holder).await;
        }
        if let Some(instance_name) = row.instance_name {
            create_running_instance(&emulator, row.region, row.availability_zone, instance_name)
                .await;
        }
        create_disk(
            &emulator,
            row.region,
            row.availability_zone,
            row.disk_name,
            row.size_in_gb,
            row.setup != DiskSetup::Pending,
        )
        .await;
        if row.setup == DiskSetup::Attached {
            emulator
                .ok(
                    row.region,
                    "AttachDisk",
                    json!({
                        "diskName": row.disk_name,
                        "instanceName": holder,
                        "diskPath": "/dev/xvdf",
                    }),
                )
                .await;
        }

        // The disk really is in the state this row is about, as a client reads
        // it — so what the refusal reports is checked against the state the
        // service itself publishes.
        let before = disk(&emulator, row.region, row.disk_name).await;
        assert_eq!(
            before["state"].as_str(),
            Some(row.state),
            "case {case}: expected the disk to be {:?}, got {before:?}",
            row.state
        );

        let request = match row.operation {
            "AttachDisk" => json!({
                "diskName": row.disk_name,
                "instanceName": row.instance_name.unwrap_or(holder.as_str()),
                "diskPath": row.disk_path,
            }),
            _ => json!({"diskName": row.disk_name}),
        };
        let (status, body) = emulator.call(row.region, row.operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "OperationFailureException"),
            "case {case}: expected the modelled operation failure, got {status} with {body:?}"
        );
        let answered = message(&body);
        let expected = format!(
            "You can't {} this disk right now. The state of this disk is: {}.",
            row.verb, row.state
        );
        assert_eq!(
            answered, expected,
            "case {case}: expected the refusal {expected:?}, got {answered:?}"
        );

        // The refusal wrote nothing: the disk is where it was, in the state it
        // was, holding the attachment it held.
        let after = disk(&emulator, row.region, row.disk_name).await;
        assert_eq!(
            (
                after["state"].as_str(),
                after["attachedTo"].as_str(),
                after["sizeInGb"].as_i64(),
                after["location"]["availabilityZone"].as_str(),
            ),
            (
                before["state"].as_str(),
                before["attachedTo"].as_str(),
                Some(row.size_in_gb),
                Some(row.availability_zone),
            ),
            "case {case}: expected the refused call to leave the disk alone, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The sentence that names the instance holding the disk
// ---------------------------------------------------------------------------

/// One delete a held disk blocks, and the instance whose name the refusal must
/// carry.
#[derive(Debug)]
struct StillAttached {
    case: &'static str,
    region: &'static str,
    availability_zone: &'static str,
    disk_name: &'static str,
    /// The instance the disk is attached to, whose name is the refusal's one
    /// variable part. The rows differ along the form a `ResourceName` takes.
    instance_name: &'static str,
    size_in_gb: i64,
    /// The device path the disk is exposed at, which the refusal never names.
    disk_path: &'static str,
}

/// The table for the holder-naming sentence: the rows differ along the form of
/// the instance's name (hyphens, underscores, digits), the Region and
/// Availability Zone, the disk's size, and the device path the attachment used.
const STILL_ATTACHED: &[StillAttached] = &[
    StillAttached {
        case: "a hyphenated instance holding an 8 GB disk",
        region: "us-east-1",
        availability_zone: "us-east-1a",
        disk_name: "attached-disk-1",
        instance_name: "disk-holder",
        size_in_gb: 8,
        disk_path: "/dev/xvdf",
    },
    StillAttached {
        case: "an underscored instance holding a 2048 GB disk at another path",
        region: "eu-west-1",
        availability_zone: "eu-west-1c",
        disk_name: "attached_disk_2",
        instance_name: "disk_holder_2",
        size_in_gb: 2048,
        disk_path: "/dev/xvdh",
    },
    StillAttached {
        case: "a digit-suffixed instance in another Region holding the largest disk",
        region: "ap-southeast-2",
        availability_zone: "ap-southeast-2b",
        disk_name: "attached-disk-3",
        instance_name: "holder9",
        size_in_gb: 16384,
        disk_path: "/dev/xvdg",
    },
];

/// RULE: a disk call blocked because an instance still holds the disk is refused
/// with HTTP 400 `OperationFailureException` whose message names that instance —
/// the holder read from the disk's own record, never the disk's name and never a
/// state word — and the disk stays attached; once the attachment is released the
/// same call is served.
#[tokio::test]
async fn deleting_a_held_disk_is_refused_with_the_name_of_the_instance_holding_it() {
    let emulator = Emulator::new();

    for row in STILL_ATTACHED {
        let case = row.case;
        create_running_instance(
            &emulator,
            row.region,
            row.availability_zone,
            row.instance_name,
        )
        .await;
        create_disk(
            &emulator,
            row.region,
            row.availability_zone,
            row.disk_name,
            row.size_in_gb,
            true,
        )
        .await;
        emulator
            .ok(
                row.region,
                "AttachDisk",
                json!({
                    "diskName": row.disk_name,
                    "instanceName": row.instance_name,
                    "diskPath": row.disk_path,
                }),
            )
            .await;

        let (status, body) = emulator
            .call(row.region, "DeleteDisk", json!({"diskName": row.disk_name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "OperationFailureException"),
            "case {case}: expected the modelled operation failure, got {status} with {body:?}"
        );
        let answered = message(&body);
        let expected = format!(
            "You can't perform this operation because the disk is still attached to a \
             Lightsail instance: {}.",
            row.instance_name
        );
        assert_eq!(
            answered, expected,
            "case {case}: expected the refusal {expected:?}, got {answered:?}"
        );

        // The refusal removed nothing: the attachment it reported still stands.
        let after = disk(&emulator, row.region, row.disk_name).await;
        assert_eq!(
            (after["attachedTo"].as_str(), after["state"].as_str()),
            (Some(row.instance_name), Some("in-use")),
            "case {case}: expected the refused delete to leave the attachment, got {after:?}"
        );

        // ROUND TRIP: the refusal was about the attachment, so detaching from
        // the instance it named is exactly what makes the same delete serve.
        emulator
            .ok(
                row.region,
                "StopInstance",
                json!({"instanceName": row.instance_name}),
            )
            .await;
        wait_for_instance_state(&emulator, row.region, row.instance_name, "stopped").await;
        emulator
            .ok(row.region, "DetachDisk", json!({"diskName": row.disk_name}))
            .await;
        wait_for_disk_state(&emulator, row.region, row.disk_name, "available").await;
        emulator
            .ok(row.region, "DeleteDisk", json!({"diskName": row.disk_name}))
            .await;
        let (status, gone) = emulator
            .call(row.region, "GetDisk", json!({"diskName": row.disk_name}))
            .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "NotFoundException"),
            "case {case}: expected the disk gone once it was free, got {gone:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusal carries its sentence alone
// ---------------------------------------------------------------------------

/// One `DeleteBucket` without `forceDelete` that what the bucket holds blocks.
#[derive(Debug)]
struct BlockedBucket {
    case: &'static str,
    region: &'static str,
    bucket_name: &'static str,
    /// The object keys written into the bucket's data plane before the delete.
    objects: &'static [&'static str],
    /// How many access keys the bucket holds when the delete is made.
    access_keys: usize,
    /// Whether the delete carries `forceDelete: false` rather than leaving the
    /// member absent — neither is a request to force anything.
    force_delete_false: bool,
}

/// The table for the blocked bucket delete. The rows differ along what blocks
/// the delete (objects, access keys, both), how many of each, the shape of the
/// object keys (a flat key, prefixed ones, a dotted one), the bucket's name
/// form, its Region, and the two forms a request that does not force takes.
const BLOCKED_BUCKETS: &[BlockedBucket] = &[
    BlockedBucket {
        case: "one object, flag absent",
        region: "us-east-1",
        bucket_name: "one-object-bucket",
        objects: &["report.txt"],
        access_keys: 0,
        force_delete_false: false,
    },
    BlockedBucket {
        case: "three objects under prefixes, forceDelete false",
        region: "us-east-1",
        bucket_name: "three-object-bucket",
        objects: &["a.txt", "nested/b.bin", "deeper/still/c"],
        access_keys: 0,
        force_delete_false: true,
    },
    BlockedBucket {
        case: "one object and two access keys in another Region",
        region: "eu-west-1",
        bucket_name: "held-bucket-2",
        objects: &["only-one"],
        access_keys: 2,
        force_delete_false: false,
    },
    BlockedBucket {
        case: "a dotted object key beside one access key",
        region: "ap-southeast-2",
        bucket_name: "dotted-object-bucket",
        objects: &["logs.2026-10-30.json"],
        access_keys: 1,
        force_delete_false: true,
    },
];

/// RULE: a `DeleteBucket` the bucket's contents block is refused with HTTP 400
/// `InvalidInputException` carrying the service's own sentence ALONE — the
/// headline naming the bucket from the request — and nothing about which
/// conditions the store found; the bucket and everything it holds are left in
/// place, and a delete that forces goes through.
#[tokio::test]
async fn a_blocked_bucket_delete_carries_its_headline_alone() {
    let emulator = Emulator::new();

    for row in BLOCKED_BUCKETS {
        let case = row.case;
        emulator
            .ok(
                row.region,
                "CreateBucket",
                json!({"bucketName": row.bucket_name, "bundleId": BUCKET_BUNDLE_ID}),
            )
            .await;
        for key in row.objects {
            emulator
                .put_object(row.region, row.bucket_name, key, key.as_bytes())
                .await;
        }
        for _ in 0..row.access_keys {
            emulator
                .ok(
                    row.region,
                    "CreateBucketAccessKey",
                    json!({"bucketName": row.bucket_name}),
                )
                .await;
        }

        let mut request = json!({"bucketName": row.bucket_name});
        if row.force_delete_false {
            request["forceDelete"] = json!(false);
        }
        let (status, body) = emulator.call(row.region, "DeleteBucket", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected the modelled input rejection, got {status} with {body:?}"
        );
        let answered = message(&body);
        // Every row holds at least one object, so the condition is the bucket's
        // contents and the headline is the one the service words for it, with
        // the bucket name from the request as its only variable part.
        let expected = format!("Bucket is not empty: {}", row.bucket_name);
        assert_eq!(
            answered, expected,
            "case {case}: expected the refusal {expected:?}, got {answered:?}"
        );

        // The refusal deleted nothing: the bucket is still there.
        let still_there = emulator
            .ok(
                row.region,
                "GetBuckets",
                json!({"bucketName": row.bucket_name}),
            )
            .await;
        assert_eq!(
            still_there["buckets"][0]["name"].as_str(),
            Some(row.bucket_name),
            "case {case}: expected the refused delete to leave the bucket, got {still_there:?}"
        );

        // ROUND TRIP: the condition is real state, so the delete that forces
        // past it is served and the bucket is gone.
        emulator
            .ok(
                row.region,
                "DeleteBucket",
                json!({"bucketName": row.bucket_name, "forceDelete": true}),
            )
            .await;
        let (status, gone) = emulator
            .call(
                row.region,
                "GetBuckets",
                json!({"bucketName": row.bucket_name}),
            )
            .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "NotFoundException"),
            "case {case}: expected the forced delete to remove the bucket, got {gone:?}"
        );
    }
}

/// RULE (the same one, where the blocking condition is not the bucket's
/// contents): the refusal still carries its sentence alone — no parenthesised
/// inventory of what the store found and no advice about how to clear it — so a
/// client reading the message reads the service's own wording and nothing else.
#[tokio::test]
async fn a_bucket_delete_blocked_without_objects_appends_no_inventory() {
    let emulator = Emulator::new();
    let region = "us-east-2";

    // The two conditions a bucket can carry with no object in it at all: an
    // access key it holds, and an instance granted access to it (the modelled
    // `DeleteBucketRequest$forceDelete` names both).
    emulator
        .ok(
            region,
            "CreateBucket",
            json!({"bucketName": "keyed-empty-bucket", "bundleId": BUCKET_BUNDLE_ID}),
        )
        .await;
    emulator
        .ok(
            region,
            "CreateBucketAccessKey",
            json!({"bucketName": "keyed-empty-bucket"}),
        )
        .await;
    emulator
        .ok(
            region,
            "CreateBucket",
            json!({"bucketName": "granted-empty-bucket", "bundleId": BUCKET_BUNDLE_ID}),
        )
        .await;
    create_running_instance(&emulator, region, "us-east-2a", "granted-instance").await;
    emulator
        .ok(
            region,
            "SetResourceAccessForBucket",
            json!({
                "bucketName": "granted-empty-bucket",
                "resourceName": "granted-instance",
                "access": "allow",
            }),
        )
        .await;

    for (case, bucket) in [
        ("a bucket holding only an access key", "keyed-empty-bucket"),
        (
            "a bucket an instance was granted access to",
            "granted-empty-bucket",
        ),
    ] {
        let (status, body) = emulator
            .call(region, "DeleteBucket", json!({"bucketName": bucket}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected the modelled input rejection, got {status} with {body:?}"
        );
        let answered = message(&body);
        assert!(
            answered.contains(bucket),
            "case {case}: expected the refusal to name {bucket:?}, got {answered:?}"
        );
        // The emulator's own account of WHICH conditions it found travelled in
        // the parenthesis and the clause after it; neither is the service's
        // wording, so neither is in the message.
        for appended in ['(', ')'] {
            assert!(
                !answered.contains(appended),
                "case {case}: expected no appended inventory in {answered:?}"
            );
        }
        assert!(
            !answered.contains("forceDelete"),
            "case {case}: expected no remediation clause in {answered:?}"
        );
    }
}
