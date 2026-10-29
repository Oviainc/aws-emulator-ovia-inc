// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the optional `name` member this service's named
//! resources take — a policy's and a policy template's.
//!
//! The rule under test: a `name` member assigns a name only when it carries one.
//! `PolicyName` and `PolicyTemplateName` are both modelled `len 0..150`
//! (`smithy_query(query="shape", shape="PolicyName")`), so the empty string
//! reaches the service, and the API gives that value exactly one meaning — "to
//! remove a name, set it to an empty string" — while what a read reports is "the
//! name ..., if one was assigned when the policy was created or last updated".
//! So a request carrying the empty string assigns no name, whichever call
//! carried it: nothing is stored for it, no read reports it, it collides with no
//! other resource's name, and it is reachable by no `name/<name>` reference.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! member only round-trips if it really reached this service's state.

use serde_json::{json, Value};

use super::harness::{call, error_code, ok, router, store_with_validation_off};

/// A static Cedar policy whose scope names all three members, distinct per
/// `index` so one store can hold a row per case.
fn statement(index: usize) -> String {
    format!(
        "permit(principal == PhotoApp::User::\"u{index}\", \
         action == PhotoApp::Action::\"viewPhoto\", \
         resource == PhotoApp::Photo::\"p{index}.jpg\");"
    )
}

/// A Cedar TEMPLATE, distinct per `index` under the same reasoning.
fn template_statement(index: usize) -> String {
    format!(
        "permit(principal == ?principal, \
         action == PhotoApp::Action::\"viewPhoto\", \
         resource == PhotoApp::Photo::\"p{index}.jpg\");"
    )
}

/// One row of the name table: the case, the `name` member the request carries
/// (`None` meaning the member is absent) and the name the resource must then
/// report.
///
/// The rows span the member's own modelled value range — `len 0..150`, pattern
/// `^[a-zA-Z0-9-/_]*$`, from `smithy_query(query="shape", shape="PolicyName")`
/// and `shape="PolicyTemplateName"`: the member absent, the empty string at the
/// bottom of that length range, a plain name, one using every character class
/// the pattern allows, and one at the published 150-character bound.
fn name_cases(suffix: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let named = |value: String| (Some(value.clone()), Some(value));
    vec![
        ("no name member at all".to_string(), None, None),
        (
            "the empty string, which assigns no name".to_string(),
            Some(String::new()),
            None,
        ),
        {
            let (member, expected) = named(format!("plain{suffix}"));
            ("a plain name".to_string(), member, expected)
        },
        {
            let (member, expected) = named(format!("Name-9/under_score{suffix}"));
            (
                "every character class the pattern allows".to_string(),
                member,
                expected,
            )
        },
        {
            // 150 is `PolicyName`'s published maximum length; the suffix keeps
            // the row unique within its store without growing past it.
            let at_bound = format!("{suffix}{}", "n".repeat(150 - suffix.len()));
            let (member, expected) = named(at_bound);
            (
                "a name at the 150-character bound".to_string(),
                member,
                expected,
            )
        },
    ]
}

/// The `name` a response reports, treating an absent member and a `null` one
/// alike. A reported empty string is itself the defect, so it comes back as
/// `Some("")` here and fails the comparison rather than being normalised away.
fn reported_name(value: &Value) -> Option<&str> {
    value.get("name").and_then(Value::as_str)
}

