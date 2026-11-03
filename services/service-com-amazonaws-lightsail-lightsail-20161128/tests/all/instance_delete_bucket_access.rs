// SPDX-License-Identifier: CC0-1.0

//! What `DeleteInstance` does to the bucket access grants other records hold to
//! the instance it removes, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suites' own
//! ([`crate::instance_lifecycle::Emulator`]), and every grant a case releases was
//! made with `SetResourceAccessForBucket` — no case reaches into the store.
//!
//! The rule under test: removing a resource releases every live reference another
//! stored record holds to it — an attachment or an access grant — so no read
//! reports a resource attached to, or holding access to, one the account no
//! longer holds. `Bucket$resourcesReceivingAccess` describes "Lightsail
//! instances that have access to the bucket", and a removed instance is none of
//! them: every bucket that granted it access reports one fewer resource
//! receiving access, each release is logged as the bucket's own
//! `SetResourceAccessForBucket` entry beside the `DeleteInstance` one, the
//! condition that grant placed on `DeleteBucket` goes with it, and a grant held
//! to any OTHER instance is left exactly as it was.
//!
//! The cases are a TABLE whose rows differ along everything the rule generalises
//! over: the Region the resources live in (including no credential scope at
//! all), how many buckets granted the removed instance access (none, one,
//! several), how many of those also granted another instance, the length and
//! form of the bucket and instance names, and the lifecycle state the instance is
//! deleted from.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The bundle every bucket in this suite is created on — the cheapest published
/// plan (`crate::state::BUCKET_BUNDLE_SPECS`); which plan a bucket is on decides
/// nothing about which resources receive access to it.
const BUCKET_BUNDLE: &str = "small_1_0";

/// The blueprint and bundle every instance in this suite is created on, from the
/// published catalogs (`crate::state::BLUEPRINT_SPECS` and
/// `crate::state::BUNDLE_SPECS`); neither decides anything about bucket access.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const INSTANCE_BUNDLE: &str = "micro_3_0";

/// A name of exactly 54 characters — the upper bound the modelled `BucketName`
/// length (3..54) admits.
const LONGEST_BUCKET_NAME: &str = "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcd";

/// The wire error a Lightsail request for a resource the account does not hold is
/// refused with, and the `code` its body carries (pinned for every operation in
/// `instance_error_envelope.rs`).
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The two values the modelled `ResourceBucketAccess` enum admits
/// (`smithy_query(query="shape",shape="com.amazonaws.lightsail#ResourceBucketAccess")`).
const ALLOW: &str = "allow";

/// Create the bucket `name` in `region`.
async fn create_bucket(emulator: &Emulator, region: Option<&str>, name: &str) {
    emulator
        .ok_at(
            None,
            region,
            "CreateBucket",
            json!({"bucketName": name, "bundleId": BUCKET_BUNDLE}),
        )
        .await;
}

/// Create the instance `name` in `zone` and leave it `running` — the state the
/// clock owes a new instance — or `stopped` after a forced stop.
async fn create_instance(emulator: &Emulator, zone: &str, name: &str, stopped: bool) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": zone,
                "blueprintId": BLUEPRINT,
                "bundleId": INSTANCE_BUNDLE,
            }),
        )
        .await;
    wait_for_state(emulator, name, "running").await;
    if stopped {
        emulator
            .ok("StopInstance", json!({"instanceName": name, "force": true}))
            .await;
        wait_for_state(emulator, name, "stopped").await;
    }
}

/// Grant `instance` access to `bucket` in `region`.
async fn grant(emulator: &Emulator, region: Option<&str>, bucket: &str, instance: &str) {
    emulator
        .ok_at(
            None,
            region,
            "SetResourceAccessForBucket",
            json!({"bucketName": bucket, "resourceName": instance, "access": ALLOW}),
        )
        .await;
}

