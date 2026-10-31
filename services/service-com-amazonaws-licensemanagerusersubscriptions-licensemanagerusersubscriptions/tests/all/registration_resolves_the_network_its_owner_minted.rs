// SPDX-License-Identifier: CC0-1.0

//! A registration's network is resolved against the service that OWNS it, through that
//! service's own API.
//!
//! `Settings` is "the registered identity provider's product related configuration settings
//! such as the subnets to provision VPC endpoints": `Subnets` is "the ID of one or more
//! subnets in which License Manager will create a VPC endpoint for products that require
//! connectivity to activation servers" and `SecurityGroupId` "a security group ID that
//! allows inbound TCP port 1688 communication between resources in your VPC and the VPC
//! endpoint for activation servers". So the rule under test is:
//!
//! * a network resource a registration names has to be one the ACCOUNT holds, as the
//!   service that owns subnets and security groups reports it at the time of the call — a
//!   request naming one it has no resource for is refused with the modelled
//!   `ValidationException` (HTTP 400) naming that resource, and persists nothing;
//! * a VPC endpoint lives in ONE VPC ("your VPC"), so the subnets and the security group of
//!   one registration are resources of one VPC;
//! * a registration whose network DID resolve is a healthy one: it settles into the
//!   terminal `REGISTERED` status the clock owes it, reporting no failure.
//!
//! What this suite adds over its siblings is WHERE the network comes from. The sibling
//! `settings_network_resolution` and `update_settings_network_propagation` suites write the
//! account's subnets and security groups straight into Amazon EC2's store, so that the ids
//! their rows name are fixed literals. Here every id a row names was MINTED by EC2 itself,
//! through `ec2:CreateVpc`/`CreateSubnet`/`CreateSecurityGroup` (and one removed again with
//! `ec2:DeleteSubnet`), driven over the AWS SDK on the in-process callback this service's own
//! resolution reaches EC2 through — the [`Fleet`] seam the instance suite already launches
//! instances with, on the same router and the same shared engine. So what is pinned is that
//! a client which builds its network with the EC2 API and then registers against it gets a
//! registration, that the same request without that network gets a refusal, and that the two
//! services therefore agree about what the account has.
//!
//! The rows differ along what the rule generalises over: how many subnets the request
//! carries, where in the list the unusable one sits, whether the resource was never created,
//! was created and then deleted, or merely belongs to another VPC, which member names it,
//! and which published id length it takes.

use serde_json::{json, Value};

// ONE wire harness per service, ONE EC2 client over the in-process callback, and ONE
// spelling of each refusal sentence: all of them come from the sibling suites rather than a
// second copy.
use crate::identity_provider_subscriptions::{
    list_providers, provider_union, register, Harness, PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use crate::instance_fleet_resolution::Fleet;
use crate::settings_network_resolution::{assert_refused, Expected, DIRECTORY};

/// The terminal status a registration whose network resolved settles into, and the
/// non-terminal one the call that starts it answers with. Spelled out here rather than read
/// from the production constants so a change to them fails a test instead of silently
/// changing what this suite accepts.
const REGISTERED: &str = "REGISTERED";
const REGISTERING: &str = "REGISTERING";

/// A VPC of the account, the subnets inside it and the one security group of it, every id as
/// Amazon EC2 minted it.
#[derive(Debug)]
struct Network {
    vpc_id: String,
    subnets: Vec<String>,
    security_group: String,
}

/// Build one VPC with `subnet_cidrs` subnets and a security group in it, through EC2's own
/// API, and answer the ids EC2 MINTED for them.
///
/// Minted, never chosen: these are the ids a real client would carry into a registration, so
/// nothing in this suite can pass off an id of its own as one the account holds.
async fn create_network(
    fleet: &Fleet,
    case: &str,
    vpc_cidr: &str,
    subnet_cidrs: &[&str],
    group_name: &str,
) -> Network {
    let created = fleet
        .ec2
        .create_vpc()
        .cidr_block(vpc_cidr)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: EC2 must create {vpc_cidr}: {error:?}"));
    let vpc_id = created
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .unwrap_or_else(|| panic!("case {case:?}: CreateVpc must report the VPC it minted"))
        .to_string();

    let mut subnets = Vec::with_capacity(subnet_cidrs.len());
    for cidr in subnet_cidrs {
        let created = fleet
            .ec2
            .create_subnet()
            .vpc_id(&vpc_id)
            .cidr_block(*cidr)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: EC2 must create subnet {cidr}: {error:?}")
            });
        subnets.push(
            created
                .subnet()
                .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: CreateSubnet must report the subnet it minted")
                })
                .to_string(),
        );
    }

    let created = fleet
        .ec2
        .create_security_group()
        .vpc_id(&vpc_id)
        .group_name(group_name)
        .description("inbound TCP 1688 to the activation-server endpoint")
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must create group {group_name}: {error:?}")
        });
    let security_group = created
        .group_id()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateSecurityGroup must report the group it minted")
        })
        .to_string();

    Network {
        vpc_id,
        subnets,
        security_group,
    }
}

