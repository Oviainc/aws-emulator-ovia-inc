// SPDX-License-Identifier: CC0-1.0

//! What a Snow device order has to say for this service to accept it, driven end to end
//! over HTTP.
//!
//! Five rules, each asserted over a TABLE rather than over the one request an issue
//! recorded:
//!
//! 1. an order that joins no cluster describes itself — it states the job type, the
//!    device type and the shipping speed, since there is no cluster to inherit them from
//!    — and an order short of one of them is refused with the modelled
//!    `InvalidInputCombinationException`;
//! 2. the device type and the job type have to be a pair the device serves, over the
//!    WHOLE cross product of the two modelled enums;
//! 3. an order that moves data between Amazon S3 and the device names the buckets it
//!    transfers and the role it transfers them under — each read by the VALUE of the member
//!    that names it, so a bucket entry carrying no `BucketArn` names no bucket;
//! 4. a `LongTermPricingId` on an order has to name a long-term pricing contract this
//!    service holds, exactly as an `AddressId` and a `ClusterId` do — and one it does
//!    hold, bought through `CreateLongTermPricing`, is accepted;
//! 5. a cluster is ordered only in the one job type and device type its own documentation
//!    says a cluster supports.
//!
//! Rules 1, 3 and 4 hold of an order whichever operation places it, so their tables are
//! driven against `CreateJob` and, where the member exists there too, against
//! `CreateCluster`.
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `string`) is the one `job_lifecycle` already drives the service with, so every call
//! here goes through the same front door a client uses.

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    JOB_TYPE_VALUES, SNOWBALL_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, router, string,
};

/// The `JobType` values each `SnowballType` of device can be ordered for.
///
/// Source: the Snow Family device table in `CreateJob`'s own documentation
/// (`smithy_query(query="doc", shape="CreateJob")`), which says what each device type is,
/// together with the device-type pages that table links. A device is restricted only where
/// its description names a job type it cannot serve: `EDGE_S` is "Storage Optimized for
/// data transfer only", `STANDARD` is the "Original Snowball device" that predates
/// on-device compute, and Snowcone (`SNC1_HDD`, `SNC1_SSD`) is offered as an import or a
/// local compute-and-storage order and not as an export one. Every other description names
/// a capacity rather than a job type the device cannot be ordered for ("Compute
/// Optimized", "Snowblade", "Storage Optimized with EC2 Compute", "Storage Optimized
/// 210TB"), so every job type is expected to be accepted for those: the service declines a
/// combination it can show the device does not serve, never one it holds no record for.
///
/// Written out here rather than read from the crate, so the catalog the service holds is
/// asserted against this table instead of against itself.
const EXPECTED_DEVICE_JOB_TYPES: &[(&str, &[&str])] = &[
    ("STANDARD", &["IMPORT", "EXPORT"]),
    ("EDGE", &["IMPORT", "EXPORT", "LOCAL_USE"]),
    ("EDGE_S", &["IMPORT", "EXPORT"]),
    ("EDGE_C", &["IMPORT", "EXPORT", "LOCAL_USE"]),
    ("EDGE_CG", &["IMPORT", "EXPORT", "LOCAL_USE"]),
    ("SNC1_HDD", &["IMPORT", "LOCAL_USE"]),
    ("SNC1_SSD", &["IMPORT", "LOCAL_USE"]),
    ("V3_5S", &["IMPORT", "EXPORT", "LOCAL_USE"]),
    ("V3_5C", &["IMPORT", "EXPORT", "LOCAL_USE"]),
    ("RACK_5U_C", &["IMPORT", "EXPORT", "LOCAL_USE"]),
];

/// The `JobType` values whose order moves data between Amazon S3 and the device.
///
/// `CreateJobRequest$Resources`: "With IMPORT jobs, you specify the bucket or buckets that
/// your transferred data will be imported into. With EXPORT jobs, you specify the bucket
/// or buckets that your transferred data will be exported from." A local-use order moves
/// nothing to or from S3.
const TRANSFER_JOB_TYPES: &[&str] = &["IMPORT", "EXPORT"];

/// The only `JobType` a cluster can be ordered for.
///
/// `CreateClusterRequest$JobType`: "The type of job for this cluster. Currently, the only
/// job type supported for clusters is LOCAL_USE." Spelled out here rather than read from
/// the crate, so the rule the service holds is asserted against the documentation instead
/// of against itself.
const CLUSTER_JOB_TYPE: &str = "LOCAL_USE";

