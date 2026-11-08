// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the preconditions a `SubnetIds` list has to meet before anything
//! about the Availability Zones it covers is counted: the subnets a DB subnet group is made
//! of have to be subnets the ACCOUNT HOLDS, in ONE VPC — the group's own.
//!
//! RULE: a `SubnetIds` list naming an identifier the service that OWNS subnets answered it
//! holds no subnet for is refused — HTTP 400 with the query protocol's `InvalidParameterValue`
//! and the sentence naming exactly those identifiers — whichever operation carries the list,
//! and BEFORE the zones the list covers are counted, since an identifier that names no subnet
//! contributes no zone to count. A subnet of a VPC other than the one the group a
//! modification addresses is in is not a subnet that group can be made of, so it is refused
//! the same way; and a list whose subnets lie in several VPCs makes up no group at all, which
//! is `InvalidSubnet`'s own documented condition.
//!
//! The rows span what the rule generalises over: where in the list the unusable identifier
//! sits, how many there are, which published identifier form each takes (`subnet-` with 8 and
//! with 17 hexadecimal characters), whether one is repeated, and which of the two operations
//! that take such a list carries it. A refused modification is asserted to leave the stored
//! group exactly as it was.
//!
//! The counterpart invariant is asserted too: the refusal is grounded in what EC2 ANSWERED,
//! so a deployment where the owner of subnets cannot be asked at all refuses nothing — one
//! case drives Amazon RDS with no EC2 beside it.
//!
//! Amazon EC2 is mounted beside Amazon RDS on ONE engine and driven through the AWS SDK over
//! the router's own in-process callback, the very seam the resolution under test uses. Every
//! identifier that resolves below is one EC2 MINTED; the ones that do not are well-formed
//! identifiers EC2 minted no subnet for.

use crate::db_instances::{call_in, emulator, expect_error, expect_ok, text, texts, REGION};
use crate::db_subnet_group_ec2_zones::{subnet_members, Network};

/// A well-formed EC2 subnet identifier in the 17-hexadecimal-character form that the account
/// holds no subnet for: this suite's network mints none with it.
const UNHELD_LONG: &str = "subnet-00000000deadbeef0";

/// A second one, so a list can name more than one identifier the account does not hold.
const UNHELD_SECOND: &str = "subnet-00000000deadbeef1";

/// The other published identifier form — `subnet-` and 8 hexadecimal characters — also
/// naming no subnet of the account.
const UNHELD_SHORT: &str = "subnet-0000beef";

/// The sentence Amazon RDS refuses a `SubnetIds` list it cannot make a DB subnet group of
/// with: the identifiers it could not use inside the brackets, comma-separated and in the
/// order the request named them, and the fixed text around them.
///
/// Spelled out here rather than read from the code under test, so the assertion is made
/// against the wording and not against the emulator's own helper.
///
/// `pub(crate)` so the sibling suite over the zones a group reports asserts the same refusal
/// with the same sentence rather than spelling a second copy of it.
pub(crate) fn invalid_subnets_message(subnet_ids: &[&str]) -> String {
    format!(
        "Some input subnets in :[{}] are invalid.",
        subnet_ids.join(", ")
    )
}

/// The `SubnetIdentifier`s one `DBSubnetGroup` envelope reports, in the order it reports
/// them: what the group is holding, read back off the wire.
fn subnet_ids_of(group: &str) -> Vec<String> {
    texts(group, "SubnetIdentifier")
}

/// One row of the table: the case, the list the request carries, and the identifiers among
/// them the refusal must name — in the order the request named them, without repetition.
struct Row {
    case: &'static str,
    subnets: Vec<String>,
    invalid: Vec<&'static str>,
}

/// The rows, built against a network EC2 minted: `held` are identifiers the account has.
fn rows(held: &[String; 2]) -> Vec<Row> {
    vec![
        Row {
            case: "an identifier the account holds no subnet for, named first",
            subnets: vec![UNHELD_LONG.to_string(), held[1].clone()],
            invalid: vec![UNHELD_LONG],
        },
        Row {
            case: "the same identifier after a subnet the account does hold",
            subnets: vec![held[0].clone(), UNHELD_LONG.to_string()],
            invalid: vec![UNHELD_LONG],
        },
        Row {
            case: "two identifiers the account holds no subnet for, in both published forms",
            subnets: vec![UNHELD_LONG.to_string(), UNHELD_SHORT.to_string()],
            invalid: vec![UNHELD_LONG, UNHELD_SHORT],
        },
        Row {
            case: "three identifiers, two of them unheld, around a subnet it does hold",
            subnets: vec![
                UNHELD_SECOND.to_string(),
                held[0].clone(),
                UNHELD_SHORT.to_string(),
            ],
            invalid: vec![UNHELD_SECOND, UNHELD_SHORT],
        },
        Row {
            case: "the same unheld identifier named twice",
            subnets: vec![
                UNHELD_LONG.to_string(),
                held[1].clone(),
                UNHELD_LONG.to_string(),
            ],
            invalid: vec![UNHELD_LONG],
        },
        Row {
            // ONE identifier, which covers no zone at all: the list is refused for the
            // identifier that names no subnet, not for the coverage it fails to add up to,
            // which is what fixes the order of the two gates.
            case: "a single unheld identifier, which covers no Availability Zone either",
            subnets: vec![UNHELD_LONG.to_string()],
            invalid: vec![UNHELD_LONG],
        },
    ]
}

