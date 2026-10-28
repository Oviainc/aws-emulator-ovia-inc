// SPDX-License-Identifier: CC0-1.0

//! `ListAccessPolicies` end to end over the wire.
//!
//! Every case drives the real request path through the harness `collection_lifecycle` owns
//! — `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rules under test:
//!
//! 1. the listing reports the stored data access policies of the `type` the request names,
//!    in one stable order, cutting at the page bound and handing back a `nextToken` only
//!    while records remain — and every summary member is the STORED record's, so a write is
//!    visible in the listing and a delete is visible by its absence;
//! 2. `resource` narrows that listing to the policies whose rules name a resource the
//!    filter's own resource patterns overlap, `*` standing for any run of characters on
//!    either side, with every factor of that matching bounded before it runs.
//!
//! The tables span what those rules generalise over: the object and list document forms,
//! the policy count, the page bound from 1 to past the count and up to the published
//! maximum, exact/wildcard/multi-entry/no-match filters over collection and index
//! resources, and each filter bound and one past it.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text, CLIENT_ERROR};

/// The values `AccessPolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="AccessPolicyType")` -> `values`): the single value
/// `data`.
const ACCESS_POLICY_TYPES: [&str; 1] = ["data"];

/// Create the access policy `body` describes, asserting it was accepted, and answer its
/// `accessPolicyDetail`.
async fn create_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateAccessPolicy", body).await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "accessPolicyDetail").clone()
}

/// One page of `ListAccessPolicies`: its summaries and the token that resumes after them.
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
    let (status, code, answer) = call(router, "ListAccessPolicies", request).await;
    assert_eq!(
        status, 200,
        "case ListAccessPolicies({case}): expected 200, got {status} {code} with {answer}"
    );
    let summaries = at(&answer, "accessPolicySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let token = at(&answer, "nextToken").as_str().map(str::to_string);
    (summaries, token)
}

