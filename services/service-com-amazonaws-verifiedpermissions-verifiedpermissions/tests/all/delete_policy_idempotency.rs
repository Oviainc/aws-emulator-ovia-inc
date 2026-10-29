// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the IDEMPOTENCY `DeletePolicy` documents.
//!
//! The rule under test: a delete whose own documentation states that an absent
//! target is a successful `HTTP 200` answers an empty 200 for every reference
//! form that resolves to no such resource in a container that exists, writing
//! nothing on that path; the modelled `ResourceNotFoundException` stays for the
//! reference the documentation gives no success for — here the `policyStoreId`
//! that names no store.
//!
//! `DeletePolicy` states it directly: "This operation is idempotent; if you
//! specify a policy that doesn't exist, the request response returns a
//! successful HTTP 200 status code"
//! (`smithy_query(query="doc", shape="DeletePolicy")`).
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! policy is only reported as gone — or as untouched — if the call really
//! reached this service's state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, error_code, ok, router, store_with_validation_off};

/// The prefix a store reference carries when it names an alias, from
/// `DeletePolicyInput$policyStoreId` ("When using an alias name, prefix it with
/// policy-store-alias/").
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The prefix a policy reference carries when it names a policy by name, from
/// `DeletePolicyInput$policyId` ("You can use the policy name in place of the
/// policy ID. When using a name, prefix it with name/").
const POLICY_NAME_PREFIX: &str = "name/";

/// `PolicyId`'s published upper length bound
/// (`smithy_query(query="operation", operation="DeletePolicy")`: `length
/// 1..200`).
const MAX_POLICY_ID_LEN: usize = 200;

/// `PolicyStoreId`'s published upper length bound, from the same call (`length
/// 1..200`).
const MAX_POLICY_STORE_ID_LEN: usize = 200;

/// A Cedar statement for a static policy, parameterised so each policy of a
/// store is distinct text.
fn statement(principal: &str) -> String {
    format!(
        "permit(principal == User::\"{principal}\", \
         action == Action::\"view\", \
         resource == Photo::\"vacation\");"
    )
}

/// Create a static policy in `store_id`, optionally named, and return its
/// `policyId`.
async fn create_policy(
    router: &MultiServiceRouter,
    store_id: &str,
    principal: &str,
    name: Option<&str>,
) -> String {
    let mut request = json!({
        "policyStoreId": store_id,
        "definition": { "static": { "statement": statement(principal) } },
    });
    if let Some(name) = name {
        request["name"] = json!(name);
    }
    let created = ok(router, "CreatePolicy", &request).await;
    created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string()
}

/// Create a template-linked policy in `store_id` and return its `policyId`.
async fn create_template_linked_policy(
    router: &MultiServiceRouter,
    store_id: &str,
    principal: &str,
) -> String {
    let template = ok(
        router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": store_id,
            "statement": "permit(principal == ?principal, \
                          action == Action::\"view\", \
                          resource == Photo::\"vacation\");",
        }),
    )
    .await;
    let template_id = template["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string();
    let created = ok(
        router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "templateLinked": {
                "policyTemplateId": template_id,
                "principal": { "entityType": "User", "entityId": principal },
            } },
        }),
    )
    .await;
    created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string()
}

