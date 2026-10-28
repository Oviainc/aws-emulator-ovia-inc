// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `ListPolicyTemplates`: what it reports about each
//! persisted policy template, how it pages, which store references it resolves
//! and what it refuses.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! listed template is only visible if `CreatePolicyTemplate` really reached this
//! service's state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, error_code, ok, store_with_validation_off as store};

/// A Cedar template statement that uses the `?principal` placeholder. One
/// statement form is enough where the rule under test is about listing rather
/// than about parsing; `policy_templates.rs` is the suite that spans the
/// placeholder forms.
const TEMPLATE_STATEMENT: &str =
    "permit(principal == ?principal, action == Action::\"view\", resource == Photo::\"p\");";

/// Create one policy template and return its `policyTemplateId`.
async fn create_template(
    router: &MultiServiceRouter,
    store_id: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> String {
    let mut request = json!({ "policyStoreId": store_id, "statement": TEMPLATE_STATEMENT });
    if let Some(name) = name {
        request["name"] = json!(name);
    }
    if let Some(description) = description {
        request["description"] = json!(description);
    }
    let created = ok(router, "CreatePolicyTemplate", &request).await;
    created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string()
}

/// List one page and return the parsed response, requiring success.
async fn list(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "ListPolicyTemplates", request).await
}

