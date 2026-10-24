// SPDX-License-Identifier: CC0-1.0

//! `ListCallAnalyticsCategories`, end to end over the wire, against the categories
//! `CreateCallAnalyticsCategory` stored and `GetCallAnalyticsCategory` reports.
//!
//! The rules these tests pin, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. The listing REPORTS every Call Analytics category this account holds and nothing
//!    else, each one as the same `CategoryProperties` record the read reports for it.
//! 2. It PAGES: a page holds at most `MaxResults` categories (the documented default of
//!    5 when the request names none), a page carrying a `NextToken` is a full page,
//!    `NextToken` is present exactly when further categories remain, and following the
//!    tokens walks every category exactly once in one total order — the same order
//!    whatever the page size. The model documents no order for this listing (unlike
//!    `ListTranscriptionJobs`, whose doc says "newest first"), so the order is this
//!    service's own stable key order: the category name, ascending.
//! 3. The listing is REPORTING, not mutating: listing twice answers the same page, and
//!    a read after a listing reports the category unchanged.
//! 4. A `NextToken` this service never handed out is refused with the modelled
//!    `BadRequestException` rather than silently restarting from the top, and an input
//!    outside a modelled constraint is refused by the boundary validator.
//! 5. Categories deleted or created between two pages are reflected on the pages that
//!    follow: no name is reported twice and no survivor the walk had not yet reached is
//!    missed.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) and the lifecycle suite's rule builders
//! (`crate::call_analytics_categories`) rather than standing up a second emulator.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::call_analytics_categories::{
    interruption_rule, non_talk_time_rule, sentiment_rule, transcript_rule,
};
use crate::transcription_jobs::Transcribe;

/// The page this listing serves when the request names no `MaxResults`: "If you do not
/// specify a value, a default of 5 is used" (model doc for
/// `ListCallAnalyticsCategoriesRequest$MaxResults`).
const DEFAULT_PAGE_SIZE: usize = 5;

/// The status this service's awsJson1_1 wire protocol puts on every client error.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The modelled client error this listing answers for a `NextToken` it never wrote.
const BAD_TOKEN_CODE: &str = "BadRequestException";

/// The error code the boundary validator answers for an input outside a modelled
/// constraint, before any handler runs.
const VALIDATION_CODE: &str = "ValidationException";

/// The modelled ceiling of `NextToken` (`@length(min: 0, max: 8192)`,
/// `smithy_query(query="shape", shape="NextToken")`).
const NEXT_TOKEN_CEILING: usize = 8192;

/// The name of the category created `index`th, spanning the forms
/// `CategoryName`'s own `@pattern(^[0-9a-zA-Z._-]+$)` allows (a dot, an underscore, a
/// hyphen, upper and lower case, digits).
///
/// The numeric part counts DOWN as the index counts up, so the creation order of a
/// table built from this function is not its name order — an ordering assertion on the
/// listing therefore cannot pass just because the service reports insertion order.
fn category_name(index: usize) -> String {
    let descending = 99 - index;
    match index % 3 {
        0 => format!("cat.{descending}"),
        1 => format!("CAT_{descending}"),
        _ => format!("cat-{descending}"),
    }
}

/// The rules the `index`th category is created with, cycling the four members of the
/// `Rule` union and varying how many rules a category has (one, or two every fifth).
fn rules_for(index: usize) -> Value {
    let first = match index % 4 {
        0 => non_talk_time_rule(1_000 * index as i64),
        1 => interruption_rule(500 * index as i64),
        2 => transcript_rule(&format!("phrase {index}")),
        _ => sentiment_rule("NEGATIVE"),
    };
    if index % 5 == 4 {
        json!([first, non_talk_time_rule(0)])
    } else {
        json!([first])
    }
}

/// The `InputType` the `index`th create sends, or `None` to send none at all (which
/// the service defaults to `POST_CALL`).
fn input_type_for(index: usize) -> Option<&'static str> {
    match index % 3 {
        0 => None,
        1 => Some("POST_CALL"),
        _ => Some("REAL_TIME"),
    }
}

