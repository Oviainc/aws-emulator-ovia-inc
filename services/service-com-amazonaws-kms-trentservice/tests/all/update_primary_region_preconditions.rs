// SPDX-License-Identifier: CC0-1.0

//! The preconditions `UpdatePrimaryRegion` judges before it moves a multi-Region key
//! set's primary-key designation, driven end-to-end over the wire through the shared
//! [`super::harness`].
//!
//! Two rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! 1. `KeyId` "identifies the current primary key. When the operation completes, this
//!    KMS key will be a replica key"
//!    (`smithy_query(query="doc",shape="UpdatePrimaryRegionRequest",member="KeyId")`), so
//!    an identifier that resolves to a REPLICA key of the set names no key this call may
//!    be made on: it is refused `KMSInvalidStateException`, naming the key it resolved to
//!    by its ARN, and the set's roles are untouched. The rows differ in the Region the
//!    replica lives in, in the identifier form the request names it by, and in which
//!    Region it asks to promote.
//! 2. "This operation changes the replica key in the specified Region to a primary key
//!    and changes the former primary key to a replica key" (the same doc), so the key
//!    state precondition of a management operation holds for BOTH keys the move changes:
//!    a disabled replica is refused `DisabledException` naming that replica, a replica
//!    awaiting deletion is refused `KMSInvalidStateException`, both sides' roles stay as
//!    they were, and the identical call succeeds once the replica is enabled again. The
//!    rows differ in the key state the replica is in and in which side of the move is in
//!    it.
//!
//! What a promotion that IS served does, and the Regions it can be asked for, are
//! [`super::multi_region_primary`]'s rule; this suite is only about what is refused
//! before anything is written.

use serde_json::json;

use super::harness::{
    call, create_key, describe_in, error_code, error_message, ok_in, replicate_into, router,
    settle_key_state, text, topology_at, REGION,
};

/// The Regions the rows replicate into. They span the Region ids `RegionType`'s modelled
/// `^([a-z]+-){2,3}\d+$` pattern admits
/// (`smithy_query(query="operation",operation="UpdatePrimaryRegion")`): two segments
/// before the digit, three of them, and a longer name.
const REPLICA_REGIONS: [&str; 3] = ["eu-west-2", "ap-southeast-2", "us-gov-west-1"];

/// The key ARN the endpoint of `region` serves the key `key_id` under.
fn arn_in(region: &str, key_id: &str) -> String {
    service_com_amazonaws_kms_trentservice::state::key_arn(region, key_id)
}

/// Rule 1: a promotion asked for with a REPLICA key's identifier is refused as a key
/// state the operation is not compatible with, and persists nothing.
#[tokio::test]
async fn a_promotion_asked_for_with_a_replica_key_is_refused() {
    let router = router();

    let key_id = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    replicate_into(&router, "setup", &key_id, &REPLICA_REGIONS).await;

    // (the Region whose endpoint the call is made at, the identifier form it names the
    // key by, the Region it asks to promote). The rows cover every replica of the set,
    // both identifier forms `KeyId` accepts, and asking for the replica's own Region, for
    // the Region holding the primary key, and for another replica's Region.
    let mut rows: Vec<(&str, String, &str)> = Vec::new();
    for replica_region in REPLICA_REGIONS {
        let other_replica = REPLICA_REGIONS
            .iter()
            .find(|region| **region != replica_region)
            .copied()
            .unwrap_or(REGION);
        for target in [replica_region, REGION, other_replica] {
            rows.push((replica_region, key_id.clone(), target));
            rows.push((replica_region, arn_in(replica_region, &key_id), target));
        }
    }

    for (endpoint, identifier, target) in rows {
        let case = format!("the {endpoint} replica, named by {identifier}, promoting {target}");
        let (status, payload) = call(
            &router,
            endpoint,
            "UpdatePrimaryRegion",
            &json!({"KeyId": identifier, "PrimaryRegion": target}),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "KMSInvalidStateException"),
            "case {case}: a replica key is not the current primary key, so the call must be \
             refused the modelled key state fault, got HTTP {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!(
                "{} is not a multi-Region primary key.",
                arn_in(endpoint, &key_id)
            ),
            "case {case}: the refusal must name the key the request resolved to, got \
             {payload}"
        );

        // Nothing moved: every endpoint of the set still reports the designation where
        // it was, with the same membership.
        for region in REPLICA_REGIONS.iter().chain(std::iter::once(&REGION)) {
            let (key_type, primary_region, _) = topology_at(&router, &case, region, &key_id).await;
            let expected_type = if *region == REGION { "PRIMARY" } else { "REPLICA" };
            assert_eq!(
                (key_type.as_str(), primary_region.as_str()),
                (expected_type, REGION),
                "case {case}: a refused promotion must leave the {region} key's role as it \
                 was"
            );
        }
    }
}

