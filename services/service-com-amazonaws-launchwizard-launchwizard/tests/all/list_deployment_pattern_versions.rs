// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDeploymentPatternVersions`: the paged, filterable view of
//! the versions ONE deployment pattern of the seeded catalogue publishes, driven over
//! HTTP through the same router the production emulator serves with (the harness in
//! `tests/all/harness.rs`).
//!
//! Every assertion is about a RULE rather than one recorded call, so each runs over a
//! table:
//!
//! * the page a request with no filter answers is EVERY version the pattern publishes,
//!   in catalogue order, each reading exactly as `GetDeploymentPatternVersion` reports
//!   it — over every workload and every pattern of each;
//! * a page carries at most `maxResults` items and a token resumes after the one the
//!   previous page ended on, over several page sizes (one, several, the whole set and
//!   past it);
//! * `updateFromVersion` selects the versions an update can move a deployment onto from
//!   the version it names — one row per published version, a row naming several
//!   (OR-joined), a row naming a version the pattern does not publish, and a mixed row;
//! * a filter that names no value, a filter name outside the modelled enum and a filter
//!   value outside the modelled value shape are each refused, with the clause the
//!   refusal states;
//! * an absent workload or pattern is a 404 whose message echoes nothing of the request.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES;
use service_com_amazonaws_launchwizard_launchwizard::state::{
    CATALOGUE_PATTERN_VERSIONS, INVALID_NEXT_TOKEN_MESSAGE,
};

use crate::deployment_pattern_versions::get_version;
use crate::deployment_update::{error_message, every_pattern, one_pattern_per_workload};
use crate::harness::{walk_pages, Harness};
use crate::list_deployments_filter_pairs::{empty_clause, null_clause, report};

/// The route the model binds `ListDeploymentPatternVersions` to:
/// `POST /listDeploymentPatternVersions`.
const PATH: &str = "/listDeploymentPatternVersions";

/// The member a page of versions travels on.
const ITEMS: &str = "deploymentPatternVersions";

/// The wire error code and HTTP status a refused request carries
/// (`ValidationException`, `errorKind: client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// A `ListDeploymentPatternVersions` request body naming only the two required members.
fn list_body(workload: &str, pattern: &str) -> Value {
    json!({
        "workloadName": workload,
        "deploymentPatternName": pattern,
    })
}

/// `body` with `member` set to `value`.
fn with(body: &Value, member: &str, value: Value) -> Value {
    let mut object = body
        .as_object()
        .expect("a request body is an object")
        .clone();
    object.insert(member.to_string(), value);
    Value::Object(object)
}

/// The summaries one request reports, requiring HTTP 200.
async fn listed(harness: &Harness, case: &str, body: Value) -> Vec<Value> {
    let reply = harness.ok(case, PATH, body).await;
    items(case, &reply)
}

/// The summaries a response body carries.
fn items(case: &str, reply: &Value) -> Vec<Value> {
    reply[ITEMS]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {PATH} reported no {ITEMS}: {reply}"))
        .clone()
}

/// The `deploymentPatternVersionName`s of `summaries`, in the order they were reported.
fn names(case: &str, summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["deploymentPatternVersionName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a reported version carries no name: {summary}")
                })
                .to_string()
        })
        .collect()
}

/// Every version the catalogue publishes per pattern, as a set.
fn published() -> BTreeSet<String> {
    CATALOGUE_PATTERN_VERSIONS
        .iter()
        .map(|version| (*version).to_string())
        .collect()
}

/// One `updateFromVersion` filter over `values`.
fn update_from(values: &[&str]) -> Value {
    json!([{"name": "updateFromVersion", "values": values}])
}

