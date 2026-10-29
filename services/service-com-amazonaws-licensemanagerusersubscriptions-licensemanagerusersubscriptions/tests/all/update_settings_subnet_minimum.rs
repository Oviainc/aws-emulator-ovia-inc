// SPDX-License-Identifier: CC0-1.0

//! WHAT the `Settings$Subnets` minimum refuses on `UpdateIdentityProviderSettings`, driven
//! over the real wire path: every call goes through the router's `dispatch`, so each
//! request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The rule under test, asserted over a TABLE rather than one request: a minimum a stored
//! resource already does not meet is not something a request that removes nothing
//! violates, so an update is refused for emptying a list only when it TAKES AWAY the last
//! element the resource holds. `Settings$Subnets` is `@required` with `@length(min: 1)`
//! while `Settings` itself is optional on `RegisterIdentityProvider`
//! (`smithy_query(query="shape", shape="Settings,RegisterIdentityProviderRequest")`), so a
//! registration stored with no subnet is already below that minimum: a payload that
//! removes none of its subnets leaves them exactly as they were and is applied — its
//! `SecurityGroupId` reaches the record, and a payload naming nothing at all is the no-op
//! the operation's `@idempotent` trait asks for — while a payload that removes the last
//! subnet a registration does hold is refused.
//!
//! The rows differ along what the rule generalises over: whether the registration was
//! stored with `Settings` at all, how many subnets it holds, how many the payload adds and
//! removes, whether what it removes is held or not, and whether it names a security group
//! (at both lengths the member's pattern admits).

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity, the account's
// directory fleet, its network and the payload builder come from the sibling suites rather
// than a second copy.
use crate::identity_provider_subscriptions::{
    as_str, harness, list_providers, provider_union, register, UpdateSettingsPayload, PRODUCTS,
};
use crate::settings_network_resolution::update_settings;

/// A directory the shared harness put in the account's fleet, so no row here is refused
/// for its `DirectoryId`.
const DIRECTORY: &str = "d-0123456789";

/// The security group a row that is registered WITH `Settings` supplies; one the shared
/// harness holds in the account's Amazon EC2 store, so the registration is accepted.
const INITIAL_GROUP: &str = "sg-0123abcd";

/// The subnet the SEQUENCE test's registration gains and then tries to remove — one the
/// account holds, so adding it is accepted.
const GAINED_SUBNET: &str = "subnet-aaaaaaaaaaaaaaaab";
const GAINED_SUBNETS: &[&str] = &[GAINED_SUBNET];

/// What the service must answer a row with.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// HTTP 200, and the `(subnets, security group)` the registration must carry after the
    /// update — read back through `ListIdentityProviders` as well as off the answer.
    Applied(&'static [&'static str], &'static str),
    /// HTTP 400 `ValidationException`: the payload takes the last subnet away.
    LastSubnetRemoved,
}

