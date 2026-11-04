// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `ListPolicyStores`: what it reports about each persisted
//! policy store, how it pages, how it follows state and what it refuses.
//!
//! The rule under test: the operation enumerates the policy stores the ACCOUNT
//! holds in a stable order, reports each as a `PolicyStoreItem` whose every
//! member is what `GetPolicyStore` reports for the same stored store, pages by
//! this service's one pagination rule, and writes nothing.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! listed store is only visible if `CreatePolicyStore` really reached this
//! service's state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, error_code, ok, ok_in, router};

/// The members a `PolicyStoreItem` carries, in the model's order — from
/// `smithy_query(query="shape", shape="PolicyStoreItem")`: `policyStoreId`,
/// `arn`, `createdDate`, `lastUpdatedDate` and `description`. Everything else a
/// store holds is reported by `GetPolicyStore` alone.
const ITEM_MEMBERS: &[&str] = &[
    "policyStoreId",
    "arn",
    "createdDate",
    "lastUpdatedDate",
    "description",
];

/// Members `GetPolicyStore` reports that a `PolicyStoreItem` does NOT carry, so
/// an item cannot quietly grow a member the model does not give it.
const MEMBERS_THE_ITEM_NEVER_CARRIES: &[&str] = &[
    "validationSettings",
    "deletionProtection",
    "cedarVersion",
    "encryptionState",
    "tags",
];

/// One `CreatePolicyStore` request this suite seeds a store with.
struct Seed {
    case: &'static str,
    request: Value,
}

/// Create a policy store from `request` and return its `policyStoreId`.
async fn create_store(router: &MultiServiceRouter, request: &Value) -> String {
    ok(router, "CreatePolicyStore", request).await["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string()
}

/// Create a bare store (validation `OFF`, nothing else set) and return its id.
async fn bare_store(router: &MultiServiceRouter) -> String {
    create_store(router, &json!({ "validationSettings": { "mode": "OFF" } })).await
}

/// List one page and return the parsed response, requiring success.
async fn list(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "ListPolicyStores", request).await
}

/// The ids one page carries. `policyStores` is **required**, so a response that
/// omits it is a defect rather than an empty page.
fn ids_of(case: &str, page: &Value) -> Vec<String> {
    page["policyStores"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: policyStores is required, got {page}"))
        .iter()
        .map(|item| {
            item["policyStoreId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: policyStoreId is required, got {item}"))
                .to_string()
        })
        .collect()
}

