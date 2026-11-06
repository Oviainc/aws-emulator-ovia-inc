// SPDX-License-Identifier: CC0-1.0

//! The effective PAGE SIZE of this service's enumerations, end to end over the
//! `awsJson1_1` wire.
//!
//! THE RULE: a paginated enumeration answers at most ONE page of its collection — the
//! caller's `MaxResults` when it named one, and this service's default page size
//! ([`state::MAX_PAGE_SIZE`], the ceiling the model's `MaxResults` shape declares) when it
//! named none — and reports a `NextToken` of the modelled `Token` shape exactly when
//! entries remain beyond the page it served, absent on the last page. Only a FULL page can
//! report one, and the token it reports is accepted back as that same operation's
//! `NextToken`, so following it enumerates the rest of the collection without repeating or
//! dropping an entry.
//!
//! The rows span what the rule generalises over: the ENUMERATION (the AWS-owned
//! application-provider catalogue, which holds more than one default page, and a
//! caller-driven permission-set listing, which holds fewer entries than one page) and the
//! page size (absent, both published bounds of `range(1..100)`, the default itself, a size
//! that divides a collection and sizes that do not).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_providers::expected_arns as catalogue_arns;
use crate::ssoadmin_permission_sets::{call, organization_router};

/// The page size an enumeration that names no `MaxResults` is served with: this service's
/// own default, which is also the ceiling the model's `MaxResults` declares
/// (`smithy_query(query="shape",shape="MaxResults")`: `range(min: 1, max: 100)`).
const DEFAULT_PAGE_SIZE: usize = state::MAX_PAGE_SIZE as usize;

/// The model's `MaxResults` bounds, the page sizes a caller may ask for.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// How many permission sets the caller-driven row creates: fewer than one default page, so
/// that collection is served whole and reports no cursor.
const PERMISSION_SETS: usize = 3;

/// Whether `token` satisfies the model's `Token` constraints: 0..2048 of
/// `^[-a-zA-Z0-9+=/_]*$` (`smithy_query(query="shape",shape="Token")`).
fn is_token(token: &str) -> bool {
    token.len() <= 2048
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-+=/_".contains(c))
}

/// One enumeration the rows drive: the operation, the request that starts it
/// (`{instance}` standing in for the account's instance ARN), the output member its items
/// arrive in, how an item names itself, and every key the collection holds (from the
/// permission sets [`harness`] created, for the listing that has any).
struct Enumeration {
    operation: &'static str,
    request: fn(&str) -> Value,
    items: &'static str,
    key: fn(&Value) -> String,
    expected: fn(&[String]) -> Vec<String>,
}

