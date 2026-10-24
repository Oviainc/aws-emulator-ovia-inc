// SPDX-License-Identifier: CC0-1.0

//! `ListRegexPatternSets`, end to end over the wire.
//!
//! The rule asserted here: `ListRegexPatternSets` "retrieves an array of
//! RegexPatternSetSummary objects for the regex pattern sets that you manage", so
//! the page it answers is computed from the sets this service has PERSISTED in the
//! requested scope — one summary per stored set, carrying that set's stored `Name`,
//! `Id`, `Description`, `ARN` and current `LockToken` — and never a value derived
//! from the list request itself. Concretely —
//!
//! * a scope is a namespace, so a listing enumerates the sets of the scope it names
//!   and no others;
//! * the listing is a REPORT: calling it twice, in any order relative to other
//!   reads, answers the same summaries, and every summary matches what
//!   `GetRegexPatternSet` answers for the same set;
//! * `Limit` is "the maximum number of objects that you want WAF to return for this
//!   request" and `NextMarker` resumes "the next batch of objects" after the prior
//!   call's marker, so paging enumerates every stored set exactly once whatever the
//!   page size, and a `Limit` outside the modelled `PaginationLimit` range is
//!   refused rather than clamped.
//!
//! Every call goes through the same front door a real client uses (the `Waf`
//! harness in [`crate::web_acl_lifecycle`]), so what these tests observe is what an
//! SDK observes.

use serde_json::{json, Value};

use crate::regex_pattern_set_lifecycle::{
    create_regex_pattern_set, get_regex_pattern_set, regex_pattern_set_arn,
};
use crate::web_acl_lifecycle::{is_model_uuid, Waf};

/// The summaries of one `ListRegexPatternSets` page, as an array.
fn summaries(page: &Value) -> Vec<Value> {
    page["RegexPatternSets"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries RegexPatternSets, got {page}"))
        .clone()
}

/// The `Name` of every summary on a page, in the order the page lists them.
fn listed_names(page: &Value) -> Vec<String> {
    summaries(page)
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One stored set a listing must report: the scope it lives in, its name, the
/// patterns it holds and the description it was created with, if any.
struct StoredSet {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The scope the set is created in.
    scope: &'static str,
    /// The set's name within its scope.
    name: &'static str,
    /// The patterns the create submits.
    patterns: Vec<&'static str>,
    /// The description the create submits, or `None` when it omits the member.
    description: Option<&'static str>,
}

/// A listing reports exactly the sets its scope holds, with each set's stored
/// members, and reports nothing about the sets of the other scope.
///
/// The rows differ along everything the rule generalises over: the SCOPE (both of
/// the two the model declares), the NUMBER of sets per scope (three REGIONAL, two
/// CLOUDFRONT), the set's NAME (including the same name in both scopes, which is
/// two different resources), the NUMBER and CONTENT of its patterns, and whether
/// the create carried a `Description` at all.
#[tokio::test]
async fn a_listing_reports_the_stored_sets_of_its_scope() {
    let stored = vec![
        StoredSet {
            case: "a regional set with one pattern and a description",
            scope: "REGIONAL",
            name: "alpha-set",
            patterns: vec!["^/admin"],
            description: Some("admin paths"),
        },
        StoredSet {
            case: "a regional set with three patterns and no description",
            scope: "REGIONAL",
            name: "bravo-set",
            patterns: vec!["^/a$", "^/b$", "^/c$"],
            description: None,
        },
        StoredSet {
            case: "a regional set whose name is also taken in the other scope",
            scope: "REGIONAL",
            name: "shared-name",
            patterns: vec![r"(?i)\bselect\b"],
            description: Some("regional shared"),
        },
        StoredSet {
            case: "a cloudfront set under the name the other scope also holds",
            scope: "CLOUDFRONT",
            name: "shared-name",
            patterns: vec!["^/global", r"\.js$"],
            description: Some("edge shared"),
        },
        StoredSet {
            case: "a cloudfront set with a non-ASCII pattern",
            scope: "CLOUDFRONT",
            name: "zulu-set",
            patterns: vec!["東京+", "🙈"],
            description: None,
        },
    ];

    let waf = Waf::new();
    // (scope, name) -> (id, arn, lock token), as the create answered them.
    let mut created: Vec<(&StoredSet, String, String, String)> = Vec::new();
    for set in &stored {
        let (id, arn, lock) =
            create_regex_pattern_set(&waf, set.scope, set.name, &set.patterns, set.description)
                .await;
        created.push((set, id, arn, lock));
    }

    for scope in ["REGIONAL", "CLOUDFRONT"] {
        let page = waf
            .ok("ListRegexPatternSets", json!({ "Scope": scope }))
            .await;

        // Only this scope's sets, in ascending name order: the sort key a marker
        // resumes from.
        let mut expected_names: Vec<String> = created
            .iter()
            .filter(|(set, ..)| set.scope == scope)
            .map(|(set, ..)| set.name.to_string())
            .collect();
        expected_names.sort();
        assert_eq!(
            listed_names(&page),
            expected_names,
            "case {scope:?}: a listing must report this scope's sets only, in name \
             order, got {page}"
        );

        for (set, id, arn, lock) in created.iter().filter(|(set, ..)| set.scope == scope) {
            let case = set.case;
            let summary = summaries(&page)
                .into_iter()
                .find(|summary| summary["Id"] == json!(id))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the listing must report the stored set, got {page}")
                });
            assert_eq!(
                summary["Name"],
                json!(set.name),
                "case {case:?}: expected the stored name, got {summary}"
            );
            assert_eq!(
                summary["ARN"],
                json!(arn),
                "case {case:?}: expected the stored ARN, got {summary}"
            );
            assert_eq!(
                summary["ARN"],
                json!(regex_pattern_set_arn(set.scope, set.name, id)),
                "case {case:?}: expected this scope's regexpatternset ARN, got {summary}"
            );
            assert_eq!(
                summary["LockToken"],
                json!(lock),
                "case {case:?}: expected the set's current lock token, got {summary}"
            );
            assert!(
                is_model_uuid(summary["Id"].as_str().unwrap_or_default()),
                "case {case:?}: every summary carries an Id of the modelled form, got \
                 {summary}"
            );
            // A description the create gave must be reported verbatim. A create
            // that gave none has nothing to report, so the member is absent or
            // empty — the summary never invents one.
            match set.description {
                Some(description) => assert_eq!(
                    summary["Description"],
                    json!(description),
                    "case {case:?}: expected the stored description, got {summary}"
                ),
                None => assert!(
                    summary["Description"].is_null()
                        || summary["Description"] == json!(""),
                    "case {case:?}: a set created without a description must report \
                     none, got {summary}"
                ),
            }

            // The listing and the by-id read answer the SAME stored resource.
            let got = get_regex_pattern_set(&waf, set.scope, set.name, id).await;
            assert_eq!(
                summary["Name"], got["RegexPatternSet"]["Name"],
                "case {case:?}: the summary must name the set Get answers, got {got}"
            );
            assert_eq!(
                summary["ARN"], got["RegexPatternSet"]["ARN"],
                "case {case:?}: the summary's ARN must be the stored one, got {got}"
            );
            assert_eq!(
                summary["LockToken"], got["LockToken"],
                "case {case:?}: the summary's lock token must be the stored one, got \
                 {got}"
            );
        }

        // A report changes nothing a later caller sees.
        let again = waf
            .ok("ListRegexPatternSets", json!({ "Scope": scope }))
            .await;
        assert_eq!(
            again, page,
            "case {scope:?}: listing twice must answer the same page, got {again}"
        );
    }
}

