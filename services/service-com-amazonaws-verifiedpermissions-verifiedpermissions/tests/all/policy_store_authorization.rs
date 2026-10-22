// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the policy-store -> policy -> authorization loop.
//!
//! Every assertion goes over the wire: an awsJson1_0 `POST /` with
//! `X-Amz-Target: VerifiedPermissions.<Operation>`, dispatched through the same
//! `MultiServiceRouter` the emulator binary serves, over ONE shared engine — so a
//! write made by one request is only visible to the next if it really reached
//! state.

use serde_json::{json, Value};

// The wire harness (`router`, `call`, `ok`, `store_with_mode`,
// `store_with_validation_off`, `error_code`, `fault_message`) lives once in
// `tests/all/harness.rs` and is shared with every other suite in this crate.
use super::harness::{
    call, error_code, fault_message as error_message, ok, router, store_with_validation_off,
};

/// `{entityType, entityId}` pairs, sorted, for order-insensitive comparison.
fn pairs(value: &Value, type_key: &str, id_key: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                item[type_key].as_str().unwrap_or_default().to_string(),
                item[id_key].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// The policy store itself
// ---------------------------------------------------------------------------

/// A store's every response field is read back out of state, across the whole
/// range of optional members a create may or may not set.
#[tokio::test]
async fn policy_store_round_trips_every_member_it_was_created_with() {
    // Rows differ along validation mode, whether a description was given,
    // deletion protection, and whether tags were given.
    let cases: &[(&str, Value)] = &[
        ("minimal", json!({ "validationSettings": { "mode": "OFF" } })),
        (
            "described",
            json!({ "validationSettings": { "mode": "OFF" }, "description": "demo" }),
        ),
        (
            "strict-and-protected",
            json!({
                "validationSettings": { "mode": "STRICT" },
                "description": "locked down",
                "deletionProtection": "ENABLED",
            }),
        ),
        (
            "tagged",
            json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": "DISABLED",
                "tags": { "team": "photos", "env": "test" },
            }),
        ),
    ];

    let router = router();
    let mut seen_ids: Vec<String> = Vec::new();
    for (case, request) in cases {
        let created = ok(&router, "CreatePolicyStore", request).await;
        let id = created["policyStoreId"].as_str().unwrap_or_default();
        assert!(
            !id.is_empty() && id.starts_with("PS"),
            "case {case:?}: expected a minted PS-prefixed policyStoreId, got {created}"
        );
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-/_".contains(c)),
            "case {case:?}: policyStoreId must match ^[a-zA-Z0-9-/_]*$, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id.to_string()),
            "case {case:?}: policyStoreId {id:?} was already minted for another store"
        );
        seen_ids.push(id.to_string());
        assert_eq!(
            created["arn"].as_str().unwrap_or_default(),
            format!("arn:aws:verifiedpermissions::000000000000:policy-store/{id}"),
            "case {case:?}: unexpected arn, got {created}"
        );
        assert!(
            !created["createdDate"].is_null() && !created["lastUpdatedDate"].is_null(),
            "case {case:?}: createdDate/lastUpdatedDate are required, got {created}"
        );

        // Reading the store back must report exactly what was persisted.
        let fetched = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": id, "tags": true }),
        )
        .await;
        assert_eq!(
            fetched["policyStoreId"], created["policyStoreId"],
            "case {case:?}: GetPolicyStore reported a different id, got {fetched}"
        );
        assert_eq!(
            fetched["arn"], created["arn"],
            "case {case:?}: GetPolicyStore reported a different arn, got {fetched}"
        );
        assert_eq!(
            fetched["createdDate"], created["createdDate"],
            "case {case:?}: createdDate changed between create and get, got {fetched}"
        );
        assert_eq!(
            fetched["validationSettings"]["mode"],
            request["validationSettings"]["mode"],
            "case {case:?}: validationSettings.mode was not persisted, got {fetched}"
        );
        assert_eq!(
            fetched["description"], request["description"],
            "case {case:?}: description was not persisted, got {fetched}"
        );
        let expected_protection = request["deletionProtection"]
            .as_str()
            .unwrap_or("DISABLED")
            .to_string();
        assert_eq!(
            fetched["deletionProtection"].as_str().unwrap_or_default(),
            expected_protection,
            "case {case:?}: deletionProtection defaults to DISABLED, got {fetched}"
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            fetched["tags"], expected_tags,
            "case {case:?}: tags were not persisted, got {fetched}"
        );

        // `tags` defaults to false, so an unasked-for tag map is absent.
        let untagged = ok(&router, "GetPolicyStore", &json!({ "policyStoreId": id })).await;
        assert!(
            untagged["tags"].is_null(),
            "case {case:?}: tags must be absent unless the request asked for them, got {untagged}"
        );
    }
}

