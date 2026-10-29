// SPDX-License-Identifier: CC0-1.0

//! The `policyStoreId` members the model restricts to an ID.
//!
//! Most `policyStoreId` members document "To specify a policy store, use its ID
//! or alias name", and the alias stands in for the id there. Two do not:
//! `DeletePolicyStoreInput$policyStoreId` ("To specify a policy store, the alias
//! name cannot be used. Only the ID can be used.") and
//! `CreatePolicyStoreAliasInput$policyStoreId` ("The associated policy store must
//! be specified using its ID. The alias name cannot be used.") — both from
//! `smithy_query(query="shape", shape="DeletePolicyStoreInput,CreatePolicyStoreAliasInput")`.
//! For those two a `policy-store-alias/<name>` value is not an alias reference at
//! all: it is an id that no policy store carries, whatever alias holds that name.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! write made by one request is only visible to the next if it really reached
//! state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, error_code, ok, router, store_with_validation_off as store};

/// The prefix every alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`: "The alias name must always be
/// prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// A Cedar statement every store accepts, so the policy a row creates is only
/// there to be found again afterwards.
const STATEMENT: &str = "permit(principal, action, resource);";

/// How the alias a row names exists when the ID-only call is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AliasSetup {
    /// No alias carries the name.
    Absent,
    /// An `Active` alias names the row's store — the form that DOES resolve for
    /// every member documented to take an alias.
    Active,
    /// The alias was soft-deleted, so it is `PendingDeletion`.
    PendingDeletion,
}

/// One alias-shaped value handed to an ID-only member.
struct Case {
    name: &'static str,
    /// The alias name after the required prefix.
    suffix: &'static str,
    setup: AliasSetup,
}

/// The rows differ along the alias name's form (plain, nested separators,
/// digits, the bare prefix) and along whether an alias carries it at all —
/// `Active`, `PendingDeletion`, or nothing. None of them may ever name a store
/// through an ID-only member.
const CASES: &[Case] = &[
    Case {
        name: "an active alias",
        suffix: "id-only-active",
        setup: AliasSetup::Active,
    },
    Case {
        name: "an active alias with nested separators",
        suffix: "eu/prod/photos",
        setup: AliasSetup::Active,
    },
    Case {
        name: "an active alias named only with digits",
        suffix: "20261028",
        setup: AliasSetup::Active,
    },
    Case {
        name: "an alias pending deletion",
        suffix: "id-only-pending",
        setup: AliasSetup::PendingDeletion,
    },
    Case {
        name: "a name no alias carries",
        suffix: "never-created",
        setup: AliasSetup::Absent,
    },
    Case {
        name: "the bare alias prefix",
        suffix: "",
        setup: AliasSetup::Absent,
    },
];

/// What one row needs in state: a store holding one policy, and the row's alias
/// in the state the row asks for. Returns `(store id, policy id, alias name)`.
async fn fixture(router: &MultiServiceRouter, case: &Case) -> (String, String, String) {
    let store_id = store(router).await;
    let created = ok(
        router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement": STATEMENT } },
        }),
    )
    .await;
    let policy_id = created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();

    let alias_name = format!("{ALIAS_PREFIX}{}", case.suffix);
    if case.setup != AliasSetup::Absent {
        ok(
            router,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
        )
        .await;
    }
    if case.setup == AliasSetup::PendingDeletion {
        ok(
            router,
            "DeletePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "deletionMode": "SoftDelete" }),
        )
        .await;
    }
    (store_id, policy_id, alias_name)
}

