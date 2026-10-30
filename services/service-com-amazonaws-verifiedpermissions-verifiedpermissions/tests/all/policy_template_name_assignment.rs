// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the name a `CreatePolicyTemplate` request ASSIGNS, read
//! back through every operation that reports a policy template's name and
//! through the `name/<name>` reference form.
//!
//! The rule under test: a `name` member assigns a name only when it carries one,
//! so a request carrying the empty string assigns none — whichever call carried
//! it. `PolicyTemplateName` is modelled `len 0..150`, pattern
//! `^[a-zA-Z0-9-/_]*$` (`smithy_query(query="shape", shape="PolicyTemplateName")`),
//! so the empty string reaches the service, and the only meaning the API gives
//! that value is "no name" — `UpdatePolicyTemplate`'s member documents "to remove
//! a name, set it to an empty string", and what a read reports is "the name of
//! the policy template, if one was assigned when the policy template was created
//! or last updated" (`smithy_query(query="shape", shape="PolicyTemplateItem")`).
//! A template that was assigned no name therefore stores none, is reported by no
//! read, takes part in no name-uniqueness check, is reachable by no `name/`
//! reference, and is the same request — for `clientToken` replay — as one that
//! left the member out.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! member only round-trips if it really reached this service's state.

use serde_json::{json, Value};

use super::harness::{call, error_code, ok, router, store_with_validation_off};

/// A Cedar TEMPLATE, distinct per `index` so one store can hold a row per case.
fn template_statement(index: usize) -> String {
    format!(
        "permit(principal == ?principal, \
         action == PhotoApp::Action::\"viewPhoto\", \
         resource == PhotoApp::Photo::\"p{index}.jpg\");"
    )
}