/// The `policyId`s `ListPolicies` reports for `store_id`, sorted so two readings
/// compare as sets.
async fn listed_policy_ids(router: &MultiServiceRouter, store_id: &str) -> Vec<String> {
    let listed = ok(router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
    let mut ids: Vec<String> = listed["policies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["policyId"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// Delete `policy_id` from `store_reference`, requiring the documented empty
/// 200, and return the parsed body.
async fn delete_policy(
    router: &MultiServiceRouter,
    case: &str,
    store_reference: &str,
    policy_id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DeletePolicy",
        &json!({ "policyStoreId": store_reference, "policyId": policy_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting {policy_id} must answer 200, got {status} {body}"
    );
    body
}

/// Every reference form that resolves to NO policy in a store that exists is a
/// successful, empty 200, and leaves the store exactly as it was.
///
/// The rows differ along the FORM of the reference that resolves to nothing (an
/// id never minted, the id of a policy this store just deleted, the id of a
/// template-linked policy just deleted, a `name/` reference to no policy, a
/// `name/` reference whose policy was deleted, the id of a policy that lives in
/// ANOTHER store, an id at the modelled length bound, an id carrying every
/// punctuation character the pattern admits) and along the form of the store
/// reference carrying it (an id, an alias).
#[tokio::test]
async fn an_absent_policy_is_a_successful_empty_delete() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let other_store = store_with_validation_off(&router).await;

    // One alias for the store, so a row can name the store the other documented
    // way.
    let alias = format!("{ALIAS_PREFIX}idempotent-delete");
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias, "policyStoreId": store_id }),
    )
    .await;

    // The survivor: no row's delete may remove it, which is what makes an
    // idempotent no-op a no-op rather than a sweep of the store.
    let keeper = create_policy(&router, &store_id, "keeper", Some("keeper")).await;

    // A policy that lived in this store and was deleted, by id and by name, plus
    // a template-linked one: their references resolve to nothing afterwards.
    let deleted_by_id = create_policy(&router, &store_id, "gone", None).await;
    let deleted_by_name = create_policy(&router, &store_id, "named", Some("named-gone")).await;
    let deleted_linked = create_template_linked_policy(&router, &store_id, "linked").await;
    for policy_id in [&deleted_by_id, &deleted_by_name, &deleted_linked] {
        delete_policy(&router, "setup delete", &store_id, policy_id).await;
    }

    // A policy that really exists — but in another store, so this store resolves
    // nothing for its id. Each store mints its own ids, so the candidate is
    // drawn until it is one THIS store does not hold.
    let held_here = listed_policy_ids(&router, &store_id).await;
    let mut elsewhere = create_policy(&router, &other_store, "elsewhere", None).await;
    for attempt in 1..10 {
        if !held_here.contains(&elsewhere) {
            break;
        }
        elsewhere =
            create_policy(&router, &other_store, &format!("elsewhere{attempt}"), None).await;
    }
    assert!(
        !held_here.contains(&elsewhere),
        "case \"another store\": the row needs an id this store does not hold, \
         got {elsewhere} among {held_here:?}"
    );

    // Exactly at `PolicyId`'s published length bound, and every punctuation
    // character its pattern `^[a-zA-Z0-9-/_]*$` admits.
    let longest_policy = format!("SP{}", "a".repeat(MAX_POLICY_ID_LEN - 2));
    let punctuated_policy = "SP-policy_1/2".to_string();

    // (case, store reference, policy reference)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a policy id that was never minted",
            store_id.as_str(),
            "SPNEVERMINTED0000000".to_string(),
        ),
        (
            "the id of a static policy this store just deleted",
            store_id.as_str(),
            deleted_by_id.clone(),
        ),
        (
            "the id of a template-linked policy this store just deleted",
            store_id.as_str(),
            deleted_linked.clone(),
        ),
        (
            "a name/ reference to a policy that never existed",
            store_id.as_str(),
            format!("{POLICY_NAME_PREFIX}no-such-policy"),
        ),
        (
            "a name/ reference whose policy was deleted",
            alias.as_str(),
            format!("{POLICY_NAME_PREFIX}named-gone"),
        ),
        (
            "the id of a policy that exists only in another store",
            store_id.as_str(),
            elsewhere.clone(),
        ),
        (
            "an id at the modelled length bound",
            store_id.as_str(),
            longest_policy.clone(),
        ),
        (
            "an id carrying every admitted punctuation character",
            alias.as_str(),
            punctuated_policy.clone(),
        ),
    ];

    for (case, store_reference, policy_reference) in rows {
        let before = listed_policy_ids(&router, &store_id).await;

        // Twice: idempotence means the second call answers exactly like the
        // first.
        let first = delete_policy(&router, case, store_reference, &policy_reference).await;
        let second = delete_policy(&router, case, store_reference, &policy_reference).await;
        assert_eq!(
            first, second,
            "case {case:?}: a repeated delete must answer the same, got {first} and {second}"
        );
        assert!(
            first.get("__type").is_none() && first.get("message").is_none(),
            "case {case:?}: the success must carry no fault members, got {first}"
        );

        let after = listed_policy_ids(&router, &store_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: an absent policy writes nothing, got {after:?} after {before:?}"
        );
        assert!(
            after.contains(&keeper),
            "case {case:?}: the other policy of the store must survive, got {after:?}"
        );
    }

    // The policy in the other store was never touched by the row that named it.
    let elsewhere_still = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": other_store, "policyId": elsewhere }),
    )
    .await;
    assert_eq!(
        elsewhere_still["policyId"].as_str(),
        Some(elsewhere.as_str()),
        "case \"another store\": the policy of the other store must survive, got {elsewhere_still}"
    );
}

