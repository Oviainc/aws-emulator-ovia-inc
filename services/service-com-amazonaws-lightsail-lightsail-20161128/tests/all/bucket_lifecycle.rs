// SPDX-License-Identifier: CC0-1.0

//! `CreateBucket` and `GetBuckets` — the Lightsail object-storage bucket, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has, called
//! with a credential scope where a case needs a region.
//!
//! The rule under test: `CreateBucket` stores ONE bucket record per (Region,
//! name) built from the request — the bundle it named, the tags it carried and
//! whether it asked for object versioning kept verbatim, every other member
//! assigned by the service from that request plus the store's own allocators —
//! and `GetBuckets` answers from that one record, so what a create reported is
//! what a later read reports.
//!
//! So the cases are TABLES: one row per published bundle plan, per form the
//! optional `enableObjectVersioning` flag takes, per tag-list size, per name
//! length at and inside the modelled bounds, and per Region a request can be made
//! in; the refusals are a table of the ways a request can be outside what the
//! service serves.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    BUCKET_BUNDLE_SPECS, BUCKET_PAGE_SIZE, MAX_BUCKETS, MAX_TAGS,
};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The endpoint these cases reach the emulator on (its `Host`), which is the
/// address a bucket's `url` must therefore be announced under. The router's
/// default data port, as `instance_lifecycle.rs`'s sibling suites use it.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The path-style S3 address of a bucket named `name` under the endpoint `host`:
/// what `Bucket$url` announces, because that is where this emulator's Amazon S3
/// data plane answers for the bucket (`crate::state::bucket_url`).
fn announced_url(host: &str, name: &str) -> String {
    format!("https://{host}/{name}")
}

/// Every member the modelled `Bucket` structure declares that a read without
/// `includeConnectedResources` or `includeCors` reports
/// (`smithy_query(query="shape",shape="Bucket")`), in wire spelling.
///
/// `resourcesReceivingAccess` and `cors` are the two the flags select, so they
/// are deliberately absent from this set.
const BUCKET_MEMBERS: &[&str] = &[
    "ableToUpdateBundle",
    "accessLogConfig",
    "accessRules",
    "arn",
    "bundleId",
    "createdAt",
    "location",
    "name",
    "objectVersioning",
    "readonlyAccessAccounts",
    "resourceType",
    "state",
    "supportCode",
    "tags",
    "url",
];

/// A name of exactly 54 characters — the upper bound the modelled `BucketName`
/// length (3..54) admits.
const LONGEST_NAME: &str = "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcd";

/// One create case: what the request carries, and what the bucket must report for
/// the members the request decides.
struct Row {
    case: &'static str,
    /// The region the request is made in, or `None` for a request that carries no
    /// credential scope at all.
    region: Option<&'static str>,
    /// The region the bucket must report — [`DEFAULT_REGION`] for an unsigned
    /// request.
    reported_region: &'static str,
    name: &'static str,
    bundle_id: &'static str,
    /// The `enableObjectVersioning` the request carries, absent when `None`.
    enable_object_versioning: Option<bool>,
    /// The tags the request carries, as `(key, value)` — a value of `None` is a
    /// key-only tag.
    tags: &'static [(&'static str, Option<&'static str>)],
    /// The `objectVersioning` the bucket must report, from the modelled
    /// `Bucket$objectVersioning` values.
    object_versioning: &'static str,
}

