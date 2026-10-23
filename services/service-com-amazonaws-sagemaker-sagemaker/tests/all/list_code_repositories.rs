// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListCodeRepositories`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.ListCodeRepositories`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what
//! a real SDK client gets. The harness (`Emulator`, `error_code`, `str_field`) is the one
//! `inference_deployment_lifecycle` already defines for this target.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! member of the modelled `CodeRepositorySortBy` catalog crossed with every member of
//! `CodeRepositorySortOrder`, each filter member and each of its bounds, every page size
//! from one entry to past the published `MaxResults` maximum, and the summary each entry
//! carries compared against what `DescribeCodeRepository` replays.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    CODE_REPOSITORY_SORT_BY_VALUES, CODE_REPOSITORY_SORT_ORDER_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state::MAX_PAGE_SIZE;

use crate::inference_deployment_lifecycle::{
    error_code, str_field, Emulator, ACCOUNT, CREATE_SPACING,
};

/// The repositories the sort and filter tables range over, created in this order with the
/// `GitConfig` each one stores. Their names are deliberately NOT in creation order, so a
/// `Name` sort and a time sort cannot accidentally agree.
const SEEDED: [(&str, &str, &str); 4] = [
    ("alpha-one", "https://github.com/aws/alpha", "main"),
    ("beta-two", "https://github.com/aws/beta", "develop"),
    ("demo-three", "https://gitlab.com/g/three.git", "trunk"),
    ("demo-four", "https://g.io/four", "v1"),
];

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail these tests for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// Create [`SEEDED`], spaced so the stored time ordering is unambiguous, and answer each
/// repository's `(name, CreationTime, LastModifiedTime)` as `DescribeCodeRepository`
/// replays them, in creation order.
async fn seed_repositories(emulator: &Emulator) -> Vec<(String, f64, f64)> {
    let mut seeded = Vec::new();
    for (name, url, branch) in SEEDED {
        emulator
            .ok(
                "CreateCodeRepository",
                &json!({
                    "CodeRepositoryName": name,
                    "GitConfig": { "RepositoryUrl": url, "Branch": branch },
                }),
            )
            .await;
        let described = emulator
            .ok(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        let number = |member: &str| {
            described
                .get(member)
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected a numeric {member} in {described}"))
        };
        seeded.push((name.to_string(), number("CreationTime"), number("LastModifiedTime")));
        tokio::time::sleep(CREATE_SPACING).await;
    }
    seeded
}

/// The `CodeRepositoryName`s a `ListCodeRepositories` response lists, in response order.
fn listed_names(body: &Value) -> Vec<String> {
    body.get("CodeRepositorySummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a CodeRepositorySummaryList in {body}"))
        .iter()
        .map(|summary| str_field(summary, "CodeRepositoryName"))
        .collect()
}

