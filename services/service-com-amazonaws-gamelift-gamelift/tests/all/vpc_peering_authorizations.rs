// SPDX-License-Identifier: CC0-1.0

//! The VPC peering authorization contract, over the wire:
//! `CreateVpcPeeringAuthorization` issues an authorization for the pair the request names
//! and persists it, `DescribeVpcPeeringAuthorizations` reports every stored authorization
//! exactly once, re-issuing for a pair that already holds one replaces it, and
//! `DeleteVpcPeeringAuthorization` cancels the stored authorization — a pair holding none
//! is refused with `NotFoundException`.
//!
//! The harness (`GameLift`) is the one `hosting_loop` already drives the production
//! dispatch path with.

use serde_json::{json, Value};

use crate::hosting_loop::GameLift;

/// How long an authorization stays valid for, in seconds:
/// `VpcPeeringAuthorization$ExpirationTime`'s own doc — "Time stamp indicating when this
/// authorization expires (24 hours after issuance)".
const VALIDITY_SECONDS: f64 = 24.0 * 60.0 * 60.0;

/// The lowest and highest length `GameLiftAwsAccountId` and `PeerVpcId` may carry, from
/// `NonZeroAndMaxString` (`smithy_query shape=NonZeroAndMaxString`: len 1..1024), which
/// both members target.
const VALUE_LENGTH: (usize, usize) = (1, 1024);

/// One row of the table below: a pair an authorization may be issued for.
struct Case {
    label: &'static str,
    game_lift_account_id: String,
    peer_vpc_id: String,
}

/// The pairs the rule is asserted over. The rows differ along what the rule generalises
/// over: the LENGTH of each value at both modelled bounds of `NonZeroAndMaxString`
/// (len 1..1024), the form of the VPC id, and whether the Amazon GameLift Servers account
/// is another account or the caller's own — "you can peer with VPCs that are owned by any
/// Amazon Web Services account you have access to, including the account that you use to
/// manage your Amazon GameLift Servers fleets".
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "an account id and a VPC id of the documented forms",
            game_lift_account_id: "111122223333".to_string(),
            peer_vpc_id: "vpc-1a2b3c4d".to_string(),
        },
        Case {
            label: "a longer VPC id, as Amazon EC2 also mints them",
            game_lift_account_id: "444455556666".to_string(),
            peer_vpc_id: "vpc-0abc123def4567890".to_string(),
        },
        Case {
            label: "the Amazon GameLift Servers account is the caller's own account",
            game_lift_account_id: engine::emulator_account_id().to_string(),
            peer_vpc_id: "vpc-99887766".to_string(),
        },
        Case {
            label: "the shortest values the model allows (1 character each)",
            game_lift_account_id: "7".repeat(VALUE_LENGTH.0),
            peer_vpc_id: "v".repeat(VALUE_LENGTH.0),
        },
        Case {
            label: "the longest values the model allows (1024 characters each)",
            game_lift_account_id: "8".repeat(VALUE_LENGTH.1),
            peer_vpc_id: format!("vpc-{}", "a".repeat(VALUE_LENGTH.1 - "vpc-".len())),
        },
    ]
}

/// Issue an authorization for `case` and return the `VpcPeeringAuthorization` reported.
async fn authorize(gamelift: &GameLift, case: &Case) -> Value {
    let created = gamelift
        .ok(
            "CreateVpcPeeringAuthorization",
            json!({
                "GameLiftAwsAccountId": case.game_lift_account_id,
                "PeerVpcId": case.peer_vpc_id,
            }),
        )
        .await;
    created["VpcPeeringAuthorization"].clone()
}

/// Every authorization `DescribeVpcPeeringAuthorizations` reports, in the order it
/// reports them.
async fn described(gamelift: &GameLift) -> Vec<Value> {
    let listed = gamelift
        .ok("DescribeVpcPeeringAuthorizations", json!({}))
        .await;
    listed["VpcPeeringAuthorizations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("DescribeVpcPeeringAuthorizations must report a list, got {listed}")
        })
}