/// Rule 2: the key state precondition holds for the replica being promoted as well as
/// for the primary key the request names — and once that replica is enabled again, the
/// identical call performs the move.
#[tokio::test]
async fn a_promotion_judges_the_key_state_of_the_replica_it_promotes() {
    let router = router();

    // (case, how the replica is put out of a usable key state, the expected error code,
    // and the key state the refusal's message words).
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("a disabled replica", "DisableKey", "DisabledException", "disabled"),
        (
            "a replica awaiting deletion",
            "ScheduleKeyDeletion",
            "KMSInvalidStateException",
            "pending deletion",
        ),
    ];

    for (case, put_in_state, expected_code, worded_as) in rows {
        let replica_region = REPLICA_REGIONS[0];
        let key_id = text(
            case,
            &create_key(&router, &json!({"MultiRegion": true})).await,
            "KeyId",
        );
        replicate_into(&router, case, &key_id, &[replica_region]).await;
        ok_in(&router, replica_region, put_in_state, &json!({"KeyId": key_id})).await;

        let (status, payload) = call(
            &router,
            REGION,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": replica_region}),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, expected_code),
            "case {case}: promoting a replica that is not in a usable key state must be \
             refused {expected_code}, got HTTP {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!("{} is {worded_as}.", arn_in(replica_region, &key_id)),
            "case {case}: the refusal must name the REPLICA it is about, got {payload}"
        );

        // Both sides' roles are exactly as they were, and the replica's own key state is
        // untouched: a refused promotion persists nothing.
        for (region, expected_type) in [(REGION, "PRIMARY"), (replica_region, "REPLICA")] {
            let (key_type, primary_region, _) = topology_at(&router, case, region, &key_id).await;
            assert_eq!(
                (key_type.as_str(), primary_region.as_str()),
                (expected_type, REGION),
                "case {case}: a refused promotion must leave the {region} key's role as it \
                 was"
            );
        }

        // The control: the move the row refused is served once the replica is back in a
        // key state a management operation of the set is compatible with. Only the
        // disabled row can come back — a key awaiting deletion is restored by
        // `CancelKeyDeletion`, which leaves it disabled, so both rows go through
        // `EnableKey`.
        if put_in_state == "ScheduleKeyDeletion" {
            ok_in(
                &router,
                replica_region,
                "CancelKeyDeletion",
                &json!({"KeyId": key_id}),
            )
            .await;
        }
        ok_in(&router, replica_region, "EnableKey", &json!({"KeyId": key_id})).await;
        let enabled = describe_in(&router, replica_region, &key_id).await;
        assert_eq!(
            enabled["KeyState"].as_str().unwrap_or_default(),
            "Enabled",
            "case {case}: the replica is enabled again, got {enabled}"
        );

        let (status, payload) = call(
            &router,
            REGION,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": replica_region}),
        )
        .await;
        assert_eq!(
            (status, error_message(&payload).as_str()),
            (200, ""),
            "case {case}: the identical call must be served once the replica is enabled, \
             got HTTP {status} with {payload}"
        );
        settle_key_state(&router, replica_region, case, &key_id, "Updating").await;
        let (key_type, primary_region, _) =
            topology_at(&router, case, replica_region, &key_id).await;
        assert_eq!(
            (key_type.as_str(), primary_region.as_str()),
            ("PRIMARY", replica_region),
            "case {case}: the move the refusal held back is the move that then happens"
        );
    }
}
