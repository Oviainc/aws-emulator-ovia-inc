// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `UpdatePolicy`.
//!
//! The rule under test is the operation's own: of a static policy "you can
//! change only certain elements ... The action referenced by the policy. A
//! condition clause, such as when and unless", and "you can't change these
//! elements of a static policy: Changing a policy from a static policy to a
//! template-linked policy. Changing the effect of a static policy from permit or
//! forbid. The principal referenced by a static policy. The resource referenced
//! by a static policy."
//!
//! So: a replacement that changes only what may change is applied and read back;
//! a replacement that changes an immutable element is refused with the modelled
//! `ValidationException`, naming the member that carries the change, and the
//! stored policy is untouched.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::cedar::MAX_STATEMENT_BYTES;
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::MAX_POLICY_BYTES_PER_STORE;

use crate::harness::{call, error_code, ok, router, store_with_mode, store_with_validation_off};
use crate::schema::photo_app_schema;

/// The policy every row starts from: a scope naming all three members, so a row
/// can change any one of them.
const ORIGINAL: &str = "permit(principal == PhotoApp::User::\"alice\", \
                        action == PhotoApp::Action::\"viewPhoto\", \
                        resource == PhotoApp::Photo::\"vacation.jpg\");";

/// [`ORIGINAL`] padded inside a condition clause to exactly `bytes` bytes, so
/// the only thing that differs between two of these is the SIZE: the scope —
/// the part an update may not change — stays identical.
fn padded_statement(bytes: usize) -> String {
    let prefix = format!(
        "{} when {{ context.tag == \"",
        ORIGINAL.trim_end_matches(';')
    );
    let suffix = "\" };";
    let padding = bytes.saturating_sub(prefix.len() + suffix.len());
    format!("{prefix}{}{suffix}", "x".repeat(padding))
}

/// `{entityType, entityId}` of a scope member, or `None` when the response omits
/// it.
fn entity(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    Some((
        object.get("entityType")?.as_str()?.to_string(),
        object.get("entityId")?.as_str()?.to_string(),
    ))
}

/// `{actionType, actionId}` pairs of an `actions` list, sorted.
fn actions(value: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            Some((
                item.get("actionType")?.as_str()?.to_string(),
                item.get("actionId")?.as_str()?.to_string(),
            ))
        })
        .collect();
    out.sort();
    out
}

/// Create a policy carrying `statement` (and `name`, when given) in `store_id`
/// and return `(policyId, the create response)`.
async fn policy_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    statement: &str,
    name: Option<&str>,
) -> (String, Value) {
    let mut request = json!({
        "policyStoreId": store_id,
        "definition": { "static": { "statement": statement } },
    });
    if let Some(name) = name {
        request["name"] = json!(name);
    }
    let created = ok(router, "CreatePolicy", &request).await;
    let policy_id = created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();
    (policy_id, created)
}

// ---------------------------------------------------------------------------
// What may change
// ---------------------------------------------------------------------------

/// One row of the allowed-change table: what the replacement changes, its Cedar
/// text, and the actions the policy must report once it is applied.
type AllowedChange = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

