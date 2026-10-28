// SPDX-License-Identifier: CC0-1.0

//! What a registration's `Settings` may NAME: the network resources the activation-server
//! VPC endpoints are created in, resolved against the account's own Amazon EC2 store.
//!
//! `Subnets` is "the ID of one or more subnets in which License Manager will create a VPC
//! endpoint for products that require connectivity to activation servers" and
//! `SecurityGroupId` "a security group ID that allows inbound TCP port 1688 communication
//! between resources in your VPC and the VPC endpoint for activation servers", so every
//! id a registration carries names a network resource the ACCOUNT already holds, and a
//! VPC endpoint lives in ONE VPC ("your VPC"), so the subnets and the security group of
//! one registration belong to one VPC.
//!
//! The rule under test, asserted over a TABLE whose rows differ along what it generalises
//! over — which member names the resource that cannot be resolved, how many subnets the
//! request carries, where in the list that one sits, whether the resource is absent
//! altogether or merely in another VPC, and whether the settings arrive on a register or
//! on an update:
//!
//! * a `Settings` (or an `UpdateSettings` result) naming a subnet or a security group the
//!   account's Amazon EC2 store does not hold is refused with the modelled
//!   `ValidationException` (HTTP 400) naming that resource, and nothing is persisted;
//! * so is one mixing resources of two VPCs;
//! * one naming resources the account does hold, in one VPC, is accepted and round-trips
//!   through `ListIdentityProviders`;
//! * `RemoveSubnets` resolves nothing: what it names leaves the settings rather than being
//!   provisioned;
//! * the subnets a SELF-MANAGED Active Directory's `DomainNetworkSettings` names are held
//!   to the same existence rule, and to no VPC agreement, because nothing is provisioned in
//!   them;
//! * how many subnets ONE request can have resolved is bounded, and the bound is what
//!   answers past it.
//!
//! Every call goes through the router's `dispatch`, so each request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be, and the
//! network each row needs is created in Amazon EC2's own store — the shared harness's VPC,
//! plus the second VPC this suite adds.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity, the account's
// directory fleet and the account's network come from the sibling suite rather than a
// second copy.
use crate::identity_provider_subscriptions::{
    as_str, call, create_network, harness, list_providers, provider_union, register, Harness,
    Reply, ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, ACCOUNT_VPC, PRODUCTS,
};

/// A SECOND VPC of the same account, with one subnet and one security group of its own:
/// resources that exist, and that still cannot be mixed with another VPC's.
const OTHER_VPC: &str = "vpc-aaaabbbbccccdddd0";
const OTHER_VPC_SUBNET: &str = "subnet-bbbbbbbbbbbbbbbbb";
const OTHER_VPC_SECURITY_GROUP: &str = "sg-bbbbbbbb";

/// Ids of the modelled form that NO VPC of the account holds — `Subnets` matches
/// `^subnet-[a-z0-9]{8,17}` and `SecurityGroupId`
/// `^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$`, so each passes the boundary validator and
/// reaches the handler's resolution. The two subnet ids are the shortest and the longest
/// form the pattern admits.
const UNHELD_SUBNET: &str = "subnet-00000000";
const LONG_UNHELD_SUBNET: &str = "subnet-dddddddddddddddde";
const UNHELD_SECURITY_GROUP: &str = "sg-00000000";

/// A directory the shared harness put in the account's fleet, so no row here is refused
/// for its `DirectoryId`.
const DIRECTORY: &str = "d-0123456789";