/// An unknown store is a 404, and a deleted store becomes unknown. Deletion
/// protection refuses the delete with the one error the operation models.
#[tokio::test]
async fn deleting_a_policy_store_makes_it_unknown_unless_protection_refuses_it() {
    let router = router();

    let (status, body) = call(
        &router,
        "GetPolicyStore",
        &json!({ "policyStoreId": "PSdoesnotexist" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"unknown store\": expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"unknown store\": unexpected error code, got {body}"
    );

    // Rows differ along deletion protection, which is the only thing that
    // changes the outcome.
    let cases: &[(&str, &str, u16)] = &[
        ("unprotected", "DISABLED", 200),
        ("protected", "ENABLED", 406),
    ];
    for (case, protection, expected_status) in cases {
        let created = ok(
            &router,
            "CreatePolicyStore",
            &json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": protection,
            }),
        )
        .await;
        let id = created["policyStoreId"].as_str().unwrap_or_default();

        let (status, body) = call(
            &router,
            "DeletePolicyStore",
            &json!({ "policyStoreId": id }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from DeletePolicyStore, got {status} {body}"
        );

        let (get_status, get_body) =
            call(&router, "GetPolicyStore", &json!({ "policyStoreId": id })).await;
        let expected_get = if *expected_status == 200 { 404 } else { 200 };
        assert_eq!(
            get_status, expected_get,
            "case {case:?}: expected HTTP {expected_get} from the following GetPolicyStore, got {get_status} {get_body}"
        );
        if *expected_status == 406 {
            assert_eq!(
                error_code(&body),
                "InvalidStateException",
                "case {case:?}: unexpected error code, got {body}"
            );
        }
    }

    // `DeletePolicyStore` is `@idempotent` and models no not-found error, so a
    // repeat delete succeeds.
    let (status, body) = call(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": "PSneverexisted" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"repeat delete\": a delete of an absent store must succeed, got {status} {body}"
    );
}

/// The deletion-protection refusal reports a CONSTANT message.
///
/// `com.amazonaws.verifiedpermissions#InvalidStateException` documents one fixed
/// sentence pair for this refusal and its only member is `message`, so the text
/// is byte-identical on every call and embeds no request-supplied or
/// server-minted value. Rows differ along everything a templated message could
/// have spliced in: the minted store id, the description, the validation mode,
/// the tags, and how many policies the store holds.
#[tokio::test]
async fn the_deletion_protection_refusal_message_is_a_constant_without_identifiers() {
    struct Case {
        name: &'static str,
        request: Value,
        /// Static policies to put in the store before the refused delete. A
        /// STRICT store refuses every static policy (no schema yet), so those
        /// rows carry none.
        policies: usize,
    }
    let cases = [
        Case {
            name: "minimal",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": "ENABLED",
            }),
            policies: 0,
        },
        Case {
            name: "described",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": "ENABLED",
                "description": "production authorization store",
            }),
            policies: 1,
        },
        Case {
            name: "tagged-strict",
            request: json!({
                "validationSettings": { "mode": "STRICT" },
                "deletionProtection": "ENABLED",
                "description": "locked down",
                "tags": { "team": "photos", "env": "prod" },
            }),
            policies: 0,
        },
        Case {
            name: "many-policies",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "deletionProtection": "ENABLED",
            }),
            policies: 3,
        },
    ];

    let router = router();
    let mut messages: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        let name = case.name;
        let created = ok(&router, "CreatePolicyStore", &case.request).await;
        let id = created["policyStoreId"].as_str().unwrap_or_default();
        let arn = created["arn"].as_str().unwrap_or_default();
        for index in 0..case.policies {
            ok(
                &router,
                "CreatePolicy",
                &json!({
                    "policyStoreId": id,
                    "definition": { "static": {
                        "statement": format!(
                            "permit(principal, action, resource == Photo::\"p{index}\");"
                        ),
                    } },
                }),
            )
            .await;
        }

        let (status, body) = call(&router, "DeletePolicyStore", &json!({ "policyStoreId": id }))
            .await;
        assert_eq!(
            status, 406,
            "case {name:?}: expected HTTP 406 for a protected store, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidStateException",
            "case {name:?}: unexpected error code, got {body}"
        );

        let message = error_message(&body);
        assert!(
            !message.is_empty(),
            "case {name:?}: InvalidStateException must carry its required message, got {body}"
        );
        // The documented sentence pair is about deletion protection, and
        // nothing else: no identifier and no echoed request value.
        assert!(
            message.to_lowercase().contains("deletion protection"),
            "case {name:?}: expected the documented deletion-protection sentence, got {message:?}"
        );
        assert!(
            !message.contains(id),
            "case {name:?}: the message must not embed the policy store id {id:?}, got {message:?}"
        );
        assert!(
            !message.contains(arn),
            "case {name:?}: the message must not embed the store arn {arn:?}, got {message:?}"
        );
        for echoed in [
            case.request["description"].as_str(),
            case.request["validationSettings"]["mode"].as_str(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                !message.contains(echoed),
                "case {name:?}: the message must not echo the request value {echoed:?}, \
                 got {message:?}"
            );
        }
        for (key, value) in case.request["tags"]
            .as_object()
            .cloned()
            .unwrap_or_default()
            .iter()
        {
            let value = value.as_str().unwrap_or_default().to_string();
            assert!(
                !message.contains(key.as_str()) && !message.contains(&value),
                "case {name:?}: the message must not echo the tag {key:?}={value:?}, \
                 got {message:?}"
            );
        }

        // The refusal is reporting-only: the store (and its policies) survive,
        // and a second refused delete answers exactly the same text.
        let fetched = ok(&router, "GetPolicyStore", &json!({ "policyStoreId": id })).await;
        assert_eq!(
            fetched["policyStoreId"].as_str().unwrap_or_default(),
            id,
            "case {name:?}: a refused delete must leave the store readable, got {fetched}"
        );
        let listed = ok(&router, "ListPolicies", &json!({ "policyStoreId": id })).await;
        assert_eq!(
            listed["policies"].as_array().map(Vec::len).unwrap_or(0),
            case.policies,
            "case {name:?}: a refused delete must leave the store's policies in place, \
             got {listed}"
        );
        let (again_status, again_body) =
            call(&router, "DeletePolicyStore", &json!({ "policyStoreId": id })).await;
        assert_eq!(
            again_status, 406,
            "case {name:?}: a repeated protected delete must stay refused, \
             got {again_status} {again_body}"
        );
        assert_eq!(
            error_message(&again_body),
            message,
            "case {name:?}: the refusal message must not change between calls, got {again_body}"
        );

        messages.push((name, message));
    }

    // One constant for the whole class: every row reported the same text.
    let (first_case, first_message) = messages
        .first()
        .cloned()
        .expect("the case table is not empty");
    for (name, message) in &messages {
        assert_eq!(
            message, &first_message,
            "case {name:?}: the deletion-protection message must be the same constant as \
             case {first_case:?} reported, got {message:?}"
        );
    }

    // A store without protection is deleted, so the constant belongs to the
    // refusal path alone.
    let unprotected = ok(
        &router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": "OFF" }, "deletionProtection": "DISABLED" }),
    )
    .await;
    let unprotected_id = unprotected["policyStoreId"].as_str().unwrap_or_default();
    let (status, body) = call(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": unprotected_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"unprotected\": an unprotected store must delete, got {status} {body}"
    );
}