/// The only `SnowballType` a cluster's devices can be.
///
/// `CreateClusterRequest$SnowballType`: "For cluster jobs, Amazon Web Services Snow Family
/// currently supports only the EDGE device type."
const CLUSTER_SNOWBALL_TYPE: &str = "EDGE";

/// Well-formed `LongTermPricingId` values: 41 characters in the form the modelled pattern
/// `^LTPID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` admits, differing
/// in their digits, so the rule is asserted over the id's whole value space rather than
/// over one value.
const LONG_TERM_PRICING_IDS: &[&str] = &[
    "LTPID00000000-0000-0000-0000-000000000000",
    "LTPID11111111-2222-3333-4444-555555555555",
    "LTPIDdeadbeef-0f0f-1a2b-3c4d-5e6f70819293",
    "LTPIDffffffff-ffff-ffff-ffff-ffffffffffff",
];

/// Whether `job_type` is one the device table pairs with `snowball_type`.
fn expected_supported(snowball_type: &str, job_type: &str) -> bool {
    EXPECTED_DEVICE_JOB_TYPES
        .iter()
        .find(|(device, _)| *device == snowball_type)
        .map(|(_, job_types)| job_types.contains(&job_type))
        .unwrap_or_else(|| panic!("no expected row for the device type {snowball_type}"))
}

/// A complete standalone order for `job_type` on a `snowball_type` device: every member
/// such an order has to state, from the harness's order defaults
/// (`job_lifecycle::complete_order`), with the buckets and the role left off for an order
/// that moves no data to or from Amazon S3 and so needs neither.
fn order(address_id: &str, job_type: &str, snowball_type: &str) -> Value {
    let mut body = complete_order(&json!({
        "AddressId": address_id,
        "JobType": job_type,
        "SnowballType": snowball_type,
    }));
    if !TRANSFER_JOB_TYPES.contains(&job_type) {
        let members = body.as_object_mut().expect("the order is a JSON object");
        members.remove("RoleARN");
        members.remove("Resources");
    }
    body
}

/// An address every order in this suite ships to.
async fn order_address(router: &MultiServiceRouter) -> String {
    create_address(
        router,
        deliverable_address("Order Receiver", "11 Order Way", json!({})),
        "the orders' shipping address",
    )
    .await
}

