// SPDX-License-Identifier: CC0-1.0

//! `ListPolicies`' `policyTemplateId` filter, and the template REFERENCES it
//! accepts.
//!
//! The rule under test: a `policyTemplateId` request member names a policy
//! template either by that template's id or by `name/<policyTemplateName>`, and
//! the two forms name the SAME template wherever the member appears — so this
//! filter selects the policies instantiated from the template its reference
//! resolves to, whichever form the caller wrote, and a reference that resolves to
//! no template of the store selects nothing: an empty page with HTTP 200, since
//! the model declares no error for this member.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`], so a value is only visible
//! to a later request if it really reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, error_code, ok, router, store_with_validation_off};

/// The reference-path prefix that makes a `policyTemplateId` member name a
/// template by NAME, which `GetPolicyTemplate` documents as interchangeable with
/// the id ("You can use the policy template name in place of the policy template
/// ID. When using a name, prefix it with name/").
const NAME_PREFIX: &str = "name/";

/// One policy template of the fixture, and how many policies follow it.
struct Template {
    case: &'static str,
    /// The `name` to assign, or `None` to leave the template unnamed. Every
    /// character used below is inside `CreatePolicyTemplateInput$name`'s
    /// `^[a-zA-Z0-9-/_]*$` (smithy_query shape="PolicyTemplateName").
    name: Option<&'static str>,
    statement: &'static str,
    /// How many template-linked policies to instantiate from this template.
    links: usize,
}

/// The templates the suite seeds: rows differ along the SHAPE of the name a
/// reference has to resolve (a plain name, one using the remaining character
/// classes the member allows, a name nothing links to, and no name at all) and
/// along how many policies follow the template (two, one, none).
const TEMPLATES: &[Template] = &[
    Template {
        case: "a plainly named template, with two linked policies",
        name: Some("vacation-viewers"),
        statement: "permit(principal == ?principal, action, resource);",
        links: 2,
    },
    Template {
        case: "a name using the member's other character classes",
        name: Some("team/alpha_viewers-2"),
        statement: "forbid(principal == ?principal, action, resource);",
        links: 1,
    },
    Template {
        case: "a named template nothing links to",
        name: Some("unused"),
        statement: "permit(principal == ?principal, action, resource) \
                    when { context.authenticated == true };",
        links: 0,
    },
    Template {
        case: "an unnamed template, nameable by its id alone",
        name: None,
        statement: "permit(principal == ?principal, action, resource);",
        links: 1,
    },
];

/// One policy the fixture instantiated from a template: the entity its link
/// bound `?principal` to, and the id it was minted with.
struct Link {
    principal: String,
    policy_id: String,
}

/// A seeded template: the id it was minted with, and the policies that follow it.
struct Seeded {
    case: &'static str,
    name: Option<&'static str>,
    template_id: String,
    links: Vec<Link>,
}

impl Seeded {
    /// The ids of the policies following this template, sorted, which is what a
    /// filter selecting it must report.
    fn linked(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .links
            .iter()
            .map(|link| link.policy_id.clone())
            .collect();
        ids.sort();
        ids
    }
}

/// The Cedar text of the one STATIC policy the fixture holds: it follows no
/// template, so no template reference may ever select it.
const STATIC_STATEMENT: &str = "permit(principal, action, resource);";

/// The Cedar entity type every link of the fixture binds `?principal` to.
const PRINCIPAL_ENTITY_TYPE: &str = "PhotoApp::User";