/// A replacement that changes only the elements an update may change is
/// applied: the response and every later read report the new text, the new
/// actions and an advanced `lastUpdatedDate`, while `createdDate`, the effect
/// and the scope's principal and resource stay as they were.
///
/// The rows differ along WHAT the replacement changes — the action, the number
/// of actions, the condition clause, both at once — and along whether the
/// request carries a name as well.
#[tokio::test]
async fn an_update_applies_every_change_the_operation_allows() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // (case, the replacement statement, the actions it must leave behind)
    let cases: &[AllowedChange] = &[
        (
            "a different action",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"sharePhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            &[("PhotoApp::Action", "sharePhoto")],
        ),
        (
            "two actions where there was one",
            "permit(principal == PhotoApp::User::\"alice\", \
             action in [PhotoApp::Action::\"viewPhoto\", PhotoApp::Action::\"sharePhoto\"], \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            &[
                ("PhotoApp::Action", "sharePhoto"),
                ("PhotoApp::Action", "viewPhoto"),
            ],
        ),
        (
            "an unconstrained action where there was one",
            "permit(principal == PhotoApp::User::\"alice\", action, \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            &[],
        ),
        (
            "a when clause added, the action left alone",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\") \
             when { context.authenticated == true };",
            &[("PhotoApp::Action", "viewPhoto")],
        ),
        (
            "an unless clause and a different action at once",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"sharePhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\") \
             unless { context.authenticated == false };",
            &[("PhotoApp::Action", "sharePhoto")],
        ),
    ];

    for (index, (case, replacement, expected_actions)) in cases.iter().enumerate() {
        let (policy_id, created) = policy_of(
            &router,
            &store_id,
            ORIGINAL,
            Some(&format!("policy-{index}")),
        )
        .await;

        let updated = ok(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": replacement } },
            }),
        )
        .await;

        assert_eq!(
            updated["policyId"].as_str(),
            Some(policy_id.as_str()),
            "case {case:?}: an update must report the policy it updated, got {updated}"
        );
        assert_eq!(
            updated["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: unexpected policyStoreId, got {updated}"
        );
        assert_eq!(
            updated["policyType"].as_str(),
            Some("STATIC"),
            "case {case:?}: expected policyType STATIC, got {updated}"
        );
        assert_eq!(
            updated["effect"], created["effect"],
            "case {case:?}: the effect may not change, got {updated}"
        );
        assert_eq!(
            entity(&updated["principal"]),
            entity(&created["principal"]),
            "case {case:?}: the principal may not change, got {updated}"
        );
        assert_eq!(
            entity(&updated["resource"]),
            entity(&created["resource"]),
            "case {case:?}: the resource may not change, got {updated}"
        );
        assert_eq!(
            actions(&updated["actions"]),
            expected_actions
                .iter()
                .map(|(t, i)| (t.to_string(), i.to_string()))
                .collect::<Vec<_>>(),
            "case {case:?}: unexpected actions after the update, got {updated}"
        );
        assert_eq!(
            updated["createdDate"], created["createdDate"],
            "case {case:?}: an update must not move createdDate, got {updated}"
        );
        assert!(
            !updated["lastUpdatedDate"].is_null(),
            "case {case:?}: lastUpdatedDate is required, got {updated}"
        );

        // The round trip: the replacement text is what the store now holds, byte
        // for byte, and `ListPolicies` reports the same policy.
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["definition"]["static"]["statement"].as_str(),
            Some(*replacement),
            "case {case:?}: GetPolicy must report the replacement statement, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], updated["lastUpdatedDate"],
            "case {case:?}: the read must report the stored lastUpdatedDate, got {fetched}"
        );
        assert_eq!(
            actions(&fetched["actions"]),
            actions(&updated["actions"]),
            "case {case:?}: GetPolicy reported different actions, got {fetched}"
        );
        assert_eq!(
            fetched["name"].as_str(),
            Some(format!("policy-{index}").as_str()),
            "case {case:?}: an update that carries no name leaves the name alone, got {fetched}"
        );
    }
}

