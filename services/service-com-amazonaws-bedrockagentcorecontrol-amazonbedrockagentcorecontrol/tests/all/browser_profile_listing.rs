// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListBrowserProfiles`.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what `CreateBrowserProfile` writes is what the listing reads.
//!
//! The rule under test: a browser-profile listing reports every profile the account
//! still holds, each summarised from that profile's own stored record — so the listing
//! cannot disagree with `GetBrowserProfile` — in one deterministic order, narrowed to the
//! profiles whose name is exactly the `name` the caller filtered by, and cut into pages
//! of at most `maxResults` entries that resume exactly where the previous page ended.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The span `src/state.rs` gives a browser profile's clock-owed removal, plus slack, so a
/// test can wait the clock out.
const PAST_REMOVAL: Duration = Duration::from_millis(3_300);

/// The published bounds of `maxResults` (`MaxResults`, range 1..100, from `smithy_query
/// operation=ListBrowserProfiles`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreateBrowserProfile`: `PUT /browser-profiles`
/// (`smithy_query operation=CreateBrowserProfile`).
async fn create_profile(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "PUT", "/browser-profiles", Some(body)).await
}

/// `GetBrowserProfile`: `GET /browser-profiles/{profileId}`.
async fn get_profile(router: &MultiServiceRouter, profile_id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/browser-profiles/{profile_id}"),
        None,
    )
    .await
}

/// `DeleteBrowserProfile`: `DELETE /browser-profiles/{profileId}`.
async fn delete_profile(router: &MultiServiceRouter, profile_id: &str) -> (u16, Value) {
    send(
        router,
        "DELETE",
        &format!("/browser-profiles/{profile_id}"),
        None,
    )
    .await
}

/// `ListBrowserProfiles`: `POST /browser-profiles`, with `maxResults` and `nextToken` on
/// the query string and the `name` filter in the payload
/// (`smithy_query operation=ListBrowserProfiles`).
async fn list_profiles(
    router: &MultiServiceRouter,
    name: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        "/browser-profiles".to_string()
    } else {
        format!("/browser-profiles?{}", query.join("&"))
    };
    let body = match name {
        Some(name) => json!({"name": name}),
        None => json!({}),
    };
    send(router, "POST", &uri, Some(&body)).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `profileSummaries` of a listing response as a vector.
fn summaries(body: &Value) -> Vec<Value> {
    body["profileSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_else(Vec::new)
}

/// The `profileId`s a listing response reports, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| text(summary, "profileId"))
        .collect()
}

/// Create a profile and hand back its `profileId`.
async fn created_id(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, created) = create_profile(router, body).await;
    assert_eq!(
        status, 200,
        "case {body}: expected HTTP 200 from CreateBrowserProfile, got {status} ({created})"
    );
    text(&created, "profileId")
}

/// Every member `BrowserProfileSummary` declares (`smithy_query
/// shape=BrowserProfileSummary`). Each is reported off the profile's own record, so the
/// listing's summary and `GetBrowserProfile`'s response must agree member for member.
const SUMMARY_MEMBERS: [&str; 10] = [
    "profileId",
    "profileArn",
    "name",
    "description",
    "status",
    "createdAt",
    "lastUpdatedAt",
    "lastSavedAt",
    "lastSavedBrowserSessionId",
    "lastSavedBrowserId",
];

/// The request shapes the listing spans: the members a create carries and the sizes they
/// take are what the stored profile — and so its summary — reports.
///
/// `Description`'s published bounds are `len 1..4096` and `BrowserProfileName`'s pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` admits a one-letter name and 48 characters at most
/// (`smithy_query operation=CreateBrowserProfile`), so the rows sit on those edges as
/// well as in the middle.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("only the required name", json!({"name": "plainListed"})),
        (
            "the shortest description the model admits",
            json!({"name": "minDescribedListed", "description": "x"}),
        ),
        (
            "the longest description the model admits",
            json!({"name": "maxDescribedListed", "description": "d".repeat(4096)}),
        ),
        (
            "a tagged profile",
            json!({
                "name": "taggedListed",
                "description": "the shared shopping profile",
                "tags": {"owner": "retail", "environment": "test"}
            }),
        ),
        (
            "the longest name the model admits",
            json!({"name": format!("n{}", "A".repeat(47))}),
        ),
        ("a one-letter name", json!({"name": "p"})),
        (
            "a name carrying underscores and digits",
            json!({"name": "profile_2_browser"}),
        ),
    ]
}

