// SPDX-License-Identifier: CC0-1.0

//! What a multi-Region key set is once `UpdatePrimaryRegion` has finished moving its
//! primary-key designation, driven end-to-end over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: a transient state the clock ends by itself, and the change that
//! transient state covers, end at the SAME instant — "the old and new primary keys have
//! a transient key state of Updating. The original key state is restored when the update
//! is complete" and "even after this operation completes, the process of updating the
//! primary Region might still be in progress for a few more seconds. Operations such as
//! DescribeKey might display both the old and new primary keys as replicas"
//! (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`). So a caller that can
//! observe the designation in its new Region is observing a COMPLETED move: from that
//! observation on, every operation the transient state suspended is served on both keys
//! of the set, and none of them is refused the key state of a move still in flight. What
//! the service answers WHILE the move is in flight is asserted by
//! `tests/all/multi_region_primary_serialization.rs`.
//!
//! Both suites assert that over a TABLE: the rows differ in the Region the designation
//! moves to (the two-, three- and four-segment Region ids `RegionType`'s pattern
//! admits), in which key of the set the call goes to (the promoted key or the demoted
//! one) and in the operation taken on it — each of the operations a move in flight
//! refuses.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::{key_arn, PRIMARY_REGION_UPDATE_SPAN};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, create_key, describe_in, error_code, error_message, ok_in, replicate_into, text,
    topology_at, REGION,
};

/// The Regions the rows promote the designation into, beside the creation Region
/// [`REGION`]: the Region ids `RegionType`'s modelled `^([a-z]+-){2,3}\d+$` pattern
/// admits (`smithy_query(query="operation",operation="UpdatePrimaryRegion")`) — two
/// segments before the digit, three of them, and a longer name.
const TARGETS: [&str; 3] = ["eu-west-2", "ap-southeast-2", "us-gov-west-1"];

/// How often a poll watching a promotion asks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One operation a primary-Region move in flight refuses, taken once that move has been
/// reported complete.
struct Served {
    case: &'static str,
    /// Whether the call goes to the key the designation moved TO, rather than the one it
    /// moved FROM.
    on_promoted: bool,
    operation: &'static str,
    /// The `KeyState` `DescribeKey` must report for the key the call named afterwards.
    settles_in: &'static str,
}

/// The operations a move in flight refuses — "while the key state is Updating ... you
/// cannot replicate the new primary key or perform certain management operations, such as
/// enabling or disabling these keys"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`) — in an order each row
/// leaves the next one able to run.
const SERVED: &[Served] = &[
    Served {
        case: "the promoted key is disabled",
        on_promoted: true,
        operation: "DisableKey",
        settles_in: "Disabled",
    },
    Served {
        case: "the promoted key is enabled again",
        on_promoted: true,
        operation: "EnableKey",
        settles_in: "Enabled",
    },
    Served {
        case: "the demoted key is disabled",
        on_promoted: false,
        operation: "DisableKey",
        settles_in: "Disabled",
    },
    Served {
        case: "the demoted key is enabled again",
        on_promoted: false,
        operation: "EnableKey",
        settles_in: "Enabled",
    },
];