/// What the service must answer a row with.
///
/// Each refusal carries the ids the sentence has to name, so the expected message is built
/// from the ROW's own values rather than copied as one literal — a refusal that named some
/// other resource would not match.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// HTTP 200, and the settings the call must leave the registration with.
    Accepted,
    /// The subnet the request named is not in the account's store.
    NoSuchSubnet(&'static str),
    /// The security group the request named is not in the account's store.
    NoSuchGroup(&'static str),
    /// The subnets do not agree on a VPC; the id is the VPC the earlier ones share.
    SubnetsDisagree(&'static str),
    /// The security group belongs to another VPC than the subnets do.
    GroupElsewhere(&'static str, &'static str),
}

impl Expected {
    /// The sentence the refusal must carry, spelled out here rather than read from the
    /// implementation: the fixed text of the error class with the ids of this row.
    fn message(self) -> Option<String> {
        match self {
            Expected::Accepted => None,
            Expected::NoSuchSubnet(subnet_id) => {
                Some(format!("The subnet ID '{subnet_id}' does not exist"))
            }
            Expected::NoSuchGroup(group_id) => {
                Some(format!("The security group '{group_id}' does not exist"))
            }
            Expected::SubnetsDisagree(vpc_id) => {
                Some(format!("All subnets must belong to the same VPC: '{vpc_id}'"))
            }
            Expected::GroupElsewhere(group_id, vpc_id) => Some(format!(
                "The security group '{group_id}' must belong to the same VPC as the \
                 subnets: '{vpc_id}'"
            )),
        }
    }
}

/// A harness whose account also holds [`OTHER_VPC`] and its one subnet and group.
fn harness_with_two_vpcs() -> Harness {
    let h = harness();
    create_network(
        &h.engine,
        OTHER_VPC,
        &[OTHER_VPC_SUBNET],
        &[OTHER_VPC_SECURITY_GROUP],
    );
    h
}

async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/UpdateIdentityProviderSettings", body).await
}

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) carrying `message`.
fn assert_refused(reply: &Reply, message: &str, case: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.message(),
        Some(message),
        "case {case:?}: expected the refusal to say {message:?}, got {:?}",
        reply.message()
    );
}

/// The `Settings` of one `RegisterIdentityProvider` row, and what it must be answered.
#[derive(Debug, Clone, Copy)]
struct RegisterRow {
    case: &'static str,
    subnets: &'static [&'static str],
    security_group_id: &'static str,
    expected: Expected,
}

const REGISTER_ROWS: &[RegisterRow] = &[
    RegisterRow {
        case: "one held subnet and a held 8-character group",
        subnets: &[ACCOUNT_SUBNETS[0]],
        security_group_id: ACCOUNT_SECURITY_GROUPS[0],
        expected: Expected::Accepted,
    },
    RegisterRow {
        case: "three held subnets and a held 17-character group",
        subnets: &[ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2], ACCOUNT_SUBNETS[3]],
        security_group_id: ACCOUNT_SECURITY_GROUPS[3],
        expected: Expected::Accepted,
    },
    RegisterRow {
        case: "the one subnet is the shortest id no VPC holds",
        subnets: &[UNHELD_SUBNET],
        security_group_id: ACCOUNT_SECURITY_GROUPS[0],
        expected: Expected::NoSuchSubnet(UNHELD_SUBNET),
    },
    RegisterRow {
        case: "an unheld subnet LAST, after two held ones",
        subnets: &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], LONG_UNHELD_SUBNET],
        security_group_id: ACCOUNT_SECURITY_GROUPS[0],
        expected: Expected::NoSuchSubnet(LONG_UNHELD_SUBNET),
    },
    RegisterRow {
        case: "an unheld subnet FIRST, before a held one",
        subnets: &[LONG_UNHELD_SUBNET, ACCOUNT_SUBNETS[0]],
        security_group_id: ACCOUNT_SECURITY_GROUPS[0],
        expected: Expected::NoSuchSubnet(LONG_UNHELD_SUBNET),
    },
    RegisterRow {
        case: "held subnets and a security group no VPC holds",
        subnets: &[ACCOUNT_SUBNETS[0]],
        security_group_id: UNHELD_SECURITY_GROUP,
        expected: Expected::NoSuchGroup(UNHELD_SECURITY_GROUP),
    },
    RegisterRow {
        case: "two held subnets of two different VPCs",
        subnets: &[ACCOUNT_SUBNETS[0], OTHER_VPC_SUBNET],
        security_group_id: ACCOUNT_SECURITY_GROUPS[0],
        expected: Expected::SubnetsDisagree(ACCOUNT_VPC),
    },
    RegisterRow {
        case: "a held subnet of one VPC and a held group of the other",
        subnets: &[ACCOUNT_SUBNETS[0]],
        security_group_id: OTHER_VPC_SECURITY_GROUP,
        expected: Expected::GroupElsewhere(OTHER_VPC_SECURITY_GROUP, ACCOUNT_VPC),
    },
    RegisterRow {
        case: "the other VPC's own subnet and group together",
        subnets: &[OTHER_VPC_SUBNET],
        security_group_id: OTHER_VPC_SECURITY_GROUP,
        expected: Expected::Accepted,
    },
];