/// The authorizations reported for `peer_vpc_id`.
fn for_vpc<'a>(reported: &'a [Value], peer_vpc_id: &str) -> Vec<&'a Value> {
    reported
        .iter()
        .filter(|authorization| authorization["PeerVpcId"] == json!(peer_vpc_id))
        .collect()
}

/// A timestamp member of a reported authorization, in Unix seconds — the awsJson wire
/// form of a modelled `Timestamp`.
fn instant(authorization: &Value, member: &str) -> f64 {
    authorization[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be a Unix-time number, got {authorization}"))
}

/// An issued authorization carries the pair it was issued for, the caller's account as
/// the peer VPC account and an expiry 24 hours after its issue time, and every later read
/// of it reports the stored record.
#[tokio::test]
async fn an_issued_authorization_is_reported_as_stored() {
    let gamelift = GameLift::new();

    for case in cases() {
        let issued = authorize(&gamelift, &case).await;

        assert_eq!(
            issued["GameLiftAwsAccountId"],
            json!(case.game_lift_account_id),
            "case {:?}: the authorization must name the account the request did, got {}",
            case.label,
            issued["GameLiftAwsAccountId"]
        );
        assert_eq!(
            issued["PeerVpcId"],
            json!(case.peer_vpc_id),
            "case {:?}: the authorization must name the VPC the request did, got {}",
            case.label,
            issued["PeerVpcId"]
        );
        // "Call this operation from the Amazon Web Services account with the VPC that you
        // want to peer": the peer VPC account is the account the call was made from,
        // which every emulated response presents as `engine::emulator_account_id`.
        assert_eq!(
            issued["PeerVpcAwsAccountId"],
            json!(engine::emulator_account_id()),
            "case {:?}: the peer VPC account must be the calling account, got {}",
            case.label,
            issued["PeerVpcAwsAccountId"]
        );
        let created_at = instant(&issued, "CreationTime");
        let expires_at = instant(&issued, "ExpirationTime");
        assert!(
            (expires_at - created_at - VALIDITY_SECONDS).abs() < 1.0,
            "case {:?}: the authorization must expire 24 hours after issuance, got \
             CreationTime {created_at} and ExpirationTime {expires_at}",
            case.label
        );

        // The round trip: the same record, read back through the reader.
        let reported = described(&gamelift).await;
        let mine = for_vpc(&reported, &case.peer_vpc_id);
        assert_eq!(
            mine.len(),
            1,
            "case {:?}: the authorization must be reported exactly once, got {reported:?}",
            case.label
        );
        assert_eq!(
            *mine[0], issued,
            "case {:?}: the read-back authorization must be the one the create reported, got {}",
            case.label, mine[0]
        );
    }

    // Every case's authorization is still reported, so one create replaced nothing it was
    // not issued for, and the collection is ordered oldest first.
    let reported = described(&gamelift).await;
    assert_eq!(
        reported.len(),
        cases().len(),
        "every issued authorization must be reported exactly once, got {reported:?}"
    );
    let issue_times: Vec<f64> = reported
        .iter()
        .map(|authorization| instant(authorization, "CreationTime"))
        .collect();
    assert!(
        issue_times.windows(2).all(|pair| pair[0] <= pair[1]),
        "the authorizations must be reported oldest first, got issue times {issue_times:?}"
    );

    // A read changes nothing a later read sees.
    assert_eq!(
        described(&gamelift).await,
        reported,
        "two reads of the authorizations must agree, got {reported:?}"
    );
}

/// Re-issuing an authorization for a pair that already holds one replaces it: the pair
/// still holds exactly ONE authorization, valid from the later call.
#[tokio::test]
async fn re_issuing_an_authorization_replaces_the_one_the_pair_holds() {
    let gamelift = GameLift::new();

    for case in cases() {
        let first = authorize(&gamelift, &case).await;
        // The clock must move between the two calls, or "valid from the later call" is
        // not observable.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let second = authorize(&gamelift, &case).await;

        assert!(
            instant(&second, "CreationTime") >= instant(&first, "CreationTime"),
            "case {:?}: the re-issued authorization must be issued no earlier than the \
             first, got {} after {}",
            case.label,
            instant(&second, "CreationTime"),
            instant(&first, "CreationTime")
        );
        assert!(
            (instant(&second, "ExpirationTime")
                - instant(&second, "CreationTime")
                - VALIDITY_SECONDS)
                .abs()
                < 1.0,
            "case {:?}: the re-issued authorization must expire 24 hours after its own \
             issuance, got {second}",
            case.label
        );

        let reported = described(&gamelift).await;
        let mine = for_vpc(&reported, &case.peer_vpc_id);
        assert_eq!(
            mine.len(),
            1,
            "case {:?}: a pair holds exactly one authorization, got {mine:?}",
            case.label
        );
        assert_eq!(
            *mine[0], second,
            "case {:?}: the pair must hold the authorization the second call issued, got {}",
            case.label, mine[0]
        );
    }
}

/// A canceled authorization leaves state: no later read reports it, and cancelling a pair
/// that holds none is refused with `NotFoundException` — which is also what a pair whose
/// authorization was never issued answers.
#[tokio::test]
async fn a_canceled_authorization_is_gone_and_cancelling_an_unheld_pair_is_refused() {
    let gamelift = GameLift::new();

    for case in cases() {
        // Nothing has been issued for this pair yet.
        let cancel = json!({
            "GameLiftAwsAccountId": case.game_lift_account_id,
            "PeerVpcId": case.peer_vpc_id,
        });
        let (status, code) = gamelift
            .err("DeleteVpcPeeringAuthorization", cancel.clone())
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {:?}: cancelling an authorization no pair holds must be refused with a \
             400 NotFoundException, got {status} {code}",
            case.label
        );

        authorize(&gamelift, &case).await;
        gamelift
            .ok("DeleteVpcPeeringAuthorization", cancel.clone())
            .await;
        let reported = described(&gamelift).await;
        assert!(
            for_vpc(&reported, &case.peer_vpc_id).is_empty(),
            "case {:?}: a canceled authorization must not be reported, got {reported:?}",
            case.label
        );

        // The cancel is not idempotent: the pair now holds nothing.
        let (status, code) = gamelift.err("DeleteVpcPeeringAuthorization", cancel).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {:?}: cancelling an already-canceled authorization must be refused with \
             a 400 NotFoundException, got {status} {code}",
            case.label
        );
    }

    assert!(
        described(&gamelift).await.is_empty(),
        "every issued authorization was canceled, so none may be reported"
    );
}