/// The names of the resources receiving access to `bucket` in `region`, in the
/// order `GetBuckets` reports them, asserting every entry describes an instance.
async fn granted(
    case: &str,
    emulator: &Emulator,
    region: Option<&str>,
    bucket: &str,
) -> Vec<String> {
    let reported = emulator
        .ok_at(
            None,
            region,
            "GetBuckets",
            json!({"bucketName": bucket, "includeConnectedResources": true}),
        )
        .await["buckets"][0]
        .clone();
    let resources = reported["resourcesReceivingAccess"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: expected an array of resources receiving access to {bucket}, got {reported:?}")
        })
        .clone();
    resources
        .iter()
        .map(|resource| {
            assert_eq!(
                resource["resourceType"].as_str(),
                Some("Instance"),
                "case {case}: every resource receiving access to {bucket} describes an \
                 instance, got {resource:?}"
            );
            resource["name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a named resource on {bucket}, got {resource:?}")
                })
                .to_string()
        })
        .collect()
}

/// The `operations` array of a response, as a list of entries.
fn operations(case: &str, body: &Value) -> Vec<Value> {
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"))
        .clone()
}

// ---------------------------------------------------------------------------
// The release
// ---------------------------------------------------------------------------

/// One instance whose removal must release the bucket grants it held.
struct Row {
    case: &'static str,
    /// The Region every resource of the row lives in — `None` for a request made
    /// with no credential scope at all, which the service resolves to its
    /// default Region.
    region: Option<&'static str>,
    zone: &'static str,
    /// The instance the case deletes.
    deleted: &'static str,
    /// The buckets the deleted instance is granted access to.
    buckets: &'static [&'static str],
    /// Buckets of the same Region the deleted instance is NOT granted access to.
    other_buckets: &'static [&'static str],
    /// Another instance granted access to EVERY bucket of the row, when the row
    /// has one: its grant must survive the delete untouched.
    survivor: Option<&'static str>,
    /// Whether the instance is stopped before being deleted. A delete is
    /// accepted either way, so the release cannot depend on it.
    stop_first: bool,
}

/// The table for the release.
const ROWS: &[Row] = &[
    Row {
        case: "one bucket in the default Region, deleted while running",
        region: None,
        zone: "us-east-1a",
        deleted: "grant-host",
        buckets: &["grant-bucket"],
        other_buckets: &[],
        survivor: None,
        stop_first: false,
    },
    Row {
        case: "two buckets in another Region, one of them the longest name, deleted while stopped",
        region: Some("us-west-2"),
        zone: "us-west-2a",
        deleted: "data_host_1",
        buckets: &["first-grant-bucket", LONGEST_BUCKET_NAME],
        other_buckets: &["ungranted-bucket"],
        survivor: Some("kept_host_1"),
        stop_first: true,
    },
    Row {
        case: "a bucket shared with another grantee",
        region: Some("eu-central-1"),
        zone: "eu-central-1b",
        deleted: "shared-grant-host",
        buckets: &["shared-grant-bucket"],
        other_buckets: &[],
        survivor: Some("surviving-grant-host"),
        stop_first: false,
    },
    Row {
        case: "an instance holding no grant at all",
        region: Some("ap-southeast-2"),
        zone: "ap-southeast-2a",
        deleted: "bare_host_1",
        buckets: &[],
        other_buckets: &["bystander-bucket"],
        survivor: Some("bystander-host"),
        stop_first: false,
    },
];