/// `UpdatePolicyStore` writes through to the store every later read sees, and
/// disabling deletion protection is what lets the refused delete through.
///
/// Rows differ along which members the call carries: `validationSettings` is
/// required and replaces the mode, `description` follows the request (a call
/// carrying none leaves the store without one), and `deletionProtection` is the
/// one member the model documents as "unchanged unless explicitly included in
/// the call".
#[tokio::test]
async fn update_policy_store_writes_through_and_only_touches_what_the_call_carries() {
    // (case, update request members beyond policyStoreId, expected mode,
    //  expected description, expected deletionProtection)
    let cases: &[(&str, Value, &str, Option<&str>, &str)] = &[
        (
            "mode only, protection untouched",
            json!({ "validationSettings": { "mode": "STRICT" } }),
            "STRICT",
            None,
            "ENABLED",
        ),
        (
            "description replaced",
            json!({
                "validationSettings": { "mode": "OFF" },
                "description": "second description",
            }),
            "OFF",
            Some("second description"),
            "ENABLED",
        ),
        (
            "protection disabled explicitly",
            json!({
                "validationSettings": { "mode": "OFF" },
                "description": "still here",
                "deletionProtection": "DISABLED",
            }),
            "OFF",
            Some("still here"),
            "DISABLED",
        ),
        (
            "protection re-enabled",
            json!({
                "validationSettings": { "mode": "STRICT" },
                "deletionProtection": "ENABLED",
            }),
            "STRICT",
            None,
            "ENABLED",
        ),
    ];

    let router = router();
    for (case, members, expected_mode, expected_description, expected_protection) in cases {
        let created = ok(
            &router,
            "CreatePolicyStore",
            &json!({
                "validationSettings": { "mode": "OFF" },
                "description": "first description",
                "deletionProtection": "ENABLED",
            }),
        )
        .await;
        let id = created["policyStoreId"].as_str().unwrap_or_default();

        let mut request = members.clone();
        request["policyStoreId"] = json!(id);
        let updated = ok(&router, "UpdatePolicyStore", &request).await;
        assert_eq!(
            updated["policyStoreId"].as_str().unwrap_or_default(),
            id,
            "case {case:?}: the update must report the store it changed, got {updated}"
        );
        assert_eq!(
            updated["arn"], created["arn"],
            "case {case:?}: an update must not re-mint the arn, got {updated}"
        );
        assert_eq!(
            updated["createdDate"], created["createdDate"],
            "case {case:?}: createdDate must survive an update, got {updated}"
        );
        assert!(
            updated["lastUpdatedDate"].as_f64().unwrap_or_default()
                >= created["createdDate"].as_f64().unwrap_or_default(),
            "case {case:?}: lastUpdatedDate must not go backwards, got {updated}"
        );

        // Read it back: the update reached state, not just the response.
        let fetched = ok(&router, "GetPolicyStore", &json!({ "policyStoreId": id })).await;
        assert_eq!(
            fetched["validationSettings"]["mode"].as_str().unwrap_or_default(),
            *expected_mode,
            "case {case:?}: validationSettings.mode was not persisted, got {fetched}"
        );
        assert_eq!(
            fetched["description"].as_str(),
            *expected_description,
            "case {case:?}: unexpected description after the update, got {fetched}"
        );
        assert_eq!(
            fetched["deletionProtection"].as_str().unwrap_or_default(),
            *expected_protection,
            "case {case:?}: unexpected deletionProtection after the update, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], updated["lastUpdatedDate"],
            "case {case:?}: the persisted lastUpdatedDate must match the response, got {fetched}"
        );

        // Deletion protection decides the delete, from state alone.
        let (status, body) = call(&router, "DeletePolicyStore", &json!({ "policyStoreId": id })).await;
        let expected_status = if *expected_protection == "ENABLED" {
            406
        } else {
            200
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from DeletePolicyStore after the \
             update, got {status} {body}"
        );
    }

    // An unknown store is the 404 the operation models.
    let (status, body) = call(
        &router,
        "UpdatePolicyStore",
        &json!({
            "policyStoreId": "PSdoesnotexist",
            "validationSettings": { "mode": "OFF" },
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"unknown store\": expected 404 from UpdatePolicyStore, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"unknown store\": unexpected error code, got {body}"
    );
}

/// An `@idempotencyToken` replay answers with the store the first call created;
/// the same token with different parameters conflicts.
#[tokio::test]
async fn a_client_token_replay_returns_the_first_store_and_conflicts_on_new_parameters() {
    let router = router();
    let request = json!({
        "validationSettings": { "mode": "OFF" },
        "description": "first",
        "clientToken": "token-1",
    });

    let first = ok(&router, "CreatePolicyStore", &request).await;
    let replay = ok(&router, "CreatePolicyStore", &request).await;
    assert_eq!(
        replay["policyStoreId"], first["policyStoreId"],
        "case \"replay\": the same clientToken must not create a second store, got {replay}"
    );
    assert_eq!(
        replay["createdDate"], first["createdDate"],
        "case \"replay\": the replay must report the first store's createdDate, got {replay}"
    );

    let (status, body) = call(
        &router,
        "CreatePolicyStore",
        &json!({
            "validationSettings": { "mode": "OFF" },
            "description": "second",
            "clientToken": "token-1",
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case \"token reuse with new parameters\": expected 409, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case \"token reuse with new parameters\": unexpected error code, got {body}"
    );

    // An idempotency token is scoped to ONE operation, so the same token value
    // presented to a different operation is a fresh token, not a conflict.
    let store_id = first["policyStoreId"].as_str().unwrap_or_default();
    let policy_request = json!({
        "policyStoreId": store_id,
        "clientToken": "token-1",
        "definition": { "static": { "statement":
            "permit(principal == User::\"erin\", action == Action::\"view\", resource == Photo::\"p\");" } },
    });
    let policy = ok(&router, "CreatePolicy", &policy_request).await;
    let policy_replay = ok(&router, "CreatePolicy", &policy_request).await;
    assert_eq!(
        policy_replay["policyId"], policy["policyId"],
        "case \"same token on another operation\": the replay must answer with the first policy, got {policy_replay}"
    );
}

// ---------------------------------------------------------------------------
// Policies: the scope comes from the Cedar statement
// ---------------------------------------------------------------------------

/// One row of the scope table: a Cedar statement and the scope the Cedar grammar
/// says it has.
struct ScopeCase {
    case: &'static str,
    statement: &'static str,
    effect: &'static str,
    principal: Option<(&'static str, &'static str)>,
    resource: Option<(&'static str, &'static str)>,
    actions: &'static [(&'static str, &'static str)],
}

/// `CreatePolicy` reads `effect` and the scope off the Cedar statement, and
/// `GetPolicy` gives the statement back byte-identical.
///
/// The rows span the Cedar scope grammar: both effects, each principal/resource
/// constraint form (`==`, `in`, `is`, `is .. in`, bare), one and several actions,
/// a namespaced entity type, and a statement carrying a `when` condition.
#[tokio::test]
async fn a_policys_effect_and_scope_come_from_its_cedar_statement() {
    let cases = &[
        ScopeCase {
            case: "permit with an equality scope",
            statement:
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
            effect: "Permit",
            principal: Some(("User", "alice")),
            resource: Some(("Photo", "vacation")),
            actions: &[("Action", "view")],
        },
        ScopeCase {
            case: "forbid with an unconstrained scope",
            statement: "forbid(principal == User::\"bob\", action, resource);",
            effect: "Forbid",
            principal: Some(("User", "bob")),
            resource: None,
            actions: &[],
        },
        ScopeCase {
            case: "any principal, resource in a group",
            statement:
                "permit(principal, action == Action::\"list\", resource in Album::\"trips\");",
            effect: "Permit",
            principal: None,
            resource: Some(("Album", "trips")),
            actions: &[("Action", "list")],
        },
        ScopeCase {
            case: "principal in a group, two actions, resource is a type",
            statement:
                "permit(principal in Group::\"admins\", action in [Action::\"view\", Action::\"edit\"], resource is Photo);",
            effect: "Permit",
            principal: Some(("Group", "admins")),
            resource: None,
            actions: &[("Action", "view"), ("Action", "edit")],
        },
        ScopeCase {
            case: "namespaced entity types",
            statement:
                "permit(principal is PhotoFlash::User in PhotoFlash::Group::\"eng\", action == PhotoFlash::Action::\"view\", resource == PhotoFlash::Photo::\"p-1\");",
            effect: "Permit",
            principal: Some(("PhotoFlash::Group", "eng")),
            resource: Some(("PhotoFlash::Photo", "p-1")),
            actions: &[("PhotoFlash::Action", "view")],
        },
        ScopeCase {
            case: "a when condition does not change the scope",
            statement:
                "permit(principal == User::\"carol\", action == Action::\"view\", resource == Photo::\"p\") when { context.mfa == true };",
            effect: "Permit",
            principal: Some(("User", "carol")),
            resource: Some(("Photo", "p")),
            actions: &[("Action", "view")],
        },
    ];

    let router = router();
    let store_id = store_with_validation_off(&router).await;

    for ScopeCase {
        case,
        statement,
        effect,
        principal,
        resource,
        actions,
    } in cases
    {
        let created = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement, "description": case } },
            }),
        )
        .await;

        assert_eq!(
            created["policyType"].as_str().unwrap_or_default(),
            "STATIC",
            "case {case:?}: a static definition makes a STATIC policy, got {created}"
        );
        assert_eq!(
            created["effect"].as_str().unwrap_or_default(),
            *effect,
            "case {case:?}: unexpected effect, got {created}"
        );
        let expected_principal = principal.map(|(t, i)| (t.to_string(), i.to_string()));
        let actual_principal = created["principal"].as_object().map(|p| {
            (
                p["entityType"].as_str().unwrap_or_default().to_string(),
                p["entityId"].as_str().unwrap_or_default().to_string(),
            )
        });
        assert_eq!(
            actual_principal, expected_principal,
            "case {case:?}: unexpected principal, got {created}"
        );
        let expected_resource = resource.map(|(t, i)| (t.to_string(), i.to_string()));
        let actual_resource = created["resource"].as_object().map(|r| {
            (
                r["entityType"].as_str().unwrap_or_default().to_string(),
                r["entityId"].as_str().unwrap_or_default().to_string(),
            )
        });
        assert_eq!(
            actual_resource, expected_resource,
            "case {case:?}: unexpected resource, got {created}"
        );
        let mut expected_actions: Vec<(String, String)> = actions
            .iter()
            .map(|(t, i)| ((*t).to_string(), (*i).to_string()))
            .collect();
        expected_actions.sort();
        assert_eq!(
            pairs(&created["actions"], "actionType", "actionId"),
            expected_actions,
            "case {case:?}: unexpected actions, got {created}"
        );

        // The reader must give the statement back exactly, and agree with the
        // creator on every derived member.
        let policy_id = created["policyId"].as_str().unwrap_or_default();
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["definition"]["static"]["statement"]
                .as_str()
                .unwrap_or_default(),
            *statement,
            "case {case:?}: GetPolicy must return the statement byte-identical, got {fetched}"
        );
        assert_eq!(
            fetched["definition"]["static"]["description"]
                .as_str()
                .unwrap_or_default(),
            *case,
            "case {case:?}: the static definition's description was not persisted, got {fetched}"
        );
        assert_eq!(
            fetched["effect"], created["effect"],
            "case {case:?}: GetPolicy disagreed with CreatePolicy on effect, got {fetched}"
        );
        assert_eq!(
            fetched["principal"], created["principal"],
            "case {case:?}: GetPolicy disagreed with CreatePolicy on principal, got {fetched}"
        );
        assert_eq!(
            fetched["createdDate"], created["createdDate"],
            "case {case:?}: createdDate changed between create and get, got {fetched}"
        );
    }
}