/// `DeletePolicyStore` never resolves an alias-shaped `policyStoreId`: the value
/// names no store, so the call is the documented idempotent no-op and nothing in
/// state changes.
#[tokio::test]
async fn delete_policy_store_does_not_follow_an_alias_shaped_reference() {
    for case in CASES {
        let name = case.name;
        let router = router();
        let (store_id, policy_id, alias_name) = fixture(&router, case).await;

        // "If you specify a policy store that does not exist, the request
        // response will still return a successful HTTP 200 status code."
        let (status, body) = call(
            &router,
            "DeletePolicyStore",
            &json!({ "policyStoreId": alias_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from DeletePolicyStore, got {status} {body}"
        );

        // Nothing the store holds may have been destroyed.
        let (store_status, store_body) =
            call(&router, "GetPolicyStore", &json!({ "policyStoreId": store_id })).await;
        assert_eq!(
            store_status, 200,
            "case {name:?}: the store must survive, got {store_status} {store_body}"
        );
        let (policy_status, policy_body) = call(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            policy_status, 200,
            "case {name:?}: the store's policy must survive, got {policy_status} {policy_body}"
        );

        // And neither may the alias record, nor the state it was in.
        let (alias_status, alias_body) = call(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        let expected_alias_state = match case.setup {
            AliasSetup::Absent => None,
            AliasSetup::Active => Some("Active"),
            AliasSetup::PendingDeletion => Some("PendingDeletion"),
        };
        match expected_alias_state {
            Some(state) => {
                assert_eq!(
                    (alias_status, alias_body["state"].as_str().unwrap_or_default()),
                    (200, state),
                    "case {name:?}: the alias must survive unchanged, got {alias_status} {alias_body}"
                );
            }
            None => assert_eq!(
                alias_status, 404,
                "case {name:?}: no alias was created, so none may be reported, got {alias_status} {alias_body}"
            ),
        }
    }
}

/// `CreatePolicyStoreAlias` never resolves an alias-shaped `policyStoreId`
/// either: the value names no store, so the documented 404 is the answer and no
/// alias is created.
#[tokio::test]
async fn create_policy_store_alias_does_not_follow_an_alias_shaped_reference() {
    for case in CASES {
        let name = case.name;
        let router = router();
        let (_store_id, _policy_id, alias_name) = fixture(&router, case).await;
        let new_alias = format!("{ALIAS_PREFIX}second-{}", case.suffix.replace('/', "-"));

        let (status, body) = call(
            &router,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": new_alias, "policyStoreId": alias_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {name:?}: an alias-shaped policyStoreId must name no store, got {status} {body}"
        );

        let (read_status, read_body) = call(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": new_alias }),
        )
        .await;
        assert_eq!(
            read_status, 404,
            "case {name:?}: the refused alias must not have been created, got {read_status} {read_body}"
        );
    }
}

/// The ID-only rule is about those two members only: an `Active` alias still
/// stands in for its store's id everywhere the model says it may, and the store's
/// own id still deletes the store, its policies and the aliases that named it.
#[tokio::test]
async fn an_alias_still_resolves_for_the_members_that_accept_one() {
    let router = router();
    let case = Case {
        name: "control",
        suffix: "control",
        setup: AliasSetup::Active,
    };
    let (store_id, policy_id, alias_name) = fixture(&router, &case).await;

    // Members documented "use its ID or alias name" are untouched.
    let alias_accepting: &[(&str, Value)] = &[
        ("GetPolicyStore", json!({ "policyStoreId": alias_name })),
        (
            "GetPolicy",
            json!({ "policyStoreId": alias_name, "policyId": policy_id }),
        ),
        ("ListPolicies", json!({ "policyStoreId": alias_name })),
    ];
    for (operation, request) in alias_accepting {
        let answer = ok(&router, operation, request).await;
        // `ListPoliciesOutput` reports the store on each listed policy; the
        // other two report it directly.
        let reported = match *operation {
            "ListPolicies" => answer["policies"][0]["policyStoreId"]
                .as_str()
                .unwrap_or_default(),
            _ => answer["policyStoreId"].as_str().unwrap_or_default(),
        };
        assert_eq!(
            reported, store_id,
            "case {operation:?}: the alias must resolve to its store, got {answer}"
        );
    }

    // The id itself still deletes, and takes the store's contents and the
    // aliases that named it with it.
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    let gone: &[(&str, Value)] = &[
        (
            "GetPolicyStore",
            json!({ "policyStoreId": store_id.clone() }),
        ),
        (
            "GetPolicy",
            json!({ "policyStoreId": store_id.clone(), "policyId": policy_id }),
        ),
        (
            "GetPolicyStoreAlias",
            json!({ "aliasName": alias_name.clone() }),
        ),
    ];
    for (operation, request) in gone {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {operation:?}: a deleted store leaves nothing to read, got {status} {body}"
        );
    }
}