/// RULE: deleting an instance releases every bucket access grant the store holds
/// to it — each bucket that granted it access stops reporting it in
/// `resourcesReceivingAccess` — the response logs the removal plus one entry per
/// release, and a grant held to another instance is left exactly as it was.
#[tokio::test]
async fn deleting_an_instance_releases_every_bucket_grant_it_held() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        create_instance(&emulator, row.zone, row.deleted, row.stop_first).await;
        if let Some(survivor) = row.survivor {
            create_instance(&emulator, row.zone, survivor, false).await;
        }
        for bucket in row.buckets.iter().chain(row.other_buckets) {
            create_bucket(&emulator, row.region, bucket).await;
            if let Some(survivor) = row.survivor {
                grant(&emulator, row.region, bucket, survivor).await;
            }
        }
        for bucket in row.buckets {
            grant(&emulator, row.region, bucket, row.deleted).await;
        }

        // What a client sees before the delete: every bucket of `buckets`
        // reports the instance among the resources receiving access.
        for bucket in row.buckets {
            let before = granted(case, &emulator, row.region, bucket).await;
            assert!(
                before.contains(&row.deleted.to_string()),
                "case {case}: expected {bucket} to start out granting {}, got {before:?}",
                row.deleted
            );
        }

        let deleted = emulator
            .ok("DeleteInstance", json!({"instanceName": row.deleted}))
            .await;

        // The log: the removal itself, then one release per grant — each already
        // done when the call returned, so each is terminal.
        let entries = operations(case, &deleted);
        assert_eq!(
            entries.len(),
            1 + row.buckets.len(),
            "case {case}: expected one entry for the removal and one per released grant, got \
             {entries:?}"
        );
        assert_eq!(
            (
                entries[0]["operationType"].as_str(),
                entries[0]["resourceType"].as_str(),
                entries[0]["resourceName"].as_str(),
            ),
            (Some("DeleteInstance"), Some("Instance"), Some(row.deleted)),
            "case {case}: expected the removal entry first, got {:?}",
            entries[0]
        );
        let mut released: Vec<String> = Vec::new();
        for entry in &entries[1..] {
            assert_eq!(
                (
                    entry["operationType"].as_str(),
                    entry["resourceType"].as_str(),
                    entry["status"].as_str(),
                    entry["isTerminal"].as_bool(),
                    entry["location"]["regionName"].as_str(),
                    entry["location"]["availabilityZone"].as_str(),
                ),
                (
                    Some("SetResourceAccessForBucket"),
                    Some("Bucket"),
                    Some("Succeeded"),
                    Some(true),
                    Some(row.region.unwrap_or("us-east-1")),
                    None,
                ),
                "case {case}: expected a succeeded SetResourceAccessForBucket entry located \
                 where the bucket is, got {entry:?}"
            );
            let id = entry["id"].as_str().unwrap_or_default();
            assert!(
                is_uuid_shaped(id),
                "case {case}: expected a uuid-shaped operation id, got {id:?}"
            );
            released.push(
                entry["resourceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        released.sort();
        let mut expected: Vec<String> = row.buckets.iter().map(|b| (*b).to_string()).collect();
        expected.sort();
        assert_eq!(
            released, expected,
            "case {case}: expected one release logged per bucket that granted the instance"
        );

        // The instance is gone from every read...
        let (status, body) = emulator
            .call("GetInstance", json!({"instanceName": row.deleted}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, NOT_FOUND, Some(DOES_NOT_EXIST)),
            "case {case}: expected the deleted instance to be gone, got {status} {body:?}"
        );

        // ...and so is every grant that named it. A grant another instance holds
        // is untouched, in the order access was given, and a bucket the deleted
        // instance never had access to is untouched too.
        let survivors: Vec<String> = row
            .survivor
            .map(|survivor| vec![survivor.to_string()])
            .unwrap_or_default();
        for bucket in row.buckets.iter().chain(row.other_buckets) {
            let after = granted(case, &emulator, row.region, bucket).await;
            assert_eq!(
                after, survivors,
                "case {case}: expected {bucket} to report only the grants it still holds"
            );
        }
    }
}

/// One bucket a grantee's removal must free: the Region every resource of the
/// case lives in, its zone, the bucket, the grantee the case deletes, and the
/// OTHER instance the case has, when it has one — a second grantee that outlives
/// the delete, or the successor granted access after it.
struct GrantedBucket {
    case: &'static str,
    region: Option<&'static str>,
    zone: &'static str,
    bucket: &'static str,
    deleted: &'static str,
    survivor: Option<&'static str>,
}

/// RULE (the same one, read through the condition the grant placed): the grant is
/// gone, so the condition it placed on the bucket's own delete is gone with it —
/// `DeleteBucket` requires `forceDelete` for a bucket "that has instances that
/// were granted access to it using the SetResourceAccessForBucket action", and a
/// bucket whose only grantee was the deleted instance no longer has one.
#[tokio::test]
async fn a_bucket_whose_only_grantee_was_deleted_deletes_without_force() {
    let emulator = Emulator::new();
    let cases: &[GrantedBucket] = &[
        GrantedBucket {
            case: "the only grantee is deleted",
            region: None,
            zone: "us-east-1a",
            bucket: "freed-bucket",
            deleted: "freed-grant-host",
            survivor: None,
        },
        GrantedBucket {
            case: "one of two grantees is deleted",
            region: Some("eu-west-1"),
            zone: "eu-west-1c",
            bucket: "still-held-bucket",
            deleted: "leaving-grant-host",
            survivor: Some("staying_grant_host"),
        },
    ];

    for GrantedBucket {
        case,
        region,
        zone,
        bucket,
        deleted,
        survivor,
    } in cases
    {
        create_bucket(&emulator, *region, bucket).await;
        create_instance(&emulator, zone, deleted, false).await;
        grant(&emulator, *region, bucket, deleted).await;
        if let Some(survivor) = survivor {
            create_instance(&emulator, zone, survivor, false).await;
            grant(&emulator, *region, bucket, survivor).await;
        }

        // Before the delete the grant blocks an unforced delete of the bucket.
        let (status, blocked) = emulator
            .call_in(*region, "DeleteBucket", json!({"bucketName": bucket}))
            .await;
        assert_eq!(
            (status, error_code(&blocked).as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected the grant to block an unforced delete, got {status} \
             {blocked:?}"
        );

        emulator
            .ok("DeleteInstance", json!({"instanceName": deleted}))
            .await;

        let (status, body) = emulator
            .call_in(*region, "DeleteBucket", json!({"bucketName": bucket}))
            .await;
        match survivor {
            // Nothing is granted access any more, so the delete needs no force.
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected the delete to need no force once the grant was \
                     released, got {status} {body:?}"
                );
                let (status, gone) = emulator
                    .call_in(*region, "GetBuckets", json!({"bucketName": bucket}))
                    .await;
                assert_eq!(
                    (status, error_code(&gone).as_str(), gone["code"].as_str()),
                    (400, NOT_FOUND, Some(DOES_NOT_EXIST)),
                    "case {case}: expected the deleted bucket to be gone, got {status} {gone:?}"
                );
            }
            // The other grantee still has access, so the condition still holds —
            // the release took back exactly the one grant the removal freed.
            Some(survivor) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidInputException"),
                    "case {case}: expected the surviving grant to still block an unforced \
                     delete, got {status} {body:?}"
                );
                // The release took back exactly the one grant the removal freed,
                // which is read from the bucket's own set rather than out of the
                // refusal's message: the message carries the service's own
                // sentence, not an inventory of what blocked this delete.
                assert_eq!(
                    granted(case, &emulator, *region, bucket).await,
                    vec![survivor.to_string()],
                    "case {case}: expected only the surviving grant to be left in the \
                     bucket's set, got {body:?}"
                );
            }
        }
    }
}

