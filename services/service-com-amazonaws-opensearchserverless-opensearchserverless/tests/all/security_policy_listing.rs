// SPDX-License-Identifier: CC0-1.0

//! `ListSecurityPolicies` end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rules under test:
//!
//! 1. the listing reports the stored security policies of the `type` the request names, in
//!    one stable order, cutting at the page bound and handing back a `nextToken` only while
//!    records remain — and every summary member is the STORED record's, so a write is
//!    visible in the listing and a delete is visible by its absence;
//! 2. `resource` narrows that listing to the policies whose rules name a resource the
//!    filter's own resource patterns overlap, `*` standing for any run of characters on
//!    either side, with every factor of that matching bounded before it runs.
//!
//! The tables span what those rules generalise over: both `SecurityPolicyType` values, the
//! object and list document forms, the policy count, the page bound from 1 to past the
//! count and up to the published maximum, exact/wildcard/multi-entry/no-match filters over
//! collection and index resources, and each filter bound and one past it.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text};

/// Create the security policy `body` describes, asserting it was accepted, and answer its
/// `securityPolicyDetail`.
async fn create_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateSecurityPolicy", body).await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "securityPolicyDetail").clone()
}

/// One page of `ListSecurityPolicies`: its summaries and the token that resumes after them.
async fn list_page(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
    resource: Option<&[&str]>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut request = json!({ "type": kind });
    if let Some(resource) = resource {
        request["resource"] = json!(resource);
    }
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["nextToken"] = json!(next_token);
    }
    let case = request.to_string();
    let (status, code, answer) = call(router, "ListSecurityPolicies", request).await;
    assert_eq!(
        status, 200,
        "case ListSecurityPolicies({case}): expected 200, got {status} {code} with {answer}"
    );
    let summaries = at(&answer, "securityPolicySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let token = at(&answer, "nextToken").as_str().map(str::to_string);
    (summaries, token)
}

