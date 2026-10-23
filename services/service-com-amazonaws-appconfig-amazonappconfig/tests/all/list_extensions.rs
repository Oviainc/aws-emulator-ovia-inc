// SPDX-License-Identifier: CC0-1.0

//! `ListExtensions` as a paginated report over the extensions one account holds.
//!
//! The rule under test: the listing reports exactly the extensions this account created —
//! every one of them, each exactly once, each as the LATEST version `GetExtension`
//! answers for it — in an order that is stable across calls, cut into pages of at most
//! `MaxResults` entries, with a `NextToken` handed back only when entries are left over
//! and a token that resumes the traversal where the previous page ended. A `Name` narrows
//! the listing to the extension carrying that name, which is unique per account.
//!
//! The tables span what that rule generalises over: HOW MANY extensions the account holds
//! (none, one, several, and more than a page), WHICH page size the request asks for
//! (absent, the published bounds of `MaxResults` and values in between), HOW MANY versions
//! an extension has (so the reported version is the latest rather than the first), and
//! WHAT each extension carries (a description or none). Every assertion names its case and
//! prints the actual value.
//!
//! `ListExtensions` reports no Amazon Web Services authored extension: this emulator
//! stores no record for one, so a fresh account's listing is empty rather than a catalog
//! nothing here could have created.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::MAX_LIST_PAGE_SIZE;

use crate::appconfig_workflow::{json_call, router, Answer};
use crate::extensions::{actions, create_extension, get_extension, minted_id};
use crate::resource_tags::pct;

