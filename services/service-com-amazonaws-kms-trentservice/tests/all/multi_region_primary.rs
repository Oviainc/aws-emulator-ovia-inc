// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdatePrimaryRegion`, driven over the wire through the
//! shared [`super::harness`].
//!
//! The rule under test: a multi-Region key set has exactly one primary key at a
//! time, and `UpdatePrimaryRegion` moves that designation to a Region of the set
//! that already holds a replica key — making the former primary key a replica,
//! creating no key anywhere, and changing nothing else about the set — so a later
//! `DescribeKey` reports the moved designation through `MultiRegionConfiguration`.
//! The `KeyId` "identifies the current primary key", so the call is made at the
//! endpoint of the Region that holds the set's primary key now.
//! The call is refused, persisting nothing, when the key is not a multi-Region key
//! at all, when the identifier names no key of the Region the request was signed
//! for, when it is not a well-formed KMS ARN, when the key's key state is not
//! compatible with a management operation, and when the named Region holds no
//! replica of the set — the last of which is a bad `PrimaryRegion` value rather
//! than a missing resource, so it is a validation failure.
//!
//! The move itself takes a span the clock ends by itself, and the designation arrives
//! when that span is over: while it is in flight the two keys the designation moves
//! between report the transient `Updating` key state, the set still reports the
//! designation it has, and a second promotion of that set is refused as a key state the
//! operation is not compatible with, whichever Region it asks for. The concurrent case —
//! several promotions of one set issued at once — is asserted by
//! `tests/all/multi_region_primary_serialization.rs`, and what a set is once a promotion
//! has been reported by `tests/all/multi_region_primary_settlement.rs`.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in the Region
//! the designation moves to (two-, three- and four-segment Region ids), in the
//! member of the set the designation moves from, in the kind of key the request
//! names (multi-Region or single-Region, by key id, by key ARN or by alias), and in
//! the key state the named key is in.
//!
//! The replica keys the promotion rows need are created by `ReplicateKey`, driven
//! over the same wire, so the record this suite promotes within is the one a
//! replicate really leaves behind.

use serde_json::json;
use service_com_amazonaws_kms_trentservice::state::key_arn;

use super::harness::{
    assert_empty, call, create_alias, create_key, describe, error_code, failure, ok, ok_in, refused,
    replicate_into, router, settle_key_state, text, topology_at, REGION,
};

/// The Regions the promotion table moves the primary-key designation to, beside the
/// creation Region [`REGION`]. They span the Region ids `RegionType`'s modelled
/// `^([a-z]+-){2,3}\d+$` pattern admits
/// (`smithy_query(query="operation",operation="UpdatePrimaryRegion")`): two
/// segments before the digit, three of them, and a longer name.
const REPLICA_REGIONS: [&str; 3] = ["eu-west-2", "ap-southeast-2", "us-gov-west-1"];

/// The `MultiRegionConfiguration` of `key_id` as the creation Region's endpoint reports
/// it — [`topology_at`] at [`REGION`], which is the endpoint every row of this suite
/// reads the set through.
async fn topology_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
) -> (String, String, Vec<String>) {
    topology_at(router, case, REGION, key_id).await
}