/// A statement the Cedar parser refuses is refused here, as a 400
/// `ValidationException` — never stored and never guessed at.
#[tokio::test]
async fn an_unparseable_statement_is_refused_and_nothing_is_stored() {
    // Rows differ along HOW the Cedar text is broken.
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("not cedar at all", "this is not a policy"),
        (
            "missing semicolon",
            "permit(principal, action, resource)",
        ),
        (
            "unknown effect keyword",
            "allow(principal, action, resource);",
        ),
        (
            "unbalanced condition",
            "permit(principal, action, resource) when { context.a == ;",
        ),
    ];

    let router = router();
    let store_id = store_with_validation_off(&router).await;

    for (case, statement) in cases {
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unparseable statement must be refused with 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }

    let listed = ok(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    assert_eq!(
        listed["policies"].as_array().map(Vec::len),
        Some(0),
        "no refused statement may have been stored, got {listed}"
    );
}

/// Unknown stores and policies, a duplicate policy name, and a STRICT store with
/// no schema: each refusal the policy operations model.
#[tokio::test]
async fn policy_operations_refuse_what_the_model_says_they_refuse() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let statement =
        "permit(principal == User::\"dana\", action == Action::\"view\", resource == Photo::\"p\");";

    // An unknown store is a 404 on every one of these operations.
    for operation in ["CreatePolicy", "GetPolicy", "ListPolicies", "IsAuthorized"] {
        let body = match operation {
            "CreatePolicy" => json!({
                "policyStoreId": "PSnope",
                "definition": { "static": { "statement": statement } },
            }),
            "GetPolicy" => json!({ "policyStoreId": "PSnope", "policyId": "SPnope" }),
            "ListPolicies" => json!({ "policyStoreId": "PSnope" }),
            _ => json!({
                "policyStoreId": "PSnope",
                "principal": { "entityType": "User", "entityId": "dana" },
                "action": { "actionType": "Action", "actionId": "view" },
                "resource": { "entityType": "Photo", "entityId": "p" },
            }),
        };
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            status, 404,
            "case {operation:?} on an unknown store: expected 404, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {operation:?} on an unknown store: unexpected error code, got {response}"
        );
    }

    // An unknown policy in a store that does exist is also a 404.
    let (status, response) = call(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": "SPnope" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"unknown policy\": expected 404, got {status} {response}"
    );

    // A policy name is unique within its store.
    let named = json!({
        "policyStoreId": store_id,
        "name": "view-photos",
        "definition": { "static": { "statement": statement } },
    });
    let created = ok(&router, "CreatePolicy", &named).await;
    let (status, response) = call(&router, "CreatePolicy", &named).await;
    assert_eq!(
        status, 409,
        "case \"duplicate name\": expected 409, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "case \"duplicate name\": unexpected error code, got {response}"
    );

    // A named policy is reachable by `name/<name>` as well as by its id.
    let by_name = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": "name/view-photos" }),
    )
    .await;
    assert_eq!(
        by_name["policyId"], created["policyId"],
        "case \"lookup by name\": name/<name> must resolve to the same policy, got {by_name}"
    );

    // `ValidationSettings` documents that a STRICT store with no schema rejects
    // every static policy.
    let strict_store = ok(
        &router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": "STRICT" } }),
    )
    .await;
    let (status, response) = call(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": strict_store["policyStoreId"],
            "definition": { "static": { "statement": statement } },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"STRICT store with no schema\": expected 400, got {status} {response}"
    );

    // A template-linked definition has no template to link to yet, so it answers
    // the scaffold's 501 rather than a fabricated policy.
    let (status, response) = call(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "templateLinked": { "policyTemplateId": "PTnope" } },
        }),
    )
    .await;
    assert_eq!(
        status, 501,
        "case \"templateLinked definition\": expected 501, got {status} {response}"
    );
}