/// A delete that finds its policy removes it, and the delete repeated on the
/// same reference is the documented success rather than a 404.
#[tokio::test]
async fn deleting_a_policy_removes_it_and_the_repeat_succeeds() {
    let router = router();

    // Rows differ along the kind of policy deleted and the reference form used
    // to delete it.
    let cases: &[(&str, bool, bool)] = &[
        // (case, template-linked, deleted by name/ reference)
        ("a static policy, deleted by id", false, false),
        ("a static policy, deleted by name", false, true),
        ("a template-linked policy, deleted by id", true, false),
    ];

    for (index, (case, template_linked, by_name)) in cases.iter().enumerate() {
        let store_id = store_with_validation_off(&router).await;
        let name = format!("policy-{index}");
        let policy_id = if *template_linked {
            create_template_linked_policy(&router, &store_id, &format!("user{index}")).await
        } else {
            create_policy(&router, &store_id, &format!("user{index}"), Some(&name)).await
        };

        let fetched = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            fetched["policyId"].as_str(),
            Some(policy_id.as_str()),
            "case {case:?}: the policy must be readable before the delete, got {fetched}"
        );

        let reference = if *by_name {
            format!("{POLICY_NAME_PREFIX}{name}")
        } else {
            policy_id.clone()
        };
        delete_policy(&router, case, &store_id, &reference).await;

        let (status, body) = call(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted policy must be gone, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code from GetPolicy, got {body}"
        );
        let listed = listed_policy_ids(&router, &store_id).await;
        assert!(
            !listed.contains(&policy_id),
            "case {case:?}: ListPolicies still reports {policy_id}, got {listed:?}"
        );

        // The repeat, on the same reference the first delete used.
        delete_policy(&router, case, &store_id, &reference).await;
    }
}

/// The store half of the request keeps its modelled 404: a `policyStoreId` that
/// resolves to no store has no documented success, whatever the `policyId` says.
#[tokio::test]
async fn a_store_that_does_not_exist_is_still_a_resource_not_found() {
    let router = router();
    let live_store = store_with_validation_off(&router).await;
    let live_policy = create_policy(&router, &live_store, "live", Some("live")).await;

    // A store created and then deleted, and an alias soft-deleted: both
    // references were real resources once.
    let gone_store = store_with_validation_off(&router).await;
    let pending_alias = format!("{ALIAS_PREFIX}pending-deletion");
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": pending_alias, "policyStoreId": gone_store }),
    )
    .await;
    ok(
        &router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": pending_alias }),
    )
    .await;
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": gone_store }),
    )
    .await;

    // (case, store reference, policy reference, expected resourceType)
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a store id that was never minted, absent policy",
            "PSNEVERMINTED0000000".to_string(),
            "SPNEVERMINTED0000000".to_string(),
            "POLICY_STORE",
        ),
        (
            "a store id that was never minted, policy that exists elsewhere",
            "PSNEVERMINTED0000000".to_string(),
            live_policy.clone(),
            "POLICY_STORE",
        ),
        (
            "the id of a store that was deleted",
            gone_store.clone(),
            live_policy.clone(),
            "POLICY_STORE",
        ),
        (
            "a store id at the modelled length bound",
            format!("PS{}", "a".repeat(MAX_POLICY_STORE_ID_LEN - 2)),
            "SPNEVERMINTED0000000".to_string(),
            "POLICY_STORE",
        ),
        (
            "an alias that was never created",
            format!("{ALIAS_PREFIX}never-created"),
            "SPNEVERMINTED0000000".to_string(),
            "POLICY_STORE_ALIAS",
        ),
        (
            "an alias pending deletion",
            pending_alias.clone(),
            "SPNEVERMINTED0000000".to_string(),
            "POLICY_STORE_ALIAS",
        ),
        (
            "a name/ policy reference in a store that does not exist",
            "PSNEVERMINTED0000000".to_string(),
            format!("{POLICY_NAME_PREFIX}live"),
            "POLICY_STORE",
        ),
    ];

    for (case, store_reference, policy_reference, resource_type) in rows {
        let (status, body) = call(
            &router,
            "DeletePolicy",
            &json!({ "policyStoreId": store_reference, "policyId": policy_reference }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(resource_type),
            "case {case:?}: the fault must name the kind of resource the reference named, \
             got {body}"
        );
    }

    // A reference OUTSIDE the modelled constraints is refused as invalid input,
    // not accepted as an absent policy: the documented success is for a
    // well-formed reference that resolves to nothing. One past `PolicyId`'s
    // length bound, and one below its minimum.
    for (case, policy_reference) in [
        (
            "one past the modelled length bound",
            format!("SP{}", "a".repeat(MAX_POLICY_ID_LEN - 1)),
        ),
        ("below the modelled length minimum", String::new()),
    ] {
        let (status, body) = call(
            &router,
            "DeletePolicy",
            &json!({ "policyStoreId": live_store, "policyId": policy_reference }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }

    // Nothing above removed the policy of the live store.
    let fetched = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": live_store, "policyId": live_policy }),
    )
    .await;
    assert_eq!(
        fetched["policyId"].as_str(),
        Some(live_policy.as_str()),
        "case \"nothing changed\": the live policy must still be there, got {fetched}"
    );
}