impl Expected {
    /// The sentence the refusal carries, spelled out here rather than read from the
    /// implementation so a change to it fails this test.
    fn message(self) -> Option<&'static str> {
        match self {
            Expected::Applied(..) => None,
            Expected::LastSubnetRemoved => Some(
                "UpdateSettings would leave the registration with no subnet, and \
                 Settings.Subnets requires at least one",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The subnets the registration is created with, or `None` for a request that omits
    /// the optional `Settings` member altogether — the registration this crate stores with
    /// no subnet and no security group.
    initial: Option<&'static [&'static str]>,
    payload: UpdateSettingsPayload,
    expected: Expected,
}

/// Every subnet and security group named below is one the shared harness holds in the
/// account's Amazon EC2 store, except where a row removes a subnet on purpose: what
/// `RemoveSubnets` names leaves the settings rather than being provisioned, so it is not
/// resolved at all.
const ROWS: &[Row] = &[
    Row {
        case: "no settings stored, a payload that names nothing",
        initial: None,
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: None,
        },
        expected: Expected::Applied(&[], ""),
    },
    Row {
        case: "no settings stored, a security group only (8 characters)",
        initial: None,
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: Some("sg-abcd0123"),
        },
        expected: Expected::Applied(&[], "sg-abcd0123"),
    },
    Row {
        case: "no settings stored, a security group only (17 characters)",
        initial: None,
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: Some("sg-0123456789abcdef0"),
        },
        expected: Expected::Applied(&[], "sg-0123456789abcdef0"),
    },
    Row {
        case: "no settings stored, a subnet it does not carry removed",
        initial: None,
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &["subnet-1111111111111111a"],
            security_group_id: None,
        },
        expected: Expected::Applied(&[], ""),
    },
    Row {
        case: "no settings stored, its first subnet and a group added",
        initial: None,
        payload: UpdateSettingsPayload {
            add_subnets: &["subnet-4444444444444444d"],
            remove_subnets: &[],
            security_group_id: Some("sg-ffffffff"),
        },
        expected: Expected::Applied(&["subnet-4444444444444444d"], "sg-ffffffff"),
    },
    Row {
        case: "no settings stored, three subnets added and an unheld one removed",
        initial: None,
        payload: UpdateSettingsPayload {
            add_subnets: &[
                "subnet-5555555555555555e",
                "subnet-6666666666666666f",
                "subnet-7777777777777777a",
            ],
            remove_subnets: &["subnet-9999999999999999a"],
            security_group_id: None,
        },
        expected: Expected::Applied(
            &[
                "subnet-5555555555555555e",
                "subnet-6666666666666666f",
                "subnet-7777777777777777a",
            ],
            "",
        ),
    },
    Row {
        case: "one subnet stored, a payload that names nothing",
        initial: Some(&["subnet-0123456789abcdef0"]),
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: None,
        },
        expected: Expected::Applied(&["subnet-0123456789abcdef0"], INITIAL_GROUP),
    },
    Row {
        case: "one subnet stored, a security group only",
        initial: Some(&["subnet-0123456789abcdef0"]),
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: Some("sg-00000000000000001"),
        },
        expected: Expected::Applied(&["subnet-0123456789abcdef0"], "sg-00000000000000001"),
    },
    Row {
        case: "one subnet stored, that subnet removed",
        initial: Some(&["subnet-0123456789abcdef0"]),
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &["subnet-0123456789abcdef0"],
            security_group_id: None,
        },
        expected: Expected::LastSubnetRemoved,
    },
    Row {
        case: "two subnets stored, both removed and a group named",
        initial: Some(&["subnet-0123456789abcdef0", "subnet-3333333333333333c"]),
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            security_group_id: Some("sg-ffffffff"),
        },
        expected: Expected::LastSubnetRemoved,
    },
    Row {
        case: "two subnets stored, both removed and another added",
        initial: Some(&["subnet-0123456789abcdef0", "subnet-3333333333333333c"]),
        payload: UpdateSettingsPayload {
            add_subnets: &["subnet-8888888888888888b"],
            remove_subnets: &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            security_group_id: None,
        },
        expected: Expected::Applied(&["subnet-8888888888888888b"], INITIAL_GROUP),
    },
];

impl Row {
    /// The `RegisterIdentityProvider` body: with `Settings` when the row holds subnets,
    /// and without the member at all when it does not.
    fn registration(&self, product: &str) -> Value {
        let mut body = json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
        });
        if let Some(subnets) = self.initial {
            body["Settings"] = json!({
                "Subnets": subnets,
                "SecurityGroupId": INITIAL_GROUP,
            });
        }
        body
    }

    /// The settings the registration carries BEFORE the update: what the request supplied,
    /// and the empty configuration when it supplied none.
    fn stored_settings(&self) -> Value {
        match self.initial {
            Some(subnets) => json!({ "Subnets": subnets, "SecurityGroupId": INITIAL_GROUP }),
            None => json!({ "Subnets": [], "SecurityGroupId": "" }),
        }
    }
}

