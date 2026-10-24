// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdatePrimaryRegion`, driven over the wire through the
//! shared [`super::harness`].
//!
//! The rule under test: a multi-Region key set has exactly one primary key at a
//! time, and `UpdatePrimaryRegion` moves that designation to a Region of the set
//! that already holds a replica key — making the former primary key a replica,
//! creating no key anywhere, and changing nothing else about the set — so a later
//! `DescribeKey` reports the moved designation through `MultiRegionConfiguration`.
//! The call is refused, persisting nothing, when the key is not a multi-Region key
//! at all, when the identifier names no key of the Region the request was signed
//! for, when it is not a well-formed KMS ARN, when the key's key state is not
//! compatible with a management operation, and when the named Region holds no
//! replica of the set — the last of which is a bad `PrimaryRegion` value rather
//! than a missing resource, so it is a validation failure.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in the Region
//! the designation moves to (two-, three- and four-segment Region ids), in the
//! member of the set the designation moves from, in the kind of key the request
//! names (multi-Region or single-Region, by key id, by key ARN or by alias), and in
//! the key state the named key is in.
//!
//! A replica key is created by `ReplicateKey`, which is still a scaffold stub with
//! its own issue, so the promotion rows seed the replica Regions of the set
//! DIRECTLY into the store — the same record a replicate would leave behind — and
//! then drive `UpdatePrimaryRegion` and `DescribeKey` over the wire.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_kms_trentservice::state::{key_arn, service_state};

use super::harness::{
    assert_empty, call, create_alias, create_key, describe, error_code, failure, ok,
    router_with_engine, text, REGION,
};

/// The Regions the promotion table moves the primary-key designation to, beside the
/// creation Region [`REGION`]. They span the Region ids `RegionType`'s modelled
/// `^([a-z]+-){2,3}\d+$` pattern admits
/// (`smithy_query(query="operation",operation="UpdatePrimaryRegion")`): two
/// segments before the digit, three of them, and a longer name.
const REPLICA_REGIONS: [&str; 3] = ["eu-west-2", "ap-southeast-2", "us-gov-west-1"];

/// Register `regions` as Regions holding a replica key of the multi-Region key
/// `key_id`, which is the record `ReplicateKey` leaves behind once it is
/// implemented (its own issue). Written through the engine the router was built
/// over, so every later request observes it.
fn seed_replica_regions(engine: &Arc<engine::InMemoryEngine>, key_id: &str, regions: &[&str]) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    let key = state
        .keys
        .get_mut(key_id)
        .expect("the key the test just created is in the store");
    let topology = key
        .multi_region_topology
        .as_mut()
        .expect("a multi-Region key carries a multi-Region topology");
    for region in regions {
        topology.replica_regions.insert((*region).to_string());
    }
}

