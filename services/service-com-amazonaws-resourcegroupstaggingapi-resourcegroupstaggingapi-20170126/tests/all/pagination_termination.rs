// SPDX-License-Identifier: CC0-1.0

//! What ENDS a paginated walk of this API.
//!
//! The rule under test: a page filled to its item cap is never the last page —
//! it carries a resume token even when it consumed the last matching item, and
//! the walk ends on the empty page that token serves. Only a page holding fewer
//! items than its cap ends the walk at once with an empty `PaginationToken`.
//!
//! The tables span what the rule generalises over: the page size, how the result
//! count divides by it (evenly, with a remainder, not at all), and the
//! projection the page is cut from (`GetResources`' `ResourceTagMappingList` and
//! `GetTagKeys`' server-sized `TagKeys`). The one-resource-one-item case the
//! issue cited is a single row.

use std::sync::Arc;

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over its own fresh engine, so each test
/// starts from an empty store and observes the same state across requests.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization`
    // header below is present because it is what routes an awsJson request to
    // the `tagging` signing name, not to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/tagging/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// Tag `arns` with `tags`, asserting the call succeeded with no failed resource.
async fn tag(router: &MultiServiceRouter, arns: &[&str], tags: &[(&str, &str)]) {
    let tags: serde_json::Map<String, serde_json::Value> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), serde_json::json!(value)))
        .collect();
    let (status, body) = call(
        router,
        "TagResources",
        serde_json::json!({ "ResourceARNList": arns, "Tags": tags }),
    )
    .await;
    assert_eq!(status, 200, "TagResources({arns:?}): body {body}");
    assert_eq!(
        body["FailedResourcesMap"],
        serde_json::json!({}),
        "TagResources({arns:?}): expected an empty FailedResourcesMap, got {}",
        body["FailedResourcesMap"]
    );
}

/// The ARNs of a `GetResources` page, in the order the service returned them.
fn page_arns(body: &serde_json::Value) -> Vec<String> {
    body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .map(|mapping| {
            mapping["ResourceARN"]
                .as_str()
                .unwrap_or_else(|| panic!("a mapping with no ResourceARN: {mapping}"))
                .to_string()
        })
        .collect()
}

/// The token a response carries, as the client sees it: absent and empty are the
/// same "the walk is over" signal.
fn token_of(body: &serde_json::Value) -> String {
    body["PaginationToken"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One row of the walk table: how many tagged resources exist, and the
/// `ResourcesPerPage` the client asks for.
///
/// `ResourcesPerPage` is bounded to 1..=100 (`GetResourcesInput.ResourcesPerPage`
/// documentation: "You can specify a minimum of 1 and a maximum value of 100"),
/// so every size below is inside that window; the rows cover a size that divides
/// the result set exactly (including the issue's 1-over-1), one that leaves a
/// remainder, one larger than the whole set, and both published bounds.
const WALKS: &[(usize, usize)] = &[
    // The cited case: one matching resource, a one-item page.
    (1, 1),
    // Evenly dividing, several pages.
    (4, 2),
    (6, 3),
    // The whole set fills exactly one page.
    (3, 3),
    // A remainder: the walk ends on an under-full page.
    (7, 2),
    (5, 3),
    // A page larger than the result set: one under-full page, nothing more.
    (1, 100),
    (9, 100),
    // The minimum page size over several resources.
    (3, 1),
];

#[tokio::test]
async fn a_full_get_resources_page_is_never_the_last_one() {
    for &(resources, per_page) in WALKS {
        let case = format!("{resources} resources, ResourcesPerPage={per_page}");
        let router = emulator();
        let arns: Vec<String> = (0..resources)
            .map(|n| format!("arn:aws:s3:::bucket-{n:03}"))
            .collect();
        let borrowed: Vec<&str> = arns.iter().map(String::as_str).collect();
        tag(&router, &borrowed, &[("Env", "prod")]).await;

        let mut seen: Vec<String> = Vec::new();
        let mut pages: Vec<(usize, String)> = Vec::new();
        let mut token = String::new();
        loop {
            let mut request = serde_json::json!({ "ResourcesPerPage": per_page });
            if !token.is_empty() {
                request["PaginationToken"] = serde_json::json!(token);
            }
            let (status, body) = call(&router, "GetResources", request).await;
            assert_eq!(status, 200, "case {case}: body {body}");
            let page = page_arns(&body);
            assert!(
                page.len() <= per_page,
                "case {case}: a page of {} exceeds the cap",
                page.len()
            );
            token = token_of(&body);
            if page.len() == per_page {
                assert!(
                    !token.is_empty(),
                    "case {case}: a page filled to the cap must carry a resume token, got {body}"
                );
            } else {
                assert!(
                    token.is_empty(),
                    "case {case}: an under-full page of {} must end the walk, got token {token:?}",
                    page.len()
                );
            }
            pages.push((page.len(), token.clone()));
            seen.extend(page);
            assert!(
                pages.len() <= resources + 2,
                "case {case}: pagination did not terminate after {} pages",
                pages.len()
            );
            if token.is_empty() {
                break;
            }
        }

        let mut expected = arns.clone();
        expected.sort();
        assert_eq!(
            seen, expected,
            "case {case}: expected every ARN exactly once in ARN order, got {seen:?}"
        );

        // Full pages, then exactly one short page — which is empty precisely
        // when the page size divides the result set.
        let expected_pages = resources / per_page + 1;
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case}: expected {expected_pages} pages, got {}: {pages:?}",
            pages.len()
        );
        let last = pages.last().expect("at least one page");
        assert_eq!(
            last.0,
            resources % per_page,
            "case {case}: the terminal page must hold the remainder, got {last:?}"
        );
        assert!(
            last.1.is_empty(),
            "case {case}: the terminal page must carry an empty token, got {last:?}"
        );
    }
}

