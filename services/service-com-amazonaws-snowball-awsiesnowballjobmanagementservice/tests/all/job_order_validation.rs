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
//!    transfers and the role it transfers them under;
//! 4. a `LongTermPricingId` on an order has to name a long-term pricing record this
//!    service holds, exactly as an `AddressId` and a `ClusterId` do — and one it does
//!    hold is accepted;
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
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, router, router_with_engine,
    string,
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

#[tokio::test]
async fn create_job_refuses_a_transfer_order_that_names_no_bucket_or_no_role() {
    let router = router();
    let address_id = order_address(&router).await;

    // One row per way an order can fail to name what a transfer needs, for each job type
    // that transfers S3 data: no resources at all, resources that carry no bucket list, an
    // empty bucket list, resources of another kind only, and buckets with no role.
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
        ] {
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
    // The rule is about the record, not about the id's form, so an id the store DOES hold
    // is accepted while another id in the same store is still refused. `CreateLongTermPricing`
    // is what will put an id there; until it is implemented the record is seeded through
    // the state API, which is the only thing here that does not go over HTTP.
    let (router, engine) = router_with_engine();
    let address_id = order_address(&router).await;
    let held = LONG_TERM_PRICING_IDS[2];
    let never_created = LONG_TERM_PRICING_IDS[3];
    {
        let state = service_state(&*engine).expect("the engine carries this service's store");
        state
            .lock()
            .long_term_pricing_ids
            .insert(held.to_string());
    }

    let case = "an order billed under a long-term pricing record the store holds";
    let mut body = order(&address_id, "LOCAL_USE", "EDGE");
    body["LongTermPricingId"] = json!(held);
    let created = ok(&router, "CreateJob", body, case).await;
    let job_id = string(&created, "JobId", case);
    let metadata = job_metadata(&router, &job_id, case).await;
    assert_eq!(
        metadata.get("LongTermPricingId").and_then(Value::as_str),
        Some(held),
        "case {case:?}: expected the stored order to report the long-term pricing it is \
         billed under, got {metadata}"
    );

    // A cluster order's list is read the same way: the held id is accepted.
    let cluster_case = "a cluster order billed under the record the store holds";
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