/// The create cases. The rows differ along every dimension the request has: the
/// bundle plan, the versioning flag's three forms, the tag list's size and shape,
/// the name's length and alphabet, and the Region the call is made in.
const ROWS: &[Row] = &[
    Row {
        case: "the cheapest plan, no optional member at all",
        region: None,
        reported_region: DEFAULT_REGION,
        name: "storage-one",
        bundle_id: "small_1_0",
        enable_object_versioning: None,
        tags: &[],
        object_versioning: "NeverEnabled",
    },
    Row {
        case: "versioning asked for, one tag, in us-west-2",
        region: Some("us-west-2"),
        reported_region: "us-west-2",
        name: "storage-two",
        bundle_id: "medium_1_0",
        enable_object_versioning: Some(true),
        tags: &[("stage", Some("prod"))],
        object_versioning: "Enabled",
    },
    Row {
        case: "versioning explicitly declined, three tags, in eu-west-1",
        region: Some("eu-west-1"),
        reported_region: "eu-west-1",
        name: "storage-three-250",
        bundle_id: "large_1_0",
        enable_object_versioning: Some(false),
        tags: &[
            ("owner", Some("platform")),
            ("key-only", None),
            ("cost-centre", Some("4711")),
        ],
        object_versioning: "NeverEnabled",
    },
    Row {
        case: "the shortest name the model admits",
        region: Some("ap-northeast-1"),
        reported_region: "ap-northeast-1",
        name: "abc",
        bundle_id: "small_1_0",
        enable_object_versioning: Some(true),
        tags: &[],
        object_versioning: "Enabled",
    },
    Row {
        case: "the longest name the model admits",
        region: Some("sa-east-1"),
        reported_region: "sa-east-1",
        name: LONGEST_NAME,
        bundle_id: "medium_1_0",
        enable_object_versioning: None,
        tags: &[("only", Some("tag"))],
        object_versioning: "NeverEnabled",
    },
    Row {
        case: "a digits-only name",
        region: Some("ca-central-1"),
        reported_region: "ca-central-1",
        name: "012345",
        bundle_id: "large_1_0",
        enable_object_versioning: Some(false),
        tags: &[],
        object_versioning: "NeverEnabled",
    },
];

/// The `CreateBucket` body `row` describes.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"bucketName": row.name, "bundleId": row.bundle_id});
    if let Some(enable) = row.enable_object_versioning {
        body["enableObjectVersioning"] = json!(enable);
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| match value {
                    Some(value) => json!({"key": key, "value": value}),
                    None => json!({"key": key}),
                })
                .collect(),
        );
    }
    body
}

