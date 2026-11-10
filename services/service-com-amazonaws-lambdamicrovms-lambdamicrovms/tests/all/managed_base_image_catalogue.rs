// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListManagedMicrovmImages`: the AWS-MANAGED base-image
//! catalogue this service publishes.
//!
//! RULE: the listing reports that catalogue — one entry per base image the service
//! publishes, in a stable order, each carrying the AWS-owned ARN of that image in the
//! region the request was signed for and the instant the entry was published — paged by
//! the operation's `@paginated` contract, and every ARN it reports is one
//! `CreateMicrovmImage` resolves as a `baseImageArn` ("Use ListManagedMicrovmImages to
//! discover available base images").
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    base_image_not_found_message, managed_base_image_arn, managed_base_image_publication,
    MANAGED_BASE_IMAGE_IDS, MAX_LIST_PAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    as_uri_value, call, call_in_region, emulator, image_body, ACCOUNT, REGION,
};

/// `GET /2025-09-09/managed-microvm-images`, with whatever `query` adds to the query
/// string, signed for `region`.
async fn list_managed_in_region(
    router: &MultiServiceRouter,
    region: &str,
    query: &str,
) -> (u16, Value) {
    let uri = match query.is_empty() {
        true => "/2025-09-09/managed-microvm-images".to_string(),
        false => format!("/2025-09-09/managed-microvm-images?{query}"),
    };
    call_in_region(router, region, "GET", &uri, None).await
}

/// `GET /2025-09-09/managed-microvm-images`, signed for the suite's own region.
async fn list_managed(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    list_managed_in_region(router, REGION, query).await
}

/// The catalogue the listing must report in `region`, in the order it pages it: one
/// entry per base image the service publishes (`state::MANAGED_BASE_IMAGE_IDS`, the
/// catalogue `CreateMicrovmImage$baseImageArn` is resolved against), each as that
/// image's AWS-owned ARN in `region` (`state::managed_base_image_arn`).
fn expected_catalogue(region: &str) -> Vec<String> {
    let mut ids: Vec<&str> = MANAGED_BASE_IMAGE_IDS.to_vec();
    ids.sort_unstable();
    ids.into_iter()
        .map(|image_id| managed_base_image_arn(region, image_id))
        .collect()
}