/// Walk every page at `page_size` items a time and return the listed items in
/// page order. Fails the calling test if the walk does not terminate within
/// `bound` pages.
async fn walk(
    router: &MultiServiceRouter,
    page_size: usize,
    bound: usize,
    case: &str,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({ "maxResults": page_size });
        if let Some(token) = token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let page = list(router, &request).await;
        let page_items = page["policyStores"].as_array().cloned().unwrap_or_default();
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
// What a PolicyStoreItem reports
// ---------------------------------------------------------------------------

/// A listed `PolicyStoreItem` reports, for every member it carries, exactly what
/// `GetPolicyStore` reports about the same stored store — across the whole range
/// of optional members a create may or may not set — and carries none of the
/// members that read reports beyond them.
#[tokio::test]
async fn a_listed_policy_store_reports_what_get_policy_store_reports() {
    // `PolicyStoreDescription` is `len 0..150`, so the longest legal description
    // is 150 characters and must round-trip whole.
    let longest_description = "d".repeat(150);

    // Rows differ along which optional members the create carries, and along the
    // settings an item must NOT report: the description (absent, empty, a
    // multi-byte one, the longest legal one), the validation mode, the
    // deletion-protection flag, the encryption arm and a create-time tag set.
    let seeds = vec![
        Seed {
            case: "bare",
            request: json!({ "validationSettings": { "mode": "OFF" } }),
        },
        Seed {
            case: "described",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "description": "a described store",
            }),
        },
        Seed {
            case: "an empty description",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "description": "",
            }),
        },
        Seed {
            case: "a multi-byte description",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "description": "дом · 家 · ماذا",
            }),
        },
        Seed {
            case: "the longest legal description",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "description": longest_description,
            }),
        },
        Seed {
            case: "strict validation and deletion protection",
            request: json!({
                "validationSettings": { "mode": "STRICT" },
                "deletionProtection": "ENABLED",
                "description": "guarded",
            }),
        },
        Seed {
            case: "a customer-managed encryption key",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "encryptionSettings": {
                    "kmsEncryptionSettings": {
                        "key": "arn:aws:kms:us-east-1:123456789012:key/abc",
                        "encryptionContext": { "team": "photos" },
                    },
                },
            }),
        },
        Seed {
            case: "create-time tags",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "tags": { "env": "test", "owner": "alice" },
            }),
        },
    ];

    let router = router();
    let mut created: Vec<(&'static str, String)> = Vec::new();
    for Seed { case, request } in &seeds {
        created.push((case, create_store(&router, request).await));
    }

    // Every seeded store fits in one page at the documented maximum, so one
    // request reports them all.
    let listed = list(&router, &json!({ "maxResults": seeds.len() })).await;
    let items = listed["policyStores"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        seeds.len(),
        "every created policy store must be listed, got {listed}"
    );

    for (case, id) in &created {
        let item = items
            .iter()
            .find(|item| item["policyStoreId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the created policy store {id} is missing from {listed}")
            });
        let got = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": id, "tags": true }),
        )
        .await;

        for member in ITEM_MEMBERS {
            assert_eq!(
                item.get(member),
                got.get(member),
                "case {case:?}: a listed item's {member} must be what GetPolicyStore reports {got}, \
                 got {item}"
            );
        }
        for member in MEMBERS_THE_ITEM_NEVER_CARRIES {
            assert!(
                item.get(member).is_none(),
                "case {case:?}: a PolicyStoreItem carries no {member}, got {item}"
            );
        }
        assert!(
            item["arn"].is_string() && item["createdDate"].is_string(),
            "case {case:?}: policyStoreId, arn and createdDate are required on a PolicyStoreItem, \
             got {item}"
        );
        let item_keys: Vec<String> = item
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        assert!(
            item_keys
                .iter()
                .all(|key| ITEM_MEMBERS.contains(&key.as_str())),
            "case {case:?}: an item may carry only PolicyStoreItem's members {ITEM_MEMBERS:?}, \
             got {item_keys:?}"
        );
    }

    // `@readonly`: listing twice reports the same thing, and a list changes
    // nothing a later reader sees.
    let again = list(&router, &json!({ "maxResults": seeds.len() })).await;
    assert_eq!(
        again, listed,
        "a read must not change what a later caller sees, got {again} after {listed}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `maxResults` truncates, `nextToken` resumes exactly where the page stopped,
/// and the last page carries no token — for every page size, over an account
/// whose stores were created one at a time.
#[tokio::test]
async fn list_policy_stores_pages_over_every_persisted_store() {
    let router = router();

    let total = 4usize;
    let mut created_ids: Vec<String> = Vec::new();
    for _ in 0..total {
        created_ids.push(bare_store(&router).await);
    }
    created_ids.sort();

    // Rows differ along page size, which is what the rule generalises over: one
    // at a time, a page that divides the total, one that does not, exactly the
    // total, and one page size past it.
    for page_size in [1usize, 2, 3, 4, 5] {
        let case = format!("page_size={page_size}");
        let items = walk(&router, page_size, total + 1, &case).await;
        let mut walked: Vec<String> = items
            .iter()
            .map(|item| item["policyStoreId"].as_str().unwrap_or_default().to_string())
            .collect();
        let ordered = walked.clone();
        walked.sort();
        assert_eq!(
            walked, created_ids,
            "case {case:?}: walking every page must yield each policy store exactly once, \
             got {walked:?}"
        );
        assert_eq!(
            ordered, walked,
            "case {case:?}: the walk must enumerate in a stable, ascending order, got {ordered:?}"
        );
    }

    // An account holding no store is an empty list and no cursor, not a refusal.
    let empty_account = super::harness::router();
    let empty = list(&empty_account, &json!({})).await;
    assert_eq!(
        empty["policyStores"],
        json!([]),
        "case \"an account with no policy stores\": policyStores is required and empty, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "case \"an account with no policy stores\": the last page carries no nextToken, got {empty}"
    );
}

/// `ListPolicyStores` is `@paginated`, so an absent `maxResults` takes the
/// default page size its own documentation states ("the operation defaults to 10
/// policy stores per response") and an over-large one is clamped to the
/// documented cap ("a maximum of 50 policy stores per response"): one small
/// request can never materialise an unbounded response.
#[tokio::test]
async fn list_policy_stores_bounds_the_page_it_builds() {
    /// `DEFAULT_PAGE_SIZE` in `src/state.rs`, the documented default of 10.
    const DEFAULT_PAGE_SIZE: usize = 10;
    /// `MAX_PAGE_SIZE` in `src/state.rs`, the documented maximum of 50.
    const MAX_PAGE_SIZE: usize = 50;

    let router = router();
    let total = MAX_PAGE_SIZE + 5;
    for _ in 0..total {
        bare_store(&router).await;
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
        let mut request = json!({});
        if !max_results.is_null() {
            request["maxResults"] = max_results.clone();
        }
        let page = list(&router, &request).await;
        assert_eq!(
            page["policyStores"].as_array().map(Vec::len),
            Some(*expected_len),
            "case {case:?}: unexpected page length, got {}",
            page["policyStores"].as_array().map_or(0, Vec::len)
        );
        assert!(
            page["nextToken"].is_string(),
            "case {case:?}: more stores remain, so the page carries a nextToken, got {page}"
        );
    }

    // A page is never padded: an account holding fewer stores than the page size
    // reports what it holds, and that page is the last one.
    let small_account = super::harness::router();
    for _ in 0..3 {
        bare_store(&small_account).await;
    }
    let page = list(&small_account, &json!({ "maxResults": 2_000 })).await;
    assert_eq!(
        page["policyStores"].as_array().map(Vec::len),
        Some(3),
        "case \"an account smaller than the page size\": expected the three it holds, got {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "case \"an account smaller than the page size\": the last page carries no nextToken, \
         got {page}"
    );
}

// ---------------------------------------------------------------------------
// What it lists, and what it does not
// ---------------------------------------------------------------------------

/// The listing follows state: a store created after a list appears in the next
/// one, a store whose record was written reports the written values, and a
/// deleted store is gone — while the others stay.
#[tokio::test]
async fn list_policy_stores_follows_state() {
    let router = router();

    // Rows differ along what happened to the store since it was created:
    // nothing, an accepted write to its record, or a delete.
    let untouched = bare_store(&router).await;
    let updated = bare_store(&router).await;
    let deleted = bare_store(&router).await;

    let before = ids_of("the three created stores", &list(&router, &json!({})).await);
    let mut expected = vec![untouched.clone(), updated.clone(), deleted.clone()];
    expected.sort();
    let mut sorted_before = before.clone();
    sorted_before.sort();
    assert_eq!(
        sorted_before, expected,
        "case \"the three created stores\": every created store must be listed, got {before:?}"
    );

    ok(
        &router,
        "UpdatePolicyStore",
        &json!({
            "policyStoreId": updated,
            "validationSettings": { "mode": "OFF" },
            "description": "written after the create",
        }),
    )
    .await;
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": deleted }),
    )
    .await;

    let page = list(&router, &json!({})).await;
    let mut listed = ids_of("after a write and a delete", &page);
    listed.sort();
    let mut remaining = vec![untouched.clone(), updated.clone()];
    remaining.sort();
    assert_eq!(
        listed, remaining,
        "case \"after a write and a delete\": the deleted store is gone and the others remain, \
         got {page}"
    );

    let items = page["policyStores"].as_array().cloned().unwrap_or_default();
    let item = items
        .iter()
        .find(|item| item["policyStoreId"].as_str() == Some(updated.as_str()))
        .unwrap_or_else(|| panic!("the updated store must be listed, got {page}"));
    let got = ok(
        &router,
        "GetPolicyStore",
        &json!({ "policyStoreId": updated }),
    )
    .await;
    for member in ITEM_MEMBERS {
        assert_eq!(
            item.get(member),
            got.get(member),
            "case \"after a write\": a listed item's {member} must be what GetPolicyStore \
             reports {got}, got {item}"
        );
    }
    assert_eq!(
        item["description"].as_str(),
        Some("written after the create"),
        "case \"after a write\": the item reports the written description, got {item}"
    );
    assert_ne!(
        item["lastUpdatedDate"], item["createdDate"],
        "case \"after a write\": the written record's lastUpdatedDate has moved past its \
         createdDate, got {item}"
    );
}

