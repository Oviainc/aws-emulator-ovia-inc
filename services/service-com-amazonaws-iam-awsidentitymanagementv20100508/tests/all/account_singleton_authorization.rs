// SPDX-License-Identifier: CC0-1.0

//! The operations that read and write the ACCOUNT's own configuration, decided against
//! the caller's policies, end to end over the awsQuery wire.
//!
//! The rule under test: IAM authorizes its own API, so an operation that reports or
//! changes the account-wide configuration singleton — rather than one entity the account
//! holds — is decided against the signing principal's identity-based policies on the
//! account-wide resource before it reports or changes anything, and a refusal is HTTP 403
//! `AccessDenied` naming that resource, with the configuration left exactly as it was. A
//! statement reaches such a call only by covering every resource: the resource element of
//! a grant scoped to one entity's ARN does not match it.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! member of the family is called, whether it reports or writes, whether the caller holds
//! no document, a grant naming the action, a grant whose action element is a wildcard, a
//! grant for another member of the same family, a grant scoped to a named entity, or an
//! explicit `Deny` over a grant for everything.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so a request is built and read exactly one way.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// The resource an account-wide call is decided against. The Service Authorization
/// Reference lists no resource type for an IAM action that acts on the account itself, so
/// only a statement covering every resource reaches one.
const ACCOUNT_WIDE: &str = "*";

/// The property the fixture holds before a row calls, and the value a row's
/// `PutAccountProperties` would set: two different values of ONE key, so a refused write
/// is visible as the old value surviving rather than as an absent key.
const PROPERTY_KEY: &str = "RoleManager/Enabled";
const HELD_PROPERTY_VALUE: &str = "false";
const WRITTEN_PROPERTY_VALUE: &str = "true";

/// The alias the fixture holds, and the one a row's `CreateAccountAlias` would replace it
/// with. An account holds at most one alias, so a refused create is visible as the held
/// alias surviving.
const HELD_ALIAS: &str = "payroll-sandbox";
const REPLACEMENT_ALIAS: &str = "payroll-live";

/// The entity a row's resource-scoped grant names: an ARN the account does hold, so the
/// row shows the ARN failing to MATCH the account-wide resource rather than naming nothing.
fn named_entity_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{}:user/{user_name}", account_id())
}

/// An `Allow` for every IAM action on every resource: what a row about an explicit `Deny`
/// is layered on top of, so the row demonstrates the `Deny` and not a missing grant.
fn allow_everything() -> String {
    statement("Allow", "iam:*", ACCOUNT_WIDE)
}

/// The awsQuery parameters carrying one `Properties` pair: the wrapped
/// `Properties.entry.N.key` / `.value` form a query-protocol map arrives in.
fn properties_params(key: &str, value: &str) -> Vec<(String, String)> {
    vec![
        param("Properties.entry.1.key", key),
        param("Properties.entry.1.value", value),
    ]
}

/// One member of the account-singleton family: the awsQuery `Action`, the IAM action the
/// decision is made on, and the form parameters the call carries.
struct Member {
    action: &'static str,
    iam_action: &'static str,
    params: Vec<(String, String)>,
}

/// The family. Every one of these addresses the account's own configuration — its
/// properties and its alias — rather than a named entity, so every one is decided on
/// [`ACCOUNT_WIDE`].
fn family() -> Vec<Member> {
    vec![
        Member {
            action: "GetAccountProperties",
            iam_action: "iam:GetAccountProperties",
            params: Vec::new(),
        },
        Member {
            action: "PutAccountProperties",
            iam_action: "iam:PutAccountProperties",
            params: properties_params(PROPERTY_KEY, WRITTEN_PROPERTY_VALUE),
        },
        Member {
            action: "ListAccountAliases",
            iam_action: "iam:ListAccountAliases",
            params: Vec::new(),
        },
        Member {
            action: "CreateAccountAlias",
            iam_action: "iam:CreateAccountAlias",
            params: vec![param("AccountAlias", REPLACEMENT_ALIAS)],
        },
        Member {
            action: "DeleteAccountAlias",
            iam_action: "iam:DeleteAccountAlias",
            params: vec![param("AccountAlias", HELD_ALIAS)],
        },
    ]
}