/// How many jobs this emulator holds, read back through `ListJobs`. Every case here stores
/// far fewer than the modelled `MaxResults` ceiling of 100, so one page is the whole store.
async fn stored_jobs(router: &MultiServiceRouter, case: &str) -> usize {
    let listed = ok(router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    listed["JobListEntries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0)
}

/// The `JobMetadata` `DescribeJob` reports for `job_id`.
async fn job_metadata(router: &MultiServiceRouter, job_id: &str, case: &str) -> Value {
    let described = ok(router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    described["JobMetadata"].clone()
}

#[test]
fn the_device_table_covers_every_modelled_device_and_job_type() {
    let mut covered: Vec<&str> = EXPECTED_DEVICE_JOB_TYPES
        .iter()
        .map(|(device, _)| *device)
        .collect();
    covered.sort_unstable();
    let mut modelled: Vec<&str> = SNOWBALL_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        covered, modelled,
        "the device table must carry one row per modelled SnowballType value"
    );
    for (device, job_types) in EXPECTED_DEVICE_JOB_TYPES {
        assert!(
            !job_types.is_empty(),
            "case {device:?}: a device no job type can be ordered for could never be ordered"
        );
        for job_type in *job_types {
            assert!(
                JOB_TYPE_VALUES.contains(job_type),
                "case {device:?}: {job_type} is not a modelled JobType value, which are \
                 {JOB_TYPE_VALUES:?}"
            );
        }
    }
    for job_type in TRANSFER_JOB_TYPES {
        assert!(
            JOB_TYPE_VALUES.contains(job_type),
            "case {job_type:?}: not a modelled JobType value, which are {JOB_TYPE_VALUES:?}"
        );
    }
}

#[tokio::test]
async fn create_job_refuses_an_order_short_of_the_members_it_must_state() {
    let router = router();
    let address_id = order_address(&router).await;

    // The rows differ along WHICH of the members a standalone order states are left off:
    // each one on its own, each pair, and all three at once. The last row leaves one off
    // while carrying every optional member an order can have, so it is the missing member
    // that is refused rather than a sparse request.
    let order_members = ["JobType", "SnowballType", "ShippingOption"];
    let mut rows: Vec<(String, Value)> = Vec::new();
    for dropped in 1u8..(1 << order_members.len()) {
        let mut body = order(&address_id, "LOCAL_USE", "EDGE");
        let mut left_off: Vec<&str> = Vec::new();
        for (index, member) in order_members.iter().enumerate() {
            if dropped & (1 << index) != 0 {
                body.as_object_mut()
                    .expect("the order is a JSON object")
                    .remove(*member);
                left_off.push(member);
            }
        }
        rows.push((format!("an order with no {}", left_off.join(" and no ")), body));
    }
    let mut furnished = order(&address_id, "IMPORT", "EDGE");
    furnished["Description"] = json!("an order that says everything but its device type");
    furnished["KmsKeyARN"] =
        json!("arn:aws:kms:us-west-2:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab");
    furnished["SnowballCapacityPreference"] = json!("T100");
    furnished["Notification"] =
        json!({ "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:snowball", "NotifyAll": true });
    furnished
        .as_object_mut()
        .expect("the order is a JSON object")
        .remove("SnowballType");
    rows.push((
        "an order carrying every optional member but no SnowballType".to_string(),
        furnished,
    ));

    let before = stored_jobs(&router, "before the refused orders").await;
    for (case, body) in rows {
        refused(
            &router,
            "CreateJob",
            body,
            "InvalidInputCombinationException",
            &case,
        )
        .await;
        assert_eq!(
            stored_jobs(&router, &case).await,
            before,
            "case {case:?}: a refused CreateJob stored a job anyway"
        );
    }

    // The same order with every member stated is accepted and reads back, so it is the
    // missing member that the refusals above turn on.
    let case = "the same order with every member it must state";
    let created = ok(
        &router,
        "CreateJob",
        order(&address_id, "LOCAL_USE", "EDGE"),
        case,
    )
    .await;
    let job_id = string(&created, "JobId", case);
    let metadata = job_metadata(&router, &job_id, case).await;
    assert_eq!(
        metadata.get("JobType").and_then(Value::as_str),
        Some("LOCAL_USE"),
        "case {case:?}: expected the stored order to report its job type, got {metadata}"
    );
    assert_eq!(
        metadata.get("SnowballType").and_then(Value::as_str),
        Some("EDGE"),
        "case {case:?}: expected the stored order to report its device type, got {metadata}"
    );
    assert_eq!(
        metadata["ShippingDetails"].get("ShippingOption"),
        Some(&json!("SECOND_DAY")),
        "case {case:?}: expected the stored order to report its shipping speed, got {metadata}"
    );
}

#[tokio::test]
async fn create_job_accepts_exactly_the_job_types_the_device_serves() {
    let router = router();
    let address_id = order_address(&router).await;

    // Every combination of the two modelled enums is a row, so the rule is asserted over
    // the whole cross product rather than over the pair one request carried.
    for snowball_type in SNOWBALL_TYPE_VALUES {
        for job_type in JOB_TYPE_VALUES {
            let case = format!("a {job_type} order on a {snowball_type} device");
            let body = order(&address_id, job_type, snowball_type);
            if expected_supported(snowball_type, job_type) {
                let created = ok(&router, "CreateJob", body, &case).await;
                let job_id = string(&created, "JobId", &case);
                // An accepted order is persisted as what it ordered, read back through
                // the reader rather than out of the create's answer.
                let metadata = job_metadata(&router, &job_id, &case).await;
                assert_eq!(
                    metadata.get("JobType").and_then(Value::as_str),
                    Some(*job_type),
                    "case {case:?}: expected the stored order to be a {job_type} job, \
                     got {metadata}"
                );
                assert_eq!(
                    metadata.get("SnowballType").and_then(Value::as_str),
                    Some(*snowball_type),
                    "case {case:?}: expected the stored order to name a {snowball_type} \
                     device, got {metadata}"
                );
            } else {
                let before = stored_jobs(&router, &case).await;
                refused(
                    &router,
                    "CreateJob",
                    body,
                    "InvalidInputCombinationException",
                    &case,
                )
                .await;
                assert_eq!(
                    stored_jobs(&router, &case).await,
                    before,
                    "case {case:?}: a refused CreateJob stored a job anyway"
                );
            }
        }
    }
}

/// `S3Resources` lists whose entries name NO Amazon S3 bucket at all.
///
/// Source: `smithy_query(query="shape", shape="S3Resource")` — the entry has NO required
/// member, so an entry may leave `BucketArn` out altogether, and its other two members
/// (`KeyRange`, `TargetOnDeviceServices`) name no bucket. Each of these is therefore a
/// well-formed bucket list that names nothing, and the rows differ along entry count and
/// along which member of the entry is carried.
fn buckets_naming_no_bucket() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one bucket entry carrying no BucketArn",
            json!([{}]),
        ),
        (
            "one bucket entry carrying only a KeyRange",
            json!([{ "KeyRange": { "BeginMarker": "a", "EndMarker": "m" } }]),
        ),
        (
            "one bucket entry carrying only its on-device services",
            json!([{
                "TargetOnDeviceServices": [
                    { "ServiceName": "NFS_ON_DEVICE_SERVICE", "TransferOption": "IMPORT" },
                ],
            }]),
        ),
        (
            "three bucket entries that each name nothing",
            json!([
                {},
                { "KeyRange": { "BeginMarker": "a" } },
                {
                    "TargetOnDeviceServices": [
                        { "ServiceName": "S3_ON_DEVICE_SERVICE", "TransferOption": "EXPORT" },
                    ],
                },
            ]),
        ),
    ]
}