// ---------------------------------------------------------------------------
// ListPolicies
// ---------------------------------------------------------------------------

/// `maxResults` truncates, `nextToken` resumes exactly where the page stopped,
/// the last page carries no token, and `filter` selects by scope and type.
#[tokio::test]
async fn list_policies_paginates_and_filters_over_persisted_policies() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // Four policies with distinct scopes, so a filter has something to select.
    let statements = [
        "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"a\");",
        "permit(principal == User::\"bob\", action == Action::\"view\", resource == Photo::\"b\");",
        "forbid(principal == User::\"alice\", action == Action::\"edit\", resource == Photo::\"c\");",
        "permit(principal, action == Action::\"list\", resource == Album::\"all\");",
    ];
    let mut created_ids = Vec::new();
    for statement in statements {
        let created = ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        created_ids.push(
            created["policyId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    created_ids.sort();

    // Rows differ along page size, which is what the rule generalises over.
    for page_size in [1usize, 2, 3, 4, 5] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "policyStoreId": store_id, "maxResults": page_size });
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, "ListPolicies", &request).await;
            let items = page["policies"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case page_size={page_size}: a page may not exceed maxResults, got {page}"
            );
            for item in &items {
                assert_eq!(
                    item["policyStoreId"].as_str().unwrap_or_default(),
                    store_id,
                    "case page_size={page_size}: a PolicyItem must name its store, got {item}"
                );
                assert_eq!(
                    item["policyType"].as_str().unwrap_or_default(),
                    "STATIC",
                    "case page_size={page_size}: unexpected policyType, got {item}"
                );
                assert!(
                    item["definition"]["static"].is_object(),
                    "case page_size={page_size}: definition is required on a PolicyItem, got {item}"
                );
                assert!(
                    !item["createdDate"].is_null() && !item["lastUpdatedDate"].is_null(),
                    "case page_size={page_size}: timestamps are required on a PolicyItem, got {item}"
                );
                walked.push(item["policyId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= statements.len() + 1,
                "case page_size={page_size}: pagination did not terminate"
            );
            match page["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        next.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "-_=+/.".contains(c)),
                        "case page_size={page_size}: nextToken must match ^[A-Za-z0-9-_=+/\\.]*$, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        walked.sort();
        assert_eq!(
            walked, created_ids,
            "case page_size={page_size}: walking every page must yield each policy exactly once"
        );
    }

    // No `maxResults` is one page with no cursor.
    let all = ok(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    assert_eq!(
        all["policies"].as_array().map(Vec::len),
        Some(statements.len()),
        "an unbounded call returns every policy, got {all}"
    );
    assert!(
        all["nextToken"].is_null(),
        "the last page carries no nextToken, got {all}"
    );

    // A cursor this service did not mint is a bad request, not an empty page.
    let (status, body) = call(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": store_id, "nextToken": "zzzz" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"forged nextToken\": expected 400, got {status} {body}"
    );

    // Rows differ along which filter member is set.
    let filter_cases: &[(&str, Value, usize)] = &[
        (
            "by principal identifier",
            json!({ "principal": { "identifier": { "entityType": "User", "entityId": "alice" } } }),
            2,
        ),
        (
            "by unspecified principal",
            json!({ "principal": { "unspecified": true } }),
            1,
        ),
        (
            "by resource identifier",
            json!({ "resource": { "identifier": { "entityType": "Photo", "entityId": "b" } } }),
            1,
        ),
        ("by policy type", json!({ "policyType": "STATIC" }), 4),
        (
            "by a type nothing has",
            json!({ "policyType": "TEMPLATE_LINKED" }),
            0,
        ),
        (
            "principal and resource together",
            json!({
                "principal": { "identifier": { "entityType": "User", "entityId": "alice" } },
                "resource": { "identifier": { "entityType": "Photo", "entityId": "c" } },
            }),
            1,
        ),
    ];
    for (case, filter, expected) in filter_cases {
        let filtered = ok(
            &router,
            "ListPolicies",
            &json!({ "policyStoreId": store_id, "filter": filter }),
        )
        .await;
        assert_eq!(
            filtered["policies"].as_array().map(Vec::len),
            Some(*expected),
            "case {case:?}: unexpected match count, got {filtered}"
        );
    }
}

// ---------------------------------------------------------------------------
// IsAuthorized
// ---------------------------------------------------------------------------

/// `IsAuthorized` evaluates the store's persisted policies: default-deny with no
/// matching policy, allow on a matching `Permit`, and `Forbid` beating `Permit`.
/// `determiningPolicies` names the policies that decided it.
#[tokio::test]
async fn is_authorized_evaluates_the_stores_persisted_policies() {
    /// One row: the statements to store, the request triple, and the decision.
    struct AuthCase {
        case: &'static str,
        statements: &'static [&'static str],
        principal: (&'static str, &'static str),
        action: (&'static str, &'static str),
        resource: (&'static str, &'static str),
        decision: &'static str,
        /// How many policies must appear in `determiningPolicies`.
        determining: usize,
        /// How many evaluation errors Cedar must report. Reading an attribute a
        /// record does not have is an error in Cedar, not a silent `false`.
        errors: usize,
    }

    let cases = &[
        AuthCase {
            case: "an empty store default-denies",
            statements: &[],
            principal: ("User", "alice"),
            action: ("Action", "view"),
            resource: ("Photo", "vacation"),
            decision: "DENY",
            determining: 0,
            errors: 0,
        },
        AuthCase {
            case: "a matching permit allows",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
            ],
            principal: ("User", "alice"),
            action: ("Action", "view"),
            resource: ("Photo", "vacation"),
            decision: "ALLOW",
            determining: 1,
            errors: 0,
        },
        AuthCase {
            case: "a non-matching principal denies",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
            ],
            principal: ("User", "bob"),
            action: ("Action", "view"),
            resource: ("Photo", "vacation"),
            decision: "DENY",
            determining: 0,
            errors: 0,
        },
        AuthCase {
            case: "a non-matching action denies",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
            ],
            principal: ("User", "alice"),
            action: ("Action", "edit"),
            resource: ("Photo", "vacation"),
            decision: "DENY",
            determining: 0,
            errors: 0,
        },
        AuthCase {
            case: "a non-matching resource denies",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
            ],
            principal: ("User", "alice"),
            action: ("Action", "view"),
            resource: ("Photo", "other"),
            decision: "DENY",
            determining: 0,
            errors: 0,
        },
        AuthCase {
            case: "forbid beats permit",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
                "forbid(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
            ],
            principal: ("User", "alice"),
            action: ("Action", "view"),
            resource: ("Photo", "vacation"),
            decision: "DENY",
            determining: 1,
            errors: 0,
        },
        AuthCase {
            case: "a wide permit allows anything",
            statements: &["permit(principal, action, resource);"],
            principal: ("User", "zoe"),
            action: ("Action", "anything"),
            resource: ("Photo", "whatever"),
            decision: "ALLOW",
            determining: 1,
            errors: 0,
        },
        AuthCase {
            case: "two matching permits both determine the allow",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");",
                "permit(principal, action == Action::\"view\", resource == Photo::\"vacation\");",
            ],
            principal: ("User", "alice"),
            action: ("Action", "view"),
            resource: ("Photo", "vacation"),
            decision: "ALLOW",
            determining: 2,
            errors: 0,
        },
        AuthCase {
            case: "a condition reading an absent context attribute errors and denies",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\") when { context.mfa == true };",
            ],
            principal: ("User", "alice"),
            action: ("Action", "view"),
            resource: ("Photo", "vacation"),
            decision: "DENY",
            determining: 0,
            errors: 1,
        },
        AuthCase {
            case: "a namespaced policy matches a namespaced request",
            statements: &[
                "permit(principal == PhotoFlash::User::\"alice\", action == PhotoFlash::Action::\"view\", resource == PhotoFlash::Photo::\"p\");",
            ],
            principal: ("PhotoFlash::User", "alice"),
            action: ("PhotoFlash::Action", "view"),
            resource: ("PhotoFlash::Photo", "p"),
            decision: "ALLOW",
            determining: 1,
            errors: 0,
        },
    ];

    for AuthCase {
        case,
        statements,
        principal,
        action,
        resource,
        decision,
        determining,
        errors,
    } in cases
    {
        // A fresh store per row, so one row's policies cannot decide another's.
        let router = router();
        let store_id = store_with_validation_off(&router).await;
        let mut stored_ids = Vec::new();
        for statement in *statements {
            let created = ok(
                &router,
                "CreatePolicy",
                &json!({
                    "policyStoreId": store_id,
                    "definition": { "static": { "statement": statement } },
                }),
            )
            .await;
            stored_ids.push(
                created["policyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        let answer = ok(
            &router,
            "IsAuthorized",
            &json!({
                "policyStoreId": store_id,
                "principal": { "entityType": principal.0, "entityId": principal.1 },
                "action": { "actionType": action.0, "actionId": action.1 },
                "resource": { "entityType": resource.0, "entityId": resource.1 },
            }),
        )
        .await;

        assert_eq!(
            answer["decision"].as_str().unwrap_or_default(),
            *decision,
            "case {case:?}: unexpected decision, got {answer}"
        );
        let determined = answer["determiningPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            determined.len(),
            *determining,
            "case {case:?}: unexpected determiningPolicies count, got {answer}"
        );
        for item in &determined {
            let policy_id = item["policyId"].as_str().unwrap_or_default().to_string();
            assert!(
                stored_ids.contains(&policy_id),
                "case {case:?}: determiningPolicies named {policy_id:?}, which this store never held"
            );
        }
        assert_eq!(
            answer["errors"].as_array().map(Vec::len),
            Some(*errors),
            "case {case:?}: unexpected evaluation-error count, got {answer}"
        );
    }
}

/// The decision follows the stored policies: it changes when a policy is created
/// and changes back when it is deleted.
#[tokio::test]
async fn the_decision_follows_the_stored_policies() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let request = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "User", "entityId": "alice" },
        "action": { "actionType": "Action", "actionId": "view" },
        "resource": { "entityType": "Photo", "entityId": "vacation" },
    });

    let before = ok(&router, "IsAuthorized", &request).await;
    assert_eq!(
        before["decision"].as_str().unwrap_or_default(),
        "DENY",
        "case \"before create\": an empty store default-denies, got {before}"
    );

    let created = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement":
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");" } },
        }),
    )
    .await;
    let policy_id = created["policyId"].as_str().unwrap_or_default().to_string();

    let after_create = ok(&router, "IsAuthorized", &request).await;
    assert_eq!(
        after_create["decision"].as_str().unwrap_or_default(),
        "ALLOW",
        "case \"after create\": the stored permit must allow, got {after_create}"
    );
    assert_eq!(
        after_create["determiningPolicies"][0]["policyId"]
            .as_str()
            .unwrap_or_default(),
        policy_id,
        "case \"after create\": determiningPolicies must name the created policy, got {after_create}"
    );

    ok(
        &router,
        "DeletePolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;

    let after_delete = ok(&router, "IsAuthorized", &request).await;
    assert_eq!(
        after_delete["decision"].as_str().unwrap_or_default(),
        "DENY",
        "case \"after delete\": the deleted permit must no longer allow, got {after_delete}"
    );
    let (status, body) = call(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"after delete\": GetPolicy must answer 404, got {status} {body}"
    );

    // Deleting the store removes its policies too.
    let second = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement": "permit(principal, action, resource);" } },
        }),
    )
    .await;
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    let (status, body) = call(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": second["policyId"] }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"store deleted\": a policy of a deleted store must be gone, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"store deleted\": ListPolicies must answer 404, got {status} {body}"
    );
}