/// RULE: `ListCodeRepositories` reports the stored Git repositories ordered by
/// `SortBy`/`SortOrder` — defaulting to `Name` / `Ascending` as both members' model docs
/// state — with the repository name as the tie-break, so equal sort keys keep one order
/// whichever way the page is sorted.
#[tokio::test]
async fn list_code_repositories_honours_every_documented_sort() {
    let emulator = Emulator::new();
    seed_repositories(&emulator).await;

    let by_name_ascending = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];
    let by_name_descending = vec!["demo-three", "demo-four", "beta-two", "alpha-one"];
    // Creation order, which is also last-modified order: nothing has updated these
    // repositories, so each one's `LastModifiedTime` is the instant its create recorded.
    let by_time_ascending = vec!["alpha-one", "beta-two", "demo-three", "demo-four"];
    let by_time_descending = vec!["demo-four", "demo-three", "beta-two", "alpha-one"];

    // Rows: the documented defaults, then EVERY member of the modelled
    // `CodeRepositorySortBy` catalog crossed with every member of
    // `CodeRepositorySortOrder` (both read from the generated value slices below).
    let mut cases: Vec<(String, Value, Vec<&str>)> = vec![(
        "documented-defaults".to_string(),
        json!({}),
        by_name_ascending.clone(),
    )];
    for sort_by in CODE_REPOSITORY_SORT_BY_VALUES {
        for sort_order in CODE_REPOSITORY_SORT_ORDER_VALUES {
            let expected = match (*sort_by, *sort_order) {
                ("Name", "Ascending") => by_name_ascending.clone(),
                ("Name", "Descending") => by_name_descending.clone(),
                (_, "Ascending") => by_time_ascending.clone(),
                (_, _) => by_time_descending.clone(),
            };
            cases.push((
                format!("{sort_by}-{sort_order}"),
                json!({ "SortBy": sort_by, "SortOrder": sort_order }),
                expected,
            ));
        }
    }
    // A sort order on its own takes the documented default sort field, and vice versa.
    cases.push((
        "sort-order-only".to_string(),
        json!({ "SortOrder": "Descending" }),
        by_name_descending.clone(),
    ));
    cases.push((
        "sort-by-only".to_string(),
        json!({ "SortBy": "CreationTime" }),
        by_time_ascending.clone(),
    ));

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListCodeRepositories", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListCodeRepositories order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }
}

/// RULE: `NameContains` selects exactly the repositories whose name contains the
/// substring, and nothing else about the listing changes.
#[tokio::test]
async fn list_code_repositories_filters_on_name_contains() {
    let emulator = Emulator::new();
    seed_repositories(&emulator).await;

    // Rows differ along the filter's selectivity and the shape of the substring: a
    // prefix, an infix, a whole name, a hyphen, and one nothing matches. The modelled
    // `CodeRepositoryNameContains` is len 0..63, pattern ^[a-zA-Z0-9-]+$
    // (smithy_query operation=ListCodeRepositories).
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("prefix-two-matches", "demo", vec!["demo-four", "demo-three"]),
        ("prefix-one-match", "alpha", vec!["alpha-one"]),
        ("infix", "ta-tw", vec!["beta-two"]),
        ("whole-name", "demo-three", vec!["demo-three"]),
        (
            "hyphen-matches-every-name",
            "-",
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        ("no-match", "nothing-here", vec![]),
        ("case-sensitive-no-match", "DEMO", vec![]),
    ];

    for (case, needle, expected) in cases {
        let listed = emulator
            .ok(
                "ListCodeRepositories",
                &json!({ "NameContains": needle, "SortBy": "Name", "SortOrder": "Ascending" }),
            )
            .await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: NameContains={needle} selection, got {listed:?}"
        );
    }
}

