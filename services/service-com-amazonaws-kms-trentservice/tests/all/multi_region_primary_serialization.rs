// SPDX-License-Identifier: CC0-1.0

//! Concurrent and in-flight `UpdatePrimaryRegion` calls against ONE multi-Region key
//! set, driven end-to-end over the wire through the shared [`super::harness`].
//!
//! The rule under test: a multi-Region key set has ONE primary-key designation and
//! moving it is not instantaneous — "even after this operation completes, the process
//! of updating the primary Region might still be in progress for a few more seconds.
//! ... The old and new primary keys have a transient key state of Updating. The
//! original key state is restored when the update is complete. While the key state is
//! Updating, you can use the keys in cryptographic operations, but you cannot replicate
//! the new primary key or perform certain management operations, such as enabling or
//! disabling these keys"
//! (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`). So promotions of one set
//! are SERIALIZED: exactly one caller moves the designation, and every caller arriving
//! while that move is in flight is refused the modelled key state fault,
//! `KMSInvalidStateException` — the fault a racing or retrying client branches on —
//! rather than a request-validation fault, since nothing about such a request's own
//! values is wrong. The set then settles in exactly ONE configuration, which every
//! Region's endpoint reports identically.
//!
//! Both suites assert that over a TABLE rather than one call: the racing rows differ in
//! how many callers race, in the Region they promote (the two-, three- and
//! four-segment Region ids `RegionType`'s pattern admits) and in how many Regions hold a
//! key of the set; the in-flight rows differ in
//! the operation attempted, in the endpoint attempting it and in whether that operation
//! is a management operation of the set or a cryptographic one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::key_arn;

use super::harness::{
    b64, call, create_key, describe_in, error_code, error_message, ok_in, replicate_into,
    settle_key_state, text, topology_at, unb64, REGION,
};

/// One racing row: how many callers promote at once, the Region they all promote, the
/// other Regions that hold a key of the set, and the endpoints the racers call.
///
/// A promotion's `KeyId` "identifies the current primary key"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegionRequest",member="KeyId")`), so the
/// endpoint every racer of a row calls is the one serving the set's primary key; what a
/// promotion asked for with a REPLICA's identifier answers is
/// [`super::update_primary_region_preconditions`]'s rule.
struct Race {
    case: &'static str,
    racers: usize,
    target: &'static str,
    others: &'static [&'static str],
    endpoints: &'static [&'static str],
}

/// The racing table. The target Regions span the Region ids `RegionType`'s modelled
/// `^([a-z]+-){2,3}\d+$` pattern admits
/// (`smithy_query(query="operation",operation="UpdatePrimaryRegion")`).
const RACES: &[Race] = &[
    Race {
        case: "two callers race, the smallest set, one endpoint",
        racers: 2,
        target: "eu-west-2",
        others: &[],
        endpoints: &[REGION],
    },
    Race {
        case: "three callers race from the origin endpoint, a bystander Region in the set",
        racers: 3,
        target: "ap-southeast-2",
        others: &["eu-west-2"],
        endpoints: &[REGION],
    },
    Race {
        case: "five callers race from the primary key's endpoint of a four-Region set",
        racers: 5,
        target: "us-gov-west-1",
        others: &["eu-west-2", "ap-southeast-2"],
        endpoints: &[REGION],
    },
];