/// `context` and `entities` reach the Cedar evaluator: a condition on
/// `context`, and a hierarchy supplied through `entities`, both change the
/// decision.
#[tokio::test]
async fn context_and_entities_reach_the_cedar_evaluator() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement":
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p\") when { context.mfa == true && context.attempts < 3 };" } },
        }),
    )
    .await;
    ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement":
                "permit(principal in Group::\"admins\", action == Action::\"edit\", resource == Photo::\"p\");" } },
        }),
    )
    .await;

    // Rows differ along the context values, which is what the condition reads.
    let context_cases: &[(&str, Value, &str)] = &[
        ("no context", Value::Null, "DENY"),
        (
            "condition satisfied",
            json!({ "contextMap": { "mfa": { "boolean": true }, "attempts": { "long": 1 } } }),
            "ALLOW",
        ),
        (
            "mfa false",
            json!({ "contextMap": { "mfa": { "boolean": false }, "attempts": { "long": 1 } } }),
            "DENY",
        ),
        (
            "attempts past the bound",
            json!({ "contextMap": { "mfa": { "boolean": true }, "attempts": { "long": 3 } } }),
            "DENY",
        ),
        (
            "context as cedarJson",
            json!({ "cedarJson": "{\"mfa\": true, \"attempts\": 0}" }),
            "ALLOW",
        ),
    ];
    for (case, context, expected) in context_cases {
        let mut request = json!({
            "policyStoreId": store_id,
            "principal": { "entityType": "User", "entityId": "alice" },
            "action": { "actionType": "Action", "actionId": "view" },
            "resource": { "entityType": "Photo", "entityId": "p" },
        });
        if !context.is_null() {
            request["context"] = context.clone();
        }
        let answer = ok(&router, "IsAuthorized", &request).await;
        assert_eq!(
            answer["decision"].as_str().unwrap_or_default(),
            *expected,
            "case {case:?}: unexpected decision, got {answer}"
        );
    }

    // Rows differ along whether the entity hierarchy puts the principal in the
    // group the policy names.
    let entity_cases: &[(&str, Value, &str)] = &[
        ("no hierarchy", Value::Null, "DENY"),
        (
            "principal in the group",
            json!({ "entityList": [{
                "identifier": { "entityType": "User", "entityId": "mallory" },
                "parents": [{ "entityType": "Group", "entityId": "admins" }],
            }] }),
            "ALLOW",
        ),
        (
            "principal in another group",
            json!({ "entityList": [{
                "identifier": { "entityType": "User", "entityId": "mallory" },
                "parents": [{ "entityType": "Group", "entityId": "guests" }],
            }] }),
            "DENY",
        ),
        (
            "hierarchy as cedarJson",
            json!({ "cedarJson": "[{\"uid\": {\"type\": \"User\", \"id\": \"mallory\"}, \"attrs\": {}, \"parents\": [{\"type\": \"Group\", \"id\": \"admins\"}]}]" }),
            "ALLOW",
        ),
    ];
    for (case, entities, expected) in entity_cases {
        let mut request = json!({
            "policyStoreId": store_id,
            "principal": { "entityType": "User", "entityId": "mallory" },
            "action": { "actionType": "Action", "actionId": "edit" },
            "resource": { "entityType": "Photo", "entityId": "p" },
        });
        if !entities.is_null() {
            request["entities"] = entities.clone();
        }
        let answer = ok(&router, "IsAuthorized", &request).await;
        assert_eq!(
            answer["decision"].as_str().unwrap_or_default(),
            *expected,
            "case {case:?}: unexpected decision, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// Bounds on the in-process evaluator
// ---------------------------------------------------------------------------

/// Every size, count and depth bound on the Cedar evaluator turns a pathological
/// payload into a bounded modeled error, never into unbounded work.
///
/// The rows differ along WHICH dimension is abused — statement size, `cedarJson`
/// size, nesting depth (through both the modelled union and raw JSON), collection
/// length and entity count — and each dimension is also exercised AT its bound,
/// where the request must still succeed.
#[tokio::test]
async fn evaluator_input_bounds_refuse_instead_of_evaluating() {
    /// `MAX_STATEMENT_BYTES` in `src/state/cedar.rs`.
    const MAX_STATEMENT_BYTES: usize = 10_000;
    /// `MAX_CEDAR_JSON_BYTES` in `src/state/cedar.rs`.
    const MAX_CEDAR_JSON_BYTES: usize = 64 * 1024;
    /// `MAX_VALUE_DEPTH` in `src/state/cedar.rs`.
    const MAX_VALUE_DEPTH: usize = 32;
    /// `MAX_COLLECTION_LEN` in `src/state/cedar.rs`.
    const MAX_COLLECTION_LEN: usize = 1_000;
    /// `MAX_ENTITIES_PER_REQUEST` in `src/state/cedar.rs`.
    const MAX_ENTITIES_PER_REQUEST: usize = 100;

    /// A valid Cedar statement padded with a trailing comment to exactly `bytes`.
    fn statement_of(bytes: usize) -> String {
        let head =
            "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p\");\n// ";
        format!("{head}{}", "x".repeat(bytes - head.len()))
    }

    /// A `contextMap` value nesting `levels` Cedar records deep.
    fn nested_record(levels: usize) -> Value {
        let mut value = json!({ "boolean": true });
        for _ in 0..levels {
            value = json!({ "record": { "k": value } });
        }
        value
    }

    /// A raw JSON object nesting `levels` deep.
    fn nested_json(levels: usize) -> Value {
        let mut value = json!(true);
        for _ in 0..levels {
            value = json!({ "k": value });
        }
        value
    }

    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // ---- statement size ------------------------------------------------
    let size_cases: &[(&str, usize, u16)] = &[
        ("statement at the bound", MAX_STATEMENT_BYTES, 200),
        ("statement one byte over", MAX_STATEMENT_BYTES + 1, 400),
        ("statement far over", MAX_STATEMENT_BYTES * 8, 400),
    ];
    for (case, bytes, expected) in size_cases {
        let statement = statement_of(*bytes);
        assert_eq!(
            statement.len(),
            *bytes,
            "case {case:?}: the padded statement should be {bytes} bytes, got {}",
            statement.len()
        );
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: an over-size statement must be a ValidationException, got {body}"
            );
        }
    }

    // ---- context: depth, collection length, cedarJson size -------------
    let base_request = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "User", "entityId": "alice" },
        "action": { "actionType": "Action", "actionId": "view" },
        "resource": { "entityType": "Photo", "entityId": "p" },
    });

    let big_set = json!({ "set": (0..=MAX_COLLECTION_LEN).map(|n| json!({ "long": n })).collect::<Vec<_>>() });
    let at_bound_set =
        json!({ "set": (0..MAX_COLLECTION_LEN).map(|n| json!({ "long": n })).collect::<Vec<_>>() });
    let oversized_json = {
        let filler = "x".repeat(MAX_CEDAR_JSON_BYTES);
        serde_json::to_string(&json!({ "filler": filler })).expect("serializes")
    };

    let context_cases: &[(&str, Value, u16)] = &[
        (
            "record nesting inside the depth bound",
            json!({ "contextMap": { "k": nested_record(MAX_VALUE_DEPTH - 2) } }),
            200,
        ),
        (
            "record nesting past the depth bound",
            json!({ "contextMap": { "k": nested_record(MAX_VALUE_DEPTH * 2) } }),
            400,
        ),
        ("set at the length bound", json!({ "contextMap": { "k": at_bound_set } }), 200),
        ("set past the length bound", json!({ "contextMap": { "k": big_set } }), 400),
        (
            "cedarJson nesting past the depth bound",
            json!({ "cedarJson": serde_json::to_string(&nested_json(MAX_VALUE_DEPTH * 2)).expect("serializes") }),
            400,
        ),
        (
            "cedarJson past the size bound",
            json!({ "cedarJson": oversized_json }),
            400,
        ),
    ];
    for (case, context, expected) in context_cases {
        let mut request = base_request.clone();
        request["context"] = context.clone();
        let (status, body) = call(&router, "IsAuthorized", &request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: a payload past a bound must be a ValidationException, got {body}"
            );
        }
    }

    // ---- entities: count bound -----------------------------------------
    let entity_at = |n: usize| {
        json!({
            "identifier": { "entityType": "User", "entityId": format!("u{n}") },
        })
    };
    let entity_cases: &[(&str, usize, u16)] = &[
        ("entities at the count bound", MAX_ENTITIES_PER_REQUEST, 200),
        (
            "entities one past the count bound",
            MAX_ENTITIES_PER_REQUEST + 1,
            400,
        ),
    ];
    for (case, count, expected) in entity_cases {
        let mut request = base_request.clone();
        request["entities"] = json!({
            "entityList": (0..*count).map(entity_at).collect::<Vec<_>>(),
        });
        let (status, body) = call(&router, "IsAuthorized", &request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
    }
}

