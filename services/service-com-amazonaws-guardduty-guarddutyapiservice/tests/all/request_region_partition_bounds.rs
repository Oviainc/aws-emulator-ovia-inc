// SPDX-License-Identifier: CC0-1.0

//! How many Region partitions this service's stores will take.
//!
//! The rule under test, asserted over a SET of Regions rather than one:
//!
//! > A resource partitioned by the Region read from the request's SigV4 credential
//! > scope lets the caller choose the partition KEY as well as what goes in it, so a
//! > per-Region quota bounds one partition and not the number of partitions: a create
//! > that would partition the store into one Region more than that store's Region
//! > bound (`MAX_MALWARE_PROTECTION_PLAN_REGIONS` for plans,
//! > `MAX_DETECTOR_REGIONS` for detectors) is refused — HTTP 400 under the
//! > `InvalidInputException` class, with the rule stated and the caller's Region not
//! > quoted back — while a Region the store already holds a resource for is never
//! > refused by that bound and stays governed by its own per-Region rule.
//!
//! Together the two bounds make each store finite whatever a caller sends: the plan
//! map with its plan idempotency index (which is bounded with eviction by
//! `MAX_CLIENT_TOKENS` in its own right — `tests/all/client_token_store.rs`), and the
//! detector map with everything a detector owns — its members, filters, IPSets and
//! findings — plus the detector idempotency index, bounded the same way.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, new_router, Router};

/// The bound the handler enforces, read from the service state rather than restated
/// here, so these tests track the constant the handler uses.
const MAX_PLAN_REGIONS: usize =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::MAX_MALWARE_PROTECTION_PLAN_REGIONS;

/// The detector store's own Region bound, read from the service state for the same
/// reason.
const MAX_DETECTOR_REGIONS: usize =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::MAX_DETECTOR_REGIONS;

/// An IAM role ARN of the form `CreateMalwareProtectionPlan`'s `Role` takes.
const ROLE: &str = "arn:aws:iam::000000000000:role/GuardDutyMalwareProtectionRole";

/// Region names the caller invents. The Region comes from the request, so these need
/// not be Regions AWS runs — which is what the bound is for.
fn invented_regions(count: usize) -> Vec<String> {
    (0..count).map(|n| format!("zz-plan-{n}")).collect()
}

async fn create_plan_in(router: &Router, region: &str, bucket: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/malware-protection-plan",
        Some(json!({
            "role": ROLE,
            "protectedResource": {"s3Bucket": {"bucketName": bucket}},
        })),
    )
    .await
}

#[tokio::test]
async fn a_plan_create_past_the_region_bound_is_refused() {
    let router = new_router();

    let regions = invented_regions(MAX_PLAN_REGIONS);
    for (n, region) in regions.iter().enumerate() {
        let (status, created) =
            create_plan_in(&router, region, &format!("bucket-in-{region}")).await;
        assert_eq!(
            status, 200,
            "the plan in Region {n}: got {status} {created:?}"
        );
        assert!(
            created["malwareProtectionPlanId"].as_str().is_some(),
            "the plan in Region {n}: expected a malwareProtectionPlanId, got {created:?}"
        );
    }

    // One Region more is refused: the store will not partition again.
    let one_too_many = "zz-plan-one-region-too-many";
    let (status, refused) =
        create_plan_in(&router, one_too_many, "bucket-in-one-region-too-many").await;
    assert_eq!(
        status, 400,
        "a create in one Region too many: expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "a create in one Region too many: expected the InvalidInputException class, got {refused:?}"
    );
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains(one_too_many),
        "a refusal must not quote the caller's Region back, got {message:?}"
    );
    assert_eq!(
        refused["malwareProtectionPlanId"],
        Value::Null,
        "a refused create must mint no plan, got {refused:?}"
    );

    // The refusal evicted nothing: every Region the store already held still
    // reports its plan, and still accepts another one up to its own quota.
    for (n, region) in regions.iter().enumerate() {
        let (status, created) =
            create_plan_in(&router, region, &format!("second-bucket-in-{region}")).await;
        assert_eq!(
            status, 200,
            "a second plan in Region {n}, which the store already holds: got {status} {created:?}"
        );
    }
}