/// RULE: each of the four time bounds selects the repositories on its side of the bound,
/// the bound itself included, so a bound landing on a repository's stored instant (here
/// approached to within a millisecond, the wire's fractional-second value being a float)
/// still selects it; the bounds combine with each other and with `NameContains`.
#[tokio::test]
async fn list_code_repositories_filters_on_the_time_bounds() {
    let emulator = Emulator::new();
    let seeded = seed_repositories(&emulator).await;
    // A bound strictly between the second and third repository, so the split is
    // unambiguous, plus bounds a millisecond to either side of a stored instant — well
    // inside the spacing between two creates, so only the inclusive side can select it.
    let split = (seeded[1].1 + seeded[2].1) / 2.0;
    let a_millisecond = 0.001;
    let just_before_second_created = seeded[1].1 - a_millisecond;
    let just_after_second_created = seeded[1].1 + a_millisecond;
    let just_before_third_modified = seeded[2].2 - a_millisecond;

    let ascending = |extra: Value| {
        let mut body = json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        body
    };

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "creation-time-before-the-split",
            ascending(json!({ "CreationTimeBefore": split })),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "creation-time-after-the-split",
            ascending(json!({ "CreationTimeAfter": split })),
            vec!["demo-three", "demo-four"],
        ),
        (
            "creation-time-after-just-below-a-stored-instant",
            ascending(json!({ "CreationTimeAfter": just_before_second_created })),
            vec!["beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-before-just-above-a-stored-instant",
            ascending(json!({ "CreationTimeBefore": just_after_second_created })),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "last-modified-time-before-the-split",
            ascending(json!({ "LastModifiedTimeBefore": split })),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "last-modified-time-after-the-split",
            ascending(json!({ "LastModifiedTimeAfter": split })),
            vec!["demo-three", "demo-four"],
        ),
        (
            "last-modified-time-after-just-below-a-stored-instant",
            ascending(json!({ "LastModifiedTimeAfter": just_before_third_modified })),
            vec!["demo-three", "demo-four"],
        ),
        (
            "both-creation-bounds-around-one-repository",
            ascending(json!({
                "CreationTimeAfter": split,
                "CreationTimeBefore": (seeded[2].1 + seeded[3].1) / 2.0,
            })),
            vec!["demo-three"],
        ),
        (
            "a-creation-bound-and-a-last-modified-bound",
            ascending(json!({
                "CreationTimeAfter": split,
                "LastModifiedTimeBefore": (seeded[2].2 + seeded[3].2) / 2.0,
            })),
            vec!["demo-three"],
        ),
        (
            "bounds-that-exclude-everything",
            ascending(json!({ "CreationTimeAfter": seeded[3].1 + 3600.0 })),
            vec![],
        ),
        (
            "a-bound-combined-with-a-name-filter",
            json!({
                "CreationTimeAfter": split,
                "NameContains": "demo",
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
            vec!["demo-four", "demo-three"],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListCodeRepositories", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: selection, got {listed:?}"
        );
    }
}

/// RULE: every listed summary carries exactly what the store holds for that repository —
/// the name, the minted ARN, both stored instants and the stored `GitConfig` — the same
/// values `DescribeCodeRepository` replays.
#[tokio::test]
async fn every_summary_replays_the_stored_repository() {
    let emulator = Emulator::new();
    let seeded = seed_repositories(&emulator).await;

    let listed = emulator
        .ok(
            "ListCodeRepositories",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let summaries = listed
        .get("CodeRepositorySummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a CodeRepositorySummaryList in {listed}"));
    assert_eq!(
        summaries.len(),
        SEEDED.len(),
        "every stored repository must be listed, got {listed:?}"
    );

    for (index, (name, created_at, modified_at)) in seeded.iter().enumerate() {
        let summary = &summaries[index];
        let (_, url, branch) = SEEDED[index];
        assert_eq!(
            str_field(summary, "CodeRepositoryName"),
            name.as_str(),
            "case {name}: CodeRepositoryName in the summary, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "CodeRepositoryArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:code-repository/{name}"),
            "case {name}: CodeRepositoryArn in the summary, got {summary:?}"
        );
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_f64),
            Some(*created_at),
            "case {name}: CreationTime in the summary must be the stored one, \
             got {summary:?}"
        );
        assert_eq!(
            summary.get("LastModifiedTime").and_then(Value::as_f64),
            Some(*modified_at),
            "case {name}: LastModifiedTime in the summary must be the stored one, \
             got {summary:?}"
        );
        assert_eq!(
            summary.get("GitConfig"),
            Some(&json!({ "RepositoryUrl": url, "Branch": branch })),
            "case {name}: GitConfig in the summary, got {summary:?}"
        );
    }

    // A listing is a read: it changes nothing a later caller sees.
    let again = emulator
        .ok(
            "ListCodeRepositories",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        again, listed,
        "listing twice must report the same repositories, got {again:?}"
    );
    for (name, created_at, modified_at) in &seeded {
        let described = emulator
            .ok(
                "DescribeCodeRepository",
                &json!({ "CodeRepositoryName": name }),
            )
            .await;
        assert_eq!(
            described.get("CreationTime").and_then(Value::as_f64),
            Some(*created_at),
            "case {name}: a listing must not change the stored CreationTime, \
             got {described:?}"
        );
        assert_eq!(
            described.get("LastModifiedTime").and_then(Value::as_f64),
            Some(*modified_at),
            "case {name}: a listing must not change the stored LastModifiedTime, \
             got {described:?}"
        );
    }
}

