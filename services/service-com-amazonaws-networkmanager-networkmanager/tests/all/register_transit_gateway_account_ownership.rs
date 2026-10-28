// SPDX-License-Identifier: CC0-1.0

//! `RegisterTransitGateway` against the ownership constraint its own contract states,
//! driven over HTTP.
//!
//! The rule under test: a transit gateway is registered with a global network only when the
//! gateway's ARN names the SAME account the global network belongs to — "it must be owned by
//! the same Amazon Web Services account that owns the global network" (`smithy_query
//! query="doc" shape="RegisterTransitGateway"`). Any other account, however the segment is
//! spelled, is refused on the member that carried the ARN and stores nothing; the Region and
//! the partition are not narrowed by that rule, since the gateway "can be in any of the
//! supported Amazon Web Services Regions".
//!
//! Which account owns the global network is read from the global network's OWN ARN here, not
//! assumed: every row's accepted ARN is built from that account and every refused one from a
//! value derived from it, so the table spans the ways an account segment can differ (another
//! account entirely, one digit off, a digit too many, no account at all) across partitions,
//! Regions and gateway id widths.
//!
//! The follow-on the ownership rule guards is covered too: a Connect peer is only associable
//! while the global network holds a registration of a gateway in the peer's Region AND
//! account, so with no foreign-account registration possible, a foreign-account peer has no
//! gateway left to stand on.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: a registration is only ever read back through
//! `GetTransitGatewayRegistrations`.

use serde_json::json;

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::transit_gateway_connect_peer_association_lifecycle::{
    associate, connect_peer_arn, register_gateway,
};
use crate::transit_gateway_peering::transit_gateway_arn;
use crate::transit_gateway_registration_lifecycle::{
    arns_of, get_registrations, register, ARN_MEMBER,
};

/// The `Reason` a `ValidationException` of this service carries when one input member is at
/// fault (`smithy_query shape=ValidationExceptionReason` lists `CannotParse`,
/// `FieldValidationFailed`, `Other`, `UnknownOperation`).
const FIELD_AT_FAULT: &str = "FieldValidationFailed";

/// Create a global network and answer its id together with the account its own ARN names —
/// the account that owns it, which is the one a registration is judged against.
async fn global_network_and_owner(emulator: &Emulator, description: &str) -> (String, String) {
    let (status, body) = emulator
        .post("/global-networks", json!({ "Description": description }))
        .await;
    assert_eq!(
        status, 200,
        "CreateGlobalNetwork({description:?}): expected 200, got {status} with {body}"
    );
    let network = &body["GlobalNetwork"];
    let id = network["GlobalNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGlobalNetwork({description:?}): no id in {body}"))
        .to_string();
    let arn = network["GlobalNetworkArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGlobalNetwork({description:?}): no ARN in {body}"));
    // The account is an ARN's fifth colon-separated field.
    let owner = arn
        .split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("CreateGlobalNetwork({description:?}): no account in {arn}"))
        .to_string();
    assert!(
        !owner.is_empty(),
        "CreateGlobalNetwork({description:?}): the global network's ARN must name its account, got {arn}"
    );
    (id, owner)
}

/// `account` with its last digit replaced by a different one: an account id that differs
/// from the owner's in exactly one place.
fn one_digit_off(account: &str) -> String {
    let mut digits: Vec<char> = account.chars().collect();
    if let Some(last) = digits.last_mut() {
        *last = if *last == '9' { '8' } else { '9' };
    }
    digits.into_iter().collect()
}

/// One row: the ARN shape under test, and the account segment that makes it foreign.
struct Row {
    name: &'static str,
    partition: &'static str,
    region: &'static str,
    /// How many hex digits the gateway id carries; the published bounds of an EC2 resource
    /// id are 8 and 17 (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$`
    /// for `VpcArn`, `smithy_query shape=VpcArn`).
    digits: usize,
    /// The account the refused ARN carries. Every row's accepted twin carries the owner's.
    foreign_account: String,
}

