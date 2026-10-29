// SPDX-License-Identifier: CC0-1.0

//! `ListLifecyclePolicies` end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rules under test:
//!
//! 1. the listing reports the stored lifecycle policies of the `type` the request names, in
//!    one stable order, cutting at the page bound — its own documented default when the
//!    request names none — and handing back a `nextToken` only while records remain, with
//!    every summary member the STORED record's, so a write is visible in the listing and a
//!    delete is visible by its absence;
//! 2. `resources` narrows that listing to the policies whose rules name a resource the
//!    filter's own resource patterns overlap, `*` standing for any run of characters on
//!    either side, with every factor of that matching bounded before it runs.
//!
//! The tables span what those rules generalise over: the whole `LifecyclePolicyType` value
//! list, the retention window's unit and unlimited retention, the rule and resource count,
//! a described and an undescribed policy, the policy count, the page bound from 1 to past
//! the count and up to the published maximum as well as the default, exact / wildcard /
//! multi-entry / no-match filters, and each filter bound and one past it.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text};

/// The values `LifecyclePolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="LifecyclePolicyType")` -> `values`): the single
/// value `retention`. Every case below is driven over this whole list.
const LIFECYCLE_POLICY_TYPES: [&str; 1] = ["retention"];

/// The page bound this listing applies when the request names none, from
/// `ListLifecyclePoliciesRequest$maxResults`' own documentation ("The default is 10.").
const DEFAULT_PAGE_SIZE: usize = 10;

/// Create the lifecycle policy `body` describes, asserting it was accepted, and answer its
/// `lifecyclePolicyDetail`.
async fn create_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateLifecyclePolicy", body).await;
    assert_eq!(
        status, 200,
        "case CreateLifecyclePolicy({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "lifecyclePolicyDetail").clone()
}

/// One page of `ListLifecyclePolicies`: its summaries and the token that resumes after
/// them.
async fn list_page(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
    resources: Option<&[&str]>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut request = json!({ "type": kind });
    if let Some(resources) = resources {
        request["resources"] = json!(resources);
    }
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["nextToken"] = json!(next_token);
    }
    let case = request.to_string();
    let (status, code, answer) = call(router, "ListLifecyclePolicies", request).await;
    assert_eq!(
        status, 200,
        "case ListLifecyclePolicies({case}): expected 200, got {status} {code} with {answer}"
    );
    let summaries = at(&answer, "lifecyclePolicySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let token = at(&answer, "nextToken").as_str().map(str::to_string);
    (summaries, token)
}

/// Every summary `ListLifecyclePolicies` reports for `kind` under `resources`, across pages.
async fn list_all(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
    resources: Option<&[&str]>,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(router, kind, resources, Some(100), token.as_deref()).await;
        summaries.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return summaries,
        }
    }
}