/// Seed a store with [`TEMPLATES`], their linked policies and one static policy.
async fn fixture(router: &MultiServiceRouter) -> (String, Vec<Seeded>, String) {
    let store_id = store_with_validation_off(router).await;
    let mut seeded: Vec<Seeded> = Vec::new();
    for (index, row) in TEMPLATES.iter().enumerate() {
        let mut request = json!({
            "policyStoreId": store_id,
            "statement": row.statement,
        });
        if let Some(name) = row.name {
            request["name"] = json!(name);
        }
        let created = ok(router, "CreatePolicyTemplate", &request).await;
        let template_id = created["policyTemplateId"]
            .as_str()
            .expect("policyTemplateId is required on the CreatePolicyTemplate response")
            .to_string();

        let mut links: Vec<Link> = Vec::new();
        for link in 0..row.links {
            let principal = format!("user-{index}-{link}");
            let policy = ok(
                router,
                "CreatePolicy",
                &json!({
                    "policyStoreId": store_id,
                    "definition": {
                        "templateLinked": {
                            "policyTemplateId": template_id,
                            "principal": {
                                "entityType": PRINCIPAL_ENTITY_TYPE,
                                "entityId": principal,
                            },
                        },
                    },
                }),
            )
            .await;
            links.push(Link {
                principal,
                policy_id: policy["policyId"]
                    .as_str()
                    .expect("policyId is required on the CreatePolicy response")
                    .to_string(),
            });
        }
        seeded.push(Seeded {
            case: row.case,
            name: row.name,
            template_id,
            links,
        });
    }

    let static_policy = ok(
        router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement": STATIC_STATEMENT } },
        }),
    )
    .await;
    let static_id = static_policy["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();
    (store_id, seeded, static_id)
}

/// The `policyId`s a `ListPolicies` response reports, sorted, so a comparison
/// does not depend on which order the page walked state in.
fn policy_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body["policies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["policyId"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// `ListPolicies` with `filter`, asserting the call SUCCEEDED (a filter that
/// selects nothing is a 200 with an empty list, never a fault) and returning the
/// sorted `policyId`s it selected.
async fn filtered(
    router: &MultiServiceRouter,
    case: &str,
    store_id: &str,
    filter: &Value,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListPolicies",
        &json!({
            "policyStoreId": store_id,
            "filter": filter,
            // The model caps this member at 50 and defaults it to 10; the
            // fixture holds fewer policies than either, and asking for the cap
            // keeps a page boundary out of these rows.
            "maxResults": 50,
        }),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: a template-reference filter must answer 200, got {status} {body} \
         (error code {:?})",
        error_code(&body)
    );
    assert!(
        body.get("__type").is_none(),
        "case {case:?}: a filter is not a refusal, so the response carries no fault type, got \
         {body}"
    );
    policy_ids(&body)
}

/// Both reference forms of a `policyTemplateId` select the same policies through
/// the `ListPolicies` filter — and resolve to the same template through
/// `GetPolicyTemplate`, which is what makes them interchangeable.
#[tokio::test]
async fn the_template_filter_accepts_a_template_id_and_a_name_reference_alike() {
    let router = router();
    let (store_id, seeded, static_id) = fixture(&router).await;

    for template in &seeded {
        let case = template.case;
        let linked = template.linked();
        let by_id = filtered(
            &router,
            case,
            &store_id,
            &json!({ "policyTemplateId": template.template_id }),
        )
        .await;
        assert_eq!(
            by_id, linked,
            "case {case:?}: the id {:?} must select exactly the policies following it, got \
             {by_id:?}",
            template.template_id
        );
        assert!(
            !by_id.contains(&static_id),
            "case {case:?}: a static policy follows no template, got {by_id:?}"
        );

        let Some(name) = template.name else {
            continue;
        };
        let reference = format!("{NAME_PREFIX}{name}");
        let by_name = filtered(
            &router,
            case,
            &store_id,
            &json!({ "policyTemplateId": reference }),
        )
        .await;
        assert_eq!(
            by_name, linked,
            "case {case:?}: {reference:?} must select the same policies as the id {:?}, got \
             {by_name:?}",
            template.template_id
        );

        // The same reference, read by the operation the form is documented on:
        // if it resolves there, it resolves here.
        let fetched = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": reference }),
        )
        .await;
        assert_eq!(
            fetched["policyTemplateId"].as_str(),
            Some(template.template_id.as_str()),
            "case {case:?}: {reference:?} must resolve to the filtered template, got {fetched}"
        );
    }
}