/// What a row expects the signed call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the documents in force permit the call.
    Served,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
}

/// One shape of document set the caller holds, written relative to the action the row
/// calls so the same shape is applied to every member of the family.
struct Shape {
    case: &'static str,
    /// The `(PolicyName, PolicyDocument)` pairs to put on the caller, for a row whose
    /// decision is made on `iam_action` and whose sibling in the family is
    /// `other_iam_action`.
    documents: fn(iam_action: &str, other_iam_action: &str, caller_arn: &str) -> Vec<(String, String)>,
    expect: Expect,
}

/// The shapes. Each is applied to every member of the family, so no single operation or
/// document is the specification.
const SHAPES: &[Shape] = &[
    Shape {
        case: "no identity-based policy at all",
        documents: |_action, _other, _caller| Vec::new(),
        expect: Expect::NoAllow,
    },
    Shape {
        case: "an Allow naming the action on every resource",
        documents: |action, _other, _caller| {
            vec![("allow-action".to_string(), statement("Allow", action, ACCOUNT_WIDE))]
        },
        expect: Expect::Served,
    },
    Shape {
        case: "an Allow whose action element is the service wildcard",
        documents: |_action, _other, _caller| vec![("allow-everything".to_string(), allow_everything())],
        expect: Expect::Served,
    },
    Shape {
        case: "an Allow whose action element is a wildcard prefix of the action",
        documents: |action, _other, _caller| {
            // The action with its last three characters replaced by `*`, so the pattern
            // is a prefix of this action and of nothing else in the family.
            let prefix = format!("{}*", &action[..action.len() - 3]);
            vec![("allow-prefix".to_string(), statement("Allow", &prefix, ACCOUNT_WIDE))]
        },
        expect: Expect::Served,
    },
    Shape {
        case: "an Allow for another member of the same family",
        documents: |_action, other, _caller| {
            vec![("allow-sibling".to_string(), statement("Allow", other, ACCOUNT_WIDE))]
        },
        expect: Expect::NoAllow,
    },
    Shape {
        case: "an Allow naming the action but scoped to one entity's ARN",
        documents: |action, _other, caller| {
            vec![("allow-scoped".to_string(), statement("Allow", action, caller))]
        },
        expect: Expect::NoAllow,
    },
    Shape {
        case: "an explicit Deny for the action over a grant for everything",
        documents: |action, _other, _caller| {
            vec![
                ("allow-everything".to_string(), allow_everything()),
                ("deny-action".to_string(), statement("Deny", action, ACCOUNT_WIDE)),
            ]
        },
        expect: Expect::ExplicitDeny,
    },
];

/// The reason clause a refusal closes with, for each of the two refusals.
fn reason(expect: Expect, iam_action: &str) -> String {
    match expect {
        Expect::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        _ => format!("because no identity-based policy allows the {iam_action} action"),
    }
}