/// Which subnet a row's `Settings$Subnets` names, and where it came from.
#[derive(Debug, Clone, Copy)]
enum Subnet {
    /// The subnet at this index of the FIRST VPC's minted subnets.
    Here(usize),
    /// The subnet at this index of the SECOND VPC's minted subnets.
    Elsewhere(usize),
    /// A subnet EC2 minted in the first VPC and then removed through `ec2:DeleteSubnet`.
    Deleted,
    /// A well-formed id the account has no subnet for.
    Absent(&'static str),
}

/// Which security group a row's `Settings$SecurityGroupId` names.
#[derive(Debug, Clone, Copy)]
enum Group {
    /// The group EC2 minted in the first VPC.
    Here,
    /// The group EC2 minted in the second VPC.
    Elsewhere,
    /// A well-formed id the account has no security group for.
    Absent(&'static str),
}

/// What the service must answer a row with, before the row's minted ids are substituted in.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// The refusal names the subnet at this position of the row's own `subnets`.
    NoSuchSubnet(usize),
    NoSuchGroup,
    /// The subnets disagree; the VPC the refusal names is the first one.
    SubnetsDisagree,
    /// The security group is of the second VPC while the subnets are of the first.
    GroupElsewhere,
}

/// Ids of the published form that no resource of this account takes.
///
/// `Subnets` matches `^subnet-[a-z0-9]{8,17}` and `SecurityGroupId`
/// `^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$`, so each of these passes the boundary validator and
/// reaches the handler's resolution; EC2 mints none of them, and neither is one of the ids
/// the shared harness seeded. Both published security-group lengths appear, and the
/// 17-character all-zero pair is the one this suite's issue was filed with.
const ABSENT_SUBNET: &str = "subnet-00000000000000000";
const SHORT_ABSENT_SUBNET: &str = "subnet-9999ffff";
const ABSENT_GROUP: &str = "sg-00000000000000000";
const SHORT_ABSENT_GROUP: &str = "sg-9999ffff";

/// One registration whose `Settings` name a network the account HAS, every id minted by EC2.
#[derive(Debug, Clone, Copy)]
struct AcceptedRow {
    case: &'static str,
    subnets: &'static [Subnet],
    group: Group,
}

const ACCEPTED_ROWS: &[AcceptedRow] = &[
    AcceptedRow {
        case: "one subnet and the security group EC2 minted",
        subnets: &[Subnet::Here(0)],
        group: Group::Here,
    },
    AcceptedRow {
        case: "two subnets EC2 minted in one VPC",
        subnets: &[Subnet::Here(0), Subnet::Here(1)],
        group: Group::Here,
    },
    AcceptedRow {
        case: "three subnets EC2 minted in one VPC",
        subnets: &[Subnet::Here(0), Subnet::Here(1), Subnet::Here(2)],
        group: Group::Here,
    },
    AcceptedRow {
        case: "the second VPC's own minted subnet and group",
        subnets: &[Subnet::Elsewhere(0)],
        group: Group::Elsewhere,
    },
];

/// One registration whose `Settings` name something the account cannot provision in, and the
/// resource the refusal has to name.
#[derive(Debug, Clone, Copy)]
struct RefusedRow {
    case: &'static str,
    subnets: &'static [Subnet],
    group: Group,
    outcome: Outcome,
}

const REFUSED_ROWS: &[RefusedRow] = &[
    RefusedRow {
        case: "well-formed ids EC2 minted nothing for",
        subnets: &[Subnet::Absent(ABSENT_SUBNET)],
        group: Group::Absent(ABSENT_GROUP),
        outcome: Outcome::NoSuchSubnet(0),
    },
    RefusedRow {
        case: "a shorter subnet id EC2 minted nothing for, after two minted ones",
        subnets: &[
            Subnet::Here(0),
            Subnet::Here(1),
            Subnet::Absent(SHORT_ABSENT_SUBNET),
        ],
        group: Group::Here,
        outcome: Outcome::NoSuchSubnet(2),
    },
    RefusedRow {
        case: "a subnet EC2 minted nothing for, before a minted one",
        subnets: &[Subnet::Absent(ABSENT_SUBNET), Subnet::Here(0)],
        group: Group::Here,
        outcome: Outcome::NoSuchSubnet(0),
    },
    RefusedRow {
        case: "a subnet EC2 minted and then deleted",
        subnets: &[Subnet::Deleted],
        group: Group::Here,
        outcome: Outcome::NoSuchSubnet(0),
    },
    RefusedRow {
        case: "a minted subnet and a deleted one together",
        subnets: &[Subnet::Here(0), Subnet::Deleted],
        group: Group::Here,
        outcome: Outcome::NoSuchSubnet(1),
    },
    RefusedRow {
        case: "minted subnets and an 8-character group EC2 minted nothing for",
        subnets: &[Subnet::Here(0)],
        group: Group::Absent(SHORT_ABSENT_GROUP),
        outcome: Outcome::NoSuchGroup,
    },
    RefusedRow {
        case: "minted subnets of two different VPCs",
        subnets: &[Subnet::Here(0), Subnet::Elsewhere(0)],
        group: Group::Here,
        outcome: Outcome::SubnetsDisagree,
    },
    RefusedRow {
        case: "a minted subnet of one VPC and the minted group of the other",
        subnets: &[Subnet::Here(0)],
        group: Group::Elsewhere,
        outcome: Outcome::GroupElsewhere,
    },
];

/// The network one row is driven against: the two VPCs EC2 minted for this account, plus the
/// id of a subnet it minted in the first and then deleted.
#[derive(Debug)]
struct Account {
    here: Network,
    elsewhere: Network,
    deleted_subnet: String,
}

impl Account {
    /// The id a `Subnet` of a row resolves to.
    fn subnet(&self, named: Subnet) -> &str {
        match named {
            Subnet::Here(index) => &self.here.subnets[index],
            Subnet::Elsewhere(index) => &self.elsewhere.subnets[index],
            Subnet::Deleted => &self.deleted_subnet,
            Subnet::Absent(id) => id,
        }
    }

