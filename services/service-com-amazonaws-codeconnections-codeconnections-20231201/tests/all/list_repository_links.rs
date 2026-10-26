// SPDX-License-Identifier: CC0-1.0

//! `ListRepositoryLinks` over the wire: the paginated report of the repository links the
//! account holds.
//!
//! The rule these suites assert: a paginated report of this service reports exactly the
//! stored rows its input selects — here every repository link of the account, since
//! `ListRepositoryLinks` models no filter — in the store's own total `RepositoryLinkId`
//! order, each entry the SAME row the single-resource reader (`GetRepositoryLink`)
//! reports, at most `MaxResults` of them per response (clamped to the member's published
//! maximum, and that maximum when the caller asks for nothing), with the remainder
//! reachable through `NextToken` and the token absent exactly when nothing is left; and a
//! `NextToken` this service never issued names no position, so it is refused rather than
//! answered from the top of the list. A report writes nothing: repeating it answers the
//! same thing.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing
//! `connection_lifecycle`'s and `git_sync`'s helpers, so these tests exercise the same
//! path a signed SDK request takes. The recorded unit the issue cited — a list over an
//! account before a repository link is deleted — is ONE row of the tables below.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{REGION, call, create, router};
use super::git_sync::{LinkCase, create_link, error_code};

/// The published bounds of the modeled `MaxResults` member (`range 0..100`, from
/// `smithy_query(query="operation", operation="ListRepositoryLinks")`): zero is its
/// modeled default, and 100 the largest page the service renders.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// An `EncryptionKeyArn` matching the modeled `KmsKeyArn` pattern
/// `^arn:aws(-[\w]+)*:kms:[a-z\-0-9]+:\d{12}:key/[a-zA-Z0-9\-]+$`, from
/// `smithy_query(query="shape", shape="KmsKeyArn")`.
fn kms_key_arn(key: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/{key}",
        engine::emulator_account_id()
    )
}

/// The table: links differing in the provider type of the connection they are made on,
/// in owner-id and repository-name form and length (each published bound of `OwnerId`
/// 1..255 and `RepositoryName` 1..128, from `smithy_query(query="operation",
/// operation="CreateRepositoryLink")`), in whether an encryption key is supplied and in
/// how many tags the link carries — so a report that renders only the simplest row, or
/// that echoes creation order rather than the store's id order, fails.
fn listed_link_cases() -> Vec<LinkCase> {
    vec![
        LinkCase {
            label: "plain link, no key, no tags",
            provider_type: "GitHub",
            owner_id: "listing-acme".to_string(),
            repository_name: "service".to_string(),
            encryption_key_arn: None,
            tags: Vec::new(),
        },
        LinkCase {
            label: "encryption key and one tag",
            provider_type: "Bitbucket",
            owner_id: "listing-team".to_string(),
            repository_name: "infra/platform".to_string(),
            encryption_key_arn: Some(kms_key_arn("1234abcd-12ab-34cd-56ef-1234567890ab")),
            tags: vec![("team", "platform")],
        },
        LinkCase {
            label: "owner and repository at their published minimum length",
            provider_type: "GitLab",
            owner_id: "o".to_string(),
            repository_name: "r".to_string(),
            encryption_key_arn: None,
            tags: vec![("only", "one")],
        },
        LinkCase {
            label: "owner and repository at their published maximum length",
            provider_type: "GitLabSelfManaged",
            owner_id: "o".repeat(255),
            repository_name: "r".repeat(128),
            encryption_key_arn: Some(kms_key_arn("abcdef01-2345-6789-abcd-ef0123456789")),
            tags: vec![("a", "1"), ("b", "2")],
        },
        LinkCase {
            label: "a provider type the issue did not cite",
            provider_type: "AzureDevOps",
            owner_id: "contoso".to_string(),
            repository_name: "Repo.With.Dots".to_string(),
            encryption_key_arn: None,
            tags: vec![("env", "")],
        },
        LinkCase {
            label: "host-backed provider",
            provider_type: "GitHubEnterpriseServer",
            owner_id: "listing-enterprise".to_string(),
            repository_name: "deep/nested/name".to_string(),
            encryption_key_arn: None,
            tags: Vec::new(),
        },
    ]
}

/// One `LinkCase` differing from every other only in the owner and repository it names,
/// for the paging suite — where WHAT each link holds does not matter, only how many.
fn plain_link(index: usize) -> LinkCase {
    LinkCase {
        label: "paged link",
        provider_type: "GitHub",
        owner_id: format!("paged-owner-{index}"),
        repository_name: format!("paged-repo-{index}"),
        encryption_key_arn: None,
        tags: Vec::new(),
    }
}

/// Call `ListRepositoryLinks` with the page size and token a case names.
async fn list(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = json!({});
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    call(router, "ListRepositoryLinks", body).await
}

/// The `RepositoryLinks` entries a response reports, in the order it reported them. The
/// modeled member is required, so it must be an array even when empty.
fn reported_links(case: &str, body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["RepositoryLinks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: RepositoryLinks must be a list, got {body}"))
        .clone()
}