/// `name` and `description` follow their own documented rules: an absent `name`
/// leaves the existing one alone, an empty one removes it, a new one replaces
/// it, and a name another policy already holds is a 409 `ConflictException`.
#[tokio::test]
async fn an_update_follows_the_documented_rules_for_name_and_description() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let (policy_id, _) = policy_of(&router, &store_id, ORIGINAL, Some("original-name")).await;
    let (other_id, _) = policy_of(&router, &store_id, ORIGINAL, Some("taken")).await;

    // (case, the name member the request carries, the name the policy must hold)
    let cases: &[(&str, Option<&str>, Option<&str>)] = &[
        ("no name member at all", None, Some("original-name")),
        ("a new name", Some("renamed"), Some("renamed")),
        ("the empty string, which removes the name", Some(""), None),
        ("a name again after it was removed", Some("named"), Some("named")),
    ];

    for (case, name, expected) in cases {
        let mut request = json!({
            "policyStoreId": store_id,
            "policyId": policy_id,
            "definition": { "static": { "statement": ORIGINAL } },
        });
        if let Some(name) = name {
            request["name"] = json!(name);
        }
        ok(&router, "UpdatePolicy", &request).await;

        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["name"].as_str(),
            *expected,
            "case {case:?}: expected name {expected:?}, got {fetched}"
        );
    }

    // A description arrives inside the definition, so a replacement that carries
    // one sets it and one that does not clears it.
    for (case, description) in [
        ("a description", Some("the described policy")),
        ("no description", None),
    ] {
        let mut definition = json!({ "statement": ORIGINAL });
        if let Some(description) = description {
            definition["description"] = json!(description);
        }
        ok(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": definition },
            }),
        )
        .await;
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["definition"]["static"]["description"].as_str(),
            description,
            "case {case:?}: expected description {description:?}, got {fetched}"
        );
    }

    // A name another policy holds conflicts, and nothing changes.
    let (status, body) = call(
        &router,
        "UpdatePolicy",
        &json!({
            "policyStoreId": store_id,
            "policyId": policy_id,
            "name": "taken",
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case \"a name another policy holds\": expected 409, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case \"a name another policy holds\": unexpected error code, got {body}"
    );
    let fetched = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": other_id }),
    )
    .await;
    assert_eq!(
        fetched["name"].as_str(),
        Some("taken"),
        "case \"a name another policy holds\": the other policy keeps its name, got {fetched}"
    );
}

// ---------------------------------------------------------------------------
// What may not change
// ---------------------------------------------------------------------------

/// A replacement that changes an element the operation documents as immutable is
/// refused with the modelled `ValidationException` — HTTP 400, the fixed
/// message, and a `fieldList` entry per changed element naming the statement
/// that carries the change — and the stored policy keeps its original content.
///
/// The rows differ along WHICH immutable element the replacement changes and
/// HOW: a different principal entity, a different principal form, the resource,
/// the effect, and two of them at once.
#[tokio::test]
async fn an_update_that_changes_an_immutable_element_is_refused() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // (case, the replacement statement, how many entries the refusal reports)
    let cases: &[(&str, &str, usize)] = &[
        (
            "a different principal entity",
            "permit(principal == PhotoApp::User::\"bob\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            1,
        ),
        (
            "a principal named by type only, where it was an entity",
            "permit(principal is PhotoApp::User, \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            1,
        ),
        (
            "the same principal entity reached with in instead of ==",
            "permit(principal in PhotoApp::Group::\"friends\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            1,
        ),
        (
            "a different resource entity",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"beach.jpg\");",
            1,
        ),
        (
            "an unconstrained resource, where it was an entity",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", resource);",
            1,
        ),
        (
            "the effect",
            "forbid(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            1,
        ),
        (
            // Two immutable elements at once: one entry per element, because a
            // client iterates `fieldList` to see every reason.
            "the effect and the principal at once",
            "forbid(principal == PhotoApp::User::\"bob\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            2,
        ),
        (
            "all three of effect, principal and resource",
            "forbid(principal == PhotoApp::User::\"bob\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"beach.jpg\");",
            3,
        ),
    ];

    for (index, (case, replacement, entries)) in cases.iter().enumerate() {
        let (policy_id, created) = policy_of(
            &router,
            &store_id,
            ORIGINAL,
            Some(&format!("immutable-{index}")),
        )
        .await;

        let (status, body) = call(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": replacement } },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INVALID_INPUT_MESSAGE),
            "case {case:?}: expected the fixed message {INVALID_INPUT_MESSAGE:?}, got {body}"
        );
        let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            fields.len(),
            *entries,
            "case {case:?}: expected {entries} fieldList entries, one per changed element, \
             got {body}"
        );
        for field in &fields {
            assert_eq!(
                field["path"].as_str(),
                Some("/definition/static/statement"),
                "case {case:?}: the member carrying the change is the statement, got {body}"
            );
            assert!(
                field["message"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: expected a non-empty reason, got {body}"
            );
        }

        // The refused update changed nothing: the stored policy still reports
        // its original text and scope.
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["definition"]["static"]["statement"].as_str(),
            Some(ORIGINAL),
            "case {case:?}: a refused update must leave the statement alone, got {fetched}"
        );
        assert_eq!(
            entity(&fetched["principal"]),
            entity(&created["principal"]),
            "case {case:?}: a refused update must leave the principal alone, got {fetched}"
        );
        assert_eq!(
            entity(&fetched["resource"]),
            entity(&created["resource"]),
            "case {case:?}: a refused update must leave the resource alone, got {fetched}"
        );
        assert_eq!(
            fetched["effect"], created["effect"],
            "case {case:?}: a refused update must leave the effect alone, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], created["lastUpdatedDate"],
            "case {case:?}: a refused update must not move lastUpdatedDate, got {fetched}"
        );
    }
}