impl RegisterRow {
    fn request(&self, product: &str) -> Value {
        json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
            "Settings": {
                "Subnets": self.subnets,
                "SecurityGroupId": self.security_group_id,
            },
        })
    }
}

/// RULE: a registration is accepted only for subnets and a security group the account's
/// own Amazon EC2 store holds, in ONE VPC — and a refused registration persists nothing.
#[tokio::test]
async fn a_registration_resolves_its_settings_against_the_accounts_network() {
    for row in REGISTER_ROWS {
        // A fresh emulator per row, so what either store holds is exactly what this row
        // made.
        let h = harness_with_two_vpcs();
        let reply = register(&h, &row.request(PRODUCTS[0])).await;

        match row.expected.message() {
            None => {
                let summary = &reply.ok(row.case)["IdentityProviderSummary"];
                let settings = json!({
                    "Subnets": row.subnets,
                    "SecurityGroupId": row.security_group_id,
                });
                assert_eq!(
                    summary.get("Settings"),
                    Some(&settings),
                    "case {:?}: the summary must carry the settings that resolved \
                     ({settings}), got {summary:?}",
                    row.case
                );
                // The round trip: what the register persisted is what a later read reports.
                let listed = list_providers(&h, &json!({})).await;
                let body = listed.ok(row.case);
                let summaries = body["IdentityProviderSummaries"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {:?}: no summaries in {body:?}", row.case));
                assert_eq!(
                    summaries.len(),
                    1,
                    "case {:?}: the accepted registration must be the one reported, got \
                     {summaries:?}",
                    row.case
                );
                assert_eq!(
                    summaries[0].get("Settings"),
                    Some(&settings),
                    "case {:?}: the list must report the stored settings ({settings}), got \
                     {:?}",
                    row.case,
                    summaries[0]
                );
            }
            Some(message) => {
                assert_refused(&reply, &message, row.case);
                // Nothing was persisted: the store holds no registration at all, so the
                // refusal came before the record was minted.
                let listed = list_providers(&h, &json!({})).await;
                let body = listed.ok(row.case);
                assert_eq!(
                    body["IdentityProviderSummaries"].as_array().map(Vec::len),
                    Some(0),
                    "case {:?}: a refused registration must persist nothing, got {body:?}",
                    row.case
                );
                // And the pair it addressed is still free: the same provider and product
                // register once the settings name resources the account holds.
                let accepted = register(
                    &h,
                    &json!({
                        "IdentityProvider": provider_union(DIRECTORY),
                        "Product": PRODUCTS[0],
                        "Settings": {
                            "Subnets": [ACCOUNT_SUBNETS[0]],
                            "SecurityGroupId": ACCOUNT_SECURITY_GROUPS[0],
                        },
                    }),
                )
                .await;
                accepted.ok(row.case);
            }
        }
    }
}

/// RULE: `Settings` is not `@required`, so a registration that names no network resolves
/// none — the empty configuration is accepted whatever the account holds.
#[tokio::test]
async fn a_registration_without_settings_resolves_no_network() {
    let h = harness_with_two_vpcs();
    let reply = register(
        &h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": PRODUCTS[1],
        }),
    )
    .await;
    let summary = &reply.ok("no settings at all")["IdentityProviderSummary"];
    assert_eq!(
        summary.get("Settings"),
        Some(&json!({ "Subnets": [], "SecurityGroupId": "" })),
        "a registration that supplied no settings must carry the empty configuration, got \
         {summary:?}"
    );
}

