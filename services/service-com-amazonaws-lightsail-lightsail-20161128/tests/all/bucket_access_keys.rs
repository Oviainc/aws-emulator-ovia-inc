// SPDX-License-Identifier: CC0-1.0

//! `CreateBucketAccessKey`, `GetBucketAccessKeys` and `DeleteBucketAccessKey` —
//! the access keys that grant programmatic access to a Lightsail bucket, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has, called
//! with a credential scope where a case needs a Region.
//!
//! The rule under test: an access key is a record OF ONE BUCKET. A create mints
//! one key against the bucket its request names in the Region that request is
//! made in, answers that key's secret in that one response and nowhere else, and
//! `GetBucketAccessKeys` answers exactly the keys that bucket still holds —
//! without a secret — while a bucket holds at most
//! [`MAX_BUCKET_ACCESS_KEYS`] of them at a time and a delete withdraws only the
//! key its request names from the bucket its request names.
//!
//! So the cases are TABLES: one row per number of keys a bucket may hold, per
//! name length at and inside the modelled `BucketName` bounds, per Region a
//! request can be made in (including no credential scope at all), and one row per
//! way a request can name a bucket or a key that is not there.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_BUCKET_ACCESS_KEYS;

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// A bundle every bucket in this suite is created on — the cheapest published
/// plan (`crate::state::BUCKET_BUNDLE_SPECS`); which plan a bucket is on decides
/// nothing about its access keys.
const BUNDLE: &str = "small_1_0";

/// A name of exactly 54 characters — the upper bound the modelled `BucketName`
/// length (3..54) admits.
const LONGEST_NAME: &str = "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcd";

/// The `AccessKeyLastUsed$region` and `$serviceName` of a key nothing has signed
/// a request with, from those members' own documentation: "This value is `N/A` if
/// the access key has not been used."
const NEVER_USED: &str = "N/A";

/// Whether `value` satisfies the modelled `IAMAccessKeyId` constraints the
/// `AccessKey$accessKeyId` member carries: length exactly 20 and pattern
/// `^[A-Z0-9]{20}$`
/// (`smithy_query(query="shape",shape="com.amazonaws.lightsail#AccessKey")`).
fn is_modelled_access_key_id(value: &str) -> bool {
    value.len() == 20
        && value
            .chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
}

/// Create the bucket `name` in `region`, so the suite has a bucket to mint keys
/// for.
async fn create_bucket(emulator: &Emulator, region: Option<&str>, name: &str) {
    emulator
        .ok_at(
            None,
            region,
            "CreateBucket",
            json!({"bucketName": name, "bundleId": BUNDLE}),
        )
        .await;
}

/// Mint one access key for `bucket` in `region`, answering the reported
/// `accessKey` and `operations`.
async fn mint(emulator: &Emulator, region: Option<&str>, bucket: &str) -> (Value, Value) {
    let reported = emulator
        .ok_at(
            None,
            region,
            "CreateBucketAccessKey",
            json!({"bucketName": bucket}),
        )
        .await;
    (
        reported["accessKey"].clone(),
        reported["operations"].clone(),
    )
}

/// The access keys of `bucket` in `region`, as `GetBucketAccessKeys` reports
/// them.
async fn read_keys(emulator: &Emulator, region: Option<&str>, bucket: &str) -> Vec<Value> {
    let reported = emulator
        .ok_at(
            None,
            region,
            "GetBucketAccessKeys",
            json!({"bucketName": bucket}),
        )
        .await;
    reported["accessKeys"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case reading the keys of {bucket}: expected an accessKeys array, got {reported:?}")
        })
        .clone()
}

/// The `accessKeyId`s of `bucket` in `region`, in the order the read reports
/// them.
async fn read_key_ids(emulator: &Emulator, region: Option<&str>, bucket: &str) -> Vec<String> {
    read_keys(emulator, region, bucket)
        .await
        .iter()
        .map(|key| {
            key["accessKeyId"]
                .as_str()
                .unwrap_or_else(|| panic!("every reported key carries an accessKeyId, got {key:?}"))
                .to_string()
        })
        .collect()
}