/// Create `count` categories through the wire and return their names in CREATION
/// order.
async fn create_categories(transcribe: &Transcribe, count: usize) -> Vec<String> {
    let mut created: Vec<String> = Vec::new();
    for index in 0..count {
        let name = category_name(index);
        let mut request = json!({"CategoryName": name, "Rules": rules_for(index)});
        if let Some(input_type) = input_type_for(index) {
            request["InputType"] = json!(input_type);
        }
        if index % 4 == 1 {
            request["Tags"] = json!([{"Key": "suite", "Value": "listing"}]);
        }
        transcribe
            .call("CreateCallAnalyticsCategory", request)
            .await
            .ok(&name);
        created.push(name);
    }
    created
}

/// One page of the listing: the names it reported, the `CategoryProperties` it
/// reported them as, and the cursor it carried (if any).
struct Page {
    names: Vec<String>,
    properties: Vec<Value>,
    next_token: Option<String>,
}

/// One `ListCallAnalyticsCategories` call, with `MaxResults` and `NextToken` sent only
/// when given.
async fn list(
    transcribe: &Transcribe,
    max_results: Option<u64>,
    next_token: Option<&str>,
    case: &str,
) -> Page {
    let mut request = json!({});
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["NextToken"] = json!(next_token);
    }
    let reply = transcribe
        .call("ListCallAnalyticsCategories", request)
        .await;
    let body = reply.ok(case).clone();
    let properties = body
        .get("Categories")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a Categories list in the answer, got {body:?}")
        })
        .clone();
    let names = properties
        .iter()
        .map(|category| {
            category
                .get("CategoryName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: every listed category must report its name, \
                         got {category:?}"
                    )
                })
                .to_string()
        })
        .collect();
    Page {
        names,
        properties,
        next_token: body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Walk every page of the listing with `max_results` and return the names in the order
/// they were reported, asserting RULE 2 on each page along the way. `total` is how many
/// categories the account holds, which bounds the walk so a listing that repeated a
/// page fails instead of looping.
async fn walk(
    transcribe: &Transcribe,
    max_results: Option<u64>,
    total: usize,
    case: &str,
) -> Vec<String> {
    let page_size = max_results.unwrap_or(DEFAULT_PAGE_SIZE as u64) as usize;
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = list(transcribe, max_results, token.as_deref(), case).await;
        assert!(
            page.names.len() <= page_size,
            "case {case:?}: a page must hold at most {page_size} categories, got {:?}",
            page.names
        );
        seen.extend(page.names.iter().cloned());
        match page.next_token {
            None => break,
            Some(next) => {
                assert_eq!(
                    page.names.len(),
                    page_size,
                    "case {case:?}: a page carrying a NextToken must be a full page, got {:?}",
                    page.names
                );
                assert!(
                    seen.len() <= total,
                    "case {case:?}: paging must terminate after {total} categories, got {seen:?}"
                );
                token = Some(next);
            }
        }
    }
    seen
}

/// RULE 2: every page size walks the same total order, reporting every category exactly
/// once, over a table of account sizes: none at all, one, one short of the default
/// page, exactly it, one past it, and several pages' worth.
#[tokio::test]
async fn paging_walks_every_category_once_whatever_the_page_size() {
    for count in [0_usize, 1, 4, 5, 6, 13] {
        let transcribe = Transcribe::new();
        let created = create_categories(&transcribe, count).await;
        // The store's own key order: the category name, ascending. `BTreeMap` orders
        // its `String` keys byte-wise, and so does `Vec<String>::sort`, so this is the
        // same order the service pages in — keep it a plain byte-wise sort rather than
        // a locale-aware one.
        let mut expected = created.clone();
        expected.sort();
        if count > 1 {
            assert_ne!(
                expected, created,
                "the table must not be in creation order already, or the order \
                 assertion below has no teeth: {created:?}"
            );
        }

        // The modelled bounds of `MaxResults` (`@range(min: 1, max: 100)`), the
        // documented default, and the sizes around it.
        for max_results in [None, Some(1_u64), Some(2), Some(5), Some(100)] {
            let case = format!("{count} categories, MaxResults={max_results:?}");
            let seen = walk(&transcribe, max_results, count, &case).await;
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must report every category exactly once, in name \
                 order, got {seen:?}"
            );
        }
    }
}