/// RULE: the subnets a SELF-MANAGED Active Directory's `DomainNetworkSettings` names are
/// resolved by the same existence rule — they are subnets of the account too ("a list of
/// subnets that apply for the Active Directory domain") — while nothing is provisioned in
/// them, so subnets of two VPCs are accepted there.
#[tokio::test]
async fn a_self_managed_domains_subnets_are_resolved_too() {
    // (case, the `DomainNetworkSettings.Subnets` the provider carries, the refusal)
    let rows: [(&str, &[&str], Expected); 5] = [
        (
            "one subnet the account has",
            &[ACCOUNT_SUBNETS[0]],
            Expected::Accepted,
        ),
        (
            "two subnets the account has",
            &[ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
            Expected::Accepted,
        ),
        (
            "subnets of two different VPCs: nothing is provisioned in them",
            &[ACCOUNT_SUBNETS[0], OTHER_VPC_SUBNET],
            Expected::Accepted,
        ),
        (
            "the one subnet is one no VPC holds",
            &[UNHELD_SUBNET],
            Expected::NoSuchSubnet(UNHELD_SUBNET),
        ),
        (
            "an unheld subnet after a held one",
            &[ACCOUNT_SUBNETS[0], LONG_UNHELD_SUBNET],
            Expected::NoSuchSubnet(LONG_UNHELD_SUBNET),
        ),
    ];

    for (case, subnets, expected) in rows {
        // A fresh emulator per row: a self-managed domain is addressed by its name, so two
        // rows of one emulator would be two registrations of the same pair.
        let h = harness_with_two_vpcs();
        let reply = register(
            &h,
            &json!({
                "IdentityProvider": {
                    "ActiveDirectoryIdentityProvider": {
                        "ActiveDirectorySettings": {
                            "DomainName": "self.managed.example",
                            "DomainNetworkSettings": { "Subnets": subnets },
                        },
                    },
                },
                "Product": PRODUCTS[3],
            }),
        )
        .await;
        match expected.message() {
            None => {
                let summary = &reply.ok(case)["IdentityProviderSummary"];
                assert_eq!(
                    summary["IdentityProvider"]["ActiveDirectoryIdentityProvider"]
                        ["ActiveDirectorySettings"]["DomainNetworkSettings"]["Subnets"],
                    json!(subnets),
                    "case {case:?}: the summary must carry the domain subnets it resolved, \
                     got {summary:?}"
                );
            }
            Some(message) => {
                assert_refused(&reply, &message, case);
                let listed = list_providers(&h, &json!({})).await;
                let body = listed.ok(case);
                assert_eq!(
                    body["IdentityProviderSummaries"].as_array().map(Vec::len),
                    Some(0),
                    "case {case:?}: a refused registration must persist nothing, got {body:?}"
                );
            }
        }
    }
}

/// The subnets the registration every update row is applied to is registered with.
const INITIAL_SUBNETS: &[&str] = &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1]];

/// The most distinct subnet ids one request may have resolved, spelled out here rather than
/// read from the production constant so a change to it fails a test instead of silently
/// changing what this suite accepts.
const MAX_SUBNETS_RESOLVED: usize = 1_000;