/// A scope holding no set at all answers an empty page and no marker — the
/// paginator's terminal page.
#[tokio::test]
async fn an_empty_scope_answers_an_empty_page_and_no_marker() {
    let waf = Waf::new();
    // One REGIONAL set, so the CLOUDFRONT listing is empty because that scope holds
    // nothing, not because the store does.
    create_regex_pattern_set(&waf, "REGIONAL", "only-regional", &["^/only$"], None).await;

    for (case, scope, expected) in [
        ("a scope holding one set", "REGIONAL", 1_usize),
        ("a scope holding none", "CLOUDFRONT", 0),
    ] {
        let page = waf
            .ok("ListRegexPatternSets", json!({ "Scope": scope }))
            .await;
        assert_eq!(
            summaries(&page).len(),
            expected,
            "case {case:?}: expected {expected} summaries, got {page}"
        );
        assert_eq!(
            page["NextMarker"].is_null(),
            expected == 0,
            "case {case:?}: only an empty page carries no NextMarker, got {page}"
        );
    }
}

/// Paging: at most `Limit` summaries per call, resumed with the `NextMarker` of the
/// previous page. WAF answers every non-empty page with a `NextMarker` — the name
/// of its last summary, whether or not the page is full — so the paginator's loop
/// ends on the empty page that follows the last one.
///
/// One row per page size, including a page size equal to and larger than the whole
/// collection, over a set count that is neither a multiple nor a divisor of most of
/// them.
#[tokio::test]
async fn paging_enumerates_every_stored_set_exactly_once() {
    let waf = Waf::new();
    let all_names: Vec<String> = (1..=5).map(|index| format!("set-{index}")).collect();
    for name in &all_names {
        create_regex_pattern_set(&waf, "REGIONAL", name, &["^/paged$"], None).await;
    }
    // A set in the other scope must never appear on a REGIONAL page.
    create_regex_pattern_set(&waf, "CLOUDFRONT", "set-0", &["^/edge$"], None).await;

    for limit in [1_usize, 2, 3, 5, 8] {
        let case = format!("Limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            pages += 1;
            assert!(
                pages <= all_names.len() + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut request = json!({ "Scope": "REGIONAL", "Limit": limit });
            if let Some(marker) = marker.as_deref() {
                request["NextMarker"] = json!(marker);
            }
            let page = waf.ok("ListRegexPatternSets", request).await;
            let names = listed_names(&page);
            let remaining = all_names.len() - seen.len();
            assert_eq!(
                names.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} must hold min(limit, remaining) summaries, \
                 got {page}"
            );
            marker = page["NextMarker"].as_str().map(str::to_string);
            assert_eq!(
                marker.as_deref(),
                names.last().map(String::as_str),
                "case {case:?}: page {pages} must carry the last summary's name as its \
                 NextMarker, got {page}"
            );
            seen.extend(names);
            if marker.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, all_names,
            "case {case:?}: paging must enumerate every stored set of the scope exactly \
             once, in order"
        );
        assert_eq!(
            pages,
            all_names.len().div_ceil(limit) + 1,
            "case {case:?}: expected ceil(5 / limit) full pages plus the empty page \
             that ends the loop, got {pages}"
        );
    }
}