/// The `MultiRegionConfiguration` of `key_id` as this endpoint reports it: which
/// member of the set it serves, the Region of the primary key, and the Regions of
/// the replica keys.
async fn topology_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
) -> (String, String, Vec<String>) {
    let metadata = describe(router, key_id).await;
    let configuration = metadata["MultiRegionConfiguration"].clone();
    let key_type = text(case, &configuration, "MultiRegionKeyType");
    let primary = configuration["PrimaryKey"].clone();
    let primary_region = text(case, &primary, "Region");
    assert_eq!(
        primary["Arn"],
        json!(key_arn(&primary_region, key_id)),
        "case {case}: the primary key is named by the key ARN of its own Region, got {primary}"
    );
    let replicas = configuration["ReplicaKeys"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ReplicaKeys in {configuration}"))
        .iter()
        .map(|replica| {
            let region = text(case, replica, "Region");
            assert_eq!(
                replica["Arn"],
                json!(key_arn(&region, key_id)),
                "case {case}: a replica key is named by the key ARN of its own Region, \
                 got {replica}"
            );
            region
        })
        .collect();
    (key_type, primary_region, replicas)
}

/// `UpdatePrimaryRegion` moves the primary-key designation of a multi-Region key
/// set to a Region that holds a replica of it, and the former primary key becomes a
/// replica: the membership of the set never changes, only which one of its Regions
/// is primary. Every row is read back through `DescribeKey`.
#[tokio::test]
async fn update_primary_region_moves_the_primary_key_of_the_set() {
    let (router, engine) = router_with_engine();

    let created = create_key(&router, &json!({"MultiRegion": true, "Description": "set"})).await;
    let key_id = text("seed", &created, "KeyId");
    seed_replica_regions(&engine, &key_id, &REPLICA_REGIONS);

    // Every Region of the set: the one the key was created in (its first primary)
    // and the three the replicas live in.
    let mut members: Vec<String> = REPLICA_REGIONS.iter().map(|r| (*r).to_string()).collect();
    members.push(REGION.to_string());
    members.sort();

    // The designation moves to each replica Region in turn — including back to the
    // Region the key was created in, which by then holds a replica itself.
    let moves = [
        REPLICA_REGIONS[0],
        REPLICA_REGIONS[1],
        REPLICA_REGIONS[2],
        REGION,
        REPLICA_REGIONS[2],
    ];

    for primary_region in moves {
        let case = format!("primary moves to {primary_region}");
        let answer = ok(
            &router,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": primary_region}),
        )
        .await;
        assert_empty(&case, "UpdatePrimaryRegion", &answer);

        let (key_type, reported_primary, replicas) = topology_of(&router, &case, &key_id).await;
        assert_eq!(
            reported_primary, primary_region,
            "case {case}: expected the primary key in {primary_region}, got {reported_primary}"
        );
        // The endpoint serving this record is the creation Region, so the key it
        // serves is the set's primary only while the designation rests there.
        let expected_type = if primary_region == REGION {
            "PRIMARY"
        } else {
            "REPLICA"
        };
        assert_eq!(
            key_type, expected_type,
            "case {case}: expected the key served in {REGION} to be the {expected_type} of the \
             set, got {key_type}"
        );
        let expected_replicas: Vec<String> = members
            .iter()
            .filter(|region| *region != primary_region)
            .cloned()
            .collect();
        assert_eq!(
            replicas, expected_replicas,
            "case {case}: expected replica keys in {expected_replicas:?}, got {replicas:?}"
        );

        // Nothing else about the key moved: a multi-Region set shares one key id,
        // one key ARN per Region and one piece of key material.
        let metadata = describe(&router, &key_id).await;
        for member in ["KeyId", "Arn", "KeySpec", "KeyUsage", "KeyState", "CurrentKeyMaterialId"] {
            assert_eq!(
                metadata[member], created[member],
                "case {case}: {member} must be untouched by a primary-Region move, got {metadata}"
            );
        }
        assert_eq!(
            metadata["MultiRegion"],
            json!(true),
            "case {case}: the key is still a multi-Region key, got {metadata}"
        );

        // The Region that now holds the primary key holds no replica, so asking for
        // it again names no key to promote.
        let (status, code) = failure(
            &router,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": primary_region}),
        )
        .await;
        // Every client refusal of this service answers HTTP 400 with the error
        // shape in the body's `__type`, whatever the per-shape `@httpError` says.
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case}: the Region already holding the primary key holds no replica to \
             promote, got HTTP {status} {code}"
        );
        let (_, unchanged_primary, unchanged_replicas) =
            topology_of(&router, &case, &key_id).await;
        assert_eq!(
            (unchanged_primary, unchanged_replicas),
            (primary_region.to_string(), expected_replicas),
            "case {case}: a refused call must leave the set exactly as it was"
        );
    }
}