#[tokio::test]
async fn following_a_full_pages_token_serves_an_empty_terminal_page() {
    let router = emulator();
    // One tagged resource selected by a key nothing else carries, so the page is
    // exactly full at ResourcesPerPage=1 with nothing left behind it. The ARN is
    // under a service this crate does not resolve live resources against, so no
    // second service has to be standing for the write to land.
    let arn = "arn:aws:s3:::sized-bucket";
    tag(&router, &[arn], &[("PageSizeProbe", "sized")]).await;
    let filter = serde_json::json!([{ "Key": "PageSizeProbe" }]);

    let (status, full) = call(
        &router,
        "GetResources",
        serde_json::json!({ "TagFilters": filter, "ResourcesPerPage": 1 }),
    )
    .await;
    assert_eq!(status, 200, "case the full page: body {full}");
    assert_eq!(
        page_arns(&full),
        vec![arn.to_string()],
        "case the full page: expected the one tagged ARN, got {full}"
    );
    let token = token_of(&full);
    assert!(
        !token.is_empty(),
        "case the full page: a 1-of-1 page fills its cap and must carry a token, got {full}"
    );

    let (status, terminal) = call(
        &router,
        "GetResources",
        serde_json::json!({
            "TagFilters": filter,
            "ResourcesPerPage": 1,
            "PaginationToken": token,
        }),
    )
    .await;
    assert_eq!(status, 200, "case the terminal page: body {terminal}");
    assert!(
        page_arns(&terminal).is_empty(),
        "case the terminal page: expected no mappings, got {terminal}"
    );
    assert_eq!(
        token_of(&terminal),
        "",
        "case the terminal page: expected an empty token, got {terminal}"
    );

    // The control: the same single resource under a page size it cannot fill
    // ends the walk at once.
    let (status, under) = call(
        &router,
        "GetResources",
        serde_json::json!({ "TagFilters": filter, "ResourcesPerPage": 100 }),
    )
    .await;
    assert_eq!(status, 200, "case the under-full page: body {under}");
    assert_eq!(
        page_arns(&under),
        vec![arn.to_string()],
        "case the under-full page: expected the one tagged ARN, got {under}"
    );
    assert_eq!(
        token_of(&under),
        "",
        "case the under-full page: expected an empty token, got {under}"
    );
}

#[tokio::test]
async fn a_full_get_tag_keys_page_is_never_the_last_one() {
    let router = emulator();
    // `GetTagKeys` exposes no page-size member: the server serves 100 keys a
    // page (`crate::state::KEYS_PER_PAGE`). Exactly 100 distinct keys therefore
    // fill one page and consume the whole projection — spread over two
    // resources, since one resource holds at most 50 tags.
    for chunk in 0..2 {
        let arn = format!("arn:aws:s3:::keys-bucket-{chunk}");
        let keys: Vec<String> = (0..50)
            .map(|n| format!("Key{:03}", chunk * 50 + n))
            .collect();
        let tags: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "shared")).collect();
        tag(&router, &[arn.as_str()], &tags).await;
    }

    let (status, full) = call(&router, "GetTagKeys", serde_json::json!({})).await;
    assert_eq!(status, 200, "case the full key page: body {full}");
    let keys: Vec<String> = full["TagKeys"]
        .as_array()
        .unwrap_or_else(|| panic!("no TagKeys: {full}"))
        .iter()
        .map(|key| key.as_str().expect("a TagKeys entry").to_string())
        .collect();
    let expected: Vec<String> = (0..100).map(|n| format!("Key{n:03}")).collect();
    assert_eq!(
        keys, expected,
        "case the full key page: expected all 100 keys in order, got {keys:?}"
    );
    let token = token_of(&full);
    assert!(
        !token.is_empty(),
        "case the full key page: a page filled to the server's cap must carry a token, got {full}"
    );

    let (status, terminal) = call(
        &router,
        "GetTagKeys",
        serde_json::json!({ "PaginationToken": token }),
    )
    .await;
    assert_eq!(status, 200, "case the terminal key page: body {terminal}");
    assert_eq!(
        terminal["TagKeys"],
        serde_json::json!([]),
        "case the terminal key page: expected no keys, got {terminal}"
    );
    assert_eq!(
        token_of(&terminal),
        "",
        "case the terminal key page: expected an empty token, got {terminal}"
    );
}