/// The `RepositoryLinkId`s a response reports, in the order it reported them.
fn reported_ids(case: &str, body: &serde_json::Value) -> Vec<String> {
    reported_links(case, body)
        .iter()
        .map(|link| {
            link["RepositoryLinkId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every entry carries a RepositoryLinkId, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// Create every row of the table and return the ids, in the order the rows were created.
async fn seed(router: &MultiServiceRouter, cases: &[LinkCase]) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (id, _) = create_link(router, case, &format!("listing-conn-{index}")).await;
        ids.push(id);
    }
    ids
}

/// The report holds exactly the links the store holds, in `RepositoryLinkId` order, each
/// entry the row `GetRepositoryLink` reports; it reports nothing for an account that
/// holds none, it changes nothing a later caller sees, and a deleted link leaves it.
#[tokio::test]
async fn listing_reports_every_stored_link_as_get_repository_link_reports_it() {
    let router = router();

    // An account that holds no repository link at all: the report is an empty list,
    // never a refusal and never a token.
    let (status, empty) = list(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"an account with no link\": expected 200 from ListRepositoryLinks, got {status} {empty}"
    );
    assert!(
        reported_links("an account with no link", &empty).is_empty(),
        "case \"an account with no link\": expected no repository link, got {empty}"
    );
    assert!(
        empty["NextToken"].is_null(),
        "case \"an account with no link\": nothing is left, so no NextToken may come back, got {empty}"
    );

    // A connection that no link is made on, so a report that over-reads the store
    // (reporting every resource rather than every repository link) fails here.
    create(&router, "not-a-link", "GitHub").await;

    let cases = listed_link_cases();
    let mut ids: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (id, _) = create_link(&router, case, &format!("listing-conn-{index}")).await;

        // Every link created so far is reported, in `RepositoryLinkId` order, as soon as
        // it is persisted: the lifecycle the report reflects.
        ids.push(id);
        let mut expected = ids.clone();
        expected.sort();
        let (status, body) = list(&router, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListRepositoryLinks, got {status} {body}",
            case.label
        );
        assert_eq!(
            reported_ids(case.label, &body),
            expected,
            "case {:?}: expected every stored link in RepositoryLinkId order, got {body}",
            case.label
        );
    }

    let (_, body) = list(&router, None, None).await;
    let mut expected = ids.clone();
    expected.sort();

    // Every reported entry is the stored row, not a recomputed one: it is exactly what
    // the single-resource reader reports for the same id.
    for (position, id) in expected.iter().enumerate() {
        let (status, got) = call(
            &router,
            "GetRepositoryLink",
            json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected 200 from GetRepositoryLink, got {status} {got}"
        );
        assert_eq!(
            body["RepositoryLinks"][position], got["RepositoryLinkInfo"],
            "case {id:?}: entry {position} must be the stored row, got {body}"
        );
    }

    // A report changes nothing: the same call answers the same thing.
    let (status, again) = list(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"a repeated report\": expected 200 from ListRepositoryLinks, got {status} {again}"
    );
    assert_eq!(
        again["RepositoryLinks"], body["RepositoryLinks"],
        "case \"a repeated report\": listing must not change what a later caller sees, got {again}"
    );

    // A link this service no longer holds leaves the report, and only it does.
    let removed = expected.remove(0);
    let (status, deleted) = call(
        &router,
        "DeleteRepositoryLink",
        json!({ "RepositoryLinkId": removed }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a deleted link\": expected 200 from DeleteRepositoryLink, got {status} {deleted}"
    );
    let (status, body) = list(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"a deleted link\": expected 200 from ListRepositoryLinks, got {status} {body}"
    );
    assert_eq!(
        reported_ids("a deleted link", &body),
        expected,
        "case \"a deleted link\": {removed:?} must leave the report and nothing else, got {body}"
    );
}

/// Page sizes: each published bound of the modeled `MaxResults` (its `0` default and its
/// maximum of 100), the member omitted entirely, and sizes below, equal to and above the
/// number of links the account holds.
const PAGE_SIZES: &[Option<i64>] = &[
    None,
    Some(MAX_RESULTS_MIN),
    Some(1),
    Some(2),
    Some(3),
    Some(7),
    Some(8),
    Some(MAX_RESULTS_MAX),
];

/// Paging covers every link exactly once, in `RepositoryLinkId` order, for every page
/// size: the invariant `@paginated(inputToken=NextToken, outputToken=NextToken,
/// pageSize=MaxResults)` states. No page exceeds the size the caller asked for, nor the
/// member's published maximum when it asked for nothing, and the token comes back exactly
/// while items are left.
#[tokio::test]
async fn paging_covers_every_repository_link_exactly_once() {
    let router = router();
    let cases: Vec<LinkCase> = (0..7).map(plain_link).collect();
    let mut expected = seed(&router, &cases).await;
    expected.sort();

    for page_size in PAGE_SIZES {
        let bound = match page_size {
            Some(asked) if *asked > 0 => (*asked).min(MAX_RESULTS_MAX),
            _ => MAX_RESULTS_MAX,
        };
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let (status, body) = list(&router, *page_size, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {page_size:?}: expected 200 from ListRepositoryLinks, got {status} {body}"
            );
            let page = reported_ids("paged", &body);
            assert!(
                page.len() as i64 <= bound,
                "case {page_size:?}: a page holds at most {bound} entries, got {} ({body})",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {page_size:?}: paging must terminate, saw {pages} pages and {seen:?}"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {page_size:?}: paging must report every link exactly once in RepositoryLinkId order, got {seen:?}"
        );
    }
}

/// `NextToken` values this service never issued: text that is not hex at all, hex of
/// text no `RepositoryLinkId` could be (the modeled pattern
/// `^[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}$`
/// admits only hex in the 8-4-4-4-12 grouping), and the position key another report of
/// this service is paged by.
fn foreign_tokens(connection_arn: &str) -> Vec<(&'static str, String)> {
    vec![
        ("not hex at all", "not-a-page-token".to_string()),
        ("hex with an odd length", "abc".to_string()),
        (
            "hex of a connection ARN rather than a link id",
            hex::encode(connection_arn),
        ),
        (
            "hex of a resource name rather than a link id",
            hex::encode("DemoStack"),
        ),
        (
            "hex of an id whose groups are mis-sized",
            hex::encode("1234567-1234-1234-1234-123456789012"),
        ),
        (
            "hex of an id carrying a character outside hex",
            hex::encode("1234567g-1234-1234-1234-123456789012"),
        ),
        (
            "hex of an id with one group too many",
            hex::encode("12345678-1234-1234-1234-123456789012-1234"),
        ),
        ("hex of the empty string", String::new()),
    ]
}

/// A `NextToken` this service never issued is refused with the operation's modeled
/// `InvalidInputException` (400) rather than answered from the top of the list, and a
/// `MaxResults` past the member's published maximum is refused by the modeled range.
/// Neither is answered with a page, a success or the unmodeled 501, and neither disturbs
/// the stored links.
#[tokio::test]
async fn listing_refuses_a_foreign_token_or_an_out_of_range_page() {
    let router = router();
    // A link that IS there, so a refusal can never mean "the store is empty".
    let live_case = plain_link(100);
    let (live, connection) = create_link(&router, &live_case, "live-link-conn").await;

    for (label, token) in foreign_tokens(&connection) {
        let (status, body) = list(&router, None, Some(&token)).await;
        // The empty token is refused by the modeled `NextToken` length (`1..2048`)
        // before this handler runs; every other row reaches the handler. Both are
        // client errors, and neither reports a link.
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for a token this service did not issue, got {status} {body}"
        );
        assert!(
            body["RepositoryLinks"].is_null(),
            "case {label:?}: a refusal reports no repository link, got {body}"
        );
        if !token.is_empty() {
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {label:?}: expected InvalidInputException, got {body}"
            );
        }
    }

    // One past the published `MaxResults` maximum: the modeled range refuses it before
    // the handler runs, so the answer is a 400 and never a page.
    let (status, body) = list(&router, Some(MAX_RESULTS_MAX + 1), None).await;
    assert_eq!(
        status, 400,
        "case \"MaxResults past the published maximum\": expected 400, got {status} {body}"
    );
    assert!(
        body["RepositoryLinks"].is_null(),
        "case \"MaxResults past the published maximum\": a refusal reports no repository link, got {body}"
    );

    // And the live link is still the one link reported: a refusal wrote nothing.
    let (status, body) = list(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"live link\": expected 200 from ListRepositoryLinks, got {status} {body}"
    );
    assert_eq!(
        reported_ids("live link", &body),
        vec![live],
        "case \"live link\": the live link must still be reported, got {body}"
    );
}