/// The tags of `bucket`, as `(key, value)` pairs.
fn reported_tags(bucket: &Value) -> Vec<(String, Option<String>)> {
    bucket["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a bucket always reports its tags as a list, got {bucket:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// Create the bucket `body` describes in `region`, answering the reported
/// `bucket` and `operations`.
async fn create(emulator: &Emulator, region: Option<&str>, body: Value) -> (Value, Value) {
    let reported = emulator
        .ok_at(Some(ENDPOINT_HOST), region, "CreateBucket", body)
        .await;
    (reported["bucket"].clone(), reported["operations"].clone())
}

/// The bucket `name` in `region`, as `GetBuckets` reports it when asked for that
/// one bucket.
async fn read(emulator: &Emulator, region: Option<&str>, name: &str) -> Value {
    let body = emulator
        .ok_at(
            Some(ENDPOINT_HOST),
            region,
            "GetBuckets",
            json!({"bucketName": name}),
        )
        .await;
    let buckets = body["buckets"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading {name}: expected a buckets array, got {body:?}"))
        .clone();
    assert_eq!(
        buckets.len(),
        1,
        "case reading {name}: expected exactly the one named bucket, got {buckets:?}"
    );
    buckets[0].clone()
}

/// Every bucket name `GetBuckets` reports in `region`, walking every page.
///
/// Answers the names in page order together with the size of each page, so a
/// caller can assert both the contents and the paging.
async fn list(emulator: &Emulator, region: Option<&str>) -> (Vec<String>, Vec<usize>) {
    let mut names = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let body = emulator
            .ok_at(Some(ENDPOINT_HOST), region, "GetBuckets", request)
            .await;
        let page = body["buckets"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case listing buckets in {region:?}: expected an array, got {body:?}")
            })
            .clone();
        page_sizes.push(page.len());
        for bucket in page {
            names.push(
                bucket["name"]
                    .as_str()
                    .expect("every listed bucket carries a name")
                    .to_string(),
            );
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case listing buckets in {region:?}: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (names, page_sizes)
}

/// Now, in epoch seconds — the unit an awsJson timestamp is carried in.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Every create case: the bucket reports what the request asked for, the service
/// assigns the rest, and `GetBuckets` answers the very same record.
#[tokio::test]
async fn a_created_bucket_reports_the_request_and_the_members_the_service_assigns() {
    let emulator = Emulator::new();
    let mut arns = BTreeSet::new();
    let mut support_codes = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let before = now_epoch_seconds();
        let (bucket, operations) = create(&emulator, row.region, create_body(row)).await;

        // What the caller asked for, kept verbatim.
        assert_eq!(
            bucket["name"].as_str(),
            Some(row.name),
            "case {case}: expected the requested name, got {:?}",
            bucket["name"]
        );
        assert_eq!(
            bucket["bundleId"].as_str(),
            Some(row.bundle_id),
            "case {case}: expected the requested bundle, got {:?}",
            bucket["bundleId"]
        );
        assert_eq!(
            bucket["objectVersioning"].as_str(),
            Some(row.object_versioning),
            "case {case}: expected objectVersioning {}, got {:?}",
            row.object_versioning,
            bucket["objectVersioning"]
        );
        let expected_tags: Vec<(String, Option<String>)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.map(str::to_string)))
            .collect();
        assert_eq!(
            reported_tags(&bucket),
            expected_tags,
            "case {case}: expected the requested tags, got {:?}",
            bucket["tags"]
        );

        // What the service assigns, each computed from the request plus the
        // store's own allocators.
        assert_eq!(
            bucket["resourceType"].as_str(),
            Some("Bucket"),
            "case {case}: expected the Bucket resource type, got {bucket:?}"
        );
        assert_eq!(
            bucket["location"]["regionName"].as_str(),
            Some(row.reported_region),
            "case {case}: expected the region of the request, got {:?}",
            bucket["location"]
        );
        assert!(
            bucket["location"]["availabilityZone"].is_null(),
            "case {case}: a bucket is regional, so it must report no availability zone, got {:?}",
            bucket["location"]
        );
        let arn_prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Bucket/", row.reported_region);
        let arn = bucket["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an arn, got {bucket:?}"));
        let arn_id = arn.strip_prefix(&arn_prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an arn under {arn_prefix:?}, got {arn:?}")
        });
        assert!(
            is_uuid_shaped(arn_id),
            "case {case}: expected a uuid-shaped arn id, got {arn_id:?}"
        );
        let support_code = bucket["supportCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a supportCode, got {bucket:?}"));
        assert_eq!(
            support_code,
            format!("{ACCOUNT}/{arn_id}"),
            "case {case}: the support code must name the account and this bucket's id"
        );
        assert_eq!(
            bucket["url"].as_str(),
            Some(announced_url(ENDPOINT_HOST, row.name).as_str()),
            "case {case}: expected the bucket's address under the endpoint this request \
             arrived at, got {:?}",
            bucket["url"]
        );
        let created_at = bucket["createdAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected a createdAt, got {bucket:?}"));
        assert!(
            created_at >= before - 300.0 && created_at <= now_epoch_seconds() + 300.0,
            "case {case}: expected the creation time to be now, got {created_at}"
        );
        assert_eq!(
            bucket["state"]["code"].as_str(),
            Some("OK"),
            "case {case}: a bucket is running as the create returns, got {:?}",
            bucket["state"]
        );
        assert_eq!(
            bucket["ableToUpdateBundle"].as_bool(),
            Some(true),
            "case {case}: a bucket that has never been moved between bundles can be, got {:?}",
            bucket["ableToUpdateBundle"]
        );
        assert_eq!(
            bucket["accessRules"]["getObject"].as_str(),
            Some("private"),
            "case {case}: a new bucket's objects are private, got {:?}",
            bucket["accessRules"]
        );
        assert_eq!(
            bucket["accessRules"]["allowPublicOverrides"].as_bool(),
            Some(false),
            "case {case}: nothing has allowed per-object overrides, got {:?}",
            bucket["accessRules"]
        );
        assert_eq!(
            bucket["readonlyAccessAccounts"].as_array(),
            Some(&Vec::new()),
            "case {case}: no account has read-only access to a new bucket, got {:?}",
            bucket["readonlyAccessAccounts"]
        );
        assert_eq!(
            bucket["accessLogConfig"]["enabled"].as_bool(),
            Some(false),
            "case {case}: access logging is off until it is turned on, got {:?}",
            bucket["accessLogConfig"]
        );

        // Exactly the modelled members a read without the two flags reports.
        let members: BTreeSet<&str> = bucket
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: a bucket is an object, got {bucket:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            BUCKET_MEMBERS.iter().copied().collect::<BTreeSet<_>>(),
            "case {case}: expected exactly the modelled Bucket members, got {members:?}"
        );

        // The create wrote ONE operation-log entry, and the bucket exists as the
        // call returns, so that entry is terminal.
        let operations = operations
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {operations:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected one operation for one created bucket, got {operations:?}"
        );
        let operation = &operations[0];
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateBucket"),
            "case {case}: expected the CreateBucket operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Bucket"),
            "case {case}: expected the Bucket resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.name),
            "case {case}: expected the operation to name this bucket, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: the bucket exists as the call returns, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: nothing is owed on this operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.reported_region),
            "case {case}: expected the operation in the region of the request, got {operation:?}"
        );
        let operation_id = operation["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an operation id, got {operation:?}"));
        assert!(
            is_uuid_shaped(operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );

        // The round trip: the stored record is what a later read answers from.
        let read_back = read(&emulator, row.region, row.name).await;
        assert_eq!(
            read_back, bucket,
            "case {case}: expected the read to answer the stored record, got {read_back:?}"
        );

        // Every bucket gets its OWN identifiers.
        assert!(
            arns.insert(arn.to_string()),
            "case {case}: arn {arn:?} was handed out twice"
        );
        assert!(
            support_codes.insert(support_code.to_string()),
            "case {case}: support code {support_code:?} was handed out twice"
        );
    }
}