/// The enumerations under test: the AWS-owned catalogue (bigger than one default page) and
/// a caller-driven listing scoped to one instance (smaller than one default page).
const ENUMERATIONS: &[Enumeration] = &[
    Enumeration {
        operation: "ListApplicationProviders",
        request: |_| json!({}),
        items: "ApplicationProviders",
        key: |item| {
            item["ApplicationProviderArn"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry without an ApplicationProviderArn: {item}"))
                .to_string()
        },
        // The catalogue the crate publishes, read from the crate so an entry added to it
        // is covered here without editing this suite.
        expected: |_| catalogue_arns(),
    },
    Enumeration {
        operation: "ListPermissionSets",
        request: |instance| json!({"InstanceArn": instance}),
        items: "PermissionSets",
        key: |item| {
            item.as_str()
                .unwrap_or_else(|| panic!("a permission set entry that is not an ARN: {item}"))
                .to_string()
        },
        expected: |created| created.to_vec(),
    },
];

/// A router whose account holds an instance with [`PERMISSION_SETS`] permission sets, with
/// their ARNs in creation order.
async fn harness() -> (MultiServiceRouter, String, Vec<String>) {
    let (router, instance_arn) = organization_router();
    let mut arns = Vec::new();
    for index in 0..PERMISSION_SETS {
        let name = format!("PageSize{index}");
        let (status, created) = call(
            &router,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreatePermissionSet {name} expected 200, got {status} {created}"
        );
        arns.push(
            created["PermissionSet"]["PermissionSetArn"]
                .as_str()
                .unwrap_or_else(|| panic!("the create minted no PermissionSetArn: {created}"))
                .to_string(),
        );
    }
    (router, instance_arn, arns)
}

/// One page of `enumeration`: the keys it reported and the cursor it handed back, with the
/// per-page half of THE RULE asserted — a page is at most `effective` entries, and only a
/// full page may report a cursor, which has to be a modelled `Token`.
async fn page(
    router: &MultiServiceRouter,
    enumeration: &Enumeration,
    instance_arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let effective = max_results.unwrap_or(DEFAULT_PAGE_SIZE as i64) as usize;
    let mut request = (enumeration.request)(instance_arn);
    if let Some(size) = max_results {
        request["MaxResults"] = json!(size);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    let (status, body) = call(router, enumeration.operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} {body}"
    );
    let items = body[enumeration.items]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {} in {body}", enumeration.items))
        .clone();
    assert!(
        items.len() <= effective,
        "case {case:?}: a page holds at most {effective} entries, got {} in {body}",
        items.len()
    );
    let token = body["NextToken"].as_str().map(|token| {
        assert!(
            is_token(token),
            "case {case:?}: expected a modelled Token, got {token:?}"
        );
        assert_eq!(
            items.len(),
            effective,
            "case {case:?}: only a full page of {effective} may report a cursor, got {} in \
             {body}",
            items.len()
        );
        token.to_string()
    });
    let keys = items.iter().map(enumeration.key).collect();
    (keys, token)
}

/// Every key the enumeration reports, paged to its end with `max_results` per page, and
/// the number of pages it took.
async fn walk(
    router: &MultiServiceRouter,
    enumeration: &Enumeration,
    instance_arn: &str,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> (Vec<String>, usize) {
    let mut keys: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (page_keys, next) = page(
            router,
            enumeration,
            instance_arn,
            max_results,
            token.as_deref(),
            case,
        )
        .await;
        keys.extend(page_keys);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: paging did not terminate after {pages} pages for \
             {expected_total} entries"
        );
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    (keys, pages)
}

/// THE RULE, on an enumeration that names no `MaxResults`: the response carries at most the
/// default page size, reports a cursor exactly when entries remain, and following that
/// cursor enumerates the rest of the collection once per entry.
#[tokio::test]
async fn an_enumeration_that_names_no_page_size_is_served_the_default_page() {
    let (router, instance_arn, permission_sets) = harness().await;

    for enumeration in ENUMERATIONS {
        let case = format!("{} / no MaxResults", enumeration.operation);
        let expected = (enumeration.expected)(&permission_sets);

        // The FIRST page on its own: the default page size is what bounds it, and the
        // cursor is present exactly when the collection outruns that page.
        let (first, token) = page(&router, enumeration, &instance_arn, None, None, &case).await;
        assert_eq!(
            first.len(),
            expected.len().min(DEFAULT_PAGE_SIZE),
            "case {case:?}: a first page of {} entries is bounded by the default page size \
             {DEFAULT_PAGE_SIZE}, got {} entries",
            expected.len(),
            first.len()
        );
        assert_eq!(
            token.is_some(),
            expected.len() > DEFAULT_PAGE_SIZE,
            "case {case:?}: a cursor is reported exactly when entries remain beyond the \
             page ({} entries, page {DEFAULT_PAGE_SIZE}), got {token:?}",
            expected.len()
        );

        // And following it to the end reports the collection, once per entry.
        let (walked, pages) = walk(
            &router,
            enumeration,
            &instance_arn,
            None,
            expected.len(),
            &case,
        )
        .await;
        let mut sorted = walked.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            walked.len(),
            "case {case:?}: no entry may be reported twice, got {} of {} distinct",
            sorted.len(),
            walked.len()
        );
        let mut expected_sorted = expected.clone();
        expected_sorted.sort_unstable();
        assert_eq!(
            sorted, expected_sorted,
            "case {case:?}: paging must report exactly the collection, got {} entries",
            walked.len()
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(DEFAULT_PAGE_SIZE).max(1),
            "case {case:?}: {} entries in pages of {DEFAULT_PAGE_SIZE} take \
             {} page(s), got {pages}",
            expected.len(),
            expected.len().div_ceil(DEFAULT_PAGE_SIZE).max(1)
        );
    }
}