/// The `items` of one page.
fn page_items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// The `imageArn`s of one page, in the order the page carries them.
fn page_arns(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| item["imageArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A hex-encoded `nextToken` candidate, as the opaque token form this listing issues.
fn hex_token(value: &str) -> String {
    ::hex::encode(value.as_bytes())
}

/// The instants the catalogue published the entry `item` reports with
/// (`state::managed_base_image_publication`, keyed by the base image id its `imageArn`
/// names), as the epoch-seconds numbers a listing reports them as.
fn published_instants(item: &Value, case: &str) -> (Option<f64>, Option<f64>) {
    let image_arn = item["imageArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: every entry carries an imageArn, got {item}"));
    let image_id = image_arn
        .split(':')
        .next_back()
        .unwrap_or_else(|| panic!("case {case}: the ARN names a base image, got {image_arn}"));
    let (created_at, updated_at) = managed_base_image_publication(image_id)
        .unwrap_or_else(|| panic!("case {case}: the catalogue publishes {image_id}"));
    (
        Some(created_at.secs() as f64),
        Some(updated_at.secs() as f64),
    )
}

/// RULE: however the pages are cut, the listing walks the whole published catalogue —
/// every entry exactly once, in one stable order, at most `maxResults` per page — and
/// hands back a `nextToken` only while entries remain.
#[tokio::test]
async fn the_catalogue_is_served_once_per_published_entry_over_every_page_size() {
    let router = emulator();
    let expected = expected_catalogue(REGION);
    assert!(
        !expected.is_empty(),
        "the service publishes a base-image catalogue, got {expected:?}"
    );

    // Rows differ in the page size, which is what the page walk generalises over: no
    // `maxResults` at all, each published bound of `@range(min: 1, max: 50)`, and every
    // size up to the catalogue's own length.
    let mut sizes: Vec<Option<usize>> = vec![None, Some(1), Some(MAX_LIST_PAGE)];
    sizes.extend((2..=expected.len()).map(Some));

    for size in sizes {
        let case = format!("maxResults={size:?}");
        let cap = size.unwrap_or(MAX_LIST_PAGE);
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;

        // One page per entry, plus one: a listing that never terminates fails here
        // rather than hanging.
        for _ in 0..=expected.len() {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let (status, body) = list_managed(&router, &query.join("&")).await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

            let page = page_arns(&body, &case);
            assert!(
                page.len() <= cap,
                "case {case}: a page carries at most {cap} entries, got {} ({page:?})",
                page.len()
            );
            for item in page_items(&body, &case) {
                // The entry reports the catalogue's own published metadata, whichever
                // page it is served on; the distinction between the two instants is the
                // subject of `managed_catalogue_publication_metadata`.
                assert_eq!(
                    (item["createdAt"].as_f64(), item["updatedAt"].as_f64()),
                    published_instants(&item, &case),
                    "case {case}: every entry carries the instants the catalogue published \
                     it with, got {item}"
                );
            }
            seen.extend(page);
            assert!(
                seen.len() <= expected.len(),
                "case {case}: the walk served more than the catalogue holds, got {seen:?}"
            );

            token = body["nextToken"].as_str().map(str::to_string);
            match &token {
                Some(issued) => assert!(
                    seen.len() < expected.len(),
                    "case {case}: a nextToken ({issued}) is due only while entries remain, \
                     got it after {seen:?}"
                ),
                None => break,
            }
        }

        assert!(
            token.is_none(),
            "case {case}: the walk must end with no nextToken, got {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case}: expected the published catalogue exactly once in order, got {seen:?}"
        );
    }
}

/// RULE: `maxResults` is bounded by the range the model publishes, so each bound is
/// served and a value one past either bound is refused with HTTP 400
/// `ValidationException` — `maxResults` is `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListManagedMicrovmImages")`).
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();

    let rows: Vec<(&str, usize, u16)> = vec![
        ("one below the published minimum", 0, 400),
        ("the published minimum", 1, 200),
        ("the published maximum", MAX_LIST_PAGE, 200),
        ("one past the published maximum", MAX_LIST_PAGE + 1, 400),
    ];

    for (case, max_results, expected_status) in rows {
        let (status, body) = list_managed(&router, &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: maxResults={max_results} expected {expected_status}, got {status} \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                body["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: the catalogue is what a create builds on — every ARN the listing reports
/// resolves as a `baseImageArn`, and the image it mints reads back; a managed ARN of the
/// same form the listing does NOT report resolves to nothing and is refused.
#[tokio::test]
async fn every_arn_the_catalogue_reports_is_one_a_create_can_build_on() {
    let router = emulator();

    let (status, body) = list_managed(&router, "").await;
    assert_eq!(status, 200, "list the catalogue: got {status} {body}");
    let listed = page_arns(&body, "the whole catalogue");
    assert!(
        !listed.is_empty(),
        "the catalogue the create resolves against is not empty, got {body}"
    );

    for (index, base_image_arn) in listed.iter().enumerate() {
        let name = format!("built-on-{index}");
        let (status, created) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(
                &name,
                json!({"baseImageArn": base_image_arn.clone()}),
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {base_image_arn:?}: a listed base image is one a create resolves, got \
             {status} {created}"
        );
        assert_eq!(
            created["baseImageArn"].as_str(),
            Some(base_image_arn.as_str()),
            "case {base_image_arn:?}: the stored record carries the base image asked for, got \
             {:?}",
            created["baseImageArn"]
        );

        // The image the create minted reads back through `GetMicrovmImage`, so the
        // discovery -> create -> read path closes.
        let (status, image) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvm-images/{}", as_uri_value(&name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {base_image_arn:?}: the created image reads back, got {status} {image}"
        );
        assert_eq!(
            image["name"].as_str(),
            Some(name.as_str()),
            "case {base_image_arn:?}: the read names the image created, got {:?}",
            image["name"]
        );
    }

    // One past the catalogue: a managed ARN in the very same form, naming an id the
    // listing does not report, resolves to no base image.
    let unlisted = managed_base_image_arn(REGION, "not-a-published-base-image");
    assert!(
        !listed.contains(&unlisted),
        "the unlisted row must name an id the catalogue does not publish, got {listed:?}"
    );
    let (status, refused) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(
            "built-on-nothing",
            json!({"baseImageArn": unlisted.clone()}),
        )),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {unlisted:?}: a managed ARN the catalogue does not publish is refused, got \
         {status} {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some(base_image_not_found_message(&unlisted).as_str()),
        "case {unlisted:?}: the refusal is built from the ARN the request carried, got {:?}",
        refused["message"]
    );
}

/// RULE: the catalogue is AWS's own — its entries are AWS-owned ARNs, nothing an account
/// creates joins it, and reading it changes nothing a later caller sees (so the instants
/// it reports are stable across reads).
#[tokio::test]
async fn the_listing_reports_only_aws_owned_entries_and_changes_nothing() {
    let router = emulator();

    let (status, first) = list_managed(&router, "").await;
    assert_eq!(status, 200, "list the catalogue: got {status} {first}");
    for arn in page_arns(&first, "the whole catalogue") {
        assert!(
            !arn.contains(ACCOUNT),
            "case {arn:?}: a managed entry is AWS-owned, so its ARN carries no account id"
        );
    }

    // Rows differ in what the account holds when the catalogue is read again: nothing
    // an account does publishes a managed base image.
    let rows: Vec<(&str, Value)> = vec![
        ("an image with the required members only", json!({})),
        (
            "a tagged, fully specified image",
            json!({"description": "mine", "tags": {"team": "platform"}}),
        ),
    ];

    for (index, (case, extra)) in rows.into_iter().enumerate() {
        let name = format!("mine-{index}");
        let (status, created) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&name, extra)),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: create got {status} {created}");

        let (status, again) = list_managed(&router, "").await;
        assert_eq!(status, 200, "case {case:?}: re-list got {status} {again}");
        assert_eq!(
            again, first,
            "case {case:?}: the catalogue is AWS's own and a read does not change it, got {again}"
        );

        // The account's own image is still there: the listing read nothing of it away.
        let (status, image) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvm-images/{}", as_uri_value(&name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the account's image survives the listing, got {status} {image}"
        );
    }
}