/// RULE: the subnets ONE request can have resolved are BOUNDED, and the refusal says so
/// rather than the service spending the work — at the bound the request is still resolved
/// (and so refused for the first subnet the account does not have), one past it the bound
/// itself is what answers, and both members that name subnets are held to it.
#[tokio::test]
async fn the_subnets_one_request_can_have_resolved_are_bounded() {
    let h = harness_with_two_vpcs();
    // `Subnets` matches `^subnet-[a-z0-9]{8,17}`, so an 8-hex-digit id is the shortest the
    // model accepts and makes a distinct value per index. None of them is a subnet the
    // account has.
    let subnets = |count: usize| -> Vec<String> {
        (0..count).map(|i| format!("subnet-{i:08x}")).collect()
    };

    // (case, the member the ids arrive on, how many, the sentence the refusal must carry)
    let rows: [(&str, &str, usize, String); 3] = [
        (
            "Settings.Subnets at the bound",
            "Settings",
            MAX_SUBNETS_RESOLVED,
            Expected::NoSuchSubnet("subnet-00000000")
                .message()
                .expect("a refusal row"),
        ),
        (
            "Settings.Subnets one past the bound",
            "Settings",
            MAX_SUBNETS_RESOLVED + 1,
            format!(
                "Settings.Subnets names {} subnets; at most {MAX_SUBNETS_RESOLVED} may be \
                 named in one request",
                MAX_SUBNETS_RESOLVED + 1
            ),
        ),
        (
            "the domain's subnets one past the bound",
            "DomainNetworkSettings",
            MAX_SUBNETS_RESOLVED + 1,
            format!(
                "ActiveDirectorySettings.DomainNetworkSettings.Subnets names {} subnets; at \
                 most {MAX_SUBNETS_RESOLVED} may be named in one request",
                MAX_SUBNETS_RESOLVED + 1
            ),
        ),
    ];

    for (case, member, count, message) in &rows {
        let named = subnets(*count);
        let request = if *member == "Settings" {
            json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[4],
                "Settings": {
                    "Subnets": named,
                    "SecurityGroupId": ACCOUNT_SECURITY_GROUPS[0],
                },
            })
        } else {
            json!({
                "IdentityProvider": {
                    "ActiveDirectoryIdentityProvider": {
                        "ActiveDirectorySettings": {
                            "DomainName": "bounded.example",
                            "DomainNetworkSettings": { "Subnets": named },
                        },
                    },
                },
                "Product": PRODUCTS[4],
            })
        };
        assert_refused(&register(&h, &request).await, message, case);
    }
}
/// One `UpdateIdentityProviderSettings` row: the payload members, what the service must
/// answer, and the settings the registration is left with either way.
#[derive(Debug, Clone, Copy)]
struct UpdateRow {
    case: &'static str,
    add_subnets: &'static [&'static str],
    remove_subnets: &'static [&'static str],
    /// `None` leaves `UpdateSettings.SecurityGroupId` out, which keeps the stored one.
    security_group_id: Option<&'static str>,
    expected: Expected,
    /// The subnets and security group every later read must report for the registration.
    settings_after: (&'static [&'static str], &'static str),
}

const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "add a subnet the account holds in the same VPC",
        add_subnets: &[ACCOUNT_SUBNETS[2]],
        remove_subnets: &[],
        security_group_id: None,
        expected: Expected::Accepted,
        settings_after: (
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
            ACCOUNT_SECURITY_GROUPS[0],
        ),
    },
    UpdateRow {
        case: "add a subnet no VPC holds",
        add_subnets: &[UNHELD_SUBNET],
        remove_subnets: &[],
        security_group_id: None,
        expected: Expected::NoSuchSubnet(UNHELD_SUBNET),
        settings_after: (INITIAL_SUBNETS, ACCOUNT_SECURITY_GROUPS[0]),
    },
    UpdateRow {
        case: "add two subnets of which only the second is unheld",
        add_subnets: &[ACCOUNT_SUBNETS[3], LONG_UNHELD_SUBNET],
        remove_subnets: &[],
        security_group_id: None,
        expected: Expected::NoSuchSubnet(LONG_UNHELD_SUBNET),
        settings_after: (INITIAL_SUBNETS, ACCOUNT_SECURITY_GROUPS[0]),
    },
    UpdateRow {
        case: "add a held subnet of the OTHER VPC",
        add_subnets: &[OTHER_VPC_SUBNET],
        remove_subnets: &[],
        security_group_id: None,
        expected: Expected::SubnetsDisagree(ACCOUNT_VPC),
        settings_after: (INITIAL_SUBNETS, ACCOUNT_SECURITY_GROUPS[0]),
    },
    UpdateRow {
        case: "switch to another security group of the same VPC",
        add_subnets: &[],
        remove_subnets: &[],
        security_group_id: Some(ACCOUNT_SECURITY_GROUPS[2]),
        expected: Expected::Accepted,
        settings_after: (INITIAL_SUBNETS, ACCOUNT_SECURITY_GROUPS[2]),
    },
    UpdateRow {
        case: "switch to a security group no VPC holds",
        add_subnets: &[],
        remove_subnets: &[],
        security_group_id: Some(UNHELD_SECURITY_GROUP),
        expected: Expected::NoSuchGroup(UNHELD_SECURITY_GROUP),
        settings_after: (INITIAL_SUBNETS, ACCOUNT_SECURITY_GROUPS[0]),
    },
    UpdateRow {
        case: "switch to a held security group of the OTHER VPC",
        add_subnets: &[],
        remove_subnets: &[],
        security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
        expected: Expected::GroupElsewhere(OTHER_VPC_SECURITY_GROUP, ACCOUNT_VPC),
        settings_after: (INITIAL_SUBNETS, ACCOUNT_SECURITY_GROUPS[0]),
    },
    UpdateRow {
        case: "remove a subnet the account never held: a no-op, not a resolution",
        add_subnets: &[],
        remove_subnets: &[UNHELD_SUBNET],
        security_group_id: None,
        expected: Expected::Accepted,
        settings_after: (INITIAL_SUBNETS, ACCOUNT_SECURITY_GROUPS[0]),
    },
    UpdateRow {
        case: "remove a held subnet and add another held one",
        add_subnets: &[ACCOUNT_SUBNETS[4]],
        remove_subnets: &[ACCOUNT_SUBNETS[0]],
        security_group_id: None,
        expected: Expected::Accepted,
        settings_after: (
            &[ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[4]],
            ACCOUNT_SECURITY_GROUPS[0],
        ),
    },
];