/// `S3Resources` lists that DO name at least one bucket, each paired with the buckets it
/// names: one entry, an entry that names nothing beside one that does (in either order),
/// and several named buckets among entries that name none.
fn buckets_naming_a_bucket() -> Vec<(&'static str, Value, Vec<&'static str>)> {
    vec![
        (
            "one named bucket",
            json!([{ "BucketArn": "arn:aws:s3:::transfer-source" }]),
            vec!["arn:aws:s3:::transfer-source"],
        ),
        (
            "a named bucket behind an entry that names none",
            json!([{}, { "BucketArn": "arn:aws:s3:::transfer-second" }]),
            vec!["arn:aws:s3:::transfer-second"],
        ),
        (
            "a named bucket ahead of an entry that names none",
            json!([
                { "BucketArn": "arn:aws:s3:::transfer-third" },
                { "KeyRange": { "BeginMarker": "a" } },
            ]),
            vec!["arn:aws:s3:::transfer-third"],
        ),
        (
            "two named buckets among entries that name none",
            json!([
                {},
                { "BucketArn": "arn:aws:s3:::transfer-fourth" },
                { "KeyRange": { "BeginMarker": "a" } },
                {
                    "BucketArn": "arn:aws:s3:::transfer-fifth",
                    "KeyRange": { "BeginMarker": "a", "EndMarker": "m" },
                },
            ]),
            vec![
                "arn:aws:s3:::transfer-fourth",
                "arn:aws:s3:::transfer-fifth",
            ],
        ),
    ]
}