/// RULE: a managed base image is an AWS-owned REGIONAL resource, so the catalogue
/// publishes the same ids in every region and each ARN carries the region the request
/// was signed for.
#[tokio::test]
async fn the_catalogue_carries_the_region_the_request_was_signed_for() {
    let router = emulator();

    // Rows span regions in several partitions' naming forms; only the ARN's region
    // field may differ between them.
    for region in [REGION, "us-east-1", "eu-central-1", "ap-southeast-2"] {
        let (status, body) = list_managed_in_region(&router, region, "").await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            page_arns(&body, region),
            expected_catalogue(region),
            "case {region:?}: every entry carries the region the request named, got {body}"
        );
    }
}

/// RULE: a `nextToken` this listing did not issue positions nothing, so it is refused
/// with HTTP 400 `ValidationException` rather than silently restarting the listing.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let router = emulator();

    // Rows differ in HOW the token fails to be one this listing issued: not the opaque
    // encoding at all, the encoding carrying no cursor of this listing, and another
    // listing's token.
    let rows: Vec<(&str, String)> = vec![
        ("not the opaque encoding", "not-a-token".to_string()),
        ("an empty encoded value", hex_token("")),
        ("an encoded cursor with no marker", hex_token(MANAGED_BASE_IMAGE_IDS[0])),
        (
            "another listing's token",
            hex_token("ListMicrovms/1:mvm-0000000000000000"),
        ),
        (
            "a token of a marker this listing never issued",
            hex_token("ListManagedMicrovmImages/0:al2023-1"),
        ),
    ];

    for (case, token) in rows {
        let (status, body) = list_managed(&router, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: nextToken={token} must be refused, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("nextToken is not a pagination token this service issued"),
            "case {case:?}: expected the refusal to name the token member, got {:?}",
            body["message"]
        );
    }
}