/// RULE 2, the default page size, and the empty account: an account holding no category
/// gets an empty list and no cursor, and a request naming no `MaxResults` gets the
/// documented default of 5 with a cursor to the rest.
#[tokio::test]
async fn an_empty_account_gets_an_empty_page_and_the_default_page_is_five() {
    let transcribe = Transcribe::new();

    let case = "an account holding no category";
    let empty = list(&transcribe, None, None, case).await;
    assert!(
        empty.properties.is_empty(),
        "case {case:?}: expected an empty Categories list, got {:?}",
        empty.properties
    );
    assert_eq!(
        empty.next_token, None,
        "case {case:?}: a page with nothing after it must carry no NextToken, got {:?}",
        empty.next_token
    );

    let case = "13 categories, no MaxResults";
    create_categories(&transcribe, 13).await;
    let defaulted = list(&transcribe, None, None, case).await;
    assert_eq!(
        defaulted.names.len(),
        DEFAULT_PAGE_SIZE,
        "case {case:?}: the documented default page size is {DEFAULT_PAGE_SIZE}, got {:?}",
        defaulted.names
    );
    assert!(
        defaulted.next_token.is_some(),
        "case {case:?}: a truncated page must carry a NextToken, got {:?}",
        defaulted.next_token
    );
}

/// RULES 1 and 3: the listing reports each stored category as the record the read
/// reports, and reporting it changes nothing — a second listing and a read afterwards
/// answer exactly the same properties.
#[tokio::test]
async fn the_listing_reports_what_the_read_reports_and_changes_nothing() {
    let transcribe = Transcribe::new();
    let created = create_categories(&transcribe, 7).await;
    let mut expected = created.clone();
    expected.sort();

    let case = "seven categories, unpaged";
    let listed = list(&transcribe, Some(100), None, case).await;
    assert_eq!(
        listed.names, expected,
        "case {case:?}: the listing must report every stored category and nothing else, \
         got {:?}",
        listed.names
    );
    assert_eq!(
        listed.next_token, None,
        "case {case:?}: a page holding every category must carry no NextToken, got {:?}",
        listed.next_token
    );

    for properties in &listed.properties {
        let name = properties["CategoryName"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a CategoryName, got {properties:?}")
        });
        let read = transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await;
        let read = read
            .ok(name)
            .get("CategoryProperties")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {name:?}: expected CategoryProperties from the read, got {read:?}",
                    read = read.body)
            });
        assert_eq!(
            &read, properties,
            "case {name:?}: the listing must report what the read reports, got {properties:?}"
        );
    }

    let again = list(&transcribe, Some(100), None, "a second listing").await;
    assert_eq!(
        again.properties, listed.properties,
        "case {case:?}: listing the categories must not change them, got {:?}",
        again.properties
    );
}

/// `NextToken` values this service never handed out, one per form a client might send:
/// a bare word, the `{sequence}|{name}` cursor the job and vocabulary listings hand
/// out, this listing's own prefix with no name behind it, a category name with no
/// prefix, the prefix in the wrong case, and a token at the modelled `NextToken`
/// ceiling.
fn foreign_tokens() -> Vec<(String, String)> {
    vec![
        ("a bare word".to_string(), "not-a-cursor".to_string()),
        (
            "the sequence cursor the other listings hand out".to_string(),
            "3|cat.99".to_string(),
        ),
        (
            "this listing's prefix with no name behind it".to_string(),
            "name|".to_string(),
        ),
        (
            "a category name with no cursor prefix".to_string(),
            category_name(0),
        ),
        (
            "the cursor prefix in the wrong case".to_string(),
            format!("NAME|{}", category_name(0)),
        ),
        (
            format!("a token at the modelled ceiling of {NEXT_TOKEN_CEILING}"),
            "t".repeat(NEXT_TOKEN_CEILING),
        ),
    ]
}

/// RULE 4: a `NextToken` this service never wrote is refused with the modelled
/// `BadRequestException`, never served as a fresh first page.
#[tokio::test]
async fn a_next_token_this_service_did_not_hand_out_is_refused() {
    let transcribe = Transcribe::new();
    create_categories(&transcribe, 7).await;

    for (case, token) in foreign_tokens() {
        transcribe
            .call("ListCallAnalyticsCategories", json!({"NextToken": token}))
            .await
            .refused(&case, CLIENT_ERROR_STATUS, BAD_TOKEN_CODE);
    }
}