/// RULE: the subnet minimum is about what an update REMOVES — a payload that takes no
/// subnet away from a registration is applied however few subnets that registration holds,
/// and only one that removes the last subnet it does hold is refused.
#[tokio::test]
async fn the_subnet_minimum_refuses_only_an_update_that_removes_the_last_subnet() {
    for (index, row) in ROWS.iter().enumerate() {
        // A harness per row, so every row starts from the settings it declares and the
        // list below reports exactly one registration.
        let h = harness();
        let product = PRODUCTS[index % PRODUCTS.len()];
        let registered = register(&h, &row.registration(product)).await;
        let summary = registered.ok(row.case)["IdentityProviderSummary"].clone();
        assert_eq!(
            summary.get("Settings"),
            Some(&row.stored_settings()),
            "case {:?}: the registration must store {}, got {summary:?}",
            row.case,
            row.stored_settings()
        );
        let provider_arn = as_str(&summary, "IdentityProviderArn", row.case).to_string();

        let reply = update_settings(&h, &row.payload.request(&provider_arn)).await;

        let expected_settings = match row.expected {
            Expected::Applied(subnets, security_group_id) => {
                let expected =
                    json!({ "Subnets": subnets, "SecurityGroupId": security_group_id });
                let answered = reply.ok(row.case)["IdentityProviderSummary"]["Settings"].clone();
                assert_eq!(
                    answered, expected,
                    "case {:?}: expected {expected}, got {answered:?}",
                    row.case
                );
                expected
            }
            Expected::LastSubnetRemoved => {
                assert_eq!(
                    reply.status, 400,
                    "case {:?}: expected HTTP 400, got {} with body {:?}",
                    row.case, reply.status, reply.body
                );
                assert_eq!(
                    reply.error_type.as_deref(),
                    Some("ValidationException"),
                    "case {:?}: expected ValidationException, got {:?} with body {:?}",
                    row.case,
                    reply.error_type,
                    reply.body
                );
                assert_eq!(
                    reply.message(),
                    row.expected.message(),
                    "case {:?}: expected {:?}, got {:?}",
                    row.case,
                    row.expected.message(),
                    reply.message()
                );
                // A refusal persists nothing: the registration keeps the settings it had.
                row.stored_settings()
            }
        };

        // Computed from state, not just answered: a later read reports the same settings.
        let listed = list_providers(&h, &json!({})).await;
        let stored = listed.ok(row.case)["IdentityProviderSummaries"][0]["Settings"].clone();
        assert_eq!(
            stored, expected_settings,
            "case {:?}: expected the stored settings to be {expected_settings}, got {stored:?}",
            row.case
        );
    }
}

/// RULE (the same one, over a SEQUENCE): what the minimum is measured against is the
/// settings the store holds at that moment, so the subnet a `Settings`-less registration
/// gained is a last subnet whose removal is then refused — the refusal follows the state,
/// not the shape of the registration's first request.
#[tokio::test]
async fn a_subnet_an_empty_registration_gained_cannot_then_be_removed() {
    let h = harness();
    let product = PRODUCTS[0];
    let registered = register(
        &h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
        }),
    )
    .await;
    let case = "a registration stored with no settings";
    let provider_arn = as_str(
        &registered.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string();

    let subnet = GAINED_SUBNET;
    let gained = UpdateSettingsPayload {
        add_subnets: GAINED_SUBNETS,
        remove_subnets: &[],
        security_group_id: Some(INITIAL_GROUP),
    };
    let reply = update_settings(&h, &gained.request(&provider_arn)).await;
    let answered = reply.ok("its first subnet added")["IdentityProviderSummary"]["Settings"].clone();
    assert_eq!(
        answered,
        json!({ "Subnets": [subnet], "SecurityGroupId": INITIAL_GROUP }),
        "case {:?}: expected the added subnet to be stored, got {answered:?}",
        "its first subnet added"
    );

    let removed = UpdateSettingsPayload {
        add_subnets: &[],
        remove_subnets: GAINED_SUBNETS,
        security_group_id: None,
    };
    let reply = update_settings(&h, &removed.request(&provider_arn)).await;
    let case = "the subnet it gained, removed again";
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
        Expected::LastSubnetRemoved.message(),
        "case {case:?}: expected {:?}, got {:?}",
        Expected::LastSubnetRemoved.message(),
        reply.message()
    );

    // And the refusal changed nothing: the subnet it gained is still stored.
    let listed = list_providers(&h, &json!({})).await;
    let stored = listed.ok(case)["IdentityProviderSummaries"][0]["Settings"].clone();
    assert_eq!(
        stored,
        json!({ "Subnets": [subnet], "SecurityGroupId": INITIAL_GROUP }),
        "case {case:?}: a refused update must persist nothing, got {stored:?}"
    );
}