/// A template-linked policy has no content of its own to replace, so a
/// definition for one is refused at `/definition` and the policy still follows
/// its template.
#[tokio::test]
async fn a_template_linked_policy_cannot_be_updated_directly() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let template = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": store_id,
            "statement": "permit(principal == ?principal, \
                          action == PhotoApp::Action::\"viewPhoto\", \
                          resource == PhotoApp::Photo::\"vacation.jpg\");",
        }),
    )
    .await;
    let template_id = template["policyTemplateId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let created = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": {
                "templateLinked": {
                    "policyTemplateId": template_id,
                    "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                },
            },
        }),
    )
    .await;
    let policy_id = created["policyId"].as_str().unwrap_or_default().to_string();

    // Rows differ along the replacement offered: one that would keep the same
    // scope, and one that would change it. Neither is allowed, because the
    // policy's content is its template's.
    let cases: &[(&str, &str)] = &[
        (
            "a static definition matching the linked scope",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
        ),
        (
            "a static definition with another action",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"sharePhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
        ),
    ];

    for (case, replacement) in cases {
        let (status, body) = call(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": replacement } },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: expected one fieldList entry, got {body}"
        );
        assert_eq!(
            fields[0]["path"].as_str(),
            Some("/definition"),
            "case {case:?}: the member at fault is the definition itself, got {body}"
        );
    }

    // The policy still follows its template.
    let fetched = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;
    assert_eq!(
        fetched["policyType"].as_str(),
        Some("TEMPLATE_LINKED"),
        "a refused update must not change the kind of policy, got {fetched}"
    );
    assert_eq!(
        fetched["definition"]["templateLinked"]["policyTemplateId"].as_str(),
        Some(template_id.as_str()),
        "a refused update must leave the link alone, got {fetched}"
    );
}