/// Cancelling an authorization touches only the pair the request names: the other pairs'
/// authorizations stay exactly as they were.
#[tokio::test]
async fn cancelling_one_authorization_leaves_the_others_untouched() {
    let gamelift = GameLift::new();

    let cases = cases();
    let mut issued: Vec<(String, Value)> = Vec::new();
    for case in &cases {
        issued.push((case.peer_vpc_id.clone(), authorize(&gamelift, case).await));
    }

    let canceled = &cases[0];
    gamelift
        .ok(
            "DeleteVpcPeeringAuthorization",
            json!({
                "GameLiftAwsAccountId": canceled.game_lift_account_id,
                "PeerVpcId": canceled.peer_vpc_id,
            }),
        )
        .await;

    let reported = described(&gamelift).await;
    for (peer_vpc_id, authorization) in issued.iter().skip(1) {
        let mine = for_vpc(&reported, peer_vpc_id);
        assert_eq!(
            mine.len(),
            1,
            "VPC {peer_vpc_id}: its authorization must survive another pair's cancel, got \
             {reported:?}"
        );
        assert_eq!(
            *mine[0], *authorization,
            "VPC {peer_vpc_id}: its authorization must be unchanged by another pair's \
             cancel, got {}",
            mine[0]
        );
    }
    assert!(
        for_vpc(&reported, &canceled.peer_vpc_id).is_empty(),
        "the canceled pair's authorization must be gone, got {reported:?}"
    );
}