/// `UpdatePrimaryRegion` refuses a set it cannot promote within, and persists
/// nothing when it does: each row names a modelled error with its modelled status,
/// and the multi-Region key the suite set up still reports the primary key it
/// started with.
#[tokio::test]
async fn update_primary_region_refuses_what_it_cannot_promote() {
    let (router, engine) = router_with_engine();

    // A multi-Region key with a replica elsewhere: the rows that are not about the
    // set's membership use it, so a refusal is never explained by a missing
    // replica.
    let promotable = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    seed_replica_regions(&engine, &promotable, &[REPLICA_REGIONS[0]]);

    // The keys the rows name, each in the state its row is about.
    let single = text("setup", &create_key(&router, &json!({})).await, "KeyId");
    let single_explicit = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": false})).await,
        "KeyId",
    );
    let multi_region_arn = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "Arn",
    );
    let disabled = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    seed_replica_regions(&engine, &disabled, &[REPLICA_REGIONS[0]]);
    ok(&router, "DisableKey", &json!({"KeyId": disabled})).await;
    let pending_deletion = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    seed_replica_regions(&engine, &pending_deletion, &[REPLICA_REGIONS[0]]);
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_deletion}),
    )
    .await;
    let pending_import = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true, "Origin": "EXTERNAL"})).await,
        "KeyId",
    );
    seed_replica_regions(&engine, &pending_import, &[REPLICA_REGIONS[0]]);
    create_alias(&router, "setup", "alias/single-region", &single).await;

    // (case, KeyId, PrimaryRegion, expected HTTP status, expected error code). Every
    // client refusal of this service answers HTTP 400 and carries the error shape in
    // the body's `__type`, whatever the per-shape `@httpError` the model records.
    let cases: Vec<(&str, String, &str, u16, &str)> = vec![
        (
            "a single-Region key has no primary Region to move",
            single.clone(),
            REPLICA_REGIONS[0],
            400,
            "UnsupportedOperationException",
        ),
        (
            "a key created explicitly single-Region",
            single_explicit,
            REPLICA_REGIONS[1],
            400,
            "UnsupportedOperationException",
        ),
        (
            "an alias naming a single-Region key",
            "alias/single-region".to_string(),
            REPLICA_REGIONS[2],
            400,
            "UnsupportedOperationException",
        ),
        (
            "a key id no key of this Region has",
            "2a8b3c4d-1234-4abc-8def-567890abcdef".to_string(),
            REPLICA_REGIONS[0],
            400,
            "NotFoundException",
        ),
        (
            "an identifier that is an ARN of no KMS resource",
            format!(
                "arn:aws:kms:{REGION}:{account}:mrk-00112233445566778899aabbccddeeff",
                account = engine::emulator_account_id()
            ),
            REPLICA_REGIONS[0],
            400,
            "InvalidArnException",
        ),
        (
            "a key ARN of another Region's endpoint",
            key_arn(REPLICA_REGIONS[1], "mrk-00112233445566778899aabbccddeeff"),
            REPLICA_REGIONS[0],
            400,
            "NotFoundException",
        ),
        (
            "the Region that already holds the primary key",
            promotable.clone(),
            REGION,
            400,
            "ValidationException",
        ),
        (
            "a Region holding no key of the set",
            promotable.clone(),
            REPLICA_REGIONS[1],
            400,
            "ValidationException",
        ),
        (
            "a multi-Region key with no replica anywhere, named by its key ARN",
            multi_region_arn,
            REPLICA_REGIONS[0],
            400,
            "ValidationException",
        ),
        (
            "a disabled key",
            disabled,
            REPLICA_REGIONS[0],
            400,
            "DisabledException",
        ),
        (
            "a key pending deletion",
            pending_deletion,
            REPLICA_REGIONS[0],
            400,
            "KMSInvalidStateException",
        ),
        (
            "a key awaiting imported key material",
            pending_import,
            REPLICA_REGIONS[0],
            400,
            "KMSInvalidStateException",
        ),
    ];

    for (case, key_id, primary_region, expected_status, expected_code) in cases {
        let (status, code) = failure(
            &router,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": primary_region}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case}: expected HTTP {expected_status} {expected_code}, got {status} {code}"
        );

        // Every refusal persists nothing: the promotable set still has its primary
        // key where it was created, with its one replica untouched.
        let (key_type, primary, replicas) = topology_of(&router, case, &promotable).await;
        assert_eq!(
            (key_type.as_str(), primary.as_str(), replicas.as_slice()),
            (
                "PRIMARY",
                REGION,
                [REPLICA_REGIONS[0].to_string()].as_slice()
            ),
            "case {case}: a refused call must persist nothing"
        );
    }
}

/// The refusal of a key that is not part of a multi-Region set names the key the
/// request RESOLVED to, by its ARN — whichever identifier form named it, and
/// whatever kind of single-Region key it is.
#[tokio::test]
async fn the_refusal_of_a_single_region_key_names_it_by_its_arn() {
    let router = router_with_engine().0;

    // The rows span the kinds of single-Region key this service mints: the default
    // symmetric encryption key, one created explicitly single-Region, an asymmetric
    // signing key, an HMAC key and one still awaiting imported key material — none
    // of which has a primary Region to move.
    let creates: Vec<(&str, serde_json::Value)> = vec![
        ("default", json!({})),
        ("explicit single-Region", json!({"MultiRegion": false})),
        (
            "asymmetric signing",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "HMAC",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        ("awaiting imported material", json!({"Origin": "EXTERNAL"})),
    ];

    for (index, (case, request)) in creates.into_iter().enumerate() {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let arn = text(case, &metadata, "Arn");
        let alias = format!("alias/single-{index}");
        create_alias(&router, case, &alias, &key_id).await;

        // Each identifier form a `KeyId` member accepts for this key.
        for identifier in [key_id.clone(), arn.clone(), alias.clone()] {
            let (status, payload) = call(
                &router,
                REGION,
                "UpdatePrimaryRegion",
                &json!({"KeyId": identifier, "PrimaryRegion": REPLICA_REGIONS[0]}),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "UnsupportedOperationException"),
                "case {case} named by {identifier}: expected HTTP 400 \
                 UnsupportedOperationException, got {status} with {payload}"
            );
            let message = payload["message"]
                .as_str()
                .or_else(|| payload["Message"].as_str())
                .unwrap_or_default();
            assert_eq!(
                message,
                format!("{arn} is not a multi-region key."),
                "case {case} named by {identifier}: the refusal must name the resolved key \
                 by its ARN, got {message:?}"
            );
        }
    }
}