/// Concurrent promotions of one multi-Region key set are serialized: exactly one of
/// them moves the designation, every other is refused `KMSInvalidStateException`
/// naming the key it resolved to and the state the move in flight puts it in, and the
/// set settles in ONE configuration that every Region's endpoint reports identically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_promotions_of_one_set_are_refused_as_an_invalid_key_state() {
    for race in RACES {
        let case = race.case;
        let router = Arc::new(super::harness::router());

        let created = create_key(&router, &json!({"MultiRegion": true})).await;
        let key_id = text(case, &created, "KeyId");
        let mut set_regions: Vec<&str> = vec![race.target];
        set_regions.extend_from_slice(race.others);
        replicate_into(&router, case, &key_id, &set_regions).await;

        // Every caller asks for the SAME move, at once.
        let mut racing = Vec::new();
        for racer in 0..race.racers {
            let router = Arc::clone(&router);
            let endpoint = race.endpoints[racer % race.endpoints.len()].to_string();
            let body = json!({"KeyId": key_id.clone(), "PrimaryRegion": race.target});
            racing.push(tokio::spawn(async move {
                let (status, payload) =
                    call(&router, &endpoint, "UpdatePrimaryRegion", &body).await;
                (endpoint, status, payload)
            }));
        }
        let mut answers: Vec<(String, u16, Value)> = Vec::new();
        for racer in racing {
            answers.push(racer.await.expect("a racing caller finishes"));
        }

        // Exactly one caller wins; every other is told the set's state is not valid
        // for the request, by the key state the move in flight puts the key IT named
        // in — never a validation failure, which is a permanent input fault no client
        // retries.
        let winners = answers.iter().filter(|(_, status, _)| *status == 200).count();
        assert_eq!(
            winners, 1,
            "case {case}: exactly one of {} concurrent promotions moves the designation, got \
             {winners} successes in {answers:?}",
            race.racers
        );
        for (endpoint, status, payload) in &answers {
            if *status == 200 {
                continue;
            }
            assert_eq!(
                (*status, error_code(payload).as_str()),
                (400, "KMSInvalidStateException"),
                "case {case}: a caller racing at the {endpoint} endpoint must be refused the \
                 modelled key state fault, got HTTP {status} with {payload}"
            );
            assert_eq!(
                error_message(payload),
                format!("{} is updating.", key_arn(endpoint, &key_id)),
                "case {case}: the refusal at the {endpoint} endpoint must name the key it \
                 resolved to and the state the move puts it in, got {payload}"
            );
        }

        // ONE configuration, seen the same way from every endpoint of the set, once the
        // clock has made the winner's move due — the designation arrives when the
        // transient key state of the move is over: the target Region holds the primary
        // key and every other Region of the set holds a replica of it — no split brain,
        // whichever endpoint is asked.
        let mut expected_replicas: Vec<String> = set_regions
            .iter()
            .chain(std::iter::once(&REGION))
            .filter(|region| **region != race.target)
            .map(|region| (*region).to_string())
            .collect();
        expected_replicas.sort();
        settle_key_state(&router, race.target, case, &key_id, "Updating").await;
        for region in set_regions.iter().chain(std::iter::once(&REGION)) {
            let (key_type, primary_region, replicas) =
                topology_at(&router, case, region, &key_id).await;
            let expected_type = if *region == race.target {
                "PRIMARY"
            } else {
                "REPLICA"
            };
            assert_eq!(
                (key_type.as_str(), primary_region.as_str(), &replicas),
                (expected_type, race.target, &expected_replicas),
                "case {case}: the {region} endpoint must report the one configuration the \
                 race settled in"
            );
        }
    }
}