/// The `name` of every summary in `summaries`, in the order the listing reported them.
fn names(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| text(summary, "name"))
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: the listing is the stored policies of one type, in one order, paged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_the_stored_policies_of_the_requested_type() {
    let router = emulator();

    // (name, policy document, description). The documents span a window in hours and in
    // days, unlimited retention, one rule and two, and one resource and several; the
    // descriptions span absent, empty and `PolicyDescription`'s own `@length(max: 1000)`.
    let long_description = "d".repeat(1000);
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "alpha-retention",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/alpha/*"],"MinIndexRetention":"24h"}]}"#,
            Some("the alpha retention policy"),
        ),
        (
            "beta-retention",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/beta/orders","index/beta/carts"],"MinIndexRetention":"7d"}]}"#,
            None,
        ),
        (
            "gamma-retention",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/gamma/*"],"NoMinIndexRetention":true}]}"#,
            Some(long_description.as_str()),
        ),
        (
            "delta-retention",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/delta/a"],"MinIndexRetention":"3650d"},{"ResourceType":"index","Resource":["index/delta/b"],"NoMinIndexRetention":true}]}"#,
            Some(""),
        ),
    ];

    // The summary a stored policy owes, built from the CREATE's own detail: the listing may
    // not report a member differently from the record every other reader answers.
    let mut expected: Vec<(String, Value)> = Vec::new();
    for kind in LIFECYCLE_POLICY_TYPES {
        for (name, policy, description) in &cases {
            let mut request = json!({ "type": kind, "name": name, "policy": policy });
            if let Some(description) = description {
                request["description"] = json!(description);
            }
            let detail = create_policy(&router, request).await;
            let mut summary = json!({
                "type": kind,
                "name": name,
                "policyVersion": text(&detail, "policyVersion"),
                "createdDate": at(&detail, "createdDate").clone(),
                "lastModifiedDate": at(&detail, "lastModifiedDate").clone(),
            });
            if let Some(description) = description {
                summary["description"] = json!(description);
            }
            expected.push(((*name).to_string(), summary));
        }
    }

    for kind in LIFECYCLE_POLICY_TYPES {
        // The order is the policy name ascending — the cursor a page token resumes after.
        let mut wanted: Vec<(String, Value)> = expected.clone();
        wanted.sort_by(|left, right| left.0.cmp(&right.0));
        let summaries = list_all(&router, kind, None).await;
        assert_eq!(
            names(&summaries),
            wanted
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<String>>(),
            "case ListLifecyclePolicies({kind}): the listing is every stored policy of the type in policy-name order, got {summaries:?}"
        );
        for (reported, (_, wanted)) in summaries.iter().zip(&wanted) {
            assert_eq!(
                reported, wanted,
                "case ListLifecyclePolicies({kind}): the summary must be the stored record's, expected {wanted}, got {reported}"
            );
            assert!(
                is_policy_version(&text(reported, "policyVersion")),
                "case ListLifecyclePolicies({kind}): policyVersion must satisfy PolicyVersion, got {reported}"
            );
            // `LifecyclePolicySummary` models no `policy` member, so a summary never
            // carries the document itself.
            assert!(
                reported.get("policy").is_none(),
                "case ListLifecyclePolicies({kind}): a summary carries no policy document, got {reported}"
            );
        }
    }

    // The page bound: 1, 2, exactly the count, one past it, and `maxResults`' own published
    // maximum of 100. Every bound walks the same ordered sequence, and a `nextToken` comes
    // back only while a record remains after the page.
    let all = names(&list_all(&router, "retention", None).await);
    for bound in [1_i64, 2, 4, 5, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) =
                list_page(&router, "retention", None, Some(bound), token.as_deref()).await;
            let bound_as_count = usize::try_from(bound).expect("the bound is positive");
            let remaining = all.len() - collected.len();
            let wanted = remaining.min(bound_as_count);
            assert_eq!(
                page.len(),
                wanted,
                "case ListLifecyclePolicies(maxResults={bound}, after {collected:?}): expected {wanted} summaries, got {page:?}"
            );
            collected.extend(names(&page));
            assert_eq!(
                next.is_some(),
                collected.len() < all.len(),
                "case ListLifecyclePolicies(maxResults={bound}, after {collected:?}): a nextToken comes back exactly while records remain, got {next:?}"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, all,
            "case ListLifecyclePolicies(maxResults={bound}): paging walks the whole listing in order, expected {all:?}, got {collected:?}"
        );
    }

    // A write is visible in the listing: the update re-mints the `policyVersion`, and the
    // listing reports the new one.
    let before = list_all(&router, "retention", None).await;
    let delta = before
        .iter()
        .find(|summary| text(summary, "name") == "delta-retention")
        .cloned()
        .expect("case UpdateLifecyclePolicy: the delta policy was created");
    let (status, code, updated) = call(
        &router,
        "UpdateLifecyclePolicy",
        json!({
            "type": "retention",
            "name": "delta-retention",
            "policyVersion": text(&delta, "policyVersion"),
            "description": "the rewritten delta retention policy",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateLifecyclePolicy(retention/delta-retention): expected 200, got {status} {code} with {updated}"
    );
    let detail = at(&updated, "lifecyclePolicyDetail").clone();
    let after = list_all(&router, "retention", None).await;
    let listed = after
        .iter()
        .find(|summary| text(summary, "name") == "delta-retention")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        (
            text(&listed, "policyVersion"),
            text(&listed, "description"),
            at(&listed, "lastModifiedDate").clone()
        ),
        (
            text(&detail, "policyVersion"),
            text(&detail, "description"),
            at(&detail, "lastModifiedDate").clone()
        ),
        "case ListLifecyclePolicies(retention, after update): the listing reports the written record, got {listed}"
    );

    // A delete is visible by its absence, and it leaves the other records alone.
    let (status, code, answer) = call(
        &router,
        "DeleteLifecyclePolicy",
        json!({ "type": "retention", "name": "beta-retention" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteLifecyclePolicy(retention/beta-retention): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        names(&list_all(&router, "retention", None).await),
        vec![
            "alpha-retention".to_string(),
            "delta-retention".to_string(),
            "gamma-retention".to_string(),
        ],
        "case ListLifecyclePolicies(retention, after delete): the deleted policy is gone and the others remain"
    );
}

#[tokio::test]
async fn the_listing_is_empty_for_a_type_with_no_stored_policies() {
    let router = emulator();

    // Every `LifecyclePolicyType` value, with nothing stored: an empty listing, never a
    // refusal and never a `nextToken`.
    for kind in LIFECYCLE_POLICY_TYPES {
        let (summaries, token) = list_page(&router, kind, None, None, None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListLifecyclePolicies({kind}, empty store): expected no summaries and no nextToken, got {summaries:?} and {token:?}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_page_bound_gets_this_listings_own_default() {
    let router = emulator();

    // One more policy than the documented default, so the default is what cuts the page.
    // The names sort in creation order, which is the order the listing walks.
    let stored = DEFAULT_PAGE_SIZE + 1;
    for index in 0..stored {
        create_policy(
            &router,
            json!({
                "type": "retention",
                "name": format!("dflt-{index:02}"),
                "policy": format!(
                    r#"{{"Rules":[{{"ResourceType":"index","Resource":["index/dflt{index}/*"],"MinIndexRetention":"24h"}}]}}"#
                ),
            }),
        )
        .await;
    }

    let (page, token) = list_page(&router, "retention", None, None, None).await;
    assert_eq!(
        page.len(),
        DEFAULT_PAGE_SIZE,
        "case ListLifecyclePolicies(no maxResults, {stored} stored): the default page bound is {DEFAULT_PAGE_SIZE}, got {page:?}"
    );
    let token = token.expect(
        "case ListLifecyclePolicies(no maxResults): a nextToken comes back while records remain",
    );
    let (rest, next) = list_page(&router, "retention", None, None, Some(&token)).await;
    assert_eq!(
        (rest.len(), next),
        (stored - DEFAULT_PAGE_SIZE, None),
        "case ListLifecyclePolicies(no maxResults, second page): expected the remaining {} summaries and no nextToken, got {rest:?}",
        stored - DEFAULT_PAGE_SIZE
    );
}

// ---------------------------------------------------------------------------
// Rule 2: `resources` narrows the listing to the policies that apply to one of the
// resources the filter names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_resources_filter_narrows_the_listing_to_the_policies_that_apply() {
    let router = emulator();

    // The resource form a retention rule names, from AWS's "Amazon OpenSearch Serverless
    // data lifecycle policies": `index/<collection>/<index>`, with `*` standing for any run
    // of characters. Row 4 names every index; row 5 is two rules naming two different
    // collections.
    let policies: Vec<(&str, &str)> = vec![
        (
            "lp-exact",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/app-2026"],"MinIndexRetention":"24h"}]}"#,
        ),
        (
            "lp-other",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/metrics/cpu"],"MinIndexRetention":"30d"}]}"#,
        ),
        (
            "lp-prefix",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/app-*"],"MinIndexRetention":"7d"}]}"#,
        ),
        (
            "lp-all",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/*"],"NoMinIndexRetention":true}]}"#,
        ),
        (
            "lp-pair",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/orders"],"MinIndexRetention":"48h"},{"ResourceType":"index","Resource":["index/metrics/mem"],"NoMinIndexRetention":true}]}"#,
        ),
    ];
    for (name, policy) in &policies {
        create_policy(
            &router,
            json!({ "type": "retention", "name": name, "policy": policy }),
        )
        .await;
    }

    // (filter, the policies that apply, in policy-name order). The rows span an exact
    // resource meeting a wildcard rule, a wildcard filter meeting exact rules, two filter
    // entries (the union of what each selects), the two-rule document and a filter nothing
    // applies to.
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["index/logs-prod/app-2026"],
            vec!["lp-all", "lp-exact", "lp-prefix"],
        ),
        (vec!["index/logs-prod/app-2027"], vec!["lp-all", "lp-prefix"]),
        (
            vec!["index/*"],
            vec!["lp-all", "lp-exact", "lp-other", "lp-pair", "lp-prefix"],
        ),
        (vec!["index/metrics/*"], vec!["lp-all", "lp-other", "lp-pair"]),
        (
            vec!["index/metrics/cpu", "index/logs-prod/orders"],
            vec!["lp-all", "lp-other", "lp-pair"],
        ),
        (vec!["collection/logs-prod"], vec![]),
    ];
    for (filter, wanted) in &cases {
        let summaries = list_all(&router, "retention", Some(filter)).await;
        assert_eq!(
            names(&summaries),
            wanted
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case ListLifecyclePolicies(retention, resources={filter:?}): expected {wanted:?}, got {summaries:?}"
        );
    }

    // An absent filter narrows nothing: every stored policy of the type is reported.
    assert_eq!(
        names(&list_all(&router, "retention", None).await),
        vec![
            "lp-all".to_string(),
            "lp-exact".to_string(),
            "lp-other".to_string(),
            "lp-pair".to_string(),
            "lp-prefix".to_string(),
        ],
        "case ListLifecyclePolicies(retention, no filter): every stored policy of the type is reported"
    );

    // The filter composes with the page bound: the same narrowed sequence, one summary at a
    // time.
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(
            &router,
            "retention",
            Some(&["index/metrics/*"]),
            Some(1),
            token.as_deref(),
        )
        .await;
        collected.extend(names(&page));
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        collected,
        vec![
            "lp-all".to_string(),
            "lp-other".to_string(),
            "lp-pair".to_string()
        ],
        "case ListLifecyclePolicies(retention, resources=[index/metrics/*], maxResults=1): paging walks the narrowed listing in order, got {collected:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusals the listing owes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_refuses_what_it_must() {
    let router = emulator();

    // `maxResults` is `@range(min: 1, max: 100)` and `type` is required and drawn from
    // `LifecyclePolicyType`; `resources` is `@length(min: 1, max: 1000)`; a `nextToken` this
    // service did not issue is not a pagination token.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "type outside LifecyclePolicyType",
            json!({ "type": "encryption" }),
        ),
        ("type in the wrong case", json!({ "type": "Retention" })),
        ("type absent", json!({})),
        (
            "maxResults one below the 1 minimum",
            json!({ "type": "retention", "maxResults": 0 }),
        ),
        (
            "maxResults one past the 100 maximum",
            json!({ "type": "retention", "maxResults": 101 }),
        ),
        (
            "resources shorter than its 1-entry minimum",
            json!({ "type": "retention", "resources": [] }),
        ),
        (
            "nextToken this service did not issue",
            json!({ "type": "retention", "nextToken": "not-a-token" }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "ListLifecyclePolicies", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case ListLifecyclePolicies({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // `maxResults` at each published bound is accepted.
    for bound in [1_i64, 100] {
        let (summaries, token) = list_page(&router, "retention", None, Some(bound), None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListLifecyclePolicies(maxResults={bound}, empty store): expected no summaries, got {summaries:?} and {token:?}"
        );
    }
}

#[tokio::test]
async fn the_resources_filter_is_bounded_in_what_it_evaluates() {
    let router = emulator();

    // One stored policy, so the filter has something to be matched against.
    create_policy(
        &router,
        json!({
            "type": "retention",
            "name": "bounded-retention",
            "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/app-500"],"MinIndexRetention":"24h"}]}"#,
        }),
    )
    .await;

    // MAX_RESOURCE_FILTER_ENTRIES = 1000 (also `resources`' modelled maximum) and
    // MAX_RESOURCE_PATTERN_LENGTH = 1024: a filter AT each bound is evaluated, and one past
    // it is the modelled refusal rather than unbounded work.
    let at_entry_bound: Vec<String> = (0..1000)
        .map(|index| format!("index/logs-prod/app-{index}"))
        .collect();
    let at_length_bound = format!("index/logs-prod/{}*", "a".repeat(1007));
    assert_eq!(
        at_length_bound.len(),
        1024,
        "case ListLifecyclePolicies(a resources entry at its length bound): the case entry is 1024 characters, got {}",
        at_length_bound.len()
    );
    for (case, filter, expected) in [
        ("1000 entries, its maximum", at_entry_bound.clone(), true),
        (
            "an entry of 1024 characters",
            vec![at_length_bound.clone()],
            false,
        ),
    ] {
        let borrowed: Vec<&str> = filter.iter().map(String::as_str).collect();
        let (summaries, token) = list_page(&router, "retention", Some(&borrowed), None, None).await;
        assert!(
            token.is_none(),
            "case ListLifecyclePolicies(resources with {case}): expected no nextToken, got {token:?}"
        );
        // The 1000-entry filter names `index/logs-prod/app-0..app-999`, which the stored
        // policy's own entry is among; the long entry names nothing stored.
        assert_eq!(
            !summaries.is_empty(),
            expected,
            "case ListLifecyclePolicies(resources with {case}): expected the policy to be reported={expected}, got {summaries:?}"
        );
    }

    let mut past_entry_bound = at_entry_bound;
    past_entry_bound.push("index/logs-prod/one-too-many".to_string());
    let refusals: Vec<(&str, Vec<String>)> = vec![
        ("1001 entries, one past its maximum", past_entry_bound),
        (
            "an entry of 1025 characters, one past its maximum",
            vec![format!("{at_length_bound}a")],
        ),
        (
            // MAX_RESOURCE_FILTER_WORK: every entry is inside the length bound, but each
            // carries 512 wildcards, so matching them against the stored policy's own
            // resources is past the work this listing performs. The refusal comes instead
            // of the work, not after it.
            "1000 entries of 512 wildcards each, past the matching budget",
            (0..1000).map(|_| "*a".repeat(512)).collect(),
        ),
    ];
    for (case, filter) in refusals {
        let (status, code, answer) = call(
            &router,
            "ListLifecyclePolicies",
            json!({ "type": "retention", "resources": filter }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case ListLifecyclePolicies(resources with {case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }
}