/// `GET /extensions` with `query` as the raw query string (empty for a request that names
/// neither `MaxResults`, `NextToken` nor `Name`). All three are `httpQuery` members, named
/// `max_results`, `next_token` and `name`.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/extensions".to_string()
    } else {
        format!("/extensions?{query}")
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Items` of a listing, asserted to be a 200 (an answer with no `Items` at all reads
/// as empty, which is the shape a fresh account answers with).
fn items(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExtensions expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One member of every entry of a listing, in the order it was served.
fn listed(case: &str, answer: &Answer, member: &str) -> Vec<Value> {
    items(case, answer)
        .iter()
        .map(|item| {
            item.get(member).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: a listed extension must carry {member}, got {item}")
            })
        })
        .collect()
}

/// Create one extension named `name`, with `description` when it states one, and answer
/// the id the create minted.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    description: Option<&str>,
    latest_version_number: Option<i32>,
) -> String {
    let mut body = json!({
        "Name": name,
        "Actions": actions(&[ACTION_POINT_VALUES[0]]),
    });
    if let Some(description) = description {
        body["Description"] = Value::from(description);
    }
    let created = create_extension(router, &body, latest_version_number).await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateExtension expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    minted_id(name, &created.json())
}

/// Walk the whole listing one page at a time and return the entries in the order they
/// were served, asserting the per-page half of the rule as it goes: a page never exceeds
/// the size the request asked for, and a page that carries a `NextToken` is exactly that
/// size, because a traversal is only cut where a page filled up.
async fn walk(router: &MultiServiceRouter, case: &str, max_results: Option<i32>) -> Vec<Value> {
    let page_size = max_results.map_or(MAX_LIST_PAGE_SIZE, |size| size as usize);
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a listing that never ends its traversal fails instead of hanging.
    for _ in 0..=(MAX_LIST_PAGE_SIZE * 4 + 2) {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("max_results={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("next_token={token}"));
        }
        let answer = list(router, &query.join("&")).await;
        let page = items(case, &answer);
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page holds at most {page_size} entries, got {page:?}"
        );
        token = answer.string("NextToken");
        if token.is_some() {
            assert_eq!(
                page.len(),
                page_size,
                "case {case:?}: a page carrying a NextToken is a full page of {page_size}, \
                 got {page:?}"
            );
        }
        collected.extend(page);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: the paginated walk did not terminate");
}

/// The listing reports every extension the account holds, each exactly once, as the same
/// record `GetExtension` answers for it — whatever page size the walk uses.
///
/// Rows differ along how many extensions the account holds and the page size the walk asks
/// for: both published bounds of `MaxResults` (1 and 50), values in between, and no
/// `MaxResults` at all.
#[tokio::test]
async fn a_listing_reports_every_stored_extension_once_whatever_the_page_size() {
    let cases: Vec<(usize, Option<i32>)> = vec![
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (4, Some(2)),
        (7, Some(3)),
        (6, Some(MAX_LIST_PAGE_SIZE as i32)),
        (5, None),
    ];

    for (count, max_results) in cases {
        let case = format!("{count} extensions at page size {max_results:?}");
        let router = router();
        // Created out of name order, so a listing that reported insertion order rather
        // than a stable one fails here.
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let name = format!("extension-{:02}", (index * 7) % 100);
            fixture(&router, &name, None, None).await;
            expected.push(name);
        }
        expected.sort();

        let walked = walk(&router, &case, max_results).await;
        let names: Vec<String> = walked
            .iter()
            .map(|item| {
                item.get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: a summary carried no Name: {item}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected every stored extension once, in listing order"
        );

        // Each summary is the stored record, narrowed to the members `ExtensionSummary`
        // carries: a listing that recomputed a value would differ from the reader here.
        for summary in &walked {
            let identifier = summary
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary carried no Id: {summary}"));
            let read = get_extension(&router, identifier, None).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: GetExtension({identifier}) expected 200, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            for member in ["Id", "Name", "VersionNumber", "Arn", "Description"] {
                assert_eq!(
                    summary.get(member).cloned(),
                    read.json().get(member).cloned(),
                    "case {case:?}: the summary's {member} must be the stored one, got {summary}"
                );
            }
        }
    }

    // The traversal order does not move between calls.
    let router = router();
    for index in 0..5 {
        fixture(&router, &format!("stable-{}", (index * 3) % 5), None, None).await;
    }
    let first = listed("stable order", &list(&router, "").await, "Id");
    let again = listed("stable order", &list(&router, "").await, "Id");
    assert_eq!(
        first, again,
        "two listings of the same store must report the same order, got {first:?} then {again:?}"
    );
}

/// An extension is reported at its LATEST version: a second version created under the same
/// name replaces the summary rather than adding one.
///
/// Rows differ along how many versions the extension holds, so a listing that reported the
/// first version, or one summary per version, fails here.
#[tokio::test]
async fn a_listing_reports_each_extension_at_its_latest_version() {
    let router = router();
    let name = "deployment-events-to-sqs";
    let id = fixture(&router, name, Some("version 1"), None).await;

    for version in 1..=4i32 {
        let case = format!("after version {version}");
        if version > 1 {
            // The lock names the version that IS latest, so this appends the next one.
            let created = create_extension(
                &router,
                &json!({
                    "Name": name,
                    "Description": format!("version {version}"),
                    "Actions": actions(&[ACTION_POINT_VALUES[version as usize % ACTION_POINT_VALUES.len()]]),
                }),
                Some(version - 1),
            )
            .await;
            assert_eq!(
                created.status,
                201,
                "case {case:?}: CreateExtension expected 201, got {} / {}",
                created.status,
                String::from_utf8_lossy(&created.body)
            );
        }

        let answer = list(&router, "").await;
        let page = items(&case, &answer);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: an extension is one entry however many versions it holds, got {page:?}"
        );
        let read = get_extension(&router, &id, None).await;
        assert_eq!(
            page[0].get("VersionNumber").and_then(Value::as_i64),
            Some(i64::from(version)),
            "case {case:?}: expected the latest version {version}, got {}",
            page[0]
        );
        assert_eq!(
            page[0].get("Description").and_then(Value::as_str),
            Some(format!("version {version}").as_str()),
            "case {case:?}: expected the latest version's Description, got {}",
            page[0]
        );
        assert_eq!(
            page[0].get("Arn").cloned(),
            read.json().get("Arn").cloned(),
            "case {case:?}: expected the latest version's own ARN, got {}",
            page[0]
        );
    }
}

/// `Name` narrows the listing to the extension carrying that name — and to nothing at all
/// when no extension carries it.
///
/// Rows differ along which stored name is asked for (including names that share a prefix,
/// so the filter is equality rather than a prefix match) and along names nobody used.
#[tokio::test]
async fn a_name_filters_the_listing_to_the_extension_carrying_it() {
    let router = router();
    let stored = ["alpha", "alpha-two", "beta", "my extension_1.2-3"];
    let mut ids: Vec<(&str, String)> = Vec::new();
    for name in stored {
        let id = fixture(&router, name, Some(name), None).await;
        ids.push((name, id));
    }

    for (name, id) in &ids {
        let answer = list(&router, &format!("name={}", pct(name))).await;
        let page = items(name, &answer);
        assert_eq!(
            page.len(),
            1,
            "case {name:?}: a Name filter reports exactly the extension it names, got {page:?}"
        );
        assert_eq!(
            page[0].get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {name:?}: expected the extension carrying that name, got {}",
            page[0]
        );
        assert!(
            answer.string("NextToken").is_none(),
            "case {name:?}: a single-entry listing hands out no NextToken, got {:?}",
            answer.string("NextToken")
        );
    }

    for unused in ["alph", "alpha-", "ALPHA", "gamma", "my extension"] {
        let answer = list(&router, &format!("name={}", pct(unused))).await;
        let page = items(unused, &answer);
        assert!(
            page.is_empty(),
            "case {unused:?}: a name no extension carries reports nothing, got {page:?}"
        );
    }

    // The unfiltered listing still holds all of them, so the filter narrowed the report
    // rather than the store.
    let all = listed("unfiltered", &list(&router, "").await, "Name");
    assert_eq!(
        all.len(),
        stored.len(),
        "the unfiltered listing must still report every extension, got {all:?}"
    );
}

/// A `NextToken` this service did not mint for this listing is refused with the modelled
/// `BadRequestException` rather than read as "start from the beginning".
///
/// Rows differ along HOW the token is wrong: not hex at all, hex over a payload carrying
/// no listing tag, hex over ANOTHER listing's tag (the token a sibling list operation
/// hands out), and a bare resource name.
#[tokio::test]
async fn a_token_this_listing_did_not_mint_is_refused() {
    let router = router();
    fixture(&router, "only", None, None).await;

    let forged = [
        ("not hex", "not-a-token".to_string()),
        ("hex over an untagged cursor", hex::encode("only")),
        (
            "hex over another listing's tag",
            hex::encode("ListEnvironments\u{1f}only"),
        ),
        (
            "hex over a tag that is no listing",
            hex::encode("Extensions\u{1f}only"),
        ),
        ("a bare name", "only".to_string()),
    ];

    for (case, token) in forged {
        let answer = list(&router, &format!("next_token={token}")).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for token {token:?}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer
                .error_code()
                .split('#')
                .next_back()
                .unwrap_or_default(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {}",
            answer.error_code()
        );
    }

    // The listing itself still works, so the refusals above are about the token.
    let page = items("after the refusals", &list(&router, "").await);
    assert_eq!(
        page.len(),
        1,
        "the stored extension must still be listed, got {page:?}"
    );
}

/// A fresh account holds no extensions, so the listing is empty and hands out no token —
/// this emulator reports no Amazon Web Services authored catalog, because it stores no
/// record for one.
#[tokio::test]
async fn a_fresh_account_lists_no_extension() {
    let router = router();
    for query in ["", "max_results=50", "name=AWS.AppConfig.FeatureFlags"] {
        let answer = list(&router, query).await;
        let page = items(query, &answer);
        assert!(
            page.is_empty(),
            "case {query:?}: expected an empty listing on a fresh account, got {page:?}"
        );
        assert!(
            answer.string("NextToken").is_none(),
            "case {query:?}: an exhausted listing hands out no NextToken, got {:?}",
            answer.string("NextToken")
        );
    }
}