/// Assert the one operation-log entry a mutating access-key call answers with:
/// the entry names `operation_type`, the BUCKET it acted on, and is terminal
/// because the key existed (or was gone) as the call returned.
fn assert_one_terminal_operation(
    case: &str,
    operations: &Value,
    operation_type: &str,
    bucket: &str,
) {
    let operations = operations.as_array().unwrap_or_else(|| {
        panic!("case {case}: expected an operations array, got {operations:?}")
    });
    assert_eq!(
        operations.len(),
        1,
        "case {case}: expected one operation for one access-key call, got {operations:?}"
    );
    let operation = &operations[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some(operation_type),
        "case {case}: expected the {operation_type} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("Bucket"),
        "case {case}: expected the Bucket resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(bucket),
        "case {case}: expected the operation to name this bucket, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: expected a terminal operation status, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal operation, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
}

/// One row of the lifecycle table: where the bucket lives, what it is called and
/// how many keys the case mints for it.
struct Row {
    case: &'static str,
    /// The Region the requests are made in, or `None` for requests that carry no
    /// credential scope at all.
    region: Option<&'static str>,
    bucket: &'static str,
    /// How many access keys the case mints, one at a time.
    keys: usize,
}

/// The lifecycle cases. The rows differ along everything the rule generalises
/// over: how many of the keys a bucket may hold are minted, the bucket name's
/// length (both modelled bounds and between them) and alphabet, and the Region
/// the calls are made in.
const ROWS: &[Row] = &[
    Row {
        case: "one key, no credential scope at all",
        region: None,
        bucket: "keys-default-region",
        keys: 1,
    },
    Row {
        case: "both keys a bucket may hold, in us-west-2",
        region: Some("us-west-2"),
        bucket: "keys-two-of-them",
        keys: MAX_BUCKET_ACCESS_KEYS,
    },
    Row {
        case: "the shortest name the model admits, in eu-west-1",
        region: Some("eu-west-1"),
        bucket: "abc",
        keys: MAX_BUCKET_ACCESS_KEYS,
    },
    Row {
        case: "the longest name the model admits, in ap-south-1",
        region: Some("ap-south-1"),
        bucket: LONGEST_NAME,
        keys: 1,
    },
    Row {
        case: "a name of digits and hyphens, in sa-east-1",
        region: Some("sa-east-1"),
        bucket: "0ne-2-3hree9",
        keys: MAX_BUCKET_ACCESS_KEYS,
    },
];

/// Every lifecycle case: a create mints one key of the bucket it names and
/// answers its secret once, a read answers exactly the keys that bucket holds
/// without any secret, the quota refuses a create past the keys a bucket may hold
/// without minting one, and a delete removes exactly the key it names.
#[tokio::test]
async fn a_bucket_access_key_is_minted_read_back_and_deleted_per_bucket() {
    let emulator = Emulator::new();
    // Every id and every secret this suite ever saw, so no two keys — of one
    // bucket or of any other — share either.
    let mut ids = BTreeSet::new();
    let mut secrets = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_bucket(&emulator, row.region, row.bucket).await;

        // A bucket nothing has minted a key for holds none, and says so with an
        // empty list rather than an absent one.
        assert_eq!(
            read_keys(&emulator, row.region, row.bucket).await,
            Vec::<Value>::new(),
            "case {case}: a bucket with no access key must report an empty list"
        );

        let mut minted: Vec<String> = Vec::new();
        for ordinal in 0..row.keys {
            let (key, operations) = mint(&emulator, row.region, row.bucket).await;

            let id = key["accessKeyId"].as_str().unwrap_or_default();
            assert!(
                is_modelled_access_key_id(id),
                "case {case} key {ordinal}: expected a modelled access key id, got {id:?}"
            );
            assert!(
                ids.insert(id.to_string()),
                "case {case} key {ordinal}: the id {id:?} was already minted for another key"
            );

            // The secret is this response's alone: non-empty and carrying no
            // whitespace, as the modelled `NonEmptyString` pattern `\S` requires.
            let secret = key["secretAccessKey"].as_str().unwrap_or_default();
            assert!(
                !secret.is_empty() && !secret.chars().any(char::is_whitespace),
                "case {case} key {ordinal}: expected a non-empty whitespace-free secret, got \
                 {secret:?}"
            );
            assert!(
                secrets.insert(secret.to_string()),
                "case {case} key {ordinal}: the secret of another key was handed out again"
            );

            assert_eq!(
                key["status"].as_str(),
                Some("Active"),
                "case {case} key {ordinal}: expected a new key to be Active, got {key:?}"
            );
            assert!(
                key["createdAt"].is_number(),
                "case {case} key {ordinal}: expected a createdAt timestamp, got {key:?}"
            );
            // The modelled `AccessKeyLastUsed` documents that "this object does
            // not include data in the response of a CreateBucketAccessKey
            // action".
            assert!(
                key.get("lastUsed").is_none(),
                "case {case} key {ordinal}: a create must report no lastUsed, got {key:?}"
            );
            assert_one_terminal_operation(case, &operations, "CreateBucketAccessKey", row.bucket);

            minted.push(id.to_string());

            // The read answers exactly the keys minted so far, in the order they
            // were created, and reports no secret for any of them.
            let reported = read_keys(&emulator, row.region, row.bucket).await;
            let reported_ids: Vec<&str> = reported
                .iter()
                .map(|key| key["accessKeyId"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                reported_ids, minted,
                "case {case} key {ordinal}: expected the keys minted so far, got {reported:?}"
            );
            for read in &reported {
                assert!(
                    read.get("secretAccessKey").is_none(),
                    "case {case} key {ordinal}: a read must report no secret, got {read:?}"
                );
                assert_eq!(
                    read["status"].as_str(),
                    Some("Active"),
                    "case {case} key {ordinal}: expected a read key to be Active, got {read:?}"
                );
                assert_eq!(
                    read["lastUsed"]["region"].as_str(),
                    Some(NEVER_USED),
                    "case {case} key {ordinal}: expected an unused key's region, got {read:?}"
                );
                assert_eq!(
                    read["lastUsed"]["serviceName"].as_str(),
                    Some(NEVER_USED),
                    "case {case} key {ordinal}: expected an unused key's service, got {read:?}"
                );
                assert!(
                    read["lastUsed"].get("lastUsedDate").is_none(),
                    "case {case} key {ordinal}: an unused key has no lastUsedDate, got {read:?}"
                );
            }
            // The key the create reported and the key the read reports are the
            // same record: same id, same creation instant.
            let same = reported
                .iter()
                .find(|read| read["accessKeyId"].as_str() == Some(id))
                .unwrap_or_else(|| panic!("case {case} key {ordinal}: the minted key is read back"));
            assert_eq!(
                same["createdAt"], key["createdAt"],
                "case {case} key {ordinal}: expected the created instant to be stable, got \
                 {same:?}"
            );
        }

        // Reading changes nothing a later caller sees.
        assert_eq!(
            read_key_ids(&emulator, row.region, row.bucket).await,
            minted,
            "case {case}: a second read must report the very same keys"
        );

        // A bucket already holding every key it may have refuses another create,
        // with a 400 and nothing minted.
        if row.keys == MAX_BUCKET_ACCESS_KEYS {
            let (status, refused) = emulator
                .call_in(
                    row.region,
                    "CreateBucketAccessKey",
                    json!({"bucketName": row.bucket}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 past {MAX_BUCKET_ACCESS_KEYS} keys, got {status} \
                 {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidInputException",
                "case {case}: expected InvalidInputException, got {refused:?}"
            );
            assert_eq!(
                read_key_ids(&emulator, row.region, row.bucket).await,
                minted,
                "case {case}: a refused create must mint nothing"
            );
        }

        // Each delete removes exactly the key it names, leaving the rest; the
        // same delete a second time names a key the bucket no longer holds.
        let mut left = minted.clone();
        for id in &minted {
            let reported = emulator
                .ok_at(
                    None,
                    row.region,
                    "DeleteBucketAccessKey",
                    json!({"bucketName": row.bucket, "accessKeyId": id}),
                )
                .await;
            assert_one_terminal_operation(
                case,
                &reported["operations"],
                "DeleteBucketAccessKey",
                row.bucket,
            );
            left.retain(|held| held != id);
            assert_eq!(
                read_key_ids(&emulator, row.region, row.bucket).await,
                left,
                "case {case}: after deleting {id:?} the bucket must hold the rest"
            );

            let (status, missing) = emulator
                .call_in(
                    row.region,
                    "DeleteBucketAccessKey",
                    json!({"bucketName": row.bucket, "accessKeyId": id}),
                )
                .await;
            assert_eq!(
                status, 404,
                "case {case}: deleting {id:?} twice must be a 404, got {status} {missing:?}"
            );
            assert_eq!(
                error_code(&missing),
                "NotFoundException",
                "case {case}: expected NotFoundException, got {missing:?}"
            );
        }

        // The quota is on the keys a bucket HOLDS, not on the keys it ever had:
        // with its keys deleted the bucket can be given another one.
        let (fresh, _) = mint(&emulator, row.region, row.bucket).await;
        let fresh_id = fresh["accessKeyId"].as_str().unwrap_or_default();
        assert!(
            ids.insert(fresh_id.to_string()),
            "case {case}: the replacement key reused the id {fresh_id:?}"
        );
        assert_eq!(
            read_key_ids(&emulator, row.region, row.bucket).await,
            vec![fresh_id.to_string()],
            "case {case}: the replacement key must be the one key the bucket holds"
        );
    }
}

/// Every operation on an access key is about ONE bucket of ONE Region: a bucket
/// name that names nothing is a 404, and a key of one bucket is neither reported
/// nor deleted through another bucket's name or another Region's.
#[tokio::test]
async fn an_access_key_belongs_to_one_bucket_in_one_region() {
    let emulator = Emulator::new();

    // (case, the operation, the body it is called with) — every access-key
    // operation, each naming a bucket this account holds none of.
    let absent: &[(&str, &str, Value)] = &[
        (
            "creating a key for a bucket that was never created",
            "CreateBucketAccessKey",
            json!({"bucketName": "no-such-bucket"}),
        ),
        (
            "reading the keys of a bucket that was never created",
            "GetBucketAccessKeys",
            json!({"bucketName": "no-such-bucket"}),
        ),
        (
            "deleting a key of a bucket that was never created",
            "DeleteBucketAccessKey",
            json!({"bucketName": "no-such-bucket", "accessKeyId": "EMULATORAKAAAAAAAAAA"}),
        ),
    ];
    for (case, operation, body) in absent {
        let (status, missing) = emulator.call_in(None, operation, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case}: expected a 404, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "NotFoundException",
            "case {case}: expected NotFoundException, got {missing:?}"
        );
    }

    // Two buckets of one Region, each with its own key.
    create_bucket(&emulator, None, "keys-bucket-one").await;
    create_bucket(&emulator, None, "keys-bucket-two").await;
    let (first, _) = mint(&emulator, None, "keys-bucket-one").await;
    let (second, _) = mint(&emulator, None, "keys-bucket-two").await;
    let first_id = first["accessKeyId"].as_str().unwrap_or_default().to_string();
    let second_id = second["accessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Each bucket reports its own key and nothing of the other's.
    for (bucket, held, other) in [
        ("keys-bucket-one", &first_id, &second_id),
        ("keys-bucket-two", &second_id, &first_id),
    ] {
        assert_eq!(
            read_key_ids(&emulator, None, bucket).await,
            vec![held.clone()],
            "case {bucket}: a bucket reports only its own access keys"
        );

        // A key of the OTHER bucket is not deleted through this one's name, and
        // the bucket that holds it still holds it afterwards.
        let (status, missing) = emulator
            .call_in(
                None,
                "DeleteBucketAccessKey",
                json!({"bucketName": bucket, "accessKeyId": other}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {bucket}: deleting another bucket's key must be a 404, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "NotFoundException",
            "case {bucket}: expected NotFoundException, got {missing:?}"
        );
    }
    assert_eq!(
        read_key_ids(&emulator, None, "keys-bucket-one").await,
        vec![first_id.clone()],
        "case cross-bucket delete: the refused delete left the key in place"
    );

    // A bucket belongs to the Region it was created in, so its access keys are reached
    // only through that Region: every access-key call made in another Region names no
    // bucket at all, and leaves the key where it is. (The name cannot be taken by a
    // second bucket in that other Region either — a bucket's name is its address in the
    // one object storage namespace this emulator serves, which `bucket_lifecycle.rs`
    // pins.)
    let regional = "regional-keyed-bucket";
    create_bucket(&emulator, Some("us-east-2"), regional).await;
    let (east, _) = mint(&emulator, Some("us-east-2"), regional).await;
    let east_id = east["accessKeyId"].as_str().unwrap_or_default().to_string();

    assert_eq!(
        read_key_ids(&emulator, Some("us-east-2"), regional).await,
        vec![east_id.clone()],
        "case two Regions: the Region the key was minted in reports it"
    );
    let elsewhere: &[(&str, &str, Value)] = &[
        (
            "reading the keys from another Region",
            "GetBucketAccessKeys",
            json!({"bucketName": regional}),
        ),
        (
            "minting a key from another Region",
            "CreateBucketAccessKey",
            json!({"bucketName": regional}),
        ),
        (
            "deleting the key from another Region",
            "DeleteBucketAccessKey",
            json!({"bucketName": regional, "accessKeyId": &east_id}),
        ),
    ];
    for (case, operation, body) in elsewhere {
        let (status, missing) = emulator
            .call_in(Some("eu-central-1"), operation, body.clone())
            .await;
        assert_eq!(
            status, 404,
            "case {case}: expected a 404, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "NotFoundException",
            "case {case}: expected NotFoundException, got {missing:?}"
        );
    }
    assert_eq!(
        read_key_ids(&emulator, Some("us-east-2"), regional).await,
        vec![east_id],
        "case two Regions: the refused calls left the key in place"
    );
}