/// RULE: the listing reflects the store as it is at the call, so a repository created
/// after an earlier listing appears in the next one.
#[tokio::test]
async fn the_listing_reflects_every_create() {
    let emulator = Emulator::new();
    let empty = emulator.ok("ListCodeRepositories", &json!({})).await;
    assert_eq!(
        listed_names(&empty),
        Vec::<String>::new(),
        "case empty-store: an empty store lists nothing, got {empty:?}"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "case empty-store: no NextToken on an empty result, got {empty:?}"
    );

    // Rows differ in count: the expected listing grows by one with every create.
    let mut expected: Vec<String> = Vec::new();
    for (name, url, branch) in SEEDED {
        emulator
            .ok(
                "CreateCodeRepository",
                &json!({
                    "CodeRepositoryName": name,
                    "GitConfig": { "RepositoryUrl": url, "Branch": branch },
                }),
            )
            .await;
        expected.push(name.to_string());
        expected.sort();
        let listed = emulator
            .ok(
                "ListCodeRepositories",
                &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            )
            .await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case after-creating-{name}: the listing must reflect the create, \
             got {listed:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered result exactly once — each page
/// holds at most `MaxResults` entries, a `NextToken` is present only while more remain
/// (so a page that exactly exhausts the result carries none), and feeding the token back
/// resumes immediately after the previous page.
#[tokio::test]
async fn paging_with_max_results_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    seed_repositories(&emulator).await;
    let ordered = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];

    // Rows: every page size from one entry up to past the end of the result set,
    // including the page size that exactly fits it (4) and both published bounds of
    // `MaxResults` (range 1..100 — smithy_query operation=ListCodeRepositories).
    for page_size in [1_i64, 2, 3, 4, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut body = json!({
                "SortBy": "Name",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListCodeRepositories", &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() as i64 <= page_size,
                "case {case}: a page must hold at most {page_size} entries, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= ordered.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        seen.len() < ordered.len(),
                        "case {case}: a page that exhausted the result must carry no \
                         NextToken, saw {seen:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, ordered,
            "case {case}: paging must yield the whole ordered result exactly once"
        );
        let expected_pages = ordered.len().div_ceil(page_size as usize);
        assert_eq!(
            pages, expected_pages,
            "case {case}: page count, saw {seen:?}"
        );
    }

    // The same walk under a time sort, so the resume boundary is not tied to the name
    // sort it happens to share a tie-break with.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "SortBy": "LastModifiedTime",
            "SortOrder": "Descending",
            "MaxResults": 2,
        });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListCodeRepositories", &body).await;
        seen.extend(listed_names(&listed));
        assert!(
            seen.len() <= ordered.len(),
            "case time-sorted-walk: paging did not terminate, saw {seen:?}"
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        "case time-sorted-walk: the pages must cover the whole ordered result once"
    );
}