/// THE RULE over the page sizes a caller may ask for: each is the ceiling of every page,
/// and whichever it is, paging reports exactly the collection once per entry.
///
/// The rows are both published bounds of `range(1..100)`, the default itself, a size that
/// divides the permission-set listing, sizes that divide neither collection, and the size
/// omitted entirely.
#[tokio::test]
async fn every_page_size_the_caller_may_ask_for_is_the_pages_ceiling() {
    let (router, instance_arn, permission_sets) = harness().await;
    let sizes: Vec<Option<i64>> = vec![
        None,
        Some(MAX_RESULTS_MIN),
        Some(PERMISSION_SETS as i64),
        Some(7),
        Some(MAX_RESULTS_MAX - 1),
        Some(MAX_RESULTS_MAX),
    ];

    for enumeration in ENUMERATIONS {
        let expected = {
            let mut keys = (enumeration.expected)(&permission_sets);
            keys.sort_unstable();
            keys
        };
        for size in &sizes {
            let case = format!("{} / MaxResults = {size:?}", enumeration.operation);
            let (walked, _) = walk(
                &router,
                enumeration,
                &instance_arn,
                *size,
                expected.len(),
                &case,
            )
            .await;
            let mut sorted = walked.clone();
            sorted.sort_unstable();
            assert_eq!(
                sorted, expected,
                "case {case:?}: paging must report exactly the collection, got {} entries",
                walked.len()
            );
        }
    }
}

/// THE RULE's token half: the cursor a default-size page reports is accepted back as the
/// same operation's `NextToken` and resumes AFTER the page that minted it, so the two
/// pages are disjoint and consecutive in the collection's own order.
#[tokio::test]
async fn the_cursor_a_default_page_reports_resumes_after_it() {
    let (router, instance_arn, permission_sets) = harness().await;
    let enumeration = &ENUMERATIONS[0];
    let case = "the catalogue, no MaxResults";
    let expected = (enumeration.expected)(&permission_sets);
    assert!(
        expected.len() > DEFAULT_PAGE_SIZE,
        "case {case:?}: this row needs a collection bigger than one default page of \
         {DEFAULT_PAGE_SIZE}, got {} entries",
        expected.len()
    );

    let (first, token) = page(&router, enumeration, &instance_arn, None, None, case).await;
    let token = token.unwrap_or_else(|| {
        panic!("case {case:?}: entries remain beyond the first page, so it must mint a cursor")
    });
    assert_eq!(
        first,
        expected[..DEFAULT_PAGE_SIZE].to_vec(),
        "case {case:?}: the first page is the collection's first {DEFAULT_PAGE_SIZE} \
         entries, in its own order"
    );

    let (second, _) = page(
        &router,
        enumeration,
        &instance_arn,
        None,
        Some(&token),
        case,
    )
    .await;
    let remaining = expected.len() - DEFAULT_PAGE_SIZE;
    assert_eq!(
        second,
        expected[DEFAULT_PAGE_SIZE..expected.len().min(2 * DEFAULT_PAGE_SIZE)].to_vec(),
        "case {case:?}: the resumed page continues after the first, with {remaining} \
         entries left"
    );
    assert!(
        second.iter().all(|key| !first.contains(key)),
        "case {case:?}: the resumed page must repeat no entry of the page that minted the \
         cursor"
    );
}