/// Poll the endpoint of `region` the way a client watching a promotion does — until the
/// key it serves reports itself the PRIMARY key of its set — and report the
/// `KeyMetadata` that observation read.
///
/// Every observation on the way is checked against the rule: while the key reports the
/// transient `Updating` key state the designation has not moved yet, so the key cannot
/// already be reported as the set's primary key.
async fn await_promotion_reported(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    key_id: &str,
) -> Value {
    let deadline = Instant::now() + PRIMARY_REGION_UPDATE_SPAN * 20;
    loop {
        let metadata = describe_in(router, region, key_id).await;
        let key_type = metadata["MultiRegionConfiguration"]["MultiRegionKeyType"].clone();
        let reported_primary = key_type == json!("PRIMARY");
        assert!(
            !(reported_primary && metadata["KeyState"] == json!("Updating")),
            "case {case}: the designation arrives when the transient key state is over, so a \
             key reported as the set's primary key must not still be Updating, got {metadata}"
        );
        if reported_primary {
            return metadata;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the promotion must be reported at the {region} endpoint, got \
             {metadata}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A multi-Region key set with a replica in `target`, promoted there: the key id of the
/// set, once the promotion is reported at the target's endpoint.
async fn promoted_set(router: &MultiServiceRouter, case: &str, target: &str) -> String {
    let key_id = text(
        case,
        &create_key(router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    replicate_into(router, case, &key_id, &[target]).await;
    ok_in(
        router,
        REGION,
        "UpdatePrimaryRegion",
        &json!({"KeyId": key_id, "PrimaryRegion": target}),
    )
    .await;
    await_promotion_reported(router, case, target, &key_id).await;
    key_id
}

/// Once a promotion is reported, every operation the move in flight refused is served —
/// on the promoted key and on the demoted one, whichever Region the designation moved
/// to — and the key state each one leaves is read back through `DescribeKey`.
#[tokio::test]
async fn a_reported_promotion_serves_the_operations_the_move_refused() {
    let router = super::harness::router();

    for target in TARGETS {
        let key_id = promoted_set(&router, target, target).await;

        for row in SERVED {
            let case = format!("{} after a move to {target}", row.case);
            let endpoint = if row.on_promoted { target } else { REGION };
            let (status, payload) = call(
                &router,
                endpoint,
                row.operation,
                &json!({"KeyId": key_id.clone()}),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (200, ""),
                "case {case}: a reported promotion is a completed one, so {} at the {endpoint} \
                 endpoint must be served, got HTTP {status} with {payload}",
                row.operation
            );
            assert!(
                !error_message(&payload).contains("is updating"),
                "case {case}: no call is refused a move that is over, got {payload}"
            );

            let metadata = describe_in(&router, endpoint, &key_id).await;
            assert_eq!(
                (
                    metadata["KeyState"].clone(),
                    metadata["Enabled"].clone(),
                    metadata["MultiRegionConfiguration"]["PrimaryKey"]["Region"].clone()
                ),
                (
                    json!(row.settles_in),
                    json!(row.settles_in == "Enabled"),
                    json!(target)
                ),
                "case {case}: {} must leave the key in {}, with the designation where the \
                 promotion put it, got {metadata}",
                row.operation,
                row.settles_in
            );
        }

        // The two operations of the SET a move in flight refuses, taken once it is
        // reported: the promoted key replicates again, and the designation moves on.
        // A Region holding no key of the set is what a replicate adds a member in.
        // Both are asked for at the endpoint of the key that now holds the set's
        // primary designation, which is the key either of them is about.
        let elsewhere = TARGETS
            .iter()
            .find(|region| **region != target)
            .copied()
            .unwrap_or(REGION);
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
            "case a move to {target}: the promoted key is the set's primary key, so it \
             replicates, got {replicated}"
        );
        ok_in(
            &router,
            target,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": REGION}),
        )
        .await;
        let metadata = await_promotion_reported(&router, target, REGION, &key_id).await;
        assert_eq!(
            metadata["MultiRegionConfiguration"]["PrimaryKey"]["Region"],
            json!(REGION),
            "case a move to {target}: the set promotes again once the first move is \
             reported, got {metadata}"
        );
    }
}

/// The designation and the transient key state of the move that carries it are ONE
/// change: no observation of the set has the designation moved while a key of it is
/// still `Updating`, and none has the move over while the designation has not arrived.
/// So the sequence a client watching a failover runs — promote, poll, then take the
/// former primary key out of service — is never refused the state of a move in flight.
#[tokio::test]
async fn the_designation_arrives_when_the_transient_state_ends() {
    let router = super::harness::router();

    for target in TARGETS {
        let case = format!("a failover to {target}");
        let key_id = text(
            &case,
            &create_key(&router, &json!({"MultiRegion": true})).await,
            "KeyId",
        );
        replicate_into(&router, &case, &key_id, &[target]).await;

        ok_in(
            &router,
            REGION,
            "UpdatePrimaryRegion",
            &json!({"KeyId": key_id, "PrimaryRegion": target}),
        )
        .await;

        // Both keys the designation moves between, polled through their own endpoints
        // until the move is reported: the state and the designation agree at every
        // observation of either of them.
        let deadline = Instant::now() + PRIMARY_REGION_UPDATE_SPAN * 20;
        loop {
            let mut reported = 0;
            for (region, expected_type) in [(target, "PRIMARY"), (REGION, "REPLICA")] {
                let metadata = describe_in(&router, region, &key_id).await;
                let key_type = metadata["MultiRegionConfiguration"]["MultiRegionKeyType"].clone();
                let primary = metadata["MultiRegionConfiguration"]["PrimaryKey"]["Region"].clone();
                if metadata["KeyState"] == json!("Updating") {
                    assert_eq!(
                        primary,
                        json!(REGION),
                        "case {case}: while a key of the set is Updating the move has not \
                         happened yet, so the designation must still be reported where it \
                         was, got {metadata} at the {region} endpoint"
                    );
                } else if key_type == json!(expected_type) {
                    reported += 1;
                }
            }
            if reported == 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: both keys must report the moved designation once the move is \
                 over"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // What the client does next: take the former primary key out of service. It is
        // served, and `DescribeKey` reports the key state it left.
        let (status, payload) = call(&router, target, "DisableKey", &json!({"KeyId": key_id})).await;
        assert_eq!(
            (status, error_message(&payload).as_str()),
            (200, ""),
            "case {case}: a promotion that has been reported leaves no state-gated call \
             refused, got HTTP {status} with {payload}"
        );
        let (key_type, primary_region, _) = topology_at(&router, &case, target, &key_id).await;
        let metadata = describe_in(&router, target, &key_id).await;
        assert_eq!(
            (
                metadata["KeyState"].clone(),
                metadata["Enabled"].clone(),
                key_type.as_str(),
                primary_region.as_str()
            ),
            (json!("Disabled"), json!(false), "PRIMARY", target),
            "case {case}: the promoted key is the set's primary key and is now disabled, \
             got {metadata}"
        );
    }
}