/// Every plan the service publishes can be bought, and a bundle id it does not
/// publish is refused with the modelled `InvalidInputException` and creates
/// nothing.
///
/// The accepted rows come from the published catalog itself, so the rule holds
/// whatever that catalog grows to hold.
#[tokio::test]
async fn every_published_bundle_is_accepted_and_an_unpublished_one_is_refused() {
    let emulator = Emulator::new();

    for (index, spec) in BUCKET_BUNDLE_SPECS.iter().enumerate() {
        let case = spec.bundle_id;
        let name = format!("plan-{index}-bucket");
        let (bucket, _) = create(
            &emulator,
            None,
            json!({"bucketName": name, "bundleId": spec.bundle_id}),
        )
        .await;
        assert_eq!(
            bucket["bundleId"].as_str(),
            Some(spec.bundle_id),
            "case {case}: expected the plan the request named, got {:?}",
            bucket["bundleId"]
        );
    }

    // Ids the catalog does not publish: a size it never offered, a version it
    // never shipped, a bare size token and the right id in the wrong case.
    for (index, unpublished) in ["tiny_1_0", "small_9_9", "small", "SMALL_1_0"]
        .into_iter()
        .enumerate()
    {
        let name = format!("refused-{index}-bucket");
        let (status, refused) = emulator
            .call_in(
                None,
                "CreateBucket",
                json!({"bucketName": name, "bundleId": unpublished}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case bundleId {unpublished:?}: expected a 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case bundleId {unpublished:?}: expected InvalidInputException, got {refused:?}"
        );

        // A refused create persisted nothing: the name is still unknown.
        let (missing_status, missing) = emulator
            .call_in(None, "GetBuckets", json!({"bucketName": name}))
            .await;
        assert_eq!(
            missing_status, 404,
            "case bundleId {unpublished:?}: a refused create must store nothing, got \
             {missing_status} {missing:?}"
        );
    }

    // Every accepted bucket, and only those, is in the listing.
    let (names, _) = list(&emulator, None).await;
    let expected: Vec<String> = (0..BUCKET_BUNDLE_SPECS.len())
        .map(|index| format!("plan-{index}-bucket"))
        .collect();
    assert_eq!(
        names, expected,
        "case the listing: expected one bucket per published plan, got {names:?}"
    );
}

/// A bucket's name is its ADDRESS in the one object storage namespace this
/// emulator serves (`Bucket$url` is "The URL of the bucket", and the path-style S3
/// address it announces carries the name and no Region), so a name that is taken is
/// taken whatever Region the create is made in: a second bucket of that name in
/// another Region is not created at all, rather than created and unreachable. The
/// refusal leaves the bucket that holds the name, and every Region's listing,
/// exactly as they were.
///
/// The rows differ along the Region the second create is made in: the one that
/// holds the name, and another one.
#[tokio::test]
async fn a_taken_name_is_refused_in_every_region() {
    let emulator = Emulator::new();
    let body = json!({"bucketName": "shared-name", "bundleId": "small_1_0"});

    let (first, _) = create(&emulator, Some("us-east-2"), body.clone()).await;

    for (case, region) in [
        ("the same name in the same region", "us-east-2"),
        ("the same name in another region", "eu-central-1"),
    ] {
        let (status, refused) = emulator
            .call_in(Some(region), "CreateBucket", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {refused:?}"
        );
        // The refusal left the bucket that holds the name exactly as it was.
        let unchanged = read(&emulator, Some("us-east-2"), "shared-name").await;
        assert_eq!(
            unchanged, first,
            "case {case}: the refused create must change nothing, got {unchanged:?}"
        );
        // And created nothing in the Region it was made in.
        let (names, _) = list(&emulator, Some(region)).await;
        let expected: Vec<String> = if region == "us-east-2" {
            vec!["shared-name".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            names, expected,
            "case {case}: expected {expected:?} in {region}, got {names:?}"
        );
    }

    // The name is free again once the bucket that holds it is deleted, which is
    // what releases it: "the bucket name is released and can be reused for a new
    // bucket" (`DeleteBucket`).
    emulator
        .ok_at(
            Some(ENDPOINT_HOST),
            Some("us-east-2"),
            "DeleteBucket",
            json!({"bucketName": "shared-name"}),
        )
        .await;
    let (elsewhere, _) = create(&emulator, Some("eu-central-1"), body).await;
    assert_eq!(
        elsewhere["location"]["regionName"].as_str(),
        Some("eu-central-1"),
        "case the released name in another region: expected that region, got {elsewhere:?}"
    );
    assert_ne!(
        elsewhere["arn"], first["arn"],
        "case the released name in another region: expected its own arn, got {:?}",
        elsewhere["arn"]
    );
    for (region, expected) in [
        ("us-east-2", Vec::new()),
        ("eu-central-1", vec!["shared-name".to_string()]),
        (DEFAULT_REGION, Vec::new()),
    ] {
        let (names, _) = list(&emulator, Some(region)).await;
        assert_eq!(
            names, expected,
            "case listing {region}: expected {expected:?}, got {names:?}"
        );
    }
}

/// What `GetBuckets` is asked for is what it answers: a name that names nothing
/// is a 404, the two include flags select the members they document, and
/// `includeCors` outside the single-bucket form it documents is refused.
#[tokio::test]
async fn get_buckets_answers_what_the_request_selects() {
    let emulator = Emulator::new();
    create(
        &emulator,
        None,
        json!({"bucketName": "selected-bucket", "bundleId": "small_1_0"}),
    )
    .await;

    // A name that names nothing is a 404, whatever shape it has.
    for absent in ["never-created", "selected-bucket-2", "abc"] {
        let (status, missing) = emulator
            .call_in(None, "GetBuckets", json!({"bucketName": absent}))
            .await;
        assert_eq!(
            status, 404,
            "case bucketName {absent:?}: expected a 404, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "NotFoundException",
            "case bucketName {absent:?}: expected NotFoundException, got {missing:?}"
        );
    }

    // `includeConnectedResources` selects `resourcesReceivingAccess`, and
    // `includeCors` selects `cors` — each reported only when it was asked for.
    // (`includeCors` is "only supported when getting a single bucket with
    // bucketName specified", so each row names the bucket.)
    let rows: &[(&str, Value, bool, bool)] = &[
        ("no flags", json!({}), false, false),
        (
            "both flags off",
            json!({"includeConnectedResources": false, "includeCors": false}),
            false,
            false,
        ),
        (
            "connected resources only",
            json!({"includeConnectedResources": true}),
            true,
            false,
        ),
        ("cors only", json!({"includeCors": true}), false, true),
        (
            "both flags on",
            json!({"includeConnectedResources": true, "includeCors": true}),
            true,
            true,
        ),
    ];

    for (case, flags, wants_connected, wants_cors) in rows {
        let mut request = json!({"bucketName": "selected-bucket"});
        for (member, value) in flags.as_object().expect("the flags are an object") {
            request[member] = value.clone();
        }
        let body = emulator
            .ok_at(Some(ENDPOINT_HOST), None, "GetBuckets", request)
            .await;
        let bucket = body["buckets"][0].clone();
        assert_eq!(
            bucket["resourcesReceivingAccess"].is_null(),
            !wants_connected,
            "case {case}: resourcesReceivingAccess must be reported only when asked for, got {bucket:?}"
        );
        if *wants_connected {
            assert_eq!(
                bucket["resourcesReceivingAccess"].as_array(),
                Some(&Vec::new()),
                "case {case}: no instance has access to this bucket, got {:?}",
                bucket["resourcesReceivingAccess"]
            );
        }
        assert_eq!(
            bucket["cors"].is_null(),
            !wants_cors,
            "case {case}: cors must be reported only when asked for, got {bucket:?}"
        );
        if *wants_cors {
            assert_eq!(
                bucket["cors"]["rules"].as_array(),
                Some(&Vec::new()),
                "case {case}: a new bucket carries no CORS rule, got {:?}",
                bucket["cors"]
            );
        }

        // The synchronization status is reported on every read, and says that
        // nothing has been synchronized with Amazon S3.
        let sync = &body["accountLevelBpaSync"];
        assert_eq!(
            sync["status"].as_str(),
            Some("NeverSynced"),
            "case {case}: expected the never-synchronized status, got {sync:?}"
        );
        assert_eq!(
            sync["message"].as_str(),
            Some("SYNC_ON_HOLD"),
            "case {case}: expected the on-hold reason, got {sync:?}"
        );
        assert_eq!(
            sync["bpaImpactsLightsail"].as_bool(),
            Some(false),
            "case {case}: an unsynchronized account-level BPA affects no bucket here, got {sync:?}"
        );
        assert!(
            sync["lastSyncedAt"].is_null(),
            "case {case}: nothing has synchronized, so there is no lastSyncedAt, got {sync:?}"
        );
    }

    // `includeCors` without a `bucketName` is outside what the member documents.
    for (case, request) in [
        ("includeCors alone", json!({"includeCors": true})),
        (
            "includeCors with the other flag",
            json!({"includeCors": true, "includeConnectedResources": true}),
        ),
    ] {
        let (status, refused) = emulator.call_in(None, "GetBuckets", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {refused:?}"
        );
    }
}

/// A tag list of `count` distinct `(key, value)` tags.
fn tag_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("key-{index}"), "value": format!("value-{index}")}))
            .collect(),
    )
}