/// Every summary `ListAccessPolicies` reports for `kind` under `resource`, across pages.
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

    // The documents span a bare array of statements (the form AWS's data-access-policy
    // documentation uses), an object carrying one rule, a statement naming two resources,
    // and a document with no rules at all; row 3's description sits at
    // `PolicyDescription`'s own `@length(max: 1000)`.
    let long_description = "d".repeat(1000);
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "alpha-policy",
            r#"[{"Rules":[{"ResourceType":"index","Resource":["index/alpha/*"],"Permission":["aoss:ReadDocument"]}],"Principal":["arn:aws:iam::000000000000:role/Reader"]}]"#,
            Some("read-only access to the alpha indexes"),
        ),
        (
            "beta-policy",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/beta","collection/beta-two"],"Permission":["aoss:*"]}],"Principal":["arn:aws:iam::000000000000:role/Admin"]}"#,
            None,
        ),
        (
            "gamma-policy",
            r#"[{"Rules":[{"ResourceType":"collection","Resource":["collection/gamma"],"Permission":["aoss:CreateCollectionItems"]}],"Principal":["arn:aws:iam::000000000000:role/Author"]}]"#,
            Some(long_description.as_str()),
        ),
        ("delta-policy", r#"{"Rules":[]}"#, Some("no rules at all")),
    ];

    // The summary a stored policy owes, built from the CREATE's own detail: the listing may
    // not report a member differently from the record every other reader answers.
    let mut expected: Vec<(String, Value)> = Vec::new();
    for kind in ACCESS_POLICY_TYPES {
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

    // The listing's order is the policy name ascending, which the create order above
    // deliberately is not (`gamma-policy` is created before `delta-policy`).
    expected.sort_by(|left, right| left.0.cmp(&right.0));

    for kind in ACCESS_POLICY_TYPES {
        let summaries = list_all(&router, kind, None).await;
        assert_eq!(
            summaries.len(),
            expected.len(),
            "case ListAccessPolicies({kind}): expected {} summaries, got {summaries:?}",
            expected.len()
        );
        for (reported, (_, wanted)) in summaries.iter().zip(&expected) {
            assert_eq!(
                reported, wanted,
                "case ListAccessPolicies({kind}): the summary must be the stored record's, expected {wanted}, got {reported}"
            );
            assert!(
                is_policy_version(&text(reported, "policyVersion")),
                "case ListAccessPolicies({kind}): policyVersion must satisfy PolicyVersion, got {reported}"
            );
            // `AccessPolicySummary` models no `policy` member, so a summary never carries
            // the document itself.
            assert!(
                reported.get("policy").is_none(),
                "case ListAccessPolicies({kind}): a summary carries no policy document, got {reported}"
            );
            // The same record a read reports: the summary is the detail without the
            // document.
            let (status, code, read) = call(
                &router,
                "GetAccessPolicy",
                json!({ "type": kind, "name": text(reported, "name") }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case GetAccessPolicy({kind}/{}): expected 200, got {status} {code} with {read}",
                text(reported, "name")
            );
            let detail = at(&read, "accessPolicyDetail").clone();
            let mut without_policy = detail.clone();
            without_policy
                .as_object_mut()
                .expect("accessPolicyDetail is an object")
                .remove("policy");
            assert_eq!(
                reported, &without_policy,
                "case ListAccessPolicies({kind}, {}): the summary is the read record without its document, got {reported} against {detail}",
                text(reported, "name")
            );
        }
        // The order is the policy name ascending — the cursor a page token resumes after.
        let mut sorted = names(&summaries);
        sorted.sort();
        assert_eq!(
            names(&summaries),
            sorted,
            "case ListAccessPolicies({kind}): the listing is in policy-name order, got {summaries:?}"
        );
    }

    // A security policy of the same name is a different resource: it is not in this
    // listing at all.
    let (status, code, answer) = call(
        &router,
        "CreateSecurityPolicy",
        json!({
            "type": "encryption",
            "name": "alpha-policy",
            "policy": r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/alpha"]}],"AWSOwnedKey":true}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy(encryption/alpha-policy): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        names(&list_all(&router, "data", None).await).len(),
        cases.len(),
        "case ListAccessPolicies(data, with a security policy of the same name stored): the listing holds only access policies"
    );

    // The page bound: 1, 2, exactly the count, one past it, and `maxResults`' own published
    // maximum of 100. Every bound walks the same ordered sequence, and a `nextToken` comes
    // back only while a record remains after the page.
    let all_names = names(&list_all(&router, "data", None).await);
    for bound in [1_i64, 2, 4, 5, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) =
                list_page(&router, "data", None, Some(bound), token.as_deref()).await;
            let bound_size = usize::try_from(bound).expect("the bound is positive");
            let remaining = all_names.len() - collected.len();
            let wanted = remaining.min(bound_size);
            assert_eq!(
                page.len(),
                wanted,
                "case ListAccessPolicies(maxResults={bound}, after {collected:?}): expected {wanted} summaries, got {page:?}"
            );
            collected.extend(names(&page));
            assert_eq!(
                next.is_some(),
                collected.len() < all_names.len(),
                "case ListAccessPolicies(maxResults={bound}, after {collected:?}): a nextToken comes back exactly while records remain, got {next:?}"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, all_names,
            "case ListAccessPolicies(maxResults={bound}): paging walks the whole listing in order, expected {all_names:?}, got {collected:?}"
        );
    }

    // A write is visible in the listing: the update re-mints the `policyVersion`, and the
    // listing reports the new one.
    let before = list_all(&router, "data", None).await;
    let delta = before
        .iter()
        .find(|summary| text(summary, "name") == "delta-policy")
        .cloned()
        .expect("case UpdateAccessPolicy: the delta policy was created");
    let (status, code, updated) = call(
        &router,
        "UpdateAccessPolicy",
        json!({
            "type": "data",
            "name": "delta-policy",
            "policyVersion": text(&delta, "policyVersion"),
            "description": "the rewritten delta access policy",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateAccessPolicy(data/delta-policy): expected 200, got {status} {code} with {updated}"
    );
    let detail = at(&updated, "accessPolicyDetail").clone();
    let after = list_all(&router, "data", None).await;
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
        "case ListAccessPolicies(data, after update): the listing reports the written record, got {listed}"
    );

    // A delete is visible by its absence.
    let (status, code, answer) = call(
        &router,
        "DeleteAccessPolicy",
        json!({ "type": "data", "name": "alpha-policy" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteAccessPolicy(data/alpha-policy): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        names(&list_all(&router, "data", None).await),
        vec![
            "beta-policy".to_string(),
            "delta-policy".to_string(),
            "gamma-policy".to_string(),
        ],
        "case ListAccessPolicies(data, after delete): the deleted policy is gone"
    );
}

#[tokio::test]
async fn the_listing_is_empty_for_a_type_with_no_stored_policies() {
    let router = emulator();

    // Every `AccessPolicyType` value, with nothing stored: an empty listing, never a
    // refusal and never a `nextToken`.
    for kind in ACCESS_POLICY_TYPES {
        let (summaries, token) = list_page(&router, kind, None, None, None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListAccessPolicies({kind}, empty store): expected no summaries and no nextToken, got {summaries:?} and {token:?}"
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

    // The resource forms a data access policy rule names, from AWS's "Data access control
    // for Amazon OpenSearch Serverless": `collection/<name>` and
    // `index/<collection>/<index>`, with `*` standing for any run of characters. Row 5
    // carries no `Rules` at all, so it applies to no resource a filter can name; row 6 is
    // the list document form, whose two statements name two different resources.
    let policies: Vec<(&str, &str)> = vec![
        (
            "acc-exact",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/logs-prod"],"Permission":["aoss:*"]}],"Principal":["arn:aws:iam::000000000000:role/Admin"]}"#,
        ),
        (
            "acc-index",
            r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/app-*"],"Permission":["aoss:ReadDocument"]}],"Principal":["arn:aws:iam::000000000000:role/Reader"]}"#,
        ),
        (
            "acc-other",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/metrics"],"Permission":["aoss:*"]}],"Principal":["arn:aws:iam::000000000000:role/Admin"]}"#,
        ),
        (
            "acc-prefix",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/logs-*"],"Permission":["aoss:*"]}],"Principal":["arn:aws:iam::000000000000:role/Admin"]}"#,
        ),
        (
            "acc-ruleless",
            r#"{"Principal":["arn:aws:iam::000000000000:role/Admin"]}"#,
        ),
        (
            "acc-pair",
            r#"[{"Rules":[{"ResourceType":"collection","Resource":["collection/logs-prod"],"Permission":["aoss:*"]}],"Principal":["arn:aws:iam::000000000000:role/Admin"]},{"Rules":[{"ResourceType":"collection","Resource":["collection/metrics"],"Permission":["aoss:*"]}],"Principal":["arn:aws:iam::000000000000:role/Reader"]}]"#,
        ),
    ];
    for (name, policy) in &policies {
        create_policy(
            &router,
            json!({ "type": "data", "name": name, "policy": policy }),
        )
        .await;
    }

    // (filter, the policies that apply). The rows span an exact resource meeting a wildcard
    // rule, a wildcard filter meeting exact rules, an index resource, two filter entries
    // (the union of what each selects), a filter nothing applies to, and the list document
    // form whose two statements each contribute.
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["collection/logs-prod"],
            vec!["acc-exact", "acc-pair", "acc-prefix"],
        ),
        (vec!["collection/logs-staging"], vec!["acc-prefix"]),
        (
            vec!["collection/*"],
            vec![
                "acc-exact",
                "acc-other",
                "acc-pair",
                "acc-prefix",
            ],
        ),
        (vec!["index/logs-prod/app-2026"], vec!["acc-index"]),
        (
            vec!["collection/metrics", "index/logs-prod/app-1"],
            vec!["acc-index", "acc-other", "acc-pair"],
        ),
        (vec!["collection/nothing-here"], vec![]),
        (vec!["collection/metrics"], vec!["acc-other", "acc-pair"]),
    ];
    for (filter, wanted) in &cases {
        let summaries = list_all(&router, "data", Some(filter)).await;
        assert_eq!(
            names(&summaries),
            wanted
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case ListAccessPolicies(data, resource={filter:?}): expected {wanted:?}, got {summaries:?}"
        );
    }

    // An absent filter narrows nothing: every stored policy of the type is reported,
    // including the one whose document names no resource.
    assert_eq!(
        names(&list_all(&router, "data", None).await),
        vec![
            "acc-exact".to_string(),
            "acc-index".to_string(),
            "acc-other".to_string(),
            "acc-pair".to_string(),
            "acc-prefix".to_string(),
            "acc-ruleless".to_string(),
        ],
        "case ListAccessPolicies(data, no filter): every stored policy of the type is reported"
    );

    // The filter composes with the page bound: the same narrowed sequence, one summary at a
    // time.
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(
            &router,
            "data",
            Some(&["collection/logs-*"]),
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
            "acc-exact".to_string(),
            "acc-pair".to_string(),
            "acc-prefix".to_string()
        ],
        "case ListAccessPolicies(data, resource=[collection/logs-*], maxResults=1): paging walks the narrowed listing in order, got {collected:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusals the listing owes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_refuses_what_it_must() {
    let router = emulator();

    // `maxResults` is `@range(min: 1, max: 100)` and `type` is required and drawn from
    // `AccessPolicyType`; `resource` is `@length(min: 1, max: 1000)`; a `nextToken` this
    // service did not issue is not a pagination token.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "type outside AccessPolicyType",
            json!({ "type": "encryption" }),
        ),
        ("type in the wrong case", json!({ "type": "Data" })),
        ("type absent", json!({})),
        (
            "maxResults one below the 1 minimum",
            json!({ "type": "data", "maxResults": 0 }),
        ),
        (
            "maxResults one past the 100 maximum",
            json!({ "type": "data", "maxResults": 101 }),
        ),
        (
            "resource shorter than its 1-entry minimum",
            json!({ "type": "data", "resource": [] }),
        ),
        (
            "nextToken this service did not issue",
            json!({ "type": "data", "nextToken": "not-a-token" }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "ListAccessPolicies", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case ListAccessPolicies({case}): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // `maxResults` at each published bound is accepted.
    for bound in [1_i64, 100] {
        let (summaries, token) = list_page(&router, "data", None, Some(bound), None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListAccessPolicies(maxResults={bound}, empty store): expected no summaries, got {summaries:?} and {token:?}"
        );
    }
}

#[tokio::test]
async fn the_resource_filter_is_bounded_in_what_it_evaluates() {
    let router = emulator();

    // MAX_POLICY_RESOURCES = 200: a document contributes at most that many `Resource`
    // entries to the filtering, so this one's 201st entry is past the guard. The document
    // stays well inside `PolicyDocument`'s own `@length(max: 20480)`.
    let named: Vec<String> = (0..201)
        .map(|index| format!("collection/r{index}"))
        .collect();
    create_policy(
        &router,
        json!({
            "type": "data",
            "name": "many-resources",
            "policy": json!({
                "Rules": [{ "ResourceType": "collection", "Resource": named, "Permission": ["aoss:*"] }],
                "Principal": ["arn:aws:iam::000000000000:role/Admin"],
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
        let summaries = list_all(&router, "data", Some(&[filter])).await;
        assert_eq!(
            !summaries.is_empty(),
            wanted,
            "case ListAccessPolicies(resource=[{filter}], {case}): expected the policy to be reported={wanted}, got {summaries:?}"
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
                "type": "data",
                "name": name,
                "policy": json!({
                    "Rules": [{ "ResourceType": "collection", "Resource": [resource], "Permission": ["aoss:*"] }],
                    "Principal": ["arn:aws:iam::000000000000:role/Admin"],
                })
                .to_string(),
            }),
        )
        .await;
    }
    assert_eq!(
        names(&list_all(&router, "data", Some(&["collection/a*"])).await),
        vec!["short-resource".to_string()],
        "case ListAccessPolicies(resource=[collection/a*]): a stored Resource entry past the pattern bound is not matched"
    );

    // MAX_RESOURCE_FILTER_ENTRIES = 1000 (also `resource`'s modelled maximum) and
    // MAX_RESOURCE_PATTERN_LENGTH = 1024: a filter AT each bound is evaluated, and one past
    // it is the modelled refusal rather than unbounded work.
    let at_entry_bound: Vec<String> = (0..1000)
        .map(|index| format!("collection/r{index}"))
        .collect();
    let at_length_bound = format!("collection/{}*", "a".repeat(1012));
    assert_eq!(
        at_length_bound.len(),
        1024,
        "case ListAccessPolicies(resource entry at its length bound): the case entry is 1024 characters, got {}",
        at_length_bound.len()
    );
    for (case, filter) in [
        ("1000 entries, its maximum", at_entry_bound.clone()),
        ("an entry of 1024 characters", vec![at_length_bound.clone()]),
    ] {
        let borrowed: Vec<&str> = filter.iter().map(String::as_str).collect();
        let (summaries, token) = list_page(&router, "data", Some(&borrowed), None, None).await;
        assert!(
            token.is_none(),
            "case ListAccessPolicies(resource with {case}): expected no nextToken, got {token:?}"
        );
        // The 1000-entry filter names `collection/r0..r999`, which the stored policy's own
        // entries meet; the long entry names nothing stored.
        let expected = case.starts_with("1000");
        assert_eq!(
            !summaries.is_empty(),
            expected,
            "case ListAccessPolicies(resource with {case}): expected a policy to be reported={expected}, got {summaries:?}"
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
            "ListAccessPolicies",
            json!({ "type": "data", "resource": filter }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case ListAccessPolicies(resource with {case}): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }
}