/// RULE (the same one, read back through a write): the released grant is really
/// out of the bucket's set, so a grant given afterwards to ANOTHER instance
/// leaves that instance alone in it — the deleted name is not still occupying a
/// place in the set.
#[tokio::test]
async fn a_grant_released_by_a_delete_leaves_the_set_free_for_another_instance() {
    let emulator = Emulator::new();
    let cases: &[GrantedBucket] = &[
        GrantedBucket {
            case: "a successor of another name",
            region: None,
            zone: "us-east-1a",
            bucket: "reused-grant-bucket",
            deleted: "first-grant-host",
            // The successor the case grants access to afterwards.
            survivor: Some("second-grant-host"),
        },
        GrantedBucket {
            case: "a successor reusing the deleted instance's name",
            region: Some("ca-central-1"),
            zone: "ca-central-1a",
            bucket: "reused-name-bucket",
            deleted: "recycled_host_1",
            survivor: Some("recycled_host_1"),
        },
    ];

    for GrantedBucket {
        case,
        region,
        zone,
        bucket,
        deleted,
        survivor,
    } in cases
    {
        let successor = survivor.expect("every case of this suite names a successor");
        create_bucket(&emulator, *region, bucket).await;
        create_instance(&emulator, zone, deleted, false).await;
        grant(&emulator, *region, bucket, deleted).await;
        emulator
            .ok("DeleteInstance", json!({"instanceName": deleted}))
            .await;
        assert_eq!(
            granted(case, &emulator, *region, bucket).await,
            Vec::<String>::new(),
            "case {case}: expected the released grant to leave the set empty"
        );

        create_instance(&emulator, zone, successor, false).await;
        grant(&emulator, *region, bucket, successor).await;
        assert_eq!(
            granted(case, &emulator, *region, bucket).await,
            vec![successor.to_string()],
            "case {case}: expected the new grantee to be alone in the set"
        );
    }
}