/// A policy store belongs to the ACCOUNT and is reached by its id alone — its
/// ARN carries no region segment — so the listing reports every store the
/// account holds whichever Region a request was signed for.
#[tokio::test]
async fn list_policy_stores_reports_the_whole_account_from_any_region() {
    let router = router();

    // Rows differ along the Region the creating request was signed for.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let mut created: Vec<String> = Vec::new();
    for region in regions {
        let body = ok_in(
            &router,
            region,
            "CreatePolicyStore",
            &json!({ "validationSettings": { "mode": "OFF" }, "description": region }),
        )
        .await;
        created.push(
            body["policyStoreId"]
                .as_str()
                .expect("policyStoreId is required on the CreatePolicyStore response")
                .to_string(),
        );
    }
    created.sort();

    for region in regions {
        let case = format!("listed from {region}");
        let page = ok_in(
            &router,
            region,
            "ListPolicyStores",
            &json!({ "maxResults": regions.len() }),
        )
        .await;
        let mut listed = ids_of(&case, &page);
        listed.sort();
        assert_eq!(
            listed, created,
            "case {case:?}: every store the account holds must be listed, got {page}"
        );
    }
}

// ---------------------------------------------------------------------------
// What it refuses
// ---------------------------------------------------------------------------

/// A `nextToken` this service did not mint FOR THIS LISTING is a 400 naming the
/// member — not an empty or silently truncated page, which would hide the
/// client's mistake.
#[tokio::test]
async fn list_policy_stores_refuses_a_cursor_it_did_not_mint() {
    let router = router();
    let store_id = bare_store(&router).await;
    bare_store(&router).await;

    // A real cursor of another listing of this service, minted by asking that
    // listing for a page smaller than what it holds — never built here, so what
    // this asserts is about the service's own cursors.
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({
            "aliasName": "policy-store-alias/the-store",
            "policyStoreId": store_id,
        }),
    )
    .await;
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({
            "aliasName": "policy-store-alias/the-other-name",
            "policyStoreId": store_id,
        }),
    )
    .await;
    let alias_cursor = ok(
        &router,
        "ListPolicyStoreAliases",
        &json!({ "maxResults": 1 }),
    )
    .await["nextToken"]
        .as_str()
        .expect("a page smaller than the listing must hand back a cursor")
        .to_string();

    // Rows differ along HOW the cursor is wrong: not this service's encoding at
    // all, an odd number of hex digits, hex that decodes to bytes which are not
    // text, hex that decodes perfectly well to text naming no listing of this
    // service, and a cursor another listing of this service really minted.
    let forged: Vec<(String, String)> = vec![
        ("not a cursor".to_string(), "notacursor".to_string()),
        ("odd-length hex".to_string(), "953".to_string()),
        ("hex that is not text".to_string(), "ff".to_string()),
        (
            "hex of a published id shape".to_string(),
            hex::encode("PSEXAMPLEabcdefg111111"),
        ),
        (
            "a cursor minted by ListPolicyStoreAliases".to_string(),
            alias_cursor,
        ),
    ];

    for (case, token) in &forged {
        let (status, body) = call(&router, "ListPolicyStores", &json!({ "nextToken": token })).await;
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
        assert_eq!(
            body["fieldList"][0]["message"].as_str(),
            Some("it is not a cursor this service issued"),
            "case {case:?}: unexpected fieldList reason, got {body}"
        );
        assert_eq!(
            body["fieldList"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: only the cursor is at fault, got {body}"
        );
    }

    // The refusals above are about the cursor, not about the listing: its own
    // cursor resumes the walk.
    let first = list(&router, &json!({ "maxResults": 1 })).await;
    let token = first["nextToken"]
        .as_str()
        .expect("a page smaller than the account must hand back a cursor");
    let second = list(&router, &json!({ "maxResults": 1, "nextToken": token })).await;
    let resumed = ids_of("its own cursor", &second);
    assert_eq!(
        resumed.len(),
        1,
        "case \"its own cursor\": the listing must resume its own walk, got {second}"
    );
    assert_ne!(
        resumed, ids_of("the first page", &first),
        "case \"its own cursor\": the resumed page must start past the first, got {second}"
    );
}