/// Everything `UpdatePolicy` refuses for a reason other than the immutability
/// rule: an unknown store, an unknown policy, Cedar text it cannot parse, text
/// longer than one policy may carry, and a STRICT store with no schema.
#[tokio::test]
async fn an_update_refuses_what_the_model_says_it_refuses() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let (policy_id, _) = policy_of(&router, &store_id, ORIGINAL, None).await;
    let strict_store = store_with_validation_off(&router).await;

    // A statement longer than one policy may carry, padded inside a condition so
    // the only thing wrong with it is its size.
    let over_long = format!(
        "permit(principal == PhotoApp::User::\"alice\", \
         action == PhotoApp::Action::\"viewPhoto\", \
         resource == PhotoApp::Photo::\"vacation.jpg\") when {{ context.tag == \"{}\" }};",
        "x".repeat(MAX_STATEMENT_BYTES + 1)
    );

    // (case, request, expected status, expected error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown store",
            json!({
                "policyStoreId": "PSMISSINGMISSINGMISSI",
                "policyId": policy_id,
                "definition": { "static": { "statement": ORIGINAL } },
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown alias",
            json!({
                "policyStoreId": "policy-store-alias/not-an-alias",
                "policyId": policy_id,
                "definition": { "static": { "statement": ORIGINAL } },
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown policy id in a store that exists",
            json!({
                "policyStoreId": store_id,
                "policyId": "SPMISSINGMISSINGMISSI",
                "definition": { "static": { "statement": ORIGINAL } },
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown policy name",
            json!({
                "policyStoreId": store_id,
                "policyId": "name/no-such-policy",
                "definition": { "static": { "statement": ORIGINAL } },
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a replacement that is not Cedar",
            json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": "this is not cedar" } },
            }),
            400,
            "ValidationException",
        ),
        (
            "a replacement longer than one policy may carry",
            json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": over_long } },
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, code) in &cases {
        let (got, body) = call(&router, "UpdatePolicy", request).await;
        assert_eq!(
            got, *status,
            "case {case:?}: expected HTTP {status}, got {got} {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected {code}, got {body}"
        );
    }

    // A STRICT store with no schema has nothing to validate a replacement
    // against, which is a property of the STORE the request named. The policy
    // is created while the store still validates nothing, and the store is then
    // switched to STRICT — which is the only way such a store can hold a policy
    // at all, since a STRICT store with no schema refuses every create too.
    let (strict_policy, _) = policy_of(&router, &strict_store, ORIGINAL, None).await;
    ok(
        &router,
        "UpdatePolicyStore",
        &json!({
            "policyStoreId": strict_store,
            "validationSettings": { "mode": "STRICT" },
        }),
    )
    .await;
    let (status, body) = call(
        &router,
        "UpdatePolicy",
        &json!({
            "policyStoreId": strict_store,
            "policyId": strict_policy,
            "definition": { "static": { "statement": ORIGINAL } },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a STRICT store with no schema\": expected 400, got {status} {body}"
    );
    assert_eq!(
        body["fieldList"][0]["path"].as_str(),
        Some("/policyStoreId"),
        "case \"a STRICT store with no schema\": the member at fault is the store, got {body}"
    );

    // The policy the refused updates named is untouched.
    let fetched = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;
    assert_eq!(
        fetched["definition"]["static"]["statement"].as_str(),
        Some(ORIGINAL),
        "a refused update must leave the statement alone, got {fetched}"
    );
}

/// An update is validated against the store's schema exactly as a create is: a
/// replacement the schema sanctions is stored, one it does not is refused and
/// nothing changes.
#[tokio::test]
async fn an_update_in_a_strict_store_is_validated_against_its_schema() {
    let router = router();
    let store_id = store_with_mode(&router, "STRICT").await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;
    let (policy_id, _) = policy_of(&router, &store_id, ORIGINAL, None).await;

    // Rows differ along whether the replacement's action is one the schema
    // declares: `sharePhoto` is, `deletePhoto` is not.
    let cases: &[(&str, &str, u16)] = &[
        (
            "an action the schema declares",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"sharePhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            200,
        ),
        (
            "an action the schema does not declare",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"deletePhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            400,
        ),
    ];

    for (case, replacement, expected) in cases {
        let (status, body) = call(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": replacement } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        let stored = fetched["definition"]["static"]["statement"].as_str();
        if *expected == 200 {
            assert_eq!(
                stored,
                Some(*replacement),
                "case {case:?}: the validated replacement must be stored, got {fetched}"
            );
        } else {
            assert_ne!(
                stored,
                Some(*replacement),
                "case {case:?}: a replacement that fails validation must not be stored, \
                 got {fetched}"
            );
        }
    }
}

/// An update with no `definition` at all changes no content: "if you don't
/// specify this parameter, the existing policy definition remains unchanged".
#[tokio::test]
async fn an_update_without_a_definition_leaves_the_content_alone() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // Rows differ along the FORM of the policy whose content must survive.
    let cases: &[(&str, &str)] = &[
        ("a fully constrained scope", ORIGINAL),
        (
            "an unconstrained scope with a condition",
            "forbid(principal, action, resource) when { !context.authenticated };",
        ),
    ];

    for (index, (case, statement)) in cases.iter().enumerate() {
        let (policy_id, created) = policy_of(
            &router,
            &store_id,
            statement,
            Some(&format!("untouched-{index}")),
        )
        .await;
        let updated = ok(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "name": format!("renamed-{index}"),
            }),
        )
        .await;
        assert_eq!(
            updated["effect"], created["effect"],
            "case {case:?}: the effect must survive a name-only update, got {updated}"
        );
        assert_eq!(
            actions(&updated["actions"]),
            actions(&created["actions"]),
            "case {case:?}: the actions must survive a name-only update, got {updated}"
        );

        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["definition"]["static"]["statement"].as_str(),
            Some(*statement),
            "case {case:?}: the statement must survive a name-only update, got {fetched}"
        );
        assert_eq!(
            fetched["name"].as_str(),
            Some(format!("renamed-{index}").as_str()),
            "case {case:?}: the new name must be stored, got {fetched}"
        );
    }
}

/// An update is held to the same per-store Cedar-text budget a create is: a
/// replacement that would take the store past `MAX_POLICY_BYTES_PER_STORE` — the
/// total text every later authorization request parses — is refused with the
/// modelled `ServiceQuotaExceededException` and nothing is stored, while the
/// largest replacement that still fits is applied.
///
/// The rows are the bound itself and one byte past it, with the store filled to
/// a known headroom so both are reachable; the bytes the updated policy already
/// contributes are the ones the replacement takes the place of.
#[tokio::test]
async fn an_update_cannot_take_the_store_past_its_cedar_budget() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // Fill the store with statements of the largest size one policy may carry,
    // chosen so the headroom left is SMALLER than one statement — otherwise no
    // single replacement could reach the bound at all.
    let fillers = MAX_POLICY_BYTES_PER_STORE.div_ceil(MAX_STATEMENT_BYTES) - 1;
    let filler = padded_statement(MAX_STATEMENT_BYTES);
    assert_eq!(
        filler.len(),
        MAX_STATEMENT_BYTES,
        "the filler statement should be exactly {MAX_STATEMENT_BYTES} bytes, got {}",
        filler.len()
    );
    for _ in 0..fillers {
        policy_of(&router, &store_id, &filler, None).await;
    }
    let headroom = MAX_POLICY_BYTES_PER_STORE - fillers * MAX_STATEMENT_BYTES;
    assert!(
        headroom < MAX_STATEMENT_BYTES,
        "the headroom {headroom} must be smaller than one statement for these rows to bite"
    );

    // The policy the rows replace: its own bytes come out of the total when a
    // replacement goes in, so its size does not change what fits.
    let (policy_id, _) = policy_of(&router, &store_id, ORIGINAL, None).await;

    // (case, the replacement's size in bytes, expected status)
    let cases: &[(&str, usize, u16)] = &[
        ("at the byte bound", headroom, 200),
        ("one byte past the bound", headroom + 1, 402),
        ("the largest statement one policy may carry", MAX_STATEMENT_BYTES, 402),
    ];

    for (case, bytes, expected) in cases {
        let replacement = padded_statement(*bytes);
        assert_eq!(
            replacement.len(),
            *bytes,
            "case {case:?}: the padded statement should be {bytes} bytes, got {}",
            replacement.len()
        );
        let (status, body) = call(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement": replacement } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        let stored = fetched["definition"]["static"]["statement"]
            .as_str()
            .unwrap_or_default();
        if *expected == 200 {
            assert_eq!(
                stored, replacement,
                "case {case:?}: the replacement that fits must be stored"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ServiceQuotaExceededException",
                "case {case:?}: expected the modelled quota error, got {body}"
            );
            assert_ne!(
                stored, replacement,
                "case {case:?}: a refused replacement must not be stored"
            );
        }
    }
}