#[tokio::test]
async fn the_region_bound_leaves_a_region_already_held_alone() {
    let router = new_router();

    // A store well short of the bound refuses nothing: each fresh Region is a new
    // partition, and each one accepts its plans.
    for (n, region) in invented_regions(3).iter().enumerate() {
        for bucket in [format!("first-in-{region}"), format!("second-in-{region}")] {
            let (status, created) = create_plan_in(&router, region, &bucket).await;
            assert_eq!(
                status, 200,
                "the plan {bucket:?} in Region {n}: got {status} {created:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The detector store takes the same bound: one detector per Region, and a bounded
// number of Regions.
// ---------------------------------------------------------------------------

async fn create_detector_in(router: &Router, region: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await
}

#[tokio::test]
async fn a_detector_create_past_the_region_bound_is_refused() {
    let router = new_router();

    // Region names the caller invents: the Region comes from the request, so these
    // need not be Regions AWS runs — which is what the bound is for.
    let regions: Vec<String> = (0..MAX_DETECTOR_REGIONS)
        .map(|n| format!("zz-detector-{n}"))
        .collect();
    let mut ids = Vec::new();
    for (n, region) in regions.iter().enumerate() {
        let (status, created) = create_detector_in(&router, region).await;
        assert_eq!(
            status, 200,
            "the detector in Region {n}: got {status} {created:?}"
        );
        ids.push(
            created["detectorId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("the detector in Region {n}: expected a detectorId, got {created:?}")
                })
                .to_string(),
        );
    }

    // One Region more is refused: the store will not partition again.
    let one_too_many = "zz-detector-one-region-too-many";
    let (status, refused) = create_detector_in(&router, one_too_many).await;
    assert_eq!(
        status, 400,
        "a create in one Region too many: expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "a create in one Region too many: expected the InvalidInputException class, got {refused:?}"
    );
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains(one_too_many),
        "a refusal must not quote the caller's Region back, got {message:?}"
    );
    assert_eq!(
        refused["detectorId"],
        Value::Null,
        "a refused create must mint no detector, got {refused:?}"
    );

    // And it is not served afterwards either: the refused Region holds nothing.
    let (_, listed) = call_in(&router, one_too_many, "GET", "/detector", None).await;
    assert_eq!(
        listed["detectorIds"],
        json!([]),
        "the refused Region must hold no detector, got {listed:?}"
    );

    // The refusal evicted nothing: every Region the store already held still lists
    // the detector it minted, and still refuses a SECOND detector by the
    // one-per-Region rule rather than by the Region bound.
    for (n, region) in regions.iter().enumerate() {
        let (status, listed) = call_in(&router, region, "GET", "/detector", None).await;
        assert_eq!(
            status, 200,
            "list-detectors in Region {n}: got {status} {listed:?}"
        );
        assert_eq!(
            listed["detectorIds"],
            json!([ids[n].clone()]),
            "Region {n} must still hold the detector it minted, got {listed:?}"
        );
        let (status, refused) = create_detector_in(&router, region).await;
        assert_eq!(
            status, 400,
            "a second detector in Region {n}: expected 400, got {status} {refused:?}"
        );
    }
}

#[tokio::test]
async fn the_detector_region_bound_leaves_a_region_already_held_alone() {
    let router = new_router();

    // A store well short of the bound refuses nothing: each fresh Region takes its
    // own detector.
    for (n, region) in ["zz-detector-a", "zz-detector-b", "zz-detector-c"]
        .iter()
        .enumerate()
    {
        let (status, created) = create_detector_in(&router, region).await;
        assert_eq!(
            status, 200,
            "the detector in Region {n}: got {status} {created:?}"
        );
    }
}