    /// The id a `Group` of a row resolves to.
    fn group(&self, named: Group) -> &str {
        match named {
            Group::Here => &self.here.security_group,
            Group::Elsewhere => &self.elsewhere.security_group,
            Group::Absent(id) => id,
        }
    }

    /// The `Settings` a row sends, and the ids it resolved to.
    fn settings(&self, subnets: &[Subnet], group: Group) -> (Vec<&str>, &str, Value) {
        let named: Vec<&str> = subnets.iter().map(|subnet| self.subnet(*subnet)).collect();
        let group = self.group(group);
        let settings = json!({ "Subnets": named, "SecurityGroupId": group });
        (named, group, settings)
    }
}

/// A fresh emulator, with two VPCs EC2 MINTED for its account — each with its own subnets
/// and security group — and one more subnet of the first VPC that EC2 minted and then
/// deleted.
///
/// The account also holds what [`Fleet`] seeds straight into EC2's store for the suites that
/// name fixed ids (one more VPC with eleven subnets and five security groups). No row here
/// names any of those: every id a row names is one EC2 minted for it, or one of the
/// deliberately absent ids above, so what the seeded network holds cannot decide any
/// assertion.
///
/// The blocks are RFC 1918 ranges that overlap nothing else the account holds; a /24 per
/// subnet leaves room for every one a row names.
async fn account(case: &str) -> (Fleet, Account) {
    let fleet = Fleet::new();
    let here = create_network(
        &fleet,
        case,
        "172.20.0.0/16",
        &[
            "172.20.1.0/24",
            "172.20.2.0/24",
            "172.20.3.0/24",
            "172.20.4.0/24",
        ],
        "activation-endpoints-here",
    )
    .await;
    let elsewhere = create_network(
        &fleet,
        case,
        "172.21.0.0/16",
        &["172.21.1.0/24"],
        "activation-endpoints-elsewhere",
    )
    .await;

    // The last subnet of the first VPC is deleted again through EC2's own API: an id the
    // account HELD and holds no longer, which is a different thing from one it never had.
    let mut here = here;
    let deleted_subnet = here
        .subnets
        .pop()
        .expect("the first VPC was created with subnets");
    fleet
        .ec2
        .delete_subnet()
        .subnet_id(&deleted_subnet)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must delete {deleted_subnet}: {error:?}")
        });

    (
        fleet,
        Account {
            here,
            elsewhere,
            deleted_subnet,
        },
    )
}