/// A `Limit` outside the model's `PaginationLimit` range (`range 1..100`, per
/// `smithy_query(query="operation",operation="ListRegexPatternSets")`) is refused,
/// not clamped: a caller never receives a silently empty terminal page. One row per
/// published bound and one past each.
#[tokio::test]
async fn a_limit_outside_the_modelled_range_is_refused() {
    let waf = Waf::new();
    create_regex_pattern_set(&waf, "REGIONAL", "limit-set", &["^/limit$"], None).await;

    for limit in [1_i64, 100] {
        let page = waf
            .ok(
                "ListRegexPatternSets",
                json!({ "Scope": "REGIONAL", "Limit": limit }),
            )
            .await;
        assert_eq!(
            summaries(&page).len(),
            1,
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }

    for limit in [0_i64, -1, 101] {
        let (status, body) = waf
            .call(
                "ListRegexPatternSets",
                json!({ "Scope": "REGIONAL", "Limit": limit }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case Limit={limit}: a limit outside 1..100 must be refused, got status \
             {status} and body {body}"
        );
        assert!(
            body.is_object()
                && body["__type"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with("Exception"),
            "case Limit={limit}: a refusal must answer a parsed JSON envelope naming \
             a modelled exception, got {body}"
        );
        assert!(
            body["RegexPatternSets"].is_null(),
            "case Limit={limit}: a refusal must carry no page, got {body}"
        );
    }
}

/// `Scope` names one of the two namespaces the model's `Scope` enum declares
/// (`smithy_query(query="shape",shape="Scope")` → `CLOUDFRONT`, `REGIONAL`); any
/// other value names no namespace and is refused with no page.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    create_regex_pattern_set(&waf, "REGIONAL", "scoped-set", &["^/scoped$"], None).await;

    // A row whose scope carries JSON metacharacters and a newline: the refusal
    // body must still be a well-formed, parsed JSON envelope carrying the
    // modelled error code, so the value is encoded rather than spliced in.
    for scope in [
        "GLOBAL",
        "regional",
        "",
        "REGIONAL ",
        "REGIONAL\"},{\"x\":1",
        "back\\slash\nnewline",
    ] {
        let (status, body) = waf
            .call("ListRegexPatternSets", json!({ "Scope": scope }))
            .await;
        assert_eq!(
            status, 400,
            "case Scope={scope:?}: an undeclared scope must be refused, got status \
             {status} and body {body}"
        );
        assert!(
            body.is_object(),
            "case Scope={scope:?}: a refusal must answer a parsed JSON envelope, got \
             {body}"
        );
        assert!(
            body["__type"].as_str().unwrap_or_default().ends_with("Exception"),
            "case Scope={scope:?}: a refusal must name a modelled exception in \
             __type, got {body}"
        );
        assert!(
            body["RegexPatternSets"].is_null(),
            "case Scope={scope:?}: a refusal must carry no page, got {body}"
        );
    }

    // The refusals left the stored set listable.
    let page = waf
        .ok("ListRegexPatternSets", json!({ "Scope": "REGIONAL" }))
        .await;
    assert_eq!(
        listed_names(&page),
        vec!["scoped-set".to_string()],
        "a refused listing must leave the stored set listable, got {page}"
    );
}
