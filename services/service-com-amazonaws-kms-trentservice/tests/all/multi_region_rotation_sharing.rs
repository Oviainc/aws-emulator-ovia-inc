// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for automatic key rotation as a property the keys of a multi-Region
//! set SHARE, driven over the wire through the shared [`super::harness`].
//!
//! Two halves of one rule:
//!
//! 1. The property is set on the set's PRIMARY key. A request aimed at a replica is
//!    refused with `UnsupportedOperationException`, and the refusal NAMES the replica
//!    key's ARN and the operation it is not a valid resource for — which is the only
//!    thing that tells the caller which key to call instead.
//! 2. KMS "automatically synchronizes these shared properties among related
//!    multi-Region keys" (`smithy_query(query="doc",shape="ReplicateKey")`), so the
//!    schedule one call configures on the primary is what EVERY member's own endpoint
//!    reports — without the caller ever configuring it there — and clearing it clears
//!    it everywhere.
//!
//! The rows span the operations a replica key is not a valid resource for, the Regions
//! holding a replica of one set, and the rotation periods inside the modelled bound of
//! `RotationPeriodInDaysType` (90..2560,
//! `smithy_query(query="operation",operation="EnableKeyRotation")`), including both
//! published bounds and a request that names no period at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, describe_in, error_code, error_message, ok_in, replicate_into, router, text, REGION,
};

/// The Regions holding a replica key of the set under test.
const REPLICA_REGIONS: [&str; 2] = ["us-west-2", "eu-west-1"];

/// A multi-Region key set with its primary key in [`REGION`] and a replica in each of
/// [`REPLICA_REGIONS`], reported by the key id every member of the set shares.
async fn multi_region_set(router: &MultiServiceRouter, case: &str) -> String {
    let metadata = ok_in(
        router,
        REGION,
        "CreateKey",
        &json!({"MultiRegion": true, "Description": "shared rotation"}),
    )
    .await["KeyMetadata"]
        .clone();
    let key_id = text(case, &metadata, "KeyId");
    replicate_into(router, case, &key_id, &REPLICA_REGIONS).await;
    key_id
}

/// `GetKeyRotationStatus` for `key_id` at `region`'s endpoint.
async fn rotation_status_in(
    router: &MultiServiceRouter,
    region: &str,
    key_id: &str,
) -> Value {
    ok_in(
        router,
        region,
        "GetKeyRotationStatus",
        &json!({"KeyId": key_id}),
    )
    .await
}

/// A rotation request aimed at a replica key is refused with the error code AND the
/// sentence that names the replica and the operation.
#[tokio::test]
async fn a_replica_key_is_not_a_valid_resource_for_the_rotation_operations() {
    let router = router();
    let key_id = multi_region_set(&router, "replica refusal").await;

    for replica_region in REPLICA_REGIONS {
        let replica_arn = text(
            replica_region,
            &describe_in(&router, replica_region, &key_id).await,
            "Arn",
        );
        // Rows: every operation that configures or performs a rotation of the set's key
        // material, each of which belongs on the primary key.
        for operation in [
            "EnableKeyRotation",
            "DisableKeyRotation",
            "RotateKeyOnDemand",
        ] {
            let case = format!("{operation} at {replica_region}");
            let (status, payload) =
                call(&router, replica_region, operation, &json!({"KeyId": key_id})).await;
            let code = error_code(&payload);
            let message = error_message(&payload);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, "UnsupportedOperationException",
                "case {case:?}: a replica key is not a valid resource for this \
                 operation, got {code:?} in {payload}"
            );
            assert_eq!(
                message,
                format!(
                    "{replica_arn} is a multi-region replica key which is not valid for \
                     {operation}."
                ),
                "case {case:?}: expected the refusal to name the replica key and the \
                 operation, got {message:?}"
            );
        }
    }

    // Nothing was configured anywhere: a refused request persists nothing.
    for region in [REGION, REPLICA_REGIONS[0], REPLICA_REGIONS[1]] {
        let report = rotation_status_in(&router, region, &key_id).await;
        assert_eq!(
            report["KeyRotationEnabled"],
            json!(false),
            "case {region}: a refused rotation request configures nothing, got {report}"
        );
    }
}

/// The schedule configured on the primary key is the schedule every member's own
/// endpoint reports, and clearing it clears it for the whole set.
#[tokio::test]
async fn the_rotation_schedule_is_shared_by_every_key_of_the_set() {
    let router = router();
    let key_id = multi_region_set(&router, "shared schedule").await;

    // Rows: a rotation period the request names (or none), and the period the set must
    // then report — both published bounds of the modelled range, one inside it, and the
    // documented default of a request that names no period.
    let periods: [(Option<i64>, i64); 4] = [(None, 365), (Some(90), 90), (Some(180), 180), (Some(2_560), 2_560)];
    for (named, expected) in periods {
        let mut request = json!({"KeyId": key_id});
        if let Some(days) = named {
            request["RotationPeriodInDays"] = json!(days);
        }
        let case = format!("a period of {named:?} set on the primary key");
        ok_in(&router, REGION, "EnableKeyRotation", &request).await;

        // EVERY member reports the set's schedule, each naming its own key — including
        // the replicas, whose endpoints were never asked to configure anything.
        for region in [REGION, REPLICA_REGIONS[0], REPLICA_REGIONS[1]] {
            let arn = text(region, &describe_in(&router, region, &key_id).await, "Arn");
            let report = rotation_status_in(&router, region, &key_id).await;
            assert_eq!(
                report["KeyRotationEnabled"],
                json!(true),
                "case {case:?} at {region}: the set's rotation is enabled, got {report}"
            );
            assert_eq!(
                report["RotationPeriodInDays"],
                json!(expected),
                "case {case:?} at {region}: expected the set's period {expected}, got \
                 {report}"
            );
            assert_eq!(
                report["KeyId"],
                json!(arn),
                "case {case:?} at {region}: the report names the key of the Region that \
                 served it, got {report}"
            );
        }
    }

    // Disabling it on the primary key clears it for the set, at every endpoint.
    ok_in(
        &router,
        REGION,
        "DisableKeyRotation",
        &json!({"KeyId": key_id}),
    )
    .await;
    for region in [REGION, REPLICA_REGIONS[0], REPLICA_REGIONS[1]] {
        let report = rotation_status_in(&router, region, &key_id).await;
        assert_eq!(
            report["KeyRotationEnabled"],
            json!(false),
            "case disabled at {region}: the set's rotation is off everywhere, got {report}"
        );
        assert!(
            report["RotationPeriodInDays"].is_null(),
            "case disabled at {region}: a set with no schedule reports no period, got \
             {report}"
        );
    }
}