/// Walk every page of a request, `page_size` at a time, and return the listed
/// items in the order they arrived. Fails the calling test if the walk does not
/// terminate within `bound` pages.
async fn walk(
    router: &MultiServiceRouter,
    request: &Value,
    page_size: usize,
    bound: usize,
    case: &str,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut page_request = request.clone();
        page_request["maxResults"] = json!(page_size);
        if let Some(token) = token.as_deref() {
            page_request["nextToken"] = json!(token);
        }
        let page = list(router, &page_request).await;
        let page_items = page["policyTemplates"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page_items.len() <= page_size,
            "case {case:?}: a page may not exceed maxResults {page_size}, got {page}"
        );
        items.extend(page_items);
        pages += 1;
        assert!(
            pages <= bound,
            "case {case:?}: pagination did not terminate within {bound} pages, got {page}"
        );
        match page["nextToken"].as_str() {
            Some(next) => {
                assert!(
                    next.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_=+/.".contains(c)),
                    "case {case:?}: nextToken must match ^[A-Za-z0-9-_=+/\\.]*$, got {next:?}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    items
}

// ---------------------------------------------------------------------------
// What a PolicyTemplateItem reports
// ---------------------------------------------------------------------------

/// A listed `PolicyTemplateItem` reports, for every member it carries, exactly
/// what `GetPolicyTemplate` reports about the same stored template — across the
/// whole range of optional members a create may or may not set. The item carries
/// no `statement`: `PolicyTemplateItem`'s member set, from
/// `smithy_query(query="shape", shape="PolicyTemplateItem")`, is
/// `policyStoreId`, `policyTemplateId`, `description`, `createdDate`,
/// `lastUpdatedDate` and `name`, so the Cedar text is reported by the Get read
/// alone.
#[tokio::test]
async fn a_listed_policy_template_reports_what_get_policy_template_reports() {
    /// The members a `PolicyTemplateItem` carries, in the model's order.
    const ITEM_MEMBERS: &[&str] = &[
        "policyStoreId",
        "policyTemplateId",
        "description",
        "createdDate",
        "lastUpdatedDate",
        "name",
    ];

    // Rows differ along which optional members the create carries.
    let cases: &[(&str, Option<&str>, Option<&str>)] = &[
        ("bare", None, None),
        ("named", Some("named-template"), None),
        ("described", None, Some("a described template")),
        (
            "named and described",
            Some("both-members"),
            Some("both members"),
        ),
        (
            "an empty description",
            Some("empty-description"),
            // `PolicyTemplateDescription` is `len 0..150`, so the empty string
            // is a legal description and must round-trip as one.
            Some(""),
        ),
    ];

    let router = super::harness::router();
    let store_id = store(&router).await;
    let mut created: Vec<(String, String)> = Vec::new();
    for (case, name, description) in cases {
        let id = create_template(&router, &store_id, *name, *description).await;
        created.push(((*case).to_string(), id));
    }

    let listed = list(&router, &json!({ "policyStoreId": store_id })).await;
    let items = listed["policyTemplates"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        cases.len(),
        "every created policy template must be listed, got {listed}"
    );

    for (case, id) in &created {
        let item = items
            .iter()
            .find(|item| item["policyTemplateId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the created policy template {id} is missing from {listed}")
            });
        let got = ok(
            &router,
            "GetPolicyTemplate",
            &json!({ "policyStoreId": store_id, "policyTemplateId": id }),
        )
        .await;
        for member in ITEM_MEMBERS {
            assert_eq!(
                item.get(member),
                got.get(member),
                "case {case:?}: a listed item's {member} must be what GetPolicyTemplate reports {got}, got {item}"
            );
        }
        assert!(
            item.get("statement").is_none(),
            "case {case:?}: a PolicyTemplateItem carries no statement, got {item}"
        );
        assert_eq!(
            item["policyStoreId"],
            json!(store_id),
            "case {case:?}: an item must name its store, got {item}"
        );
        assert!(
            item["createdDate"].is_string() && item["lastUpdatedDate"].is_string(),
            "case {case:?}: createdDate and lastUpdatedDate are required on a PolicyTemplateItem, got {item}"
        );
        let item_keys: Vec<&String> = item
            .as_object()
            .map(|object| object.keys().collect())
            .unwrap_or_default();
        assert!(
            item_keys.iter().all(|key| ITEM_MEMBERS.contains(&key.as_str())),
            "case {case:?}: an item may carry only PolicyTemplateItem's members {ITEM_MEMBERS:?}, got {item_keys:?}"
        );
    }

    // `@readonly`: listing twice reports the same thing, and a list changes
    // nothing a later reader sees.
    let again = list(&router, &json!({ "policyStoreId": store_id })).await;
    assert_eq!(
        again, listed,
        "a read must not change what a later caller sees, got {again} after {listed}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `maxResults` truncates, `nextToken` resumes exactly where the page stopped,
/// and the last page carries no token — for every page size, over a store whose
/// templates were created one at a time.
#[tokio::test]
async fn list_policy_templates_pages_over_every_persisted_template() {
    let router = super::harness::router();
    let store_id = store(&router).await;

    let total = 4usize;
    let mut created_ids: Vec<String> = Vec::new();
    for n in 0..total {
        created_ids
            .push(create_template(&router, &store_id, Some(&format!("tpl-{n}")), None).await);
    }
    created_ids.sort();

    // Rows differ along page size, which is what the rule generalises over: one
    // at a time, a page that divides the total, one that does not, exactly the
    // total, and one page size past it.
    for page_size in [1usize, 2, 3, 4, 5] {
        let case = format!("page_size={page_size}");
        let items = walk(
            &router,
            &json!({ "policyStoreId": store_id }),
            page_size,
            total + 1,
            &case,
        )
        .await;
        let mut walked: Vec<String> = items
            .iter()
            .map(|item| {
                item["policyTemplateId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        walked.sort();
        assert_eq!(
            walked, created_ids,
            "case {case:?}: walking every page must yield each policy template exactly once, got {walked:?}"
        );
    }

    // An empty store is an empty list and no cursor, not a refusal.
    let empty_store = store(&router).await;
    let empty = list(&router, &json!({ "policyStoreId": empty_store })).await;
    assert_eq!(
        empty["policyTemplates"],
        json!([]),
        "case \"store with no policy templates\": policyTemplates is required and empty, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "case \"store with no policy templates\": the last page carries no nextToken, got {empty}"
    );
}

/// `ListPolicyTemplates` is `@paginated`, so an absent `maxResults` takes the
/// default page size its own documentation states ("the operation defaults to 10
/// policy templates per response") and an over-large one is clamped to the
/// documented cap ("a maximum of 50 policy templates per response"): one small
/// request can never materialise an unbounded response.
#[tokio::test]
async fn list_policy_templates_bounds_the_page_it_builds() {
    /// `DEFAULT_PAGE_SIZE` in `src/state.rs`, the documented default of 10.
    const DEFAULT_PAGE_SIZE: usize = 10;
    /// `MAX_PAGE_SIZE` in `src/state.rs`, the documented maximum of 50.
    const MAX_PAGE_SIZE: usize = 50;

    let router = super::harness::router();
    let store_id = store(&router).await;
    let total = MAX_PAGE_SIZE + 5;
    for n in 0..total {
        create_template(&router, &store_id, Some(&format!("bounded-{n}")), None).await;
    }

    // Rows differ along what the caller asked for: nothing, below the default,
    // between the default and the cap, exactly the cap, and past it.
    let cases: &[(&str, Value, usize)] = &[
        ("no maxResults", Value::Null, DEFAULT_PAGE_SIZE),
        ("maxResults of one", json!(1), 1),
        ("maxResults below the default", json!(7), 7),
        ("maxResults between the default and the cap", json!(23), 23),
        ("maxResults at the cap", json!(MAX_PAGE_SIZE), MAX_PAGE_SIZE),
        ("maxResults above the cap", json!(2_000), MAX_PAGE_SIZE),
    ];
    for (case, max_results, expected_len) in cases {
        let mut request = json!({ "policyStoreId": store_id });
        if !max_results.is_null() {
            request["maxResults"] = max_results.clone();
        }
        let page = list(&router, &request).await;
        assert_eq!(
            page["policyTemplates"].as_array().map(Vec::len),
            Some(*expected_len),
            "case {case:?}: unexpected page length, got {}",
            page["policyTemplates"].as_array().map_or(0, Vec::len)
        );
        assert!(
            page["nextToken"].is_string(),
            "case {case:?}: more templates remain, so the page carries a nextToken, got {page}"
        );
    }

    // A page is never padded: a store holding fewer templates than the page size
    // reports what it holds, and that page is the last one.
    let small_store = store(&router).await;
    for n in 0..3 {
        create_template(&router, &small_store, Some(&format!("small-{n}")), None).await;
    }
    let page = list(
        &router,
        &json!({ "policyStoreId": small_store, "maxResults": 2_000 }),
    )
    .await;
    assert_eq!(
        page["policyTemplates"].as_array().map(Vec::len),
        Some(3),
        "case \"a store smaller than the page size\": expected the three it holds, got {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "case \"a store smaller than the page size\": the last page carries no nextToken, got {page}"
    );
}

// ---------------------------------------------------------------------------
// What it lists, and what it does not
// ---------------------------------------------------------------------------

/// A list reports the templates of the store it names and only those, and it
/// tracks state: a template deleted after the first list is gone from the next.
#[tokio::test]
async fn list_policy_templates_reports_only_the_named_store_and_follows_state() {
    let router = super::harness::router();
    // Rows differ along how many templates each store holds, so a list cannot
    // reach past the store it was asked about.
    let counts = [0usize, 1, 3];
    let mut stores: Vec<(String, Vec<String>)> = Vec::new();
    for (index, count) in counts.iter().enumerate() {
        let store_id = store(&router).await;
        let mut ids: Vec<String> = Vec::new();
        for n in 0..*count {
            ids.push(create_template(&router, &store_id, Some(&format!("s{index}-t{n}")), None).await);
        }
        ids.sort();
        stores.push((store_id, ids));
    }

    for (store_id, ids) in &stores {
        let case = format!("store holding {} templates", ids.len());
        let page = list(&router, &json!({ "policyStoreId": store_id })).await;
        let mut listed: Vec<String> = page["policyTemplates"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item["policyTemplateId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        listed.sort();
        assert_eq!(
            &listed, ids,
            "case {case:?}: a list reports exactly the named store's templates, got {page}"
        );
    }

    // Deleting a template removes it from the next list; the others stay.
    let (store_id, ids) = stores
        .iter()
        .find(|(_, ids)| ids.len() == 3)
        .expect("one store in the table holds three templates");
    let removed = &ids[0];
    ok(
        &router,
        "DeletePolicyTemplate",
        &json!({ "policyStoreId": store_id, "policyTemplateId": removed }),
    )
    .await;
    let page = list(&router, &json!({ "policyStoreId": store_id })).await;
    let mut remaining: Vec<String> = page["policyTemplates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["policyTemplateId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    remaining.sort();
    assert_eq!(
        remaining,
        ids[1..].to_vec(),
        "case \"after a delete\": the deleted template is gone and the others remain, got {page}"
    );
}

// ---------------------------------------------------------------------------
// What it refuses, and which store references it resolves
// ---------------------------------------------------------------------------

/// A `policyStoreId` that names no live store is the model's 404, and a
/// `nextToken` this service did not mint is a 400 naming the member — not an
/// empty page, which would hide the client's mistake.
#[tokio::test]
async fn list_policy_templates_refuses_an_unknown_store_and_a_forged_cursor() {
    let router = super::harness::router();
    let store_id = store(&router).await;
    create_template(&router, &store_id, Some("the-template"), None).await;

    // Rows differ along WHICH reference is unknown: a store id, and an alias
    // name — the two forms a `policyStoreId` member may carry.
    let unknown: &[(&str, &str, &str)] = &[
        ("unknown store id", "PSnotastore", "POLICY_STORE"),
        (
            "unknown alias name",
            "policy-store-alias/nope",
            "POLICY_STORE_ALIAS",
        ),
    ];
    for (case, reference, resource_type) in unknown {
        let (status, body) = call(
            &router,
            "ListPolicyTemplates",
            &json!({ "policyStoreId": reference }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(reference.trim_start_matches("policy-store-alias/")),
            "case {case:?}: the exception must name the missing resource, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: unexpected resourceType, got {body}"
        );
    }

    // Rows differ along HOW the cursor is wrong: not this service's encoding at
    // all, an odd number of hex digits, and hex that decodes to bytes which are
    // not text.
    for (case, token) in [
        ("not a cursor", "notacursor"),
        ("odd-length hex", "953"),
        ("hex that is not text", "ff"),
    ] {
        let (status, body) = call(
            &router,
            "ListPolicyTemplates",
            &json!({ "policyStoreId": store_id, "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["path"].as_str(),
            Some("/nextToken"),
            "case {case:?}: fieldList must name the offending member, got {body}"
        );
    }
}

/// A `policyStoreId` may name its store by ID or by alias name, so both reach
/// the same store's policy templates.
#[tokio::test]
async fn list_policy_templates_resolves_a_store_by_id_or_by_alias() {
    let router = super::harness::router();
    let store_id = store(&router).await;
    let template_id = create_template(&router, &store_id, Some("aliased"), None).await;

    // The prefix every alias name carries, from
    // `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias
    // name must always be prefixed with `policy-store-alias/`."
    let alias_name = "policy-store-alias/the-store";
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
    )
    .await;

    for (case, reference) in [("by id", store_id.as_str()), ("by alias name", alias_name)] {
        let page = list(&router, &json!({ "policyStoreId": reference })).await;
        assert_eq!(
            page["policyTemplates"][0]["policyTemplateId"].as_str(),
            Some(template_id.as_str()),
            "case {case:?}: both references must reach the same store's templates, got {page}"
        );
        assert_eq!(
            page["policyTemplates"][0]["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: an item names the store's ID, whichever reference was used, got {page}"
        );
    }
}