/// A token one page issued resumes exactly after the link that page ended on, for a store
/// whose links differ in every member: the position is a key in the store's own order, so
/// the pages a token walks partition the links.
#[tokio::test]
async fn a_token_resumes_after_the_link_its_page_ended_on() {
    let router = router();
    let cases = listed_link_cases();
    let mut expected = seed(&router, &cases).await;
    expected.sort();

    let (status, first) = list(&router, Some(2), None).await;
    assert_eq!(
        status, 200,
        "case \"first page of two\": expected 200 from ListRepositoryLinks, got {status} {first}"
    );
    assert_eq!(
        reported_ids("first page of two", &first),
        expected[..2].to_vec(),
        "case \"first page of two\": expected the first two links in RepositoryLinkId order, got {first}"
    );
    let token = first["NextToken"].as_str().unwrap_or_else(|| {
        panic!("case \"first page of two\": links are left, so a NextToken must come back, got {first}")
    });

    let (status, second) = list(&router, Some(MAX_RESULTS_MAX), Some(token)).await;
    assert_eq!(
        status, 200,
        "case \"resumed page\": expected 200 from ListRepositoryLinks, got {status} {second}"
    );
    assert_eq!(
        reported_ids("resumed page", &second),
        expected[2..].to_vec(),
        "case \"resumed page\": a token must resume after the link its page ended on, got {second}"
    );
    assert!(
        second["NextToken"].is_null(),
        "case \"resumed page\": nothing is left, so no NextToken may come back, got {second}"
    );
}