/// One row of the name table: the case, the `name` member the request carries
/// (`None` meaning the member is absent) and the name every read must then
/// report for the created template.
///
/// The rows span the member's own modelled value range — `len 0..150`, pattern
/// `^[a-zA-Z0-9-/_]*$`, from
/// `smithy_query(query="shape", shape="PolicyTemplateName")`: the member absent,
/// the empty string at the bottom of that length range, a plain name, one using
/// every character class the pattern allows, and one at the published
/// 150-character maximum.
fn name_cases() -> Vec<(String, Option<String>, Option<String>)> {
    let named = |value: String| (Some(value.clone()), Some(value));
    vec![
        ("no name member at all".to_string(), None, None),
        (
            "the empty string, which assigns no name".to_string(),
            Some(String::new()),
            None,
        ),
        {
            let (member, expected) = named("plain-template".to_string());
            ("a plain name".to_string(), member, expected)
        },
        {
            let (member, expected) = named("Template-9/under_score".to_string());
            (
                "every character class the pattern allows".to_string(),
                member,
                expected,
            )
        },
        {
            // 150 is `PolicyTemplateName`'s published maximum length.
            let (member, expected) = named(format!("bound-{}", "n".repeat(150 - "bound-".len())));
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

/// `GetPolicyTemplate` for one reference — a template id or a `name/<name>`.
async fn get_template(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    reference: &str,
) -> (u16, Value) {
    call(
        router,
        "GetPolicyTemplate",
        &json!({ "policyStoreId": store_id, "policyTemplateId": reference }),
    )
    .await
}

/// A create assigns the name its `name` member carries, and no name at all when
/// that member is absent or empty — reported identically by `GetPolicyTemplate`
/// and by `ListPolicyTemplates`, and resolvable through `name/<name>` exactly
/// when a name was assigned.
#[tokio::test]
async fn every_read_reports_the_name_a_created_template_was_assigned() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    let cases = name_cases();
    let mut created: Vec<(String, String, Option<String>)> = Vec::new();
    for (index, (case, member, expected)) in cases.iter().enumerate() {
        // Every row goes into the SAME store, so the two rows that assign no name
        // also prove a nameless template collides with nothing: `ok` inside
        // `create_template` requires 200.
        let template_id = create_template(&router, &store_id, index, member.as_deref()).await;

        let (status, fetched) = get_template(&router, &store_id, &template_id).await;
        assert_eq!(
            (status, reported_name(&fetched)),
            (200, expected.as_deref()),
            "case {case:?}: GetPolicyTemplate expected name {expected:?}, got {status} {fetched}"
        );
        created.push((case.clone(), template_id, expected.clone()));
    }

    let listed = ok(
        &router,
        "ListPolicyTemplates",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    let items = listed["policyTemplates"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    for (case, template_id, expected) in &created {
        let item = items
            .iter()
            .find(|item| item["policyTemplateId"].as_str() == Some(template_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: ListPolicyTemplates omitted {template_id}"));
        assert_eq!(
            reported_name(item),
            expected.as_deref(),
            "case {case:?}: ListPolicyTemplates expected name {expected:?}, got {item}"
        );

        // `name/<name>` is the reference form a caller may use in place of an id,
        // and it names this template exactly when one was assigned — a row that
        // assigned none is reachable only by `name/`, which names no template.
        let reference = format!("name/{}", expected.clone().unwrap_or_default());
        let (status, fetched) = get_template(&router, &store_id, &reference).await;
        match expected {
            Some(_) => assert_eq!(
                (status, fetched["policyTemplateId"].as_str()),
                (200, Some(template_id.as_str())),
                "case {case:?}: {reference:?} must resolve {template_id}, got {status} {fetched}"
            ),
            None => assert_eq!(
                (status, error_code(&fetched).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: {reference:?} must resolve no template, got {status} {fetched}"
            ),
        }
    }
}

/// A `clientToken` replay compares the name the request ASSIGNS, so a retry that
/// spells "no name" the other way is the same request and replays the template
/// the first call created, while a retry that assigns a different name is the
/// modelled 409 `ConflictException`.
#[tokio::test]
async fn a_client_token_replay_compares_the_name_a_create_assigns() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // (case, the first request's name member, the retry's, whether they are the
    // same request)
    let cases: &[(&str, Option<&str>, Option<&str>, bool)] = &[
        ("an empty name, retried with none", Some(""), None, true),
        ("no name, retried with an empty one", None, Some(""), true),
        ("a name, retried with itself", Some("kept"), Some("kept"), true),
        (
            "an empty name, retried with a name",
            Some(""),
            Some("assigned-later"),
            false,
        ),
        (
            "a name, retried with the empty string",
            Some("removed-later"),
            Some(""),
            false,
        ),
    ];

    for (index, (case, first, retry, same_request)) in cases.iter().enumerate() {
        let token = format!("token-{index}");
        let mut request = json!({
            "policyStoreId": store_id,
            "statement": template_statement(index),
            "clientToken": token,
        });
        if let Some(first) = first {
            request["name"] = json!(first);
        }
        let created = ok(&router, "CreatePolicyTemplate", &request).await;
        let template_id = created["policyTemplateId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let mut retried = json!({
            "policyStoreId": store_id,
            "statement": template_statement(index),
            "clientToken": token,
        });
        if let Some(retry) = retry {
            retried["name"] = json!(retry);
        }
        let (status, body) = call(&router, "CreatePolicyTemplate", &retried).await;
        if *same_request {
            assert_eq!(
                (status, body["policyTemplateId"].as_str()),
                (200, Some(template_id.as_str())),
                "case {case:?}: the retry must replay {template_id}, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (409, "ConflictException"),
                "case {case:?}: a retry assigning a different name conflicts, got {status} {body}"
            );
        }

        // Whatever the retry answered, the store still holds exactly the one
        // template the first call created for this row, with the name that call
        // assigned.
        let (status, fetched) = get_template(&router, &store_id, &template_id).await;
        let expected = first.filter(|name| !name.is_empty());
        assert_eq!(
            (status, reported_name(&fetched)),
            (200, expected),
            "case {case:?}: the stored template expected name {expected:?}, got {status} {fetched}"
        );
    }
}