/// RULES 2 and 4 at the modelled bounds: `MaxResults` is honoured at each published
/// bound of `@range(min: 1, max: 100)` and refused one past either, and a `NextToken`
/// one past its `@length` ceiling is refused by the boundary validator.
#[tokio::test]
async fn max_results_is_honoured_at_each_bound_and_refused_past_it() {
    let transcribe = Transcribe::new();
    let created = create_categories(&transcribe, 13).await;

    for (case, max_results, expected) in [
        ("the MaxResults floor of 1", 1_u64, 1),
        ("the MaxResults ceiling of 100", 100, created.len()),
    ] {
        let page = list(&transcribe, Some(max_results), None, case).await;
        assert_eq!(
            page.names.len(),
            expected,
            "case {case:?}: expected {expected} categories on the page, got {:?}",
            page.names
        );
    }

    for (case, max_results) in [
        ("one below the MaxResults floor", 0_i64),
        ("one past the MaxResults ceiling", 101),
    ] {
        transcribe
            .call(
                "ListCallAnalyticsCategories",
                json!({"MaxResults": max_results}),
            )
            .await
            .refused(case, CLIENT_ERROR_STATUS, VALIDATION_CODE);
    }

    let case = format!("a token one past the ceiling of {NEXT_TOKEN_CEILING}");
    transcribe
        .call(
            "ListCallAnalyticsCategories",
            json!({"NextToken": "t".repeat(NEXT_TOKEN_CEILING + 1)}),
        )
        .await
        .refused(&case, CLIENT_ERROR_STATUS, VALIDATION_CODE);
}

/// RULE 5: a category deleted or created between two pages is reflected on the pages
/// that follow — the cursor resumes strictly past the last name of the previous page,
/// so a deletion of that very category does not restart the walk and no name is
/// reported twice.
#[tokio::test]
async fn categories_written_between_two_pages_are_reflected() {
    let transcribe = Transcribe::new();
    let created = create_categories(&transcribe, 7).await;
    let mut expected = created.clone();
    expected.sort();

    let case = "a delete and a create between two pages";
    let page_size = 2_u64;
    let first = list(&transcribe, Some(page_size), None, case).await;
    let token = first.next_token.clone().unwrap_or_else(|| {
        panic!(
            "case {case:?}: a truncated page must carry a NextToken, got {:?}",
            first.names
        )
    });

    // One category the first page already reported (the cursor itself) and one it has
    // not reached yet are deleted, and one sorting after every name is created.
    let reported = first.names[1].clone();
    let not_yet_reached = expected
        .last()
        .cloned()
        .expect("the table holds seven categories");
    for name in [&reported, &not_yet_reached] {
        transcribe
            .call(
                "DeleteCallAnalyticsCategory",
                json!({"CategoryName": name}),
            )
            .await
            .ok_status(case, 200);
    }
    let late = "zzz.created-mid-walk";
    transcribe
        .call(
            "CreateCallAnalyticsCategory",
            json!({"CategoryName": late, "Rules": rules_for(0)}),
        )
        .await
        .ok(case);

    let mut seen = first.names.clone();
    let mut token = Some(token);
    while let Some(current) = token {
        let page = list(&transcribe, Some(page_size), Some(&current), case).await;
        seen.extend(page.names.iter().cloned());
        token = page.next_token;
        assert!(
            seen.len() <= expected.len() + 1,
            "case {case:?}: paging must terminate, got {seen:?}"
        );
    }

    let unique: BTreeSet<&String> = seen.iter().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "case {case:?}: no category may be reported twice, got {seen:?}"
    );
    assert_eq!(
        seen.iter().filter(|name| *name == &reported).count(),
        1,
        "case {case:?}: deleting the cursor's own category must not restart the walk or \
         repeat it, got {seen:?}"
    );
    assert!(
        !seen[first.names.len()..].contains(&not_yet_reached),
        "case {case:?}: a category deleted before the walk reached it must not be \
         reported, got {seen:?}"
    );
    assert!(
        seen.contains(&late.to_string()),
        "case {case:?}: a category created before the later pages must be reported on \
         them, got {seen:?}"
    );
    for name in &expected {
        if name == &not_yet_reached {
            continue;
        }
        assert!(
            seen.contains(name),
            "case {case:?}: the surviving category {name:?} must still be reported, \
             got {seen:?}"
        );
    }
}