/// `UpdatePrimaryRegion` moves the primary-key designation of a multi-Region key
/// set to a Region that holds a replica of it, and the former primary key becomes a
/// replica: the membership of the set never changes, only which one of its Regions
/// is primary. Every row is read back through `DescribeKey`.
#[tokio::test]
async fn update_primary_region_moves_the_primary_key_of_the_set() {
    let router = router();

    let created = create_key(&router, &json!({"MultiRegion": true, "Description": "set"})).await;
    let key_id = text("seed", &created, "KeyId");
    replicate_into(&router, "seed", &key_id, &REPLICA_REGIONS).await;

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

    // Where the designation rests before each row's move: with that row's target it is
    // the PAIR of keys the move goes through, which is what decides which endpoints
    // report the transient key state of a move in flight.
    let mut former_primary = REGION.to_string();

    for primary_region in moves {
        let case = format!("primary moves to {primary_region}");
        // "Identifies the current primary key"
        // (`smithy_query(query="doc",shape="UpdatePrimaryRegionRequest",member="KeyId")`),
        // and an identifier resolves to the key the endpoint it is presented at serves,
        // so each move is asked for at the endpoint of the Region that holds the set's
        // primary key right now — which this row's predecessor moved.
        let answer = ok_in(
            &router,
            &former_primary,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": primary_region}),
        )
        .await;
        assert_empty(&case, "UpdatePrimaryRegion", &answer);

        // "Even after this operation completes, the process of updating the primary
        // Region might still be in progress for a few more seconds. ... The old and
        // new primary keys have a transient key state of Updating. The original key
        // state is restored when the update is complete"
        // (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`): while the clock
        // still owes the set that move, the move has not happened, so the set goes on
        // reporting the designation it has.
        let in_flight_replicas: Vec<String> = members
            .iter()
            .filter(|region| **region != former_primary)
            .cloned()
            .collect();
        let (in_flight_type, in_flight_primary, in_flight_reported_replicas) =
            topology_of(&router, &case, &key_id).await;
        assert_eq!(
            (
                in_flight_primary.as_str(),
                &in_flight_reported_replicas,
                in_flight_type.as_str()
            ),
            (
                former_primary.as_str(),
                &in_flight_replicas,
                if former_primary == REGION {
                    "PRIMARY"
                } else {
                    "REPLICA"
                }
            ),
            "case {case}: while the move is in flight the set still reports the designation \
             it has"
        );

        // This endpoint serves one key of the set, so it reports the transient state
        // exactly while the key it serves is one of the two the designation moves
        // between. "When KeyState is Enabled this value is true, otherwise it is
        // false" (`smithy_query(query="doc",shape="KeyMetadata",member="Enabled")`),
        // so the two members move together.
        let moves_this_key = former_primary == REGION || primary_region == REGION;
        let in_flight_state = if moves_this_key { "Updating" } else { "Enabled" };
        let metadata = describe(&router, &key_id).await;
        assert_eq!(
            metadata["KeyState"],
            json!(in_flight_state),
            "case {case}: the key served in {REGION} is {}a key the designation moved \
             between, so its key state while the move is in flight must be \
             {in_flight_state}, got {metadata}",
            if moves_this_key { "" } else { "not " }
        );
        assert_eq!(
            metadata["Enabled"],
            json!(!moves_this_key),
            "case {case}: Enabled is true only while KeyState is Enabled, got {metadata}"
        );

        // While the clock still owes the set this move, the SET is busy: another
        // promotion of it is refused as a key state this operation is not compatible
        // with — whichever Region it asks for, including one that does hold a replica
        // — and persists nothing. A busy set is not a malformed request.
        for busy_target in [primary_region, former_primary.as_str()] {
            let (status, code, message) = refused(
                &router,
                REGION,
                "UpdatePrimaryRegion",
                &json!({"KeyId": key_id, "PrimaryRegion": busy_target}),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "KMSInvalidStateException"),
                "case {case}: a promotion to {busy_target} while the set is still moving \
                 its primary key must be refused as an invalid key state, got {status} {code}"
            );
            assert_eq!(
                message,
                format!("{} is updating.", key_arn(REGION, &key_id)),
                "case {case}: the refusal of a set that is busy must name the key the \
                 request resolved to and the state the move puts it in, got {message:?}"
            );
        }
        let (_, unchanged_primary, unchanged_replicas) =
            topology_of(&router, &case, &key_id).await;
        assert_eq!(
            (unchanged_primary, unchanged_replicas),
            (former_primary.clone(), in_flight_replicas),
            "case {case}: a refused call must leave the set exactly as it was"
        );

        // Once the clock has made the move due, the designation has arrived and the
        // original key state is restored — one change, so the set reports the moved
        // designation exactly from the instant the transient state is over. The Region
        // the designation moved TO is always one of the two keys the move went through,
        // so its endpoint is where the move in flight is observable whichever Regions
        // this row moved between.
        settle_key_state(&router, primary_region, &case, &key_id, "Updating").await;
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
        let settled = describe(&router, &key_id).await;
        for member in ["KeyId", "Arn", "KeySpec", "KeyUsage", "CurrentKeyMaterialId"] {
            assert_eq!(
                settled[member], created[member],
                "case {case}: {member} must be untouched by a primary-Region move, got {settled}"
            );
        }
        assert_eq!(
            settled["MultiRegion"],
            json!(true),
            "case {case}: the key is still a multi-Region key, got {settled}"
        );
        assert_eq!(
            (settled["KeyState"].clone(), settled["Enabled"].clone()),
            (created["KeyState"].clone(), created["Enabled"].clone()),
            "case {case}: the key state the move suspended must be restored, got {settled}"
        );

        // The Region that now holds the primary key holds no replica, so asking for
        // it again names no key to promote — a bad `PrimaryRegion` value rather than
        // a set that is busy. It is asked for at that Region's endpoint, which is the
        // one serving the set's primary key now.
        let (status, payload) = call(
            &router,
            primary_region,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": primary_region}),
        )
        .await;
        let code = error_code(&payload);
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

        former_primary = primary_region.to_string();
    }
}

/// `UpdatePrimaryRegion` refuses a set it cannot promote within, and persists
/// nothing when it does: each row names a modelled error with its modelled status,
/// and the multi-Region key the suite set up still reports the primary key it
/// started with.
#[tokio::test]
async fn update_primary_region_refuses_what_it_cannot_promote() {
    let router = router();

    // A multi-Region key with a replica elsewhere: the rows that are not about the
    // set's membership use it, so a refusal is never explained by a missing
    // replica.
    let promotable = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    replicate_into(&router, "setup", &promotable, &[REPLICA_REGIONS[0]]).await;

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
    replicate_into(&router, "setup", &disabled, &[REPLICA_REGIONS[0]]).await;
    ok(&router, "DisableKey", &json!({"KeyId": disabled})).await;
    let pending_deletion = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    replicate_into(&router, "setup", &pending_deletion, &[REPLICA_REGIONS[0]]).await;
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_deletion}),
    )
    .await;
    // A key still awaiting imported key material cannot be replicated either, so the
    // set this row names has no replica anywhere: its key state is what the row is
    // about, and it is judged before the membership of the set is.
    let pending_import = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true, "Origin": "EXTERNAL"})).await,
        "KeyId",
    );
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
            "InvalidArnException",
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
    let router = router();

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

        // Each identifier form this operation accepts for a key: "specify the key ID or
        // key ARN of a multi-Region primary key". An alias is not one of them, and is
        // refused for its KIND before anything is resolved (see the alias row of
        // `update_primary_region_refuses_what_it_cannot_promote`), so the alias created
        // above is what proves the key is reachable by one, not an input of this rule.
        for identifier in [key_id.clone(), arn.clone()] {
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