/// A `policyTemplateId` reference that names no template of the store selects
/// nothing, with HTTP 200: the member models no error, so an unresolvable
/// reference is an empty page.
#[tokio::test]
async fn a_template_reference_naming_no_template_selects_nothing() {
    let router = router();
    let (store_id, seeded, _) = fixture(&router).await;
    let named = seeded
        .iter()
        .find(|template| template.name.is_some())
        .expect("the fixture seeds named templates");
    let name = named.name.unwrap_or_default();

    // (case, the reference to filter on). Rows differ along WHY the reference
    // resolves to no template of this store.
    let cases: Vec<(String, String)> = vec![
        (
            "a name no template of the store carries".to_string(),
            format!("{NAME_PREFIX}no-such-template"),
        ),
        (
            "an id no template of the store carries".to_string(),
            "PT0000000000000000000000".to_string(),
        ),
        (
            "a bare name: without the prefix it is read as an id, and it is not one".to_string(),
            name.to_string(),
        ),
        (
            "the prefix alone, naming the empty name no template can be assigned".to_string(),
            NAME_PREFIX.to_string(),
        ),
        (
            "the prefix doubled, so the name it names carries the prefix itself".to_string(),
            format!("{NAME_PREFIX}{NAME_PREFIX}{name}"),
        ),
    ];

    for (case, reference) in &cases {
        let selected = filtered(
            &router,
            case,
            &store_id,
            &json!({ "policyTemplateId": reference }),
        )
        .await;
        assert!(
            selected.is_empty(),
            "case {case:?}: {reference:?} names no template, so it must select no policy, got \
             {selected:?}"
        );
    }
}

/// A name reference is just one member of the filter: the other members still
/// narrow the selection it makes, and the page boundary still applies to it.
#[tokio::test]
async fn a_name_reference_combines_with_the_filter_s_other_members() {
    let router = router();
    let (store_id, seeded, _) = fixture(&router).await;
    let template = seeded
        .iter()
        .find(|template| template.links.len() > 1 && template.name.is_some())
        .expect("the fixture seeds a named template with more than one linked policy");
    let reference = format!("{NAME_PREFIX}{}", template.name.unwrap_or_default());
    let linked = template.linked();
    let one_link = &template.links[1];

    // (case, the other members to AND with the reference, the expected ids)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "policyType TEMPLATE_LINKED, which every policy following a template is",
            json!({ "policyType": "TEMPLATE_LINKED" }),
            linked.clone(),
        ),
        (
            "policyType STATIC, which no policy following a template is",
            json!({ "policyType": "STATIC" }),
            Vec::new(),
        ),
        (
            "the principal one link bound, so only that link matches",
            json!({
                "principal": {
                    "identifier": {
                        "entityType": PRINCIPAL_ENTITY_TYPE,
                        "entityId": one_link.principal,
                    },
                },
            }),
            vec![one_link.policy_id.clone()],
        ),
        (
            "a principal no link bound",
            json!({
                "principal": {
                    "identifier": {
                        "entityType": PRINCIPAL_ENTITY_TYPE,
                        "entityId": "nobody",
                    },
                },
            }),
            Vec::new(),
        ),
    ];

    for (case, extra, expected) in &cases {
        let mut filter = json!({ "policyTemplateId": reference });
        for (member, value) in extra.as_object().expect("each row's members are an object") {
            filter[member] = value.clone();
        }
        let selected = filtered(&router, case, &store_id, &filter).await;
        assert_eq!(
            &selected, expected,
            "case {case:?}: filter {filter} must select {expected:?}, got {selected:?}"
        );
    }

    // The same reference across a page boundary: one item per page, and the two
    // pages together are exactly what the unpaged filter selected.
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..linked.len() {
        let mut request = json!({
            "policyStoreId": store_id,
            "filter": { "policyTemplateId": reference },
            "maxResults": 1,
        });
        if let Some(token) = next_token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let body = ok(&router, "ListPolicies", &request).await;
        let ids = policy_ids(&body);
        assert_eq!(
            ids.len(),
            1,
            "page {page}: a one-item page of {reference:?} must carry one policy, got {body}"
        );
        seen.extend(ids);
        next_token = body["nextToken"].as_str().map(str::to_string);
        let last_page = page + 1 == linked.len();
        assert_eq!(
            next_token.is_none(),
            last_page,
            "page {page}: a page that ends the filtered listing carries no nextToken and one \
             that does not carries one, got {body}"
        );
    }
    seen.sort();
    assert_eq!(
        seen, linked,
        "paging {reference:?} one policy at a time must yield every policy following it, got \
         {seen:?}"
    );
}