/// The bucket ARNs a stored order reports, in the order they are reported, reading an entry
/// that names no bucket as naming none.
fn reported_buckets(metadata: &Value) -> Vec<String> {
    metadata["Resources"]["S3Resources"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.get("BucketArn").and_then(Value::as_str))
        .filter(|bucket_arn| !bucket_arn.is_empty())
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn create_job_reads_a_named_bucket_by_its_value_not_by_the_entry_carrying_it() {
    let router = router();
    let address_id = order_address(&router).await;

    // A transfer order names the buckets it moves by the VALUE of each entry's `BucketArn`,
    // so a bucket list whose entries carry no value for it names nothing and is refused
    // exactly as an empty list is — however many such entries it carries — while a list
    // carrying one value is accepted however many entries name nothing beside it.
    for job_type in TRANSFER_JOB_TYPES {
        let before = stored_jobs(&router, "before the refused transfer orders").await;
        for (what, s3_resources) in buckets_naming_no_bucket() {
            let case = format!("a {job_type} order with {what}");
            let mut body = order(&address_id, job_type, "EDGE");
            body["Resources"] = json!({ "S3Resources": s3_resources });
            refused(
                &router,
                "CreateJob",
                body,
                "InvalidInputCombinationException",
                &case,
            )
            .await;
            assert_eq!(
                stored_jobs(&router, &case).await,
                before,
                "case {case:?}: a refused CreateJob stored a job anyway"
            );
        }

        for (what, s3_resources, named) in buckets_naming_a_bucket() {
            let case = format!("a {job_type} order with {what}");
            let mut body = order(&address_id, job_type, "EDGE");
            body["Resources"] = json!({ "S3Resources": s3_resources });
            let created = ok(&router, "CreateJob", body, &case).await;
            let job_id = string(&created, "JobId", &case);
            let metadata = job_metadata(&router, &job_id, &case).await;
            let reported = reported_buckets(&metadata);
            assert_eq!(
                reported, named,
                "case {case:?}: expected the stored order to name {named:?}, got {reported:?} \
                 in {metadata}"
            );
        }
    }

    // The one bucket value the modelled member does NOT admit is the empty string: the
    // member's pattern (`^arn:aws.*:*$`, `smithy_query(query="shape", shape="S3Resource")`)
    // matches no empty value, so such an entry is turned away by the request walk before any
    // order is read — a different refusal from the one above, asserted here so the rule's
    // boundary is pinned rather than assumed.
    for job_type in TRANSFER_JOB_TYPES {
        let case = format!("a {job_type} order with a bucket entry whose BucketArn is empty");
        let mut body = order(&address_id, job_type, "EDGE");
        body["Resources"] = json!({ "S3Resources": [{ "BucketArn": "" }] });
        refused(&router, "CreateJob", body, "ValidationException", &case).await;
    }

    // The rule belongs to an order that transfers S3 data: a local-use order moves nothing
    // to or from Amazon S3, so the same bucket list that names nothing is no reason to
    // refuse it, and it is stored as submitted.
    for (what, s3_resources) in buckets_naming_no_bucket() {
        let case = format!("a LOCAL_USE order with {what}");
        let mut body = order(&address_id, "LOCAL_USE", "EDGE");
        body["Resources"] = json!({ "S3Resources": s3_resources });
        let created = ok(&router, "CreateJob", body, &case).await;
        let job_id = string(&created, "JobId", &case);
        let metadata = job_metadata(&router, &job_id, &case).await;
        let reported = reported_buckets(&metadata);
        assert!(
            reported.is_empty(),
            "case {case:?}: expected the stored order to name no bucket, got {reported:?} \
             in {metadata}"
        );
    }
}

#[tokio::test]
async fn create_job_refuses_a_transfer_order_that_names_no_bucket_or_no_role() {
    let router = router();
    let address_id = order_address(&router).await;

    // One row per way an order can fail to name what a transfer needs, for each job type
    // that transfers S3 data: no resources at all, resources that carry no bucket list, an
    // empty bucket list, resources of another kind only, a bucket list whose entries name
    // no bucket, and buckets with no role.
    for job_type in TRANSFER_JOB_TYPES {
        let mut rows: Vec<(String, Value)> = Vec::new();
        for (missing, resources) in [
            ("no Resources member", None),
            ("Resources naming no bucket list", Some(json!({}))),
            (
                "an empty bucket list",
                Some(json!({ "S3Resources": [] })),
            ),
            (
                "resources of another kind only",
                Some(json!({ "LambdaResources": [] })),
            ),
        ]
        .into_iter()
        .chain(
            buckets_naming_no_bucket()
                .into_iter()
                .map(|(what, s3_resources)| (what, Some(json!({ "S3Resources": s3_resources })))),
        ) {
            let mut body = order(&address_id, job_type, "EDGE");
            match resources {
                Some(resources) => body["Resources"] = resources,
                None => {
                    body.as_object_mut()
                        .expect("the order is a JSON object")
                        .remove("Resources");
                }
            }
            rows.push((format!("a {job_type} order with {missing}"), body));
        }
        let mut no_role = order(&address_id, job_type, "EDGE");
        no_role
            .as_object_mut()
            .expect("the order is a JSON object")
            .remove("RoleARN");
        rows.push((format!("a {job_type} order with no RoleARN"), no_role));

        let before = stored_jobs(&router, "before the refused transfer orders").await;
        for (case, body) in rows {
            refused(
                &router,
                "CreateJob",
                body,
                "InvalidInputCombinationException",
                &case,
            )
            .await;
            assert_eq!(
                stored_jobs(&router, &case).await,
                before,
                "case {case:?}: a refused CreateJob stored a job anyway"
            );
        }
    }

    // A local-use order transfers nothing to or from Amazon S3, so it is accepted with
    // neither buckets nor a role — and reads back carrying neither.
    let case = "a local-use order naming neither buckets nor a role";
    let created = ok(
        &router,
        "CreateJob",
        order(&address_id, "LOCAL_USE", "SNC1_SSD"),
        case,
    )
    .await;
    let job_id = string(&created, "JobId", case);
    let metadata = job_metadata(&router, &job_id, case).await;
    for absent in ["Resources", "RoleARN"] {
        let got = metadata.get(absent).unwrap_or(&Value::Null);
        assert!(
            got.is_null(),
            "case {case:?}: expected no {absent} on an order that named none, got {got}"
        );
    }
}

#[tokio::test]
async fn create_job_refuses_a_long_term_pricing_id_no_record_answers_to() {
    let router = router();
    let address_id = order_address(&router).await;

    // A long-term pricing record is created by `CreateLongTermPricing`, so a store nothing
    // has created one in holds none and every well-formed id names nothing — the same
    // refusal an `AddressId` that was never created earns. The rows differ along the id's
    // value and along which create names it: a standalone order, and a node of a cluster,
    // whose own long-term pricing a node request supplies.
    let cluster_case = "the cluster the node rows join";
    let cluster = ok(
        &router,
        "CreateCluster",
        order(&address_id, "LOCAL_USE", "EDGE"),
        cluster_case,
    )
    .await;
    let cluster_id = string(&cluster, "ClusterId", cluster_case);

    let before = stored_jobs(&router, "before the refused orders").await;
    for long_term_pricing_id in LONG_TERM_PRICING_IDS {
        assert_eq!(
            long_term_pricing_id.len(),
            41,
            "{long_term_pricing_id} is not a 41-character LongTermPricingId"
        );
        let mut standalone = order(&address_id, "LOCAL_USE", "EDGE");
        standalone["LongTermPricingId"] = json!(long_term_pricing_id);
        let node = json!({
            "ClusterId": cluster_id,
            "LongTermPricingId": long_term_pricing_id,
        });
        for (what, body) in [
            ("a standalone order", standalone),
            ("a node of a cluster", node),
        ] {
            let case = format!("{what} billed under {long_term_pricing_id}");
            refused(
                &router,
                "CreateJob",
                body,
                "InvalidResourceException",
                &case,
            )
            .await;
            assert_eq!(
                stored_jobs(&router, &case).await,
                before,
                "case {case:?}: a refused CreateJob stored a job anyway"
            );
        }

        // A cluster order carries a LIST of them, and the same rule holds of each entry.
        for (what, ids) in [
            ("one id", vec![*long_term_pricing_id]),
            (
                "the id behind another",
                vec![LONG_TERM_PRICING_IDS[0], long_term_pricing_id],
            ),
        ] {
            let case = format!("a cluster order billed under {what}: {ids:?}");
            let mut body = order(&address_id, "LOCAL_USE", "EDGE");
            body["LongTermPricingIds"] = json!(ids);
            refused(
                &router,
                "CreateCluster",
                body,
                "InvalidResourceException",
                &case,
            )
            .await;
        }
    }

    // The same orders without a long-term pricing id are accepted, so it is the id naming
    // no record that the refusals turn on.
    let case = "an order naming no long-term pricing";
    let created = ok(
        &router,
        "CreateJob",
        order(&address_id, "LOCAL_USE", "EDGE"),
        case,
    )
    .await;
    let job_id = string(&created, "JobId", case);
    let metadata = job_metadata(&router, &job_id, case).await;
    let reported = metadata.get("LongTermPricingId").unwrap_or(&Value::Null);
    assert!(
        reported.is_null(),
        "case {case:?}: expected no LongTermPricingId on an order that named none, \
         got {reported}"
    );
}

#[tokio::test]
async fn create_job_accepts_a_long_term_pricing_id_the_store_holds() {
    // The rule is about the contract, not about the id's form, so an id the store DOES hold
    // is accepted while another id in the same store is still refused. `CreateLongTermPricing`
    // is what puts a contract there, so the held id is the one it minted — bought over the
    // wire like every other call here.
    let router = router();
    let address_id = order_address(&router).await;
    let bought_case = "a long-term pricing contract bought for the order";
    let bought = ok(
        &router,
        "CreateLongTermPricing",
        json!({ "LongTermPricingType": "OneYear", "SnowballType": "EDGE" }),
        bought_case,
    )
    .await;
    let held = string(&bought, "LongTermPricingId", bought_case);
    let never_created = LONG_TERM_PRICING_IDS[3];

    let case = "an order billed under a long-term pricing contract the store holds";
    let mut body = order(&address_id, "LOCAL_USE", "EDGE");
    body["LongTermPricingId"] = json!(held);
    let created = ok(&router, "CreateJob", body, case).await;
    let job_id = string(&created, "JobId", case);
    let metadata = job_metadata(&router, &job_id, case).await;
    assert_eq!(
        metadata.get("LongTermPricingId").and_then(Value::as_str),
        Some(held.as_str()),
        "case {case:?}: expected the stored order to report the long-term pricing it is \
         billed under, got {metadata}"
    );

    // A cluster order's list is read the same way: the held id is accepted.
    let cluster_case = "a cluster order billed under the contract the store holds";
    let mut cluster = order(&address_id, CLUSTER_JOB_TYPE, CLUSTER_SNOWBALL_TYPE);
    cluster["LongTermPricingIds"] = json!([held]);
    let cluster_created = ok(&router, "CreateCluster", cluster, cluster_case).await;
    string(&cluster_created, "ClusterId", cluster_case);

    // And an id the same store does not hold is still refused, so it is the record that
    // decides and not the shape of the request.
    let refused_case = "an order billed under an id the same store does not hold";
    let mut body = order(&address_id, "LOCAL_USE", "EDGE");
    body["LongTermPricingId"] = json!(never_created);
    refused(
        &router,
        "CreateJob",
        body,
        "InvalidResourceException",
        refused_case,
    )
    .await;
}

#[tokio::test]
async fn create_cluster_accepts_only_the_job_and_device_type_a_cluster_supports() {
    let router = router();
    let address_id = order_address(&router).await;

    // A cluster is not ordered out of the whole device catalog: the documented pair is
    // the only one it can be ordered in, so the rule is asserted over the same whole
    // cross product of the two modelled enums — one accepted row, twenty-nine refused.
    assert!(
        expected_supported(CLUSTER_SNOWBALL_TYPE, CLUSTER_JOB_TYPE),
        "the pair a cluster supports must be one the device table serves for a job too"
    );
    for snowball_type in SNOWBALL_TYPE_VALUES {
        for job_type in JOB_TYPE_VALUES {
            let case = format!("a {job_type} cluster on {snowball_type} devices");
            let body = order(&address_id, job_type, snowball_type);
            if *job_type == CLUSTER_JOB_TYPE && *snowball_type == CLUSTER_SNOWBALL_TYPE {
                let created = ok(&router, "CreateCluster", body, &case).await;
                let cluster_id = string(&created, "ClusterId", &case);
                let described = ok(
                    &router,
                    "DescribeCluster",
                    json!({ "ClusterId": cluster_id }),
                    &case,
                )
                .await;
                let metadata = &described["ClusterMetadata"];
                assert_eq!(
                    metadata.get("JobType").and_then(Value::as_str),
                    Some(*job_type),
                    "case {case:?}: expected the stored cluster to be a {job_type} cluster, \
                     got {metadata}"
                );
                assert_eq!(
                    metadata.get("SnowballType").and_then(Value::as_str),
                    Some(*snowball_type),
                    "case {case:?}: expected the stored cluster to name {snowball_type} \
                     devices, got {metadata}"
                );
                // A cluster ordered for local use transfers nothing to or from Amazon S3,
                // so it is accepted naming neither buckets nor a role.
                for absent in ["Resources", "RoleARN"] {
                    let got = metadata.get(absent).unwrap_or(&Value::Null);
                    assert!(
                        got.is_null(),
                        "case {case:?}: expected no {absent} on a cluster that named none, \
                         got {got}"
                    );
                }
            } else {
                refused(
                    &router,
                    "CreateCluster",
                    body,
                    "InvalidInputCombinationException",
                    &case,
                )
                .await;
            }
        }
    }
}