/// Every summary `ListSecurityPolicies` reports for `kind` under `resource`, across pages.
async fn list_all(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
    resource: Option<&[&str]>,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(router, kind, resource, Some(100), token.as_deref()).await;
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

    // `SecurityPolicyType` = encryption | network (smithy_query shape values). The
    // documents span an object with one rule, an object with a nested list and a boolean,
    // and a bare JSON array of rule-carrying objects; row 4 reuses row 1's NAME under the
    // other type, which is a different policy; row 3's description sits at
    // `PolicyDescription`'s own `@length(max: 1000)`.
    let long_description = "d".repeat(1000);
    let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "encryption",
            "alpha-policy",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/alpha"]}],"AWSOwnedKey":true}"#,
            Some("the alpha encryption policy"),
        ),
        (
            "encryption",
            "beta-policy",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/beta","collection/beta-two"]}],"AWSOwnedKey":true}"#,
            None,
        ),
        (
            "encryption",
            "gamma-policy",
            r#"[{"Rules":[{"ResourceType":"collection","Resource":["collection/gamma"]}],"AllowFromPublic":true}]"#,
            Some(long_description.as_str()),
        ),
        (
            "network",
            "alpha-policy",
            r#"[{"Rules":[{"ResourceType":"dashboard","Resource":["collection/alpha"]}],"AllowFromPublic":false,"SourceVPCEs":[]}]"#,
            None,
        ),
        (
            "network",
            "delta-policy",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/delta"]}],"AllowFromPublic":true}"#,
            Some("the delta network policy"),
        ),
    ];

    // The summary a stored policy owes, built from the CREATE's own detail: the listing may
    // not report a member differently from the record every other reader answers.
    let mut expected: Vec<(String, String, Value)> = Vec::new();
    for (kind, name, policy, description) in &cases {
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
        expected.push(((*kind).to_string(), (*name).to_string(), summary));
    }

    for kind in ["encryption", "network"] {
        let wanted: Vec<&Value> = expected
            .iter()
            .filter(|(policy_kind, _, _)| policy_kind == kind)
            .map(|(_, _, summary)| summary)
            .collect();
        let summaries = list_all(&router, kind, None).await;
        assert_eq!(
            summaries.len(),
            wanted.len(),
            "case ListSecurityPolicies({kind}): expected {} summaries, got {summaries:?}",
            wanted.len()
        );
        for (reported, wanted) in summaries.iter().zip(&wanted) {
            assert_eq!(
                reported, *wanted,
                "case ListSecurityPolicies({kind}): the summary must be the stored record's, expected {wanted}, got {reported}"
            );
            assert!(
                is_policy_version(&text(reported, "policyVersion")),
                "case ListSecurityPolicies({kind}): policyVersion must satisfy PolicyVersion, got {reported}"
            );
            // `SecurityPolicySummary` models no `policy` member, so a summary never
            // carries the document itself.
            assert!(
                reported.get("policy").is_none(),
                "case ListSecurityPolicies({kind}): a summary carries no policy document, got {reported}"
            );
        }
        // The order is the policy name ascending — the cursor a page token resumes after.
        let mut sorted = names(&summaries);
        sorted.sort();
        assert_eq!(
            names(&summaries),
            sorted,
            "case ListSecurityPolicies({kind}): the listing is in policy-name order, got {summaries:?}"
        );
    }

    // The page bound: 1, 2, exactly the count, one past it, and `maxResults`' own
    // published maximum of 100. Every bound walks the same ordered sequence, and a
    // `nextToken` comes back only while a record remains after the page.
    let all_encryption = names(&list_all(&router, "encryption", None).await);
    for bound in [1_i64, 2, 3, 4, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) =
                list_page(&router, "encryption", None, Some(bound), token.as_deref()).await;
            assert!(
                page.len() <= usize::try_from(bound).expect("the bound is positive"),
                "case ListSecurityPolicies(maxResults={bound}): a page holds at most {bound} summaries, got {page:?}"
            );
            let remaining = all_encryption.len() - collected.len();
            let wanted = remaining.min(usize::try_from(bound).expect("the bound is positive"));
            assert_eq!(
                page.len(),
                wanted,
                "case ListSecurityPolicies(maxResults={bound}, after {collected:?}): expected {wanted} summaries, got {page:?}"
            );
            collected.extend(names(&page));
            assert_eq!(
                next.is_some(),
                collected.len() < all_encryption.len(),
                "case ListSecurityPolicies(maxResults={bound}, after {collected:?}): a nextToken comes back exactly while records remain, got {next:?}"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, all_encryption,
            "case ListSecurityPolicies(maxResults={bound}): paging walks the whole listing in order, expected {all_encryption:?}, got {collected:?}"
        );
    }

    // A write is visible in the listing: the update re-mints the `policyVersion`, and the
    // listing reports the new one.
    let before = list_all(&router, "network", None).await;
    let delta = before
        .iter()
        .find(|summary| text(summary, "name") == "delta-policy")
        .cloned()
        .expect("case UpdateSecurityPolicy: the delta policy was created");
    let (status, code, updated) = call(
        &router,
        "UpdateSecurityPolicy",
        json!({
            "type": "network",
            "name": "delta-policy",
            "policyVersion": text(&delta, "policyVersion"),
            "description": "the rewritten delta network policy",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateSecurityPolicy(network/delta-policy): expected 200, got {status} {code} with {updated}"
    );
    let detail = at(&updated, "securityPolicyDetail").clone();
    let after = list_all(&router, "network", None).await;
    let listed = after
        .iter()
        .find(|summary| text(summary, "name") == "delta-policy")
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
        "case ListSecurityPolicies(network, after update): the listing reports the written record, got {listed}"
    );

    // A delete is visible by its absence, and it leaves the other type alone even when the
    // names coincide.
    let (status, code, answer) = call(
        &router,
        "DeleteSecurityPolicy",
        json!({ "type": "network", "name": "alpha-policy" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteSecurityPolicy(network/alpha-policy): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        names(&list_all(&router, "network", None).await),
        vec!["delta-policy".to_string()],
        "case ListSecurityPolicies(network, after delete): the deleted policy is gone"
    );
    assert!(
        names(&list_all(&router, "encryption", None).await).contains(&"alpha-policy".to_string()),
        "case ListSecurityPolicies(encryption, after deleting the network policy of the same name): the other type is untouched"
    );
}

#[tokio::test]
async fn the_listing_is_empty_for_a_type_with_no_stored_policies() {
    let router = emulator();

    // Both `SecurityPolicyType` values, with nothing stored: an empty listing, never a
    // refusal and never a `nextToken`.
    for kind in ["encryption", "network"] {
        let (summaries, token) = list_page(&router, kind, None, None, None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListSecurityPolicies({kind}, empty store): expected no summaries and no nextToken, got {summaries:?} and {token:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: `resource` narrows the listing to the policies that apply to one of the
// resources the filter names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_resource_filter_narrows_the_listing_to_the_policies_that_apply() {
    let router = emulator();

    // The resource forms a policy rule names, from AWS's network and encryption policy
    // documentation: `collection/<name>` and `index/<collection>/<index>`, with `*`
    // standing for any run of characters. Row 5 carries no `Rules` at all, so it applies
    // to no resource a filter can name; row 6 is the list document form, whose two rules
    // name two different collections.
    let policies: Vec<(&str, &str, &str)> = vec![
        (
            "encryption",
            "enc-exact",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/logs-prod"]}],"AWSOwnedKey":true}"#,
        ),
        (
            "encryption",
            "enc-index",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/app-*"]}],"AWSOwnedKey":true}"#,
        ),
        (
            "encryption",
            "enc-other",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/metrics"]}],"AWSOwnedKey":true}"#,
        ),
        (
            "encryption",
            "enc-prefix",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/logs-*"]}],"AWSOwnedKey":true}"#,
        ),
        (
            "encryption",
            "enc-ruleless",
            r#"{"AWSOwnedKey":true}"#,
        ),
        (
            "network",
            "net-pair",
            r#"[{"Rules":[{"ResourceType":"collection","Resource":["collection/logs-prod"]},{"ResourceType":"dashboard","Resource":["collection/metrics"]}],"AllowFromPublic":true}]"#,
        ),
    ];
    for (kind, name, policy) in &policies {
        create_policy(
            &router,
            json!({ "type": kind, "name": name, "policy": policy }),
        )
        .await;
    }

    // (type, filter, the policies that apply). The rows span an exact resource meeting a
    // wildcard rule, a wildcard filter meeting exact rules, an index resource, two filter
    // entries (the union of what each selects), a filter nothing applies to, and the list
    // document form.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "encryption",
            vec!["collection/logs-prod"],
            vec!["enc-exact", "enc-prefix"],
        ),
        (
            "encryption",
            vec!["collection/logs-staging"],
            vec!["enc-prefix"],
        ),
        (
            "encryption",
            vec!["collection/*"],
            vec!["enc-exact", "enc-other", "enc-prefix"],
        ),
        (
            "encryption",
            vec!["index/logs-prod/app-2026"],
            vec!["enc-index"],
        ),
        (
            "encryption",
            vec!["collection/metrics", "index/logs-prod/app-1"],
            vec!["enc-index", "enc-other"],
        ),
        ("encryption", vec!["collection/nothing-here"], vec![]),
        ("network", vec!["collection/metrics"], vec!["net-pair"]),
        ("network", vec!["collection/logs-prod"], vec!["net-pair"]),
        ("network", vec!["collection/elsewhere"], vec![]),
    ];
    for (kind, filter, wanted) in &cases {
        let summaries = list_all(&router, kind, Some(filter)).await;
        assert_eq!(
            names(&summaries),
            wanted.iter().map(|name| (*name).to_string()).collect::<Vec<String>>(),
            "case ListSecurityPolicies({kind}, resource={filter:?}): expected {wanted:?}, got {summaries:?}"
        );
    }

    // An absent filter narrows nothing: every stored policy of the type is reported,
    // including the one whose document names no resource.
    assert_eq!(
        names(&list_all(&router, "encryption", None).await),
        vec![
            "enc-exact".to_string(),
            "enc-index".to_string(),
            "enc-other".to_string(),
            "enc-prefix".to_string(),
            "enc-ruleless".to_string(),
        ],
        "case ListSecurityPolicies(encryption, no filter): every stored policy of the type is reported"
    );

    // The filter composes with the page bound: the same narrowed sequence, one summary at
    // a time.
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(
            &router,
            "encryption",
            Some(&["collection/*"]),
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
            "enc-exact".to_string(),
            "enc-other".to_string(),
            "enc-prefix".to_string()
        ],
        "case ListSecurityPolicies(encryption, resource=[collection/*], maxResults=1): paging walks the narrowed listing in order, got {collected:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusals the listing owes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_refuses_what_it_must() {
    let router = emulator();

    // `maxResults` is `@range(min: 1, max: 100)` and `type` is required and drawn from
    // `SecurityPolicyType`; `resource` is `@length(min: 1, max: 1000)`; a `nextToken` this
    // service did not issue is not a pagination token.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "type outside SecurityPolicyType",
            json!({ "type": "data" }),
        ),
        (
            "type in the wrong case",
            json!({ "type": "Encryption" }),
        ),
        ("type absent", json!({})),
        (
            "maxResults one below the 1 minimum",
            json!({ "type": "encryption", "maxResults": 0 }),
        ),
        (
            "maxResults one past the 100 maximum",
            json!({ "type": "encryption", "maxResults": 101 }),
        ),
        (
            "resource shorter than its 1-entry minimum",
            json!({ "type": "encryption", "resource": [] }),
        ),
        (
            "nextToken this service did not issue",
            json!({ "type": "encryption", "nextToken": "not-a-token" }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "ListSecurityPolicies", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case ListSecurityPolicies({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // `maxResults` at each published bound is accepted.
    for bound in [1_i64, 100] {
        let (summaries, token) =
            list_page(&router, "encryption", None, Some(bound), None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListSecurityPolicies(maxResults={bound}, empty store): expected no summaries, got {summaries:?} and {token:?}"
        );
    }
}

#[tokio::test]
async fn the_resource_filter_is_bounded_in_what_it_evaluates() {
    let router = emulator();

    // MAX_POLICY_RESOURCES = 200: a document contributes at most that many `Resource`
    // entries to the filtering, so this one's 201st entry is past the guard. The document
    // stays well inside `PolicyDocument`'s own `@length(max: 20480)`.
    let named: Vec<String> = (0..201).map(|index| format!("collection/r{index}")).collect();
    create_policy(
        &router,
        json!({
            "type": "encryption",
            "name": "many-resources",
            "policy": json!({
                "Rules": [{ "ResourceType": "collection", "Resource": named }],
                "AWSOwnedKey": true,
            })
            .to_string(),
        }),
    )
    .await;

    // The entries inside the guard select the policy; the one past it does not.
    for (case, filter, wanted) in [
        ("the first entry", "collection/r0", true),
        ("the last entry inside the guard", "collection/r199", true),
        ("the first entry past the guard", "collection/r200", false),
    ] {
        let summaries = list_all(&router, "encryption", Some(&[filter])).await;
        assert_eq!(
            !summaries.is_empty(),
            wanted,
            "case ListSecurityPolicies(resource=[{filter}], {case}): expected the policy to be reported={wanted}, got {summaries:?}"
        );
    }

    // MAX_RESOURCE_PATTERN_LENGTH = 1024 bounds the STORED side too: a `Resource` entry
    // longer than that is not matched against at all, while a short one of the same form
    // is. The two policies differ only in the length of the resource their rule names.
    for (name, resource) in [
        ("short-resource", format!("collection/{}", "a".repeat(3))),
        ("long-resource", format!("collection/{}", "a".repeat(1020))),
    ] {
        create_policy(
            &router,
            json!({
                "type": "encryption",
                "name": name,
                "policy": json!({
                    "Rules": [{ "ResourceType": "collection", "Resource": [resource] }],
                    "AWSOwnedKey": true,
                })
                .to_string(),
            }),
        )
        .await;
    }
    assert_eq!(
        names(&list_all(&router, "encryption", Some(&["collection/a*"])).await),
        vec!["short-resource".to_string()],
        "case ListSecurityPolicies(resource=[collection/a*]): a stored Resource entry past the pattern bound is not matched"
    );

    // MAX_RESOURCE_FILTER_ENTRIES = 1000 (also `resource`'s modelled maximum) and
    // MAX_RESOURCE_PATTERN_LENGTH = 1024: a filter AT each bound is evaluated, and one
    // past it is the modelled refusal rather than unbounded work.
    let at_entry_bound: Vec<String> = (0..1000).map(|index| format!("collection/r{index}")).collect();
    let at_length_bound = format!("collection/{}*", "a".repeat(1012));
    assert_eq!(
        at_length_bound.len(),
        1024,
        "case ListSecurityPolicies(resource entry at its length bound): the case entry is 1024 characters, got {}",
        at_length_bound.len()
    );
    for (case, filter) in [
        ("1000 entries, its maximum", at_entry_bound.clone()),
        ("an entry of 1024 characters", vec![at_length_bound.clone()]),
    ] {
        let borrowed: Vec<&str> = filter.iter().map(String::as_str).collect();
        let (summaries, token) =
            list_page(&router, "encryption", Some(&borrowed), None, None).await;
        assert!(
            token.is_none(),
            "case ListSecurityPolicies(resource with {case}): expected no nextToken, got {token:?}"
        );
        // The 1000-entry filter names `collection/r0..r999`, which the stored policy's own
        // entries meet; the long entry names nothing stored.
        let expected = case.starts_with("1000");
        assert_eq!(
            !summaries.is_empty(),
            expected,
            "case ListSecurityPolicies(resource with {case}): expected the policy to be reported={expected}, got {summaries:?}"
        );
    }

    let mut past_entry_bound = at_entry_bound;
    past_entry_bound.push("collection/one-too-many".to_string());
    let refusals: Vec<(&str, Vec<String>)> = vec![
        ("1001 entries, one past its maximum", past_entry_bound),
        (
            "an entry of 1025 characters, one past its maximum",
            vec![format!("{at_length_bound}a")],
        ),
        (
            // MAX_RESOURCE_FILTER_WORK: every entry is inside the length bound, but each
            // carries 512 wildcards, so matching them against the stored policies' own
            // resources is past the work this listing performs. The refusal comes instead
            // of the work, not after it.
            "1000 entries of 512 wildcards each, past the matching budget",
            (0..1000).map(|_| "*a".repeat(512)).collect(),
        ),
    ];
    for (case, filter) in refusals {
        let (status, code, answer) = call(
            &router,
            "ListSecurityPolicies",
            json!({ "type": "encryption", "resource": filter }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case ListSecurityPolicies(resource with {case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }
}