/// Create a static policy in `store_id`, with `name` when the row carries one,
/// and return its `policyId`.
async fn create_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    index: usize,
    name: Option<&str>,
) -> String {
    let mut request = json!({
        "policyStoreId": store_id,
        "definition": { "static": { "statement": statement(index) } },
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

/// `GetPolicy` for one policy.
async fn get_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    policy_id: &str,
) -> Value {
    ok(
        router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await
}

/// Create a policy template in `store_id`, with `name` when the row carries one,
/// and return its `policyTemplateId`.
async fn create_template(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    index: usize,
    name: Option<&str>,
) -> String {
    let mut request = json!({
        "policyStoreId": store_id,
        "statement": template_statement(index),
    });
    if let Some(name) = name {
        request["name"] = json!(name);
    }
    let created = ok(router, "CreatePolicyTemplate", &request).await;
    created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string()
}

/// `GetPolicyTemplate` for one template.
async fn get_template(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    template_id: &str,
) -> Value {
    ok(
        router,
        "GetPolicyTemplate",
        &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
    )
    .await
}

/// A create assigns the name its `name` member carries, and no name at all when
/// that member is absent or empty — reported identically by every read of a
/// policy: `GetPolicy`, `ListPolicies` and `BatchGetPolicy`.
#[tokio::test]
async fn creating_a_policy_assigns_a_name_only_when_the_member_carries_one() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    let cases = name_cases("c");
    let mut created: Vec<(String, String, Option<String>)> = Vec::new();
    for (index, (case, member, expected)) in cases.iter().enumerate() {
        // Every row goes into the SAME store, so the two rows that assign no name
        // also prove a nameless policy collides with nothing: `ok` requires 200.
        let policy_id = create_policy(&router, &store_id, index, member.as_deref()).await;

        let fetched = get_policy(&router, &store_id, &policy_id).await;
        assert_eq!(
            reported_name(&fetched),
            expected.as_deref(),
            "case {case:?}: GetPolicy expected name {expected:?}, got {fetched}"
        );
        created.push((case.clone(), policy_id, expected.clone()));
    }

    let listed = ok(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    let items = listed["policies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let batch = ok(
        &router,
        "BatchGetPolicy",
        &json!({
            "requests": created
                .iter()
                .map(|(_, policy_id, _)| json!({
                    "policyStoreId": store_id,
                    "policyId": policy_id,
                }))
                .collect::<Vec<_>>(),
        }),
    )
    .await;
    let results = batch["results"].as_array().cloned().unwrap_or_default();

    for (case, policy_id, expected) in &created {
        let item = items
            .iter()
            .find(|item| item["policyId"].as_str() == Some(policy_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: ListPolicies omitted {policy_id}"));
        assert_eq!(
            reported_name(item),
            expected.as_deref(),
            "case {case:?}: ListPolicies expected name {expected:?}, got {item}"
        );
        let result = results
            .iter()
            .find(|item| item["policyId"].as_str() == Some(policy_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: BatchGetPolicy omitted {policy_id}"));
        assert_eq!(
            reported_name(result),
            expected.as_deref(),
            "case {case:?}: BatchGetPolicy expected name {expected:?}, got {result}"
        );
    }
}

/// The same table, through `CreatePolicyTemplate` and read back with
/// `GetPolicyTemplate`: the rule is about the member, not about which resource
/// carries it.
#[tokio::test]
async fn creating_a_policy_template_assigns_a_name_only_when_the_member_carries_one() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    for (index, (case, member, expected)) in name_cases("t").iter().enumerate() {
        let template_id = create_template(&router, &store_id, index, member.as_deref()).await;
        let fetched = get_template(&router, &store_id, &template_id).await;
        assert_eq!(
            reported_name(&fetched),
            expected.as_deref(),
            "case {case:?}: GetPolicyTemplate expected name {expected:?}, got {fetched}"
        );
    }
}

/// One name member, two ways in: a resource CREATED with a name member and one
/// UPDATED to the same member reach the same reported name — so the empty string
/// removes a name on both paths instead of only on the update path.
#[tokio::test]
async fn a_name_member_reads_back_the_same_whichever_call_carried_it() {
    let router = router();

    for (index, (case, member, expected)) in name_cases("p").iter().enumerate() {
        // The two paths get a store each: a name is unique within ITS store, so
        // the created and the updated resource can carry the same one and still
        // be compared.
        let created_store = store_with_validation_off(&router).await;
        let updated_store = store_with_validation_off(&router).await;
        let created_with = create_policy(&router, &created_store, index, member.as_deref()).await;

        let updated_to =
            create_policy(&router, &updated_store, index, Some("before-update")).await;
        let mut request = json!({
            "policyStoreId": updated_store,
            "policyId": updated_to,
            "definition": { "static": { "statement": statement(index) } },
        });
        if let Some(member) = member {
            request["name"] = json!(member);
        }
        ok(&router, "UpdatePolicy", &request).await;

        let from_create = get_policy(&router, &created_store, &created_with).await;
        let from_update = get_policy(&router, &updated_store, &updated_to).await;
        // An absent member leaves an existing name alone on the update path —
        // that row's two policies therefore differ, and only the name the member
        // ASSIGNS is compared.
        let expected_from_update = match member {
            None => Some("before-update"),
            Some(_) => expected.as_deref(),
        };
        assert_eq!(
            reported_name(&from_create),
            expected.as_deref(),
            "case {case:?}: the created policy expected name {expected:?}, got {from_create}"
        );
        assert_eq!(
            reported_name(&from_update),
            expected_from_update,
            "case {case:?}: the updated policy expected name {expected_from_update:?}, \
             got {from_update}"
        );

        // The template pair, under the same rule.
        let template_created =
            create_template(&router, &created_store, index, member.as_deref()).await;
        let template_updated =
            create_template(&router, &updated_store, index, Some("before-update")).await;
        let mut request = json!({
            "policyStoreId": updated_store,
            "policyTemplateId": template_updated,
            "statement": template_statement(index),
        });
        if let Some(member) = member {
            request["name"] = json!(member);
        }
        ok(&router, "UpdatePolicyTemplate", &request).await;

        let from_create = get_template(&router, &created_store, &template_created).await;
        let from_update = get_template(&router, &updated_store, &template_updated).await;
        assert_eq!(
            reported_name(&from_create),
            expected.as_deref(),
            "case {case:?}: the created template expected name {expected:?}, got {from_create}"
        );
        assert_eq!(
            reported_name(&from_update),
            expected_from_update,
            "case {case:?}: the updated template expected name {expected_from_update:?}, \
             got {from_update}"
        );
    }
}

/// A resource that was assigned no name is referenced by no name: several of
/// them live in one store without conflicting, and a bare `name/` reference
/// resolves to none of them.
#[tokio::test]
async fn resources_with_no_name_coexist_and_answer_no_name_reference() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // (case, the name member every one of these requests carries)
    let nameless: &[(&str, Option<&str>)] = &[
        ("the empty string", Some("")),
        ("the empty string again", Some("")),
        ("no name member at all", None),
    ];

    let mut policy_ids: Vec<String> = Vec::new();
    let mut template_ids: Vec<String> = Vec::new();
    for (index, (case, member)) in nameless.iter().enumerate() {
        let policy_id = create_policy(&router, &store_id, index, *member).await;
        assert!(
            !policy_ids.contains(&policy_id),
            "case {case:?}: CreatePolicy reported an id it already minted, got {policy_id}"
        );
        policy_ids.push(policy_id);

        let template_id = create_template(&router, &store_id, index, *member).await;
        assert!(
            !template_ids.contains(&template_id),
            "case {case:?}: CreatePolicyTemplate reported an id it already minted, \
             got {template_id}"
        );
        template_ids.push(template_id);
    }

    // `name/<name>` is the reference form a caller may use in place of an id, and
    // no resource above was assigned a name for it to carry.
    for (case, reference) in [
        ("the prefix with an empty name", "name/"),
        ("the prefix with a name nothing holds", "name/not-a-name"),
    ] {
        let (status, body) = call(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": reference }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: {reference:?} must resolve no policy, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": reference }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: {reference:?} must resolve no template, got {status} {body}"
        );
    }
}

/// A `clientToken` replay compares the name the request ASSIGNS, so a retry that
/// spells "no name" the other way is the same request and replays the policy the
/// first call created.
#[tokio::test]
async fn a_client_token_replay_compares_the_name_a_request_assigns() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // (case, the first request's name member, the retry's, whether they are the
    // same request)
    let cases: &[(&str, Option<&str>, Option<&str>, bool)] = &[
        ("an empty name, retried with none", Some(""), None, true),
        ("no name, retried with an empty one", None, Some(""), true),
        ("an empty name, retried with a name", Some(""), Some("later"), false),
    ];

    for (index, (case, first, retry, same_request)) in cases.iter().enumerate() {
        let token = format!("token-{index}");
        let definition = json!({ "static": { "statement": statement(index) } });
        let mut request = json!({
            "policyStoreId": store_id,
            "definition": definition,
            "clientToken": token,
        });
        if let Some(first) = first {
            request["name"] = json!(first);
        }
        let created = ok(&router, "CreatePolicy", &request).await;
        let policy_id = created["policyId"].as_str().unwrap_or_default().to_string();

        let mut request = json!({
            "policyStoreId": store_id,
            "definition": json!({ "static": { "statement": statement(index) } }),
            "clientToken": token,
        });
        if let Some(retry) = retry {
            request["name"] = json!(retry);
        }
        let (status, body) = call(&router, "CreatePolicy", &request).await;
        if *same_request {
            assert_eq!(
                (status, body["policyId"].as_str()),
                (200, Some(policy_id.as_str())),
                "case {case:?}: the retry must replay {policy_id}, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (409, "ConflictException"),
                "case {case:?}: a retry assigning a different name conflicts, got {status} {body}"
            );
        }
    }
}