/// Assert one answer is the refusal the rule prescribes, naming `invalid`.
fn assert_refused(case: &str, label: &str, response: (u16, String), invalid: &[&str]) {
    let case = format!("{case} ({label})");
    let body = expect_error(&case, response, 400, "InvalidParameterValue");
    let message = text(&body, "Message");
    let expected = invalid_subnets_message(invalid);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// RULE: every operation that takes a `SubnetIds` list refuses one naming an identifier the
/// account holds no subnet for, with `InvalidParameterValue` naming exactly those
/// identifiers — and a refused modification leaves the stored group as it was.
#[tokio::test]
async fn a_subnet_the_account_does_not_hold_is_refused_on_create_and_on_modify() {
    let network = Network::build().await;
    let held = [
        network.first_zone[0].clone(),
        network.second_zone.clone(),
    ];

    // The control the modification rows run against: a group over subnets the account DOES
    // hold, covering the two zones a group must. It is served, so the gate below refuses a
    // list for the identifiers it names and not for naming any list at all.
    const BASE: &str = "emu-subnet-held";
    let created = expect_ok(
        "a list of subnets the account holds, covering two zones",
        network.account.create_group(BASE, &held).await,
    );
    assert_eq!(
        subnet_ids_of(&created),
        held.to_vec(),
        "case \"a list of subnets the account holds\": the group must hold the subnets the \
         request named, got {created}"
    );

    for (index, row) in rows(&held).into_iter().enumerate() {
        let name = format!("emu-subnet-unheld-{index}");
        assert_refused(
            row.case,
            "CreateDBSubnetGroup",
            network.account.create_group(&name, &row.subnets).await,
            &row.invalid,
        );
        // Nothing was stored for the refused name: the group does not exist.
        let (status, body) = network.account.describe_group(&name).await;
        assert_eq!(
            status, 404,
            "case {:?}: the refused group must not exist, got {status} with {body}",
            row.case
        );

        assert_refused(
            row.case,
            "ModifyDBSubnetGroup",
            network.account.modify_group(BASE, &row.subnets).await,
            &row.invalid,
        );
        // …and the group the refused modification addressed still holds what it held.
        let read_back = expect_ok(row.case, network.account.describe_group(BASE).await);
        assert_eq!(
            subnet_ids_of(&read_back),
            held.to_vec(),
            "case {:?}: a refused modification must leave the group's subnets alone, got \
             {read_back}",
            row.case
        );
    }

    // A modification to another list of subnets the account holds is still served, so the
    // gate is about the identifiers and not about modifying at all.
    let other = [
        network.second_zone.clone(),
        network.first_zone[1].clone(),
    ];
    let modified = expect_ok(
        "a modification to another list of subnets the account holds",
        network.account.modify_group(BASE, &other).await,
    );
    assert_eq!(
        subnet_ids_of(&modified),
        other.to_vec(),
        "case \"a modification to another list of subnets the account holds\": expected \
         {other:?}, got {modified}"
    );
}

/// The sentence a `SubnetIds` list whose subnets lie in more than one VPC is refused with:
/// the condition the `InvalidSubnet` fault states itself.
///
/// Source: `smithy_query(query="doc",shape="InvalidSubnet")` on this service's model.
const SUBNETS_NOT_IN_ONE_VPC: &str = "The requested subnet is invalid, or multiple subnets \
                                      were requested that are not all in a common VPC.";

/// RULE: a DB subnet group is a set of subnets of ONE VPC — the VPC it reports — so a list
/// whose subnets lie in several makes up no group (`InvalidSubnet`), and a modification can
/// only name subnets of the VPC the group it addresses is in; one naming a subnet of another
/// VPC names a subnet this group cannot be made of, which is refused the way an identifier
/// the account holds no subnet for is.
#[tokio::test]
async fn the_subnets_of_a_group_are_subnets_of_one_vpc() {
    let network = Network::build().await;
    let held = [
        network.first_zone[0].clone(),
        network.second_zone.clone(),
    ];
    // A SECOND VPC of the same account, with a subnet in each of the two zones the group's
    // own subnets cover: every identifier below is one EC2 minted, and the account holds
    // every one of them — what makes them unusable here is the VPC they are in.
    let other_vpc = network.account.create_vpc("10.60.0.0/16").await;
    let other = [
        network
            .account
            .create_subnet(&other_vpc, "10.60.1.0/24", &network.zones[0])
            .await,
        network
            .account
            .create_subnet(&other_vpc, "10.60.2.0/24", &network.zones[1])
            .await,
    ];

    // A create whose subnets straddle the two VPCs: the list makes up no group at all, and
    // the modelled fault for that is `InvalidSubnet`.
    let straddling: [(&str, Vec<String>); 2] = [
        (
            "one subnet of each VPC",
            vec![held[0].clone(), other[1].clone()],
        ),
        (
            "two of one VPC and one of the other",
            vec![held[0].clone(), held[1].clone(), other[0].clone()],
        ),
    ];
    for (index, (case, subnets)) in straddling.into_iter().enumerate() {
        let name = format!("emu-subnet-vpcs-{index}");
        let body = expect_error(
            case,
            network.account.create_group(&name, &subnets).await,
            400,
            "InvalidSubnet",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, SUBNETS_NOT_IN_ONE_VPC,
            "case {case:?}: expected {SUBNETS_NOT_IN_ONE_VPC:?}, got {message:?}"
        );
    }

    // The group a modification addresses is in ONE VPC, and the rows differ along how many
    // of the named subnets are outside it and where they sit in the list.
    const BASE: &str = "emu-subnet-vpc-base";
    expect_ok(
        "a group over subnets of one VPC",
        network.account.create_group(BASE, &held).await,
    );
    let rows: [(&str, Vec<String>, Vec<&str>); 3] = [
        (
            "a modification to the subnets of the other VPC",
            vec![other[0].clone(), other[1].clone()],
            vec![other[0].as_str(), other[1].as_str()],
        ),
        (
            "a modification keeping one subnet and taking one from the other VPC",
            vec![held[0].clone(), other[1].clone()],
            vec![other[1].as_str()],
        ),
        (
            "the same list in the other order",
            vec![other[0].clone(), held[1].clone()],
            vec![other[0].as_str()],
        ),
    ];
    for (case, subnets, invalid) in rows {
        assert_refused(
            case,
            "ModifyDBSubnetGroup",
            network.account.modify_group(BASE, &subnets).await,
            &invalid,
        );
        let read_back = expect_ok(case, network.account.describe_group(BASE).await);
        assert_eq!(
            subnet_ids_of(&read_back),
            held.to_vec(),
            "case {case:?}: a refused modification must leave the group's subnets alone, got \
             {read_back}"
        );
    }
}

/// RULE: the refusal is grounded in what the owner of subnets ANSWERED, so where it cannot be
/// asked at all nothing is refused on that silence: the identifiers are unknown rather than
/// absent, and the group is created.
#[tokio::test]
async fn silence_from_the_owner_of_subnets_refuses_nothing() {
    // Amazon RDS with no Amazon EC2 beside it: the hop that would resolve the identifiers
    // reaches no service, so this deployment knows nothing about any subnet.
    let router = emulator();
    let rows: [(&str, Vec<String>); 3] = [
        (
            "two identifiers in the 17-character form",
            vec![UNHELD_LONG.to_string(), UNHELD_SECOND.to_string()],
        ),
        (
            "one of each published identifier form",
            vec![UNHELD_LONG.to_string(), UNHELD_SHORT.to_string()],
        ),
        (
            "three identifiers, one of them repeated",
            vec![
                UNHELD_LONG.to_string(),
                UNHELD_SECOND.to_string(),
                UNHELD_LONG.to_string(),
            ],
        ),
    ];
    for (index, (case, subnets)) in rows.into_iter().enumerate() {
        let name = format!("emu-subnet-unasked-{index}");
        let mut params: Vec<(String, String)> = vec![
            ("DBSubnetGroupName".to_string(), name.clone()),
            (
                "DBSubnetGroupDescription".to_string(),
                "no owner of subnets to ask".to_string(),
            ),
        ];
        params.extend(subnet_members(&subnets));
        let created = expect_ok(
            case,
            call_in(&router, REGION, "CreateDBSubnetGroup", &params).await,
        );
        assert_eq!(
            subnet_ids_of(&created),
            subnets,
            "case {case:?}: expected the group to hold {subnets:?}, got {created}"
        );
    }
}