/// Every profile the account holds is listed, each summarised from its own record: the
/// listing's entry and `GetBrowserProfile`'s response agree on every member of
/// `BrowserProfileSummary`, and the page is ordered and carries no leftover token.
#[tokio::test]
async fn every_profile_is_listed_as_the_summary_its_own_read_reports() {
    let router = router();

    let rows = create_rows();
    let mut ids = Vec::new();
    for (case, body) in &rows {
        let (status, created) = create_profile(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateBrowserProfile, got {status} ({created})"
        );
        ids.push(text(&created, "profileId"));
    }

    let (status, listed) = list_profiles(&router, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"the whole listing\": expected HTTP 200 from ListBrowserProfiles, got {status} \
         ({listed})"
    );
    let entries = summaries(&listed);
    assert_eq!(
        entries.len(),
        ids.len(),
        "case \"the whole listing\": expected one summary per created profile ({}), got {} \
         ({listed})",
        ids.len(),
        entries.len()
    );
    assert!(
        listed["nextToken"].is_null(),
        "case \"the whole listing\": a page holding every profile carries no nextToken, got {}",
        listed["nextToken"]
    );

    let reported = listed_ids(&listed);
    let mut ordered = reported.clone();
    ordered.sort();
    assert_eq!(
        reported, ordered,
        "case \"the whole listing\": expected the profiles in ascending profileId order, got \
         {reported:?}"
    );

    for ((case, _), id) in rows.iter().zip(&ids) {
        let summary = entries
            .iter()
            .find(|entry| &text(entry, "profileId") == id)
            .unwrap_or_else(|| panic!("case {case:?}: profile {id} is missing from {listed}"));
        let (status, fetched) = get_profile(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBrowserProfile, got {status} ({fetched})"
        );
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                fetched.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: the summary's {member} must be what GetBrowserProfile reports \
                 ({}), got {}",
                fetched.get(member).cloned().unwrap_or(Value::Null),
                summary.get(member).cloned().unwrap_or(Value::Null)
            );
        }
    }
}

/// An account holding no profiles lists an empty page: `profileSummaries` is required, so
/// it is present and empty, and nothing is left over to resume from.
#[tokio::test]
async fn an_account_holding_no_profiles_lists_an_empty_page() {
    let router = router();

    for (case, max_results) in [
        ("no maxResults", None),
        ("the smallest page the model admits", Some(MAX_RESULTS_MIN)),
        ("the largest page the model admits", Some(MAX_RESULTS_MAX)),
    ] {
        let (status, listed) = list_profiles(&router, None, max_results, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListBrowserProfiles, got {status} ({listed})"
        );
        assert_eq!(
            listed["profileSummaries"],
            json!([]),
            "case {case:?}: expected the required profileSummaries present and empty, got {}",
            listed["profileSummaries"]
        );
        assert!(
            listed["nextToken"].is_null(),
            "case {case:?}: an empty listing carries no nextToken, got {}",
            listed["nextToken"]
        );
    }
}

/// A page holds at most `maxResults` entries and the `nextToken` it hands back resumes
/// exactly where it ended: for every page size, walking the pages reports each profile
/// once, in the order the single unpaged listing reports them.
#[tokio::test]
async fn pages_hold_at_most_max_results_entries_and_resume_where_they_ended() {
    let router = router();

    let names = [
        "pageA", "pageB", "pageC", "pageD", "pageE", "pageF", "pageG",
    ];
    for name in names {
        created_id(&router, &json!({"name": name})).await;
    }

    let (status, whole) = list_profiles(&router, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"the unpaged listing\": expected HTTP 200, got {status} ({whole})"
    );
    let expected = listed_ids(&whole);
    assert_eq!(
        expected.len(),
        names.len(),
        "case \"the unpaged listing\": expected {} profiles, got {} ({whole})",
        names.len(),
        expected.len()
    );

    // One row per page size: smaller than, equal to and larger than the number of
    // profiles held, so a page that leaves something over, one that ends exactly on the
    // last profile and one that holds them all are all walked.
    for page_size in [1_i64, 2, 3, 6, 7, 8] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, page) =
                list_profiles(&router, None, Some(page_size), token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?} (page {pages}): expected HTTP 200, got {status} ({page})"
            );
            let ids = listed_ids(&page);
            assert!(
                i64::try_from(ids.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?} (page {pages}): a page holds at most {page_size} entries, got {} \
                 ({page})",
                ids.len()
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages and {walked:?}"
            );
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages must concatenate to the unpaged listing {expected:?}, got \
             {walked:?}"
        );
    }
}

/// `maxResults` outside its published range (1..100) is refused at the boundary with the
/// modelled 400, and each published bound itself is served.
#[tokio::test]
async fn max_results_outside_the_published_range_is_refused() {
    let router = router();
    created_id(&router, &json!({"name": "boundedProfile"})).await;

    // (case, maxResults, expected status)
    let rows: [(&str, i64, u16); 4] = [
        ("one below the published minimum", MAX_RESULTS_MIN - 1, 400),
        ("the published minimum", MAX_RESULTS_MIN, 200),
        ("the published maximum", MAX_RESULTS_MAX, 200),
        ("one above the published maximum", MAX_RESULTS_MAX + 1, 400),
    ];

    for (case, max_results, expected_status) in rows {
        let (status, answered) = list_profiles(&router, None, Some(max_results), None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for maxResults={max_results}, got \
             {status} ({answered})"
        );
        if expected_status == 200 {
            assert_eq!(
                summaries(&answered).len(),
                1,
                "case {case:?}: expected the one profile held, got {answered}"
            );
        } else {
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {} ({answered})",
                error_code(&answered)
            );
        }
    }
}