/// The policy-store quota is enforced: creation succeeds up to it and is refused
/// with the modeled quota error past it, so the state map cannot grow without
/// limit.
#[tokio::test]
async fn the_policy_store_quota_bounds_how_many_stores_exist() {
    /// `MAX_POLICY_STORES` in `src/state.rs`.
    const MAX_POLICY_STORES: usize = 100;

    let router = router();
    let request = json!({ "validationSettings": { "mode": "OFF" } });
    for n in 0..MAX_POLICY_STORES {
        let (status, body) = call(&router, "CreatePolicyStore", &request).await;
        assert_eq!(
            status, 200,
            "case \"store {n} of {MAX_POLICY_STORES}\": creation inside the quota must succeed, got {status} {body}"
        );
    }
    let (status, body) = call(&router, "CreatePolicyStore", &request).await;
    assert_eq!(
        status, 402,
        "case \"one store past the quota\": expected 402, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"one store past the quota\": unexpected error code, got {body}"
    );

    // The refusal must not have minted a store: the quota is checked before
    // anything is inserted, so a second attempt sees the same refusal.
    let (status, body) = call(&router, "CreatePolicyStore", &request).await;
    assert_eq!(
        status, 402,
        "case \"second attempt past the quota\": expected 402, got {status} {body}"
    );
}