#[test]
fn the_filter_key_and_version_set_this_suite_drives_are_the_models() {
    // The one `DeploymentPatternVersionFilterKey` member the model declares — the
    // vocabulary the rows below use, read off the generated enum rather than written
    // out here.
    assert_eq!(
        DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES,
        &["updateFromVersion"],
        "the model declares one DeploymentPatternVersionFilterKey value, got \
         {DEPLOYMENT_PATTERN_VERSION_FILTER_KEY_VALUES:?}"
    );
    // Three published versions is what makes "all but the one named" a narrowing rather
    // than an empty page, so the filter rows below assert something.
    assert!(
        CATALOGUE_PATTERN_VERSIONS.len() >= 3,
        "the catalogue must publish enough versions per pattern to span the rule, got \
         {CATALOGUE_PATTERN_VERSIONS:?}"
    );
}

#[tokio::test]
async fn an_unfiltered_page_reports_every_published_version_as_the_version_read_does() {
    let harness = Harness::new();
    for (workload, pattern) in every_pattern(&harness).await {
        let case = format!("{workload}/{pattern}");
        let summaries = listed(&harness, &case, list_body(&workload, &pattern)).await;
        let got = names(&case, &summaries);
        assert_eq!(
            got,
            CATALOGUE_PATTERN_VERSIONS.to_vec(),
            "case {case:?}: expected every published version in catalogue order, got {got:?}"
        );

        let mut descriptions: BTreeSet<String> = BTreeSet::new();
        for summary in &summaries {
            let version = summary["deploymentPatternVersionName"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let case = format!("{case}@{version}");
            // The two views read the SAME record, so a listed version reads exactly as
            // the single-version read reports it.
            let read = get_version(&harness, &case, &workload, &pattern, &version).await;
            assert_eq!(
                summary, &read,
                "case {case:?}: the listed version must read as GetDeploymentPatternVersion \
                 reports it, got {summary}"
            );
            assert_eq!(
                summary["workloadName"],
                json!(workload),
                "case {case:?}: expected workloadName {workload:?}, got {}",
                summary["workloadName"]
            );
            assert_eq!(
                summary["deploymentPatternName"],
                json!(pattern),
                "case {case:?}: expected deploymentPatternName {pattern:?}, got {}",
                summary["deploymentPatternName"]
            );
            descriptions.insert(
                summary["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        // Each version describes ITSELF, so the page tells the versions apart rather
        // than repeating one canned record.
        assert_eq!(
            descriptions.len(),
            CATALOGUE_PATTERN_VERSIONS.len(),
            "case {case:?}: each published version must carry its own description, got \
             {descriptions:?}"
        );

        // The whole set fits one default-sized page, so no token is handed back, and a
        // read changes nothing a later caller sees.
        let reply = harness
            .ok(&case, PATH, list_body(&workload, &pattern))
            .await;
        assert!(
            reply.get("nextToken").is_none(),
            "case {case:?}: a page holding the whole set must hand back no token, got {reply}"
        );
        assert_eq!(
            names(&case, &listed(&harness, &case, list_body(&workload, &pattern)).await),
            got,
            "case {case:?}: listing versions must not change what a later caller sees"
        );
    }
}

#[tokio::test]
async fn a_page_holds_at_most_max_results_and_a_token_resumes_after_it() {
    let harness = Harness::new();
    let total = CATALOGUE_PATTERN_VERSIONS.len();
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        let base = list_body(&workload, &pattern);
        // Rows differ along the page size the rule generalises over: one at a time,
        // several, exactly the whole set, and past it.
        for page_size in [1usize, 2, total, total + 1] {
            let case = format!("{workload}/{pattern} at maxResults {page_size}");
            let walked = walk_pages(&harness, &case, PATH, &base, ITEMS, page_size).await;
            let got = names(&case, &walked);
            assert_eq!(
                got,
                CATALOGUE_PATTERN_VERSIONS.to_vec(),
                "case {case:?}: paging must report every published version once, in catalogue \
                 order, got {got:?}"
            );
        }

        // The first page of one item names the version the pattern currently deploys —
        // the newest one, which the catalogue publishes first — and carries a token; the
        // page that token resumes names the next one.
        let case = format!("{workload}/{pattern} first page of one");
        let first = harness
            .ok(&case, PATH, with(&base, "maxResults", json!(1)))
            .await;
        let page = names(&case, &items(&case, &first));
        assert_eq!(
            page,
            vec![CATALOGUE_PATTERN_VERSIONS[0].to_string()],
            "case {case:?}: the first page must name the newest published version, got {page:?}"
        );
        let token = first["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a short page handed back no token: {first}"))
            .to_string();
        let case = format!("{workload}/{pattern} resumed");
        let resumed = listed(
            &harness,
            &case,
            with(
                &with(&base, "maxResults", json!(1)),
                "nextToken",
                json!(token),
            ),
        )
        .await;
        let got = names(&case, &resumed);
        assert_eq!(
            got,
            vec![CATALOGUE_PATTERN_VERSIONS[1].to_string()],
            "case {case:?}: a token must resume after the version the previous page ended on, \
             got {got:?}"
        );

        // A token this service never issued is refused, and the refusal states neither
        // the token nor which way it was unusable.
        for bad in [
            // Not a token this service ever issued, a token whose cursor was never
            // encoded, and a well-formed token naming nothing in this result set.
            "notatokenatall".to_string(),
            "v1:1.0.0".to_string(),
            ::hex::encode("v1:9.9.9"),
        ] {
            let case = format!("{workload}/{pattern} with token {bad:?}");
            let body = harness
                .refused(
                    &case,
                    PATH,
                    with(&base, "nextToken", json!(bad)),
                    STATUS,
                    CODE,
                )
                .await;
            let message = error_message(&body);
            assert_eq!(
                message, INVALID_NEXT_TOKEN_MESSAGE,
                "case {case:?}: expected the fixed pagination refusal, got {message:?}"
            );
        }

        // `maxResults` is @range(min: 1, max: 100), so one past each bound is refused at
        // the boundary rather than answered with a page.
        for outside in [0, 101] {
            let case = format!("{workload}/{pattern} at maxResults {outside}");
            harness
                .refused(
                    &case,
                    PATH,
                    with(&base, "maxResults", json!(outside)),
                    STATUS,
                    CODE,
                )
                .await;
        }
        // Both published bounds are inside the shape and answer a page.
        for inside in [1, 100] {
            let case = format!("{workload}/{pattern} at maxResults {inside}");
            let got = names(
                &case,
                &listed(&harness, &case, with(&base, "maxResults", json!(inside))).await,
            );
            assert!(
                !got.is_empty() && got.len() <= inside.min(total),
                "case {case:?}: a page at a published bound must hold at most {inside} items, \
                 got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn update_from_version_reports_the_versions_an_update_can_move_onto() {
    let harness = Harness::new();
    let whole = published();
    let newest = CATALOGUE_PATTERN_VERSIONS[0];
    let oldest = CATALOGUE_PATTERN_VERSIONS[CATALOGUE_PATTERN_VERSIONS.len() - 1];

    // One row per shape the rule generalises over: each published starting version,
    // several starting versions at once (OR-joined), a starting version the pattern does
    // not publish, and a mix of the two.
    let mut rows: Vec<(String, Vec<&str>, BTreeSet<String>)> = Vec::new();
    for version in CATALOGUE_PATTERN_VERSIONS {
        let mut expected = whole.clone();
        expected.remove(*version);
        rows.push((
            format!("from the published version {version}"),
            vec![version],
            expected,
        ));
    }
    rows.push((
        "from the oldest or the newest, OR-joined".to_string(),
        vec![oldest, newest],
        whole.clone(),
    ));
    rows.push((
        "from a version the pattern does not publish".to_string(),
        vec!["9.9.9"],
        BTreeSet::new(),
    ));
    rows.push((
        "from a published version or an unpublished one".to_string(),
        vec![oldest, "9.9.9"],
        whole.iter().filter(|v| *v != oldest).cloned().collect(),
    ));

    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        let base = list_body(&workload, &pattern);
        for (label, values, expected) in &rows {
            let case = format!("{workload}/{pattern}: {label}");
            let summaries = listed(
                &harness,
                &case,
                with(&base, "filters", update_from(values)),
            )
            .await;
            let got: BTreeSet<String> = names(&case, &summaries).into_iter().collect();
            assert_eq!(
                &got, expected,
                "case {case:?}: expected the versions an update can move onto {expected:?}, got \
                 {got:?}"
            );
            // Every version a filtered page reports is a version of this pattern the
            // single-version read resolves, so the filter narrows the published set
            // rather than inventing entries.
            for version in &got {
                get_version(&harness, &case, &workload, &pattern, version).await;
            }
            // A filter never answers a version it was asked to start FROM, and a single
            // starting version really narrows: the page is smaller than the whole set.
            if values.len() == 1 && whole.contains(values[0]) {
                assert!(
                    got.len() < whole.len(),
                    "case {case:?}: one starting version must narrow the published set \
                     {whole:?}, got {got:?}"
                );
            }
        }

        // Two filters both have to hold, so naming every published version as a starting
        // version twice over leaves only what both select.
        let case = format!("{workload}/{pattern}: two filters, both narrowing");
        let both = json!([
            {"name": "updateFromVersion", "values": [oldest]},
            {"name": "updateFromVersion", "values": [newest]},
        ]);
        let got: BTreeSet<String> =
            names(&case, &listed(&harness, &case, with(&base, "filters", both)).await)
                .into_iter()
                .collect();
        let expected: BTreeSet<String> = whole
            .iter()
            .filter(|version| *version != oldest && *version != newest)
            .cloned()
            .collect();
        assert_eq!(
            &got, &expected,
            "case {case:?}: expected what both filters select {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_that_is_not_a_complete_pair_of_modelled_halves_is_refused() {
    let harness = Harness::new();
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        let base = list_body(&workload, &pattern);
        // Rows differ along WHICH half is unusable, HOW it is unusable, and the POSITION
        // of the offending filter in the list.
        let rows: Vec<(&str, Value, String)> = vec![
            (
                "a filter naming the key with an empty values list",
                update_from(&[]),
                report(&[empty_clause("filters.1.member.values")]),
            ),
            (
                "a filter with no values member at all",
                json!([{"name": "updateFromVersion"}]),
                report(&[null_clause("filters.1.member.values")]),
            ),
            (
                "a filter carrying values but no name",
                json!([{"values": ["1.0.0"]}]),
                report(&[null_clause("filters.1.member.name")]),
            ),
            (
                // Both halves are `@required`, and the boundary walk refuses the first
                // null member it reaches — so a filter carrying neither half is refused
                // for its name.
                "a filter carrying neither half",
                json!([{}]),
                report(&[null_clause("filters.1.member.name")]),
            ),
            (
                "the degenerate filter is the second of two",
                json!([
                    {"name": "updateFromVersion", "values": ["1.0.0"]},
                    {"name": "updateFromVersion", "values": []},
                ]),
                report(&[empty_clause("filters.2.member.values")]),
            ),
            (
                // The name's VOCABULARY is judged at the protocol boundary, from the
                // model's own enum (`DeploymentPatternVersionFilterKey`), so a name
                // outside it is refused for the name.
                "a filter name outside the modelled enum",
                json!([{"name": "WORKLOAD_NAME", "values": ["1.0.0"]}]),
                report(&[
                    "Value at 'filters.1.member.name' failed to satisfy constraint: Member must \
                     satisfy enum value set: [updateFromVersion]"
                        .to_string(),
                ]),
            ),
        ];

        for (label, filters, expected) in rows {
            let case = format!("{workload}/{pattern}: {label}");
            let body = harness
                .refused(
                    &case,
                    PATH,
                    with(&base, "filters", filters),
                    STATUS,
                    CODE,
                )
                .await;
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
            assert!(
                body.get(ITEMS).is_none(),
                "case {case:?}: a refused request must report no versions, got {body}"
            );
        }

        // A filter VALUE is a version number
        // (`DeploymentPatternVersionFilterValue`: @length(min: 1, max: 100) @pattern
        // ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$), so a value outside that shape is
        // refused at the boundary while one inside it reaches the handler.
        for outside in [
            "1.0".to_string(),
            "1.0.0.0".to_string(),
            "01.0.0".to_string(),
            "v1.0.0".to_string(),
            String::new(),
            // One past the published length ceiling of 100.
            format!("1.0.{}", "1".repeat(97)),
        ] {
            let case = format!("{workload}/{pattern}: filter value {outside:?}");
            harness
                .refused(
                    &case,
                    PATH,
                    with(&base, "filters", update_from(&[&outside])),
                    STATUS,
                    CODE,
                )
                .await;
        }
        for inside in [
            "9.9.9".to_string(),
            "0.0.0".to_string(),
            "10.20.30".to_string(),
            // The published length ceiling itself.
            format!("1.0.{}", "1".repeat(96)),
        ] {
            let case = format!("{workload}/{pattern}: filter value {inside:?}");
            let got = listed(
                &harness,
                &case,
                with(&base, "filters", update_from(&[&inside])),
            )
            .await;
            // None of these is a version the catalogue publishes, so each selects
            // nothing rather than widening the page to every version.
            assert!(
                got.is_empty(),
                "case {case:?}: a starting version the pattern does not publish must select \
                 nothing, got {} versions",
                got.len()
            );
        }
    }
}

#[tokio::test]
async fn an_absent_workload_or_pattern_is_a_404_that_echoes_nothing() {
    let harness = Harness::new();
    for (workload, pattern) in one_pattern_per_workload(&harness).await {
        // One row per way the request can name something the catalogue does not hold,
        // each also driven with a complete filter — a request that names nothing real is
        // a not-found whether or not it narrows.
        let rows: Vec<(&str, String, String)> = vec![
            (
                "unknown workload",
                "NoSuchWorkloadXyz".to_string(),
                pattern.clone(),
            ),
            (
                "workload that looks like a known one",
                format!("{workload}x"),
                pattern.clone(),
            ),
            (
                "unknown pattern",
                workload.clone(),
                "NoSuchPattern".to_string(),
            ),
            (
                "pattern that looks like a known one",
                workload.clone(),
                format!("{pattern}X"),
            ),
        ];
        for (label, asked_workload, asked_pattern) in rows {
            for filters in [None, Some(update_from(&[CATALOGUE_PATTERN_VERSIONS[0]]))] {
                let case = format!(
                    "{label}: {asked_workload}/{asked_pattern}{}",
                    if filters.is_some() { " filtered" } else { "" }
                );
                let mut request = list_body(&asked_workload, &asked_pattern);
                if let Some(filters) = filters {
                    request = with(&request, "filters", filters);
                }
                let body = harness
                    .refused(&case, PATH, request, 404, "ResourceNotFoundException")
                    .await;
                let message = error_message(&body);
                assert!(
                    !message.is_empty(),
                    "case {case:?}: a not-found must carry a message, got {body}"
                );
                // The message states WHICH KIND of resource was not found and never the
                // caller's own input, so the catalogue cannot be probed through the
                // error channel.
                for asked in [&asked_workload, &asked_pattern] {
                    assert!(
                        !message.contains(asked.as_str()),
                        "case {case:?}: the not-found message must not echo {asked:?}, got \
                         {message:?}"
                    );
                }
            }
        }

        // Every member the model marks required: a request that omits one is refused at
        // the boundary rather than answered with a page.
        for omitted in ["workloadName", "deploymentPatternName"] {
            let mut object = list_body(&workload, &pattern)
                .as_object()
                .expect("a request body is an object")
                .clone();
            object.remove(omitted);
            let case = format!("{workload}/{pattern} without {omitted}");
            harness
                .refused(&case, PATH, Value::Object(object), STATUS, CODE)
                .await;
        }
    }
}