/// Put the account into the configuration every row starts from: one property held at
/// [`HELD_PROPERTY_VALUE`] and one alias held, both written by an UNSIGNED call, which
/// carries no IAM user for the gate to decide against.
async fn seed_configuration(router: &MultiServiceRouter, case: &str) {
    let properties = properties_params(PROPERTY_KEY, HELD_PROPERTY_VALUE);
    let (status, body) = call(router, "PutAccountProperties", &borrowed(&properties)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutAccountProperties must succeed, got {status}: {body}"
    );
    let (status, body) = call(router, "CreateAccountAlias", &[("AccountAlias", HELD_ALIAS)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccountAlias must succeed, got {status}: {body}"
    );
}

/// The account's whole configuration as its own readers report it: the properties held and
/// the aliases held.
async fn configuration(
    router: &MultiServiceRouter,
    case: &str,
) -> (BTreeMap<String, String>, Vec<String>) {
    let (status, body) = call(router, "GetAccountProperties", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetAccountProperties to answer 200, got {status}: {body}"
    );
    let properties: BTreeMap<String, String> = elements(&body, "key")
        .into_iter()
        .zip(elements(&body, "value"))
        .collect();
    let (status, body) = call(router, "ListAccountAliases", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListAccountAliases to answer 200, got {status}: {body}"
    );
    (properties, elements(&body, "member"))
}

/// The configuration [`seed_configuration`] leaves behind, which every refused call must
/// leave untouched.
fn seeded_configuration() -> (BTreeMap<String, String>, Vec<String>) {
    (
        [(PROPERTY_KEY.to_string(), HELD_PROPERTY_VALUE.to_string())]
            .into_iter()
            .collect(),
        vec![HELD_ALIAS.to_string()],
    )
}

/// RULE: a call that reports or changes the account's own configuration is decided against
/// the signing principal's identity-based policies on the account-wide resource before it
/// reports or changes anything — so only a statement covering every resource reaches it,
/// and a refusal is HTTP 403 `AccessDenied` naming that resource with the configuration
/// unchanged.
#[tokio::test]
async fn an_account_wide_call_is_decided_on_the_account_wide_resource() {
    let members = family();
    for (position, member) in members.iter().enumerate() {
        // The member's sibling, for the row whose grant names another action of the same
        // family: the next one round the list, so every row's sibling is a different
        // action from its own.
        let sibling = members[(position + 1) % members.len()].iam_action;
        for (row, shape) in SHAPES.iter().enumerate() {
            let case = format!("{} / {}", member.action, shape.case);
            // A router per row, so the documents and the configuration a row sets up are
            // the only ones in force when it calls.
            let router = router();
            seed_configuration(&router, &case).await;
            let user_name = format!("caller{position}{row}");
            let caller_arn = named_entity_arn(&user_name);
            let documents = (shape.documents)(member.iam_action, sibling, &caller_arn);
            let access_key_id = principal(&router, &case, &user_name, &documents).await;

            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                member.action,
                &borrowed(&member.params),
            )
            .await;

            if shape.expect == Expect::Served {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the grant to serve the call, got {status}: {body}"
                );
                continue;
            }

            assert_eq!(
                status, 403,
                "case {case:?}: expected the call to be refused HTTP 403, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected the error code AccessDenied, got {:?}",
                error_code(&body)
            );
            // The refusal names the principal, the action and the resource the decision
            // was made on — the account-wide one, so no ARN appears in it at all.
            assert_access_denied_message(
                &case,
                &element(&body, "Message"),
                &caller_arn,
                member.iam_action,
                ACCOUNT_WIDE,
                &reason(shape.expect, member.iam_action),
            );
            // The decision is made BEFORE the operation reports or changes anything, so a
            // refused call leaves the configuration exactly as the fixture left it.
            let held = configuration(&router, &case).await;
            assert_eq!(
                held,
                seeded_configuration(),
                "case {case:?}: expected the refused call to leave the configuration \
                 untouched, got {held:?}"
            );
        }
    }
}

/// RULE: the resource every member of this family is decided on is the same, so what
/// separates them is the ACTION element alone — a grant naming one member serves exactly
/// that member and refuses the other four.
#[tokio::test]
async fn a_grant_for_one_account_wide_action_serves_only_that_action() {
    let members = family();
    for (position, granted) in members.iter().enumerate() {
        let case = format!("a grant for {} alone", granted.action);
        // A router per grant, so the write a served row performs cannot change what the
        // next grant's rows observe.
        let router = router();
        seed_configuration(&router, &case).await;
        let user_name = format!("granted{position}");
        let access_key_id = principal(
            &router,
            &case,
            &user_name,
            &[(
                "allow-one".to_string(),
                statement("Allow", granted.iam_action, ACCOUNT_WIDE),
            )],
        )
        .await;

        for member in &members {
            let row = format!("{case} / {}", member.action);
            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                member.action,
                &borrowed(&member.params),
            )
            .await;
            if member.iam_action == granted.iam_action {
                assert_eq!(
                    status, 200,
                    "case {row:?}: expected the granted action to be served, got \
                     {status}: {body}"
                );
            } else {
                assert_eq!(
                    status, 403,
                    "case {row:?}: expected every other action to be refused, got \
                     {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {row:?}: expected the error code AccessDenied, got {:?}",
                    error_code(&body)
                );
            }
        }
    }
}