/// The two bounds a create is refused by — the tag list's and the store's — are
/// checked BEFORE the first write, so each refusal persists nothing, and the
/// value AT each bound is still accepted.
#[tokio::test]
async fn the_pre_write_bounds_accept_the_bound_and_refuse_past_it_without_persisting() {
    let emulator = Emulator::new();

    // Exactly as many tags as a Lightsail resource may carry: accepted, and
    // every one of them kept.
    let (bucket, _) = create(
        &emulator,
        None,
        json!({
            "bucketName": "tagged-bucket",
            "bundleId": "small_1_0",
            "tags": tag_list(MAX_TAGS)
        }),
    )
    .await;
    assert_eq!(
        bucket["tags"].as_array().map(Vec::len),
        Some(MAX_TAGS),
        "case {MAX_TAGS} tags: expected every tag of the request, got {:?}",
        bucket["tags"]
    );

    // One tag past the bound: refused, and nothing stored under that name.
    let over = MAX_TAGS + 1;
    let (status, refused) = emulator
        .call_in(
            None,
            "CreateBucket",
            json!({
                "bucketName": "over-tagged-bucket",
                "bundleId": "small_1_0",
                "tags": tag_list(over)
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {over} tags: expected a 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "case {over} tags: expected InvalidInputException, got {refused:?}"
    );
    let (missing_status, missing) = emulator
        .call_in(
            None,
            "GetBuckets",
            json!({"bucketName": "over-tagged-bucket"}),
        )
        .await;
    assert_eq!(
        missing_status, 404,
        "case {over} tags: a refused create must store nothing, got {missing_status} {missing:?}"
    );

    // Fill the store to its bound (the tagged bucket above is the first of them),
    // then ask for one more.
    for index in 1..MAX_BUCKETS {
        create(
            &emulator,
            None,
            json!({
                "bucketName": format!("guard-bucket-{index:03}"),
                "bundleId": "small_1_0"
            }),
        )
        .await;
    }
    let past_the_store = MAX_BUCKETS + 1;
    let (status, refused) = emulator
        .call_in(
            None,
            "CreateBucket",
            json!({"bucketName": "one-too-many-bucket", "bundleId": "small_1_0"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case bucket {past_the_store}: expected a 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "case bucket {past_the_store}: expected InvalidInputException, got {refused:?}"
    );
    let (names, _) = list(&emulator, None).await;
    assert_eq!(
        names.len(),
        MAX_BUCKETS,
        "case bucket {past_the_store}: the refused create must store nothing, got {} buckets",
        names.len()
    );
    assert!(
        !names.contains(&"one-too-many-bucket".to_string()),
        "case bucket {past_the_store}: the refused name must not be listed, got {names:?}"
    );
}

/// The address a bucket announces is the endpoint THIS request arrived at — the
/// emulator's own inbound listener — and never an AWS hostname; a request that
/// names no host names no endpoint to announce, so `url` is omitted rather than
/// filled with an address nothing serves.
///
/// The rows differ along the endpoint the request arrived at (two different hosts,
/// and no host at all) and the Region the bucket lives in — which the address does
/// NOT carry, since a bucket's name alone addresses it in the object namespace the
/// endpoint serves.
#[tokio::test]
async fn the_announced_address_is_the_endpoint_the_request_arrived_at() {
    let emulator = Emulator::new();

    // (case, the Host the request carries, the region it is made in)
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        ("the default data port", Some(ENDPOINT_HOST), None),
        (
            "another host and port",
            Some("lightsail.test.internal:8443"),
            Some("us-west-2"),
        ),
        ("a bare host", Some("emulator.local"), Some("eu-west-1")),
        ("no host at all", None, Some("ap-northeast-1")),
    ];

    for (index, (case, host, region)) in rows.iter().enumerate() {
        let name = format!("announced-bucket-{index}");
        let created = emulator
            .ok_at(
                *host,
                *region,
                "CreateBucket",
                json!({"bucketName": name, "bundleId": "small_1_0"}),
            )
            .await["bucket"]
            .clone();

        match host {
            Some(host) => {
                let expected = announced_url(host, &name);
                assert_eq!(
                    created["url"].as_str(),
                    Some(expected.as_str()),
                    "case {case}: expected the address under this request's own endpoint, got {:?}",
                    created["url"]
                );
            }
            None => assert!(
                created["url"].is_null(),
                "case {case}: a request that names no host has no endpoint to announce, got {:?}",
                created["url"]
            ),
        }

        // Whatever the announced address is, it never names a host outside this
        // emulator.
        let url = created["url"].as_str().unwrap_or_default();
        assert!(
            !url.contains("amazonaws.com") && !url.contains("amazonlightsail.com"),
            "case {case}: the announced address must not send a client out of the emulator, \
             got {url:?}"
        );

        // The SAME stored bucket announces whatever endpoint the READ arrives at:
        // the address depends on the request, not on what the create was told.
        let elsewhere = emulator
            .ok_at(
                Some("another.endpoint:1234"),
                *region,
                "GetBuckets",
                json!({"bucketName": name}),
            )
            .await["buckets"][0]
            .clone();
        assert_eq!(
            elsewhere["url"].as_str(),
            Some(announced_url("another.endpoint:1234", &name).as_str()),
            "case {case}: expected the read's own endpoint, got {:?}",
            elsewhere["url"]
        );
        // And `url` is the ONLY member that differs between those two answers.
        let mut without_url = created.clone();
        let mut elsewhere_without_url = elsewhere.clone();
        for value in [&mut without_url, &mut elsewhere_without_url] {
            value
                .as_object_mut()
                .expect("a bucket is an object")
                .remove("url");
        }
        assert_eq!(
            without_url, elsewhere_without_url,
            "case {case}: only the announced address may depend on the request, got \
             {elsewhere_without_url:?}"
        );
    }
}

/// A listing walks every stored bucket of the Region in one stable order, a page
/// at a time, and reporting them changes nothing a later caller sees.
#[tokio::test]
async fn the_listing_pages_through_every_stored_bucket_and_consumes_nothing() {
    let emulator = Emulator::new();

    // More buckets than one page holds, created in an order that is NOT the
    // order they come back in, so the walk's order is the store's own.
    let count = BUCKET_PAGE_SIZE + 3;
    let mut created = BTreeMap::new();
    for index in (0..count).rev() {
        let name = format!("paged-bucket-{index:02}");
        let (bucket, _) = create(
            &emulator,
            None,
            json!({"bucketName": name, "bundleId": "small_1_0"}),
        )
        .await;
        created.insert(name, bucket);
    }

    let (names, page_sizes) = list(&emulator, None).await;
    let expected: Vec<String> = created.keys().cloned().collect();
    assert_eq!(
        names, expected,
        "case the walk: expected every stored bucket once, in name order, got {names:?}"
    );
    assert_eq!(
        page_sizes,
        vec![BUCKET_PAGE_SIZE, count - BUCKET_PAGE_SIZE],
        "case the walk: expected full pages until the last, got {page_sizes:?}"
    );

    // Every listed record is the record the create reported.
    let first_page = emulator
        .ok_at(Some(ENDPOINT_HOST), None, "GetBuckets", json!({}))
        .await;
    for bucket in first_page["buckets"]
        .as_array()
        .expect("the first page is an array")
    {
        let name = bucket["name"].as_str().expect("a listed bucket has a name");
        assert_eq!(
            bucket,
            &created[name],
            "case listing {name}: expected the stored record, got {bucket:?}"
        );
    }

    // Reading is a read: the same walk answers the same thing, and every bucket
    // is still there.
    let (again, again_pages) = list(&emulator, None).await;
    assert_eq!(
        (&again, &again_pages),
        (&names, &page_sizes),
        "case a second walk: two reads of the listing must agree, got {again:?} {again_pages:?}"
    );
    for name in &expected {
        let read_back = read(&emulator, None, name).await;
        assert_eq!(
            read_back, created[name],
            "case rereading {name}: the listing must consume nothing, got {read_back:?}"
        );
    }
}