/// The one registration the account holds, as `ListIdentityProviders` reports it.
async fn sole_registration(h: &Harness, case: &str) -> Value {
    let listed = list_providers(h, &json!({})).await;
    let body = listed.ok(case).clone();
    let summaries = body["IdentityProviderSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no summaries in {body:?}"));
    assert_eq!(
        summaries.len(),
        1,
        "case {case:?}: the accepted registration must be the one reported, got {summaries:?}"
    );
    summaries[0].clone()
}

/// RULE: a registration naming subnets and a security group the account's own Amazon EC2
/// minted for it, in ONE VPC, is accepted — and it is a HEALTHY registration: it settles
/// into the terminal `REGISTERED` status the clock owes it, reporting no failure, with the
/// settings that resolved.
#[tokio::test]
async fn a_registration_is_accepted_for_the_network_ec2_minted_for_the_account() {
    for row in ACCEPTED_ROWS {
        // A fresh emulator per row, so the network EC2 minted is exactly this row's.
        let (fleet, account) = account(row.case).await;
        let (_, _, settings) = account.settings(row.subnets, row.group);
        let reply = register(
            &fleet.h,
            &json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
                "Settings": settings,
            }),
        )
        .await;

        let summary = &reply.ok(row.case)["IdentityProviderSummary"];
        assert_eq!(
            summary.get("Settings"),
            Some(&settings),
            "case {:?}: the summary must carry the settings that resolved ({settings}), got \
             {summary:?}",
            row.case
        );
        // The span is never zero: a caller acting straight after the call still sees the
        // registration being made.
        assert_eq!(
            summary.get("Status").and_then(Value::as_str),
            Some(REGISTERING),
            "case {:?}: expected the call to answer {REGISTERING:?}, got {summary:?}",
            row.case
        );

        // Past the transition the clock owes it, the registration whose network resolved is
        // a healthy one: the terminal status, and nothing to report as a failure.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let stored = sole_registration(&fleet.h, row.case).await;
        assert_eq!(
            stored.get("Status").and_then(Value::as_str),
            Some(REGISTERED),
            "case {:?}: expected the registration to settle {REGISTERED:?}, got {stored:?}",
            row.case
        );
        assert_eq!(
            stored.get("FailureMessage").and_then(Value::as_str),
            None,
            "case {:?}: a registration whose network resolved has no failure to report, got \
             {stored:?}",
            row.case
        );
        assert_eq!(
            stored.get("Settings"),
            Some(&settings),
            "case {:?}: a later read must report the stored settings ({settings}), got \
             {stored:?}",
            row.case
        );
    }
}

/// RULE: a registration naming a subnet or a security group the account's own Amazon EC2 has
/// no such resource for — one it never minted, or one it minted and then deleted — or mixing
/// the resources of two VPCs, is refused with the modelled `ValidationException` (HTTP 400)
/// naming that resource, and persists nothing.
#[tokio::test]
async fn a_registration_is_refused_for_a_network_ec2_does_not_hold() {
    for row in REFUSED_ROWS {
        // A fresh emulator per row, so the network EC2 minted is exactly this row's.
        let (fleet, account) = account(row.case).await;
        let (named, group, settings) = account.settings(row.subnets, row.group);
        let expected = match row.outcome {
            Outcome::NoSuchSubnet(position) => Expected::NoSuchSubnet(named[position]),
            Outcome::NoSuchGroup => Expected::NoSuchGroup(group),
            Outcome::SubnetsDisagree => Expected::SubnetsDisagree(&account.here.vpc_id),
            Outcome::GroupElsewhere => Expected::GroupElsewhere(group, &account.here.vpc_id),
        };
        let message = expected
            .message()
            .expect("every row here is a refusal row");

        let reply = register(
            &fleet.h,
            &json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
                "Settings": settings,
            }),
        )
        .await;
        assert_refused(&reply, &message, row.case);

        // Nothing was persisted: the refusal came before the record was minted.
        let listed = list_providers(&fleet.h, &json!({})).await;
        let body = listed.ok(row.case);
        assert_eq!(
            body["IdentityProviderSummaries"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: a refused registration must persist nothing, got {body:?}",
            row.case
        );

        // And the same request succeeds once it names the network EC2 did mint for this
        // account, so the refusal was about those ids and nothing else.
        let repaired = register(
            &fleet.h,
            &json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[0],
                "Settings": {
                    "Subnets": [account.here.subnets[0].as_str()],
                    "SecurityGroupId": account.here.security_group.as_str(),
                },
            }),
        )
        .await;
        repaired.ok(row.case);
    }
}