impl UpdateRow {
    fn payload(&self) -> Value {
        let mut payload = json!({
            "AddSubnets": self.add_subnets,
            "RemoveSubnets": self.remove_subnets,
        });
        if let Some(security_group_id) = self.security_group_id {
            payload["SecurityGroupId"] = json!(security_group_id);
        }
        payload
    }
}

/// RULE: the settings an update would STORE name subnets and a security group the
/// account's own Amazon EC2 store holds, in one VPC — and a refused update leaves the
/// registration exactly as it was.
#[tokio::test]
async fn an_update_resolves_the_settings_it_would_store() {
    for row in UPDATE_ROWS {
        // A fresh emulator per row, so the registration under test starts from the same
        // settings whatever the previous row did.
        let h = harness_with_two_vpcs();
        let registered = register(
            &h,
            &json!({
                "IdentityProvider": provider_union(DIRECTORY),
                "Product": PRODUCTS[2],
                "Settings": {
                    "Subnets": INITIAL_SUBNETS,
                    "SecurityGroupId": ACCOUNT_SECURITY_GROUPS[0],
                },
            }),
        )
        .await;
        let provider_arn = as_str(
            &registered.ok(row.case)["IdentityProviderSummary"],
            "IdentityProviderArn",
            row.case,
        )
        .to_string();

        let reply = update_settings(
            &h,
            &json!({ "IdentityProviderArn": provider_arn, "UpdateSettings": row.payload() }),
        )
        .await;
        let (subnets_after, group_after) = row.settings_after;
        let expected = json!({
            "Subnets": subnets_after,
            "SecurityGroupId": group_after,
        });
        match row.expected.message() {
            None => assert_eq!(
                reply.ok(row.case)["IdentityProviderSummary"].get("Settings"),
                Some(&expected),
                "case {:?}: the update must answer the settings it stored ({expected}), got \
                 {:?}",
                row.case,
                reply.body
            ),
            Some(message) => assert_refused(&reply, &message, row.case),
        }

        // Accepted or refused, a later read reports exactly the settings the registration
        // is left with: a refused payload is not half-applied.
        let listed = list_providers(&h, &json!({})).await;
        let body = listed.ok(row.case);
        let stored = body["IdentityProviderSummaries"]
            .as_array()
            .and_then(|summaries| summaries.first())
            .unwrap_or_else(|| panic!("case {:?}: no registration in {body:?}", row.case));
        assert_eq!(
            stored.get("Settings"),
            Some(&expected),
            "case {:?}: a later read must report {expected}, got {stored:?}",
            row.case
        );
    }
}