/// `name` narrows the listing to the profiles whose name is EXACTLY it — not a prefix of
/// it — and a name no profile holds is an empty page rather than a failure.
#[tokio::test]
async fn name_filters_the_listing_to_that_exact_name() {
    let router = router();

    // `alpha` is a prefix of `alphaExtra`, so a filter that matched prefixes would report
    // two profiles for it.
    let alpha = created_id(&router, &json!({"name": "alpha"})).await;
    let alpha_extra = created_id(&router, &json!({"name": "alphaExtra"})).await;
    let beta = created_id(&router, &json!({"name": "beta", "description": "the second"})).await;

    // (case, the name filtered by, the profileIds it must report)
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("a name another profile's name starts with", "alpha", vec![alpha.clone()]),
        ("a name that starts with another profile's", "alphaExtra", vec![alpha_extra.clone()]),
        ("a name sharing no prefix", "beta", vec![beta.clone()]),
        ("a name no profile holds", "gamma", Vec::new()),
        ("a name differing only in case", "Alpha", Vec::new()),
    ];

    for (case, name, expected) in rows {
        let (status, listed) = list_profiles(&router, Some(name), None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListBrowserProfiles, got {status} ({listed})"
        );
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case:?}: expected exactly {expected:?} for name={name:?}, got {:?} ({listed})",
            listed_ids(&listed)
        );
        assert!(
            listed["nextToken"].is_null(),
            "case {case:?}: a filtered page holding every match carries no nextToken, got {}",
            listed["nextToken"]
        );
    }

    // A filter narrows the page's CONTENT, not the page's size: one profile per page over
    // the two `alpha*` names still reports them one filter at a time.
    let (status, listed) = list_profiles(&router, Some("alphaExtra"), Some(1), None).await;
    assert_eq!(
        (status, listed_ids(&listed)),
        (200, vec![alpha_extra.clone()]),
        "case \"a filtered single-entry page\": expected only {alpha_extra} ({listed})"
    );
    assert!(
        listed["nextToken"].is_null(),
        "case \"a filtered single-entry page\": nothing is left over past the only match, got {}",
        listed["nextToken"]
    );
}

/// A `nextToken` this service did not issue is the modelled 400; one it could have issued
/// is served, even when nothing sorts after it.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = router();
    created_id(&router, &json!({"name": "tokenProfile"})).await;

    for (case, token) in [
        ("a word outside the token's alphabet", "notatoken"),
        ("a pair of characters outside it", "zz"),
        ("a token truncated to an odd length", "616"),
    ] {
        let (status, answered) = list_profiles(&router, None, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for nextToken={token:?}, got {status} ({answered})"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {} ({answered})",
            error_code(&answered)
        );
        assert_eq!(
            answered["fieldList"][0]["name"],
            json!("nextToken"),
            "case {case:?}: expected the refusal to name nextToken, got {}",
            answered["fieldList"]
        );
    }

    // A token the service DID issue is the only thing a page resumes from; walking off the
    // end of the store with one is an empty page, not a refusal.
    let (status, first) = list_profiles(&router, None, Some(1), None).await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected HTTP 200, got {status} ({first})"
    );
    assert!(
        first["nextToken"].is_null(),
        "case \"the first page\": the only profile held leaves nothing over, got {}",
        first["nextToken"]
    );
}

/// A profile whose deletion has been accepted is still listed, in the status the deletion
/// put it in, until the removal the clock owes comes due; from then on the listing reports
/// it no more — the same rule `GetBrowserProfile` reads it by.
#[tokio::test]
async fn a_deleted_profile_leaves_the_listing_when_the_clock_removes_it() {
    let router = router();

    let doomed = created_id(&router, &json!({"name": "doomedProfile"})).await;
    let kept = created_id(&router, &json!({"name": "keptProfile"})).await;

    let (status, deleted) = delete_profile(&router, &doomed).await;
    assert_eq!(
        (status, text(&deleted, "status").as_str()),
        (200, "DELETING"),
        "case \"the accepted deletion\": expected HTTP 200 and DELETING, got {status} ({deleted})"
    );

    let (status, listed) = list_profiles(&router, None, None, None).await;
    assert_eq!(
        status, 200,
        "case \"during the deletion\": expected HTTP 200, got {status} ({listed})"
    );
    let under_way = summaries(&listed)
        .into_iter()
        .find(|summary| text(summary, "profileId") == doomed)
        .unwrap_or_else(|| panic!("case \"during the deletion\": {doomed} is missing from {listed}"));
    assert_eq!(
        text(&under_way, "status"),
        "DELETING",
        "case \"during the deletion\": the listing reports the status the deletion put the \
         profile in, got {}",
        text(&under_way, "status")
    );

    tokio::time::sleep(PAST_REMOVAL).await;

    let (status, listed) = list_profiles(&router, None, None, None).await;
    assert_eq!(
        (status, listed_ids(&listed)),
        (200, vec![kept.clone()]),
        "case \"past the removal\": expected only {kept} to remain, got {listed}"
    );
    let (status, fetched) = get_profile(&router, &doomed).await;
    assert_eq!(
        status, 404,
        "case \"past the removal\": the removed profile is the modelled 404, got {status} \
         ({fetched})"
    );
}