/// While the clock still owes a set its primary-Region move, the keys that move report
/// the transient `Updating` key state: the management operations of the set are refused
/// `KMSInvalidStateException` and the cryptographic operations go on succeeding. Once
/// the clock ends the move, the original key state is restored and every refused
/// management operation is served.
#[tokio::test]
async fn a_set_moving_its_primary_key_refuses_management_and_serves_cryptography() {
    let router = super::harness::router();
    let target = "eu-west-2";
    // A third Region, holding no key of the set: the Region a replication attempted
    // while the move is in flight would create a replica in.
    let elsewhere = "ap-southeast-2";

    let created = create_key(&router, &json!({"MultiRegion": true})).await;
    let key_id = text("setup", &created, "KeyId");
    replicate_into(&router, "setup", &key_id, &[target]).await;

    // A ciphertext taken before the move: the set's key material is shared, so it must
    // keep opening at either endpoint while the designation moves.
    let sealed = ok_in(
        &router,
        REGION,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"sealed before the move")}),
    )
    .await;
    let blob = b64(&unb64("setup", &sealed, "CiphertextBlob"));

    let moved = ok_in(
        &router,
        REGION,
        "UpdatePrimaryRegion",
        &json!({"KeyId": key_id, "PrimaryRegion": target}),
    )
    .await;
    assert!(
        moved.is_null() || moved.as_object().is_some_and(|members| members.is_empty()),
        "setup: UpdatePrimaryRegion answers an empty 200 body, got {moved}"
    );

    // Both keys the designation moved between report the transient state, through
    // their own endpoints, and `Enabled` follows it — and the move itself has not
    // happened yet, so the set still reports the designation it has.
    for region in [REGION, target] {
        let metadata = describe_in(&router, region, &key_id).await;
        assert_eq!(
            (metadata["KeyState"].clone(), metadata["Enabled"].clone()),
            (json!("Updating"), json!(false)),
            "case the {region} endpoint while the move is in flight: the old and the new \
             primary key are Updating, got {metadata}"
        );
        assert_eq!(
            metadata["MultiRegionConfiguration"]["PrimaryKey"]["Region"],
            json!(REGION),
            "case the {region} endpoint while the move is in flight: the designation has \
             not arrived while the move is still in progress, got {metadata}"
        );
    }

    // The management operations of a set whose move is in flight: each is refused the
    // modelled key state fault, naming the key the request resolved to.
    let management: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a second promotion, back to the origin Region",
            REGION,
            "UpdatePrimaryRegion",
            json!({"KeyId": key_id, "PrimaryRegion": REGION}),
        ),
        (
            "a promotion of the Region the move is going to",
            target,
            "UpdatePrimaryRegion",
            json!({"KeyId": key_id, "PrimaryRegion": target}),
        ),
        (
            "a promotion asked for through the new primary key's endpoint",
            target,
            "UpdatePrimaryRegion",
            json!({"KeyId": key_id, "PrimaryRegion": REGION}),
        ),
        (
            "replicating the new primary key into a Region holding no key of the set",
            target,
            "ReplicateKey",
            json!({"KeyId": key_id, "ReplicaRegion": elsewhere}),
        ),
        (
            "enabling the former primary key",
            REGION,
            "EnableKey",
            json!({"KeyId": key_id}),
        ),
        (
            "disabling the former primary key",
            REGION,
            "DisableKey",
            json!({"KeyId": key_id}),
        ),
        (
            "enabling the new primary key",
            target,
            "EnableKey",
            json!({"KeyId": key_id}),
        ),
        (
            "disabling the new primary key",
            target,
            "DisableKey",
            json!({"KeyId": key_id}),
        ),
    ];
    for (case, region, operation, body) in management {
        let (status, payload) = call(&router, region, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "KMSInvalidStateException"),
            "case {case}: a management operation of a set that is still moving its primary \
             key is refused the modelled key state fault, got HTTP {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!("{} is updating.", key_arn(region, &key_id)),
            "case {case}: the refusal names the key the request resolved to and the state \
             the move puts it in, got {payload}"
        );
    }

    // The cryptographic operations of the same set, through the same endpoints, while
    // the same move is in flight: every one of them succeeds, and what they produce is
    // real — a ciphertext taken before the move still opens, and one taken during it
    // opens at the other Region's endpoint.
    for region in [REGION, target] {
        let case = format!("cryptography at the {region} endpoint while the move is in flight");
        let opened = ok_in(
            &router,
            region,
            "Decrypt",
            &json!({"CiphertextBlob": blob.clone()}),
        )
        .await;
        assert_eq!(
            unb64(&case, &opened, "Plaintext"),
            b"sealed before the move".to_vec(),
            "case {case}: a ciphertext the set sealed before the move still opens, got {opened}"
        );

        let during = ok_in(
            &router,
            region,
            "Encrypt",
            &json!({"KeyId": key_id, "Plaintext": b64(b"sealed during the move")}),
        )
        .await;
        let other = if region == REGION { target } else { REGION };
        let reopened = ok_in(
            &router,
            other,
            "Decrypt",
            &json!({"CiphertextBlob": b64(&unb64(&case, &during, "CiphertextBlob"))}),
        )
        .await;
        assert_eq!(
            unb64(&case, &reopened, "Plaintext"),
            b"sealed during the move".to_vec(),
            "case {case}: a ciphertext sealed during the move opens at the {other} endpoint, \
             got {reopened}"
        );

        let data_key = ok_in(
            &router,
            region,
            "GenerateDataKey",
            &json!({"KeyId": key_id, "NumberOfBytes": 32}),
        )
        .await;
        assert_eq!(
            unb64(&case, &data_key, "Plaintext").len(),
            32,
            "case {case}: a data key of the asked-for length is generated, got {data_key}"
        );
    }

    // The clock ends the move by itself: the key state each key carried is restored,
    // and every management operation the move refused is served.
    for region in [REGION, target] {
        let settled = settle_key_state(&router, region, "settled", &key_id, "Updating").await;
        assert_eq!(
            (settled["KeyState"].clone(), settled["Enabled"].clone()),
            (created["KeyState"].clone(), created["Enabled"].clone()),
            "case the {region} endpoint once the move is complete: the original key state is \
             restored, got {settled}"
        );
    }
    let replicated = ok_in(
        &router,
        target,
        "ReplicateKey",
        &json!({"KeyId": key_id, "ReplicaRegion": elsewhere}),
    )
    .await;
    assert_eq!(
        replicated["ReplicaKeyMetadata"]["Arn"],
        json!(key_arn(elsewhere, &key_id)),
        "case once the move is complete: the new primary key replicates again, got {replicated}"
    );
    let promoted = ok_in(
        &router,
        target,
        "UpdatePrimaryRegion",
        &json!({"KeyId": key_id, "PrimaryRegion": REGION}),
    )
    .await;
    assert!(
        promoted.is_null() || promoted.as_object().is_some_and(|members| members.is_empty()),
        "case once the move is complete: the set is promotable again, got {promoted}"
    );
    settle_key_state(&router, REGION, "promoted back", &key_id, "Updating").await;
    let (key_type, primary_region, _) = topology_at(&router, "promoted back", REGION, &key_id).await;
    assert_eq!(
        (key_type.as_str(), primary_region.as_str()),
        ("PRIMARY", REGION),
        "case once the move is complete: the second promotion moved the designation back"
    );
}