/// `ListPolicies` is `@paginated`, so an absent `maxResults` takes the default
/// page size its own documentation states ("the operation defaults to 10
/// policies per response") and an over-large one is clamped to the documented
/// cap ("a maximum of 50 policies per response"): one small request can never
/// materialise an unbounded response.
#[tokio::test]
async fn list_policies_bounds_the_page_it_builds() {
    /// `DEFAULT_PAGE_SIZE` in `src/state.rs`, the documented default of 10.
    const DEFAULT_PAGE_SIZE: usize = 10;
    /// `MAX_PAGE_SIZE` in `src/state.rs`, the documented maximum of 50.
    const MAX_PAGE_SIZE: usize = 50;

    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let total = MAX_PAGE_SIZE + 5;
    for n in 0..total {
        ok(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": format!(
                    "permit(principal == User::\"u{n}\", action == Action::\"view\", resource == Photo::\"p\");"
                ) } },
            }),
        )
        .await;
    }

    // Rows differ along what the caller asked for: nothing, below the default,
    // between the default and the cap, exactly the cap, and past it.
    let cases: &[(&str, Value, usize, bool)] = &[
        ("no maxResults", Value::Null, DEFAULT_PAGE_SIZE, true),
        ("maxResults below the default", json!(7), 7, true),
        (
            "maxResults between the default and the cap",
            json!(23),
            23,
            true,
        ),
        (
            "maxResults at the cap",
            json!(MAX_PAGE_SIZE),
            MAX_PAGE_SIZE,
            true,
        ),
        (
            "maxResults above the cap",
            json!(2_000),
            MAX_PAGE_SIZE,
            true,
        ),
    ];
    for (case, max_results, expected_len, expect_token) in cases {
        let mut request = json!({ "policyStoreId": store_id });
        if !max_results.is_null() {
            request["maxResults"] = max_results.clone();
        }
        let page = ok(&router, "ListPolicies", &request).await;
        assert_eq!(
            page["policies"].as_array().map(Vec::len),
            Some(*expected_len),
            "case {case:?}: unexpected page length, got {}",
            page["policies"].as_array().map_or(0, Vec::len)
        );
        assert_eq!(
            page["nextToken"].is_string(),
            *expect_token,
            "case {case:?}: nextToken presence should be {expect_token}, got {page}"
        );
    }
}