/// The rows: the ways an account segment can differ from the owner's, each on a different
/// partition, Region and gateway id width, so nothing but the account decides the answer.
fn rows(owner: &str) -> Vec<Row> {
    vec![
        Row {
            name: "another account entirely, the shortest published id",
            partition: "aws",
            region: "us-west-2",
            digits: 8,
            foreign_account: "210987654321".to_string(),
        },
        Row {
            name: "an account one digit off the owner's, the longest published id",
            partition: "aws",
            region: "eu-central-1",
            digits: 17,
            foreign_account: one_digit_off(owner),
        },
        Row {
            name: "the owner's digits with one digit too many, another partition",
            partition: "aws-cn",
            region: "cn-north-1",
            digits: 12,
            foreign_account: format!("{owner}0"),
        },
        Row {
            name: "no account segment at all, the GovCloud partition",
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            digits: 16,
            foreign_account: String::new(),
        },
        Row {
            name: "another account entirely, a third Region",
            partition: "aws",
            region: "ap-southeast-2",
            digits: 10,
            foreign_account: "111122223333".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_transit_gateway_of_any_other_account_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let (network, owner) = global_network_and_owner(&emulator, "gateway ownership").await;
    let mut accepted: Vec<String> = Vec::new();

    for (index, row) in rows(&owner).iter().enumerate() {
        let case = row.name;
        let foreign = transit_gateway_arn(
            row.partition,
            row.region,
            &row.foreign_account,
            index + 1,
            row.digits,
        );
        let (status, refused) = register(&emulator, &network, &foreign).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {foreign}, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Reason"],
            json!(FIELD_AT_FAULT),
            "case {case:?}: the refusal must blame the input member, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(ARN_MEMBER),
            "case {case:?}: the refusal must name the member at fault, got {refused}"
        );
        let explanation = refused["Fields"][0]["Message"].as_str().unwrap_or("");
        assert!(
            explanation.contains(&owner) && explanation.contains(&network),
            "case {case:?}: the refusal must name the owning account {owner} and global network {network}, got {refused}"
        );

        // The refusal stored nothing: the listing still holds exactly what earlier rows had
        // accepted, and nothing of the refused gateway.
        let (status, page) = get_registrations(&emulator, &network, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransitGatewayRegistrations expected 200, got {status} with {page}"
        );
        let mut expected = accepted.clone();
        expected.sort();
        assert_eq!(
            arns_of(&page),
            expected,
            "case {case:?}: a refused register must store nothing, got {page}"
        );

        // The SAME ARN shape with the owner's account is accepted and round-trips: the
        // account segment is the only thing the refusal was about.
        let owned = transit_gateway_arn(row.partition, row.region, &owner, index + 1, row.digits);
        let (status, created) = register(&emulator, &network, &owned).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {owned}, got {status} with {created}"
        );
        assert_eq!(
            created["TransitGatewayRegistration"]["TransitGatewayArn"],
            json!(owned),
            "case {case:?}: the registration must name the gateway asked for, got {created}"
        );
        let (status, page) = get_registrations(&emulator, &network, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransitGatewayRegistrations expected 200, got {status} with {page}"
        );
        assert!(
            arns_of(&page).contains(&owned),
            "case {case:?}: the reader must list the stored registration, got {page}"
        );
        accepted.push(owned);
    }

    // Only the accepted twins are stored, one per row: every refusal left the listing alone.
    let (status, page) = get_registrations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case final read: GetTransitGatewayRegistrations expected 200, got {status} with {page}"
    );
    accepted.sort();
    assert_eq!(
        arns_of(&page),
        accepted,
        "case final read: expected exactly the accepted registrations, got {page}"
    );
}

#[tokio::test]
async fn a_connect_peer_of_another_account_has_no_registered_gateway_to_stand_on() {
    let emulator = Emulator::new();
    let (network, owner) = global_network_and_owner(&emulator, "peer ownership").await;
    let device = create_device_ok(&emulator, &network, json!({ "Description": "peers" })).await;
    let region = "us-west-2";

    // The only gateway this global network can hold a registration of is one of the owning
    // account, which is what the associate's precondition is then judged against.
    register_gateway(
        &emulator,
        &network,
        &transit_gateway_arn("aws", region, &owner, 1, 17),
    )
    .await;

    for (case, account) in [
        ("another account entirely", "210987654321".to_string()),
        ("an account one digit off the owner's", one_digit_off(&owner)),
    ] {
        let peer = connect_peer_arn("aws", region, &account, 1, 17);
        let (status, refused) = associate(
            &emulator,
            &network,
            json!({ "TransitGatewayConnectPeerArn": peer, "DeviceId": device }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {peer}, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
    }

    // A peer of the owning account sits on the registered gateway, so the same call holds.
    let peer = connect_peer_arn("aws", region, &owner, 2, 17);
    let (status, made) = associate(
        &emulator,
        &network,
        json!({ "TransitGatewayConnectPeerArn": peer, "DeviceId": device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the owning account: expected 200 for {peer}, got {status} with {made}"
    );
    assert_eq!(
        made["TransitGatewayConnectPeerAssociation"]["TransitGatewayConnectPeerArn"],
        json!(peer),
        "case the owning account: the tie must name the peer asked for, got {made}"
    );
}