/// RULE: the page is capped server-side, so no single call serialises the whole store
/// however large `MaxResults` is or whether the caller sends it at all — and the resume
/// token still walks the full set.
#[tokio::test]
async fn the_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    // The store is seeded past the service's own ceiling, so a capped page and an
    // uncapped one are distinguishable.
    let stored = PAGE_CEILING + 7;

    let emulator = Emulator::new();
    for index in 0..stored {
        emulator
            .ok(
                "CreateCodeRepository",
                &json!({
                    "CodeRepositoryName": format!("r{index:03}"),
                    "GitConfig": { "RepositoryUrl": "https://github.com/aws/repo" },
                }),
            )
            .await;
    }

    // Rows: no `MaxResults` at all, the modelled maximum (above the ceiling), and a
    // value below the ceiling, which still wins because it is the smaller cap.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("max-results-absent", json!({}), PAGE_CEILING),
        (
            "max-results-at-the-modelled-maximum",
            json!({ "MaxResults": 100 }),
            PAGE_CEILING,
        ),
        ("max-results-below-the-ceiling", json!({ "MaxResults": 10 }), 10),
    ];

    for (case, extra, expected_page) in cases {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        let listed = emulator.ok("ListCodeRepositories", &body).await;
        assert_eq!(
            listed_names(&listed).len(),
            expected_page,
            "case {case}: page size, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_some(),
            "case {case}: more repositories remain, so a NextToken is required, \
             got {listed:?}"
        );
    }

    // And the capped pages still walk the whole store exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListCodeRepositories", &body).await;
        let names = listed_names(&listed);
        assert!(
            names.len() <= PAGE_CEILING,
            "case capped-walk: a page must never exceed the ceiling, got {} entries",
            names.len()
        );
        seen.extend(names);
        assert!(
            seen.len() <= stored,
            "case capped-walk: paging did not terminate, saw {} entries",
            seen.len()
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    let expected: Vec<String> = (0..stored).map(|index| format!("r{index:03}")).collect();
    assert_eq!(
        seen, expected,
        "case capped-walk: the capped pages must cover the whole store exactly once"
    );
}

/// RULE: a request this operation cannot serve is refused with HTTP 400
/// `ValidationException` — a `SortBy` or `SortOrder` outside the modelled enum, a
/// `NextToken` that names no boundary of the result set, and anything past a member's
/// modelled constraint — never a 200 and never a 501.
#[tokio::test]
async fn requests_this_listing_cannot_serve_are_refused() {
    let emulator = Emulator::new();
    seed_repositories(&emulator).await;

    // Rows: a value outside each modelled enum (including one that is a valid member of
    // the OTHER service listings' enums, and one differing only in case), a token the
    // service did not mint, a token that is well-formed hex but names no listed
    // repository, and one past each published bound of `MaxResults` (range 1..100) and
    // `NameContains` (len 0..63, pattern ^[a-zA-Z0-9-]+$).
    let hex_of_absent_name = "61626f7665"; // "above", a name nothing created
    let refused_by_the_handler: Vec<(&str, Value)> = vec![
        ("sort-by-not-in-the-enum", json!({ "SortBy": "Status" })),
        ("sort-by-wrong-case", json!({ "SortBy": "name" })),
        ("sort-by-empty", json!({ "SortBy": "" })),
        (
            "sort-order-not-in-the-enum",
            json!({ "SortOrder": "Sideways" }),
        ),
        ("sort-order-wrong-case", json!({ "SortOrder": "ascending" })),
        (
            "next-token-not-minted-here",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            "next-token-naming-no-listed-repository",
            json!({ "NextToken": hex_of_absent_name }),
        ),
    ];

    for (case, body) in refused_by_the_handler {
        let (status, parsed) = emulator.call("ListCodeRepositories", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: the request must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
    }

    // One past each published bound of `MaxResults` (range 1..100) and `NameContains`
    // (len 0..63, pattern ^[a-zA-Z0-9-]+$) is refused by the modelled constraint, before
    // the handler runs.
    let too_long_needle = "a".repeat(64);
    let outside_the_modelled_constraints: Vec<(&str, Value)> = vec![
        ("max-results-below-the-minimum", json!({ "MaxResults": 0 })),
        (
            "max-results-one-past-the-maximum",
            json!({ "MaxResults": 101 }),
        ),
        (
            "name-contains-one-past-the-63-character-maximum",
            json!({ "NameContains": too_long_needle }),
        ),
        (
            "name-contains-outside-the-modelled-pattern",
            json!({ "NameContains": "under_score" }),
        ),
    ];

    for (case, body) in outside_the_modelled_constraints {
        let (status, parsed) = emulator.call("ListCodeRepositories", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: the request must be refused, got {status} {parsed}"
        );
    }

    // Every refusal above left the store intact.
    let listed = emulator
        .ok(
            "ListCodeRepositories",
            &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&listed),
        vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        "a refused listing must leave the store intact, got {listed:?}"
    );
}
