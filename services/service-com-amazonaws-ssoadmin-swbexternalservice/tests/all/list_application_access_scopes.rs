// SPDX-License-Identifier: CC0-1.0

//! `ListApplicationAccessScopes`, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the operation reports the access scopes the application the
//! request names currently carries ("Lists the access scopes and authorized targets
//! associated with an application") — each scope exactly once, as a `ScopeDetails` whose
//! `Scope` is the stored name and whose `AuthorizedTargets` are the targets the last
//! `PutApplicationAccessScope` stored for it (the member ABSENT for a scope put without
//! any, since `ScopeTargets` is `length 1..10` and cannot express an empty list), in
//! ascending `Scope` order, one page per call under the `@paginated{inputToken:
//! NextToken, outputToken: NextToken, pageSize: MaxResults, items: Scopes}` contract,
//! with a continuation token only while scopes are left over. An application that carries
//! no scope is an EMPTY page, while an `ApplicationArn` this account holds no application
//! for is a `ResourceNotFoundException` and a `NextToken` this enumeration did not issue
//! is a `ValidationException`. Being a read, it changes nothing a later caller sees.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the NUMBER of scopes an application carries (none, one, several, the ten
//! `Scopes` (`length 0..10`) can report at once, and more than one page's worth), the
//! segment counts, charset and published 50-character segment bound the model's `Scope`
//! pattern admits, the number of `AuthorizedTargets` each scope was put with, the ORDER
//! the puts were made in, and every `MaxResults` the model's `range(min: 1, max: 10)`
//! admits plus one past each published bound.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints, the `awsJson1_1` envelope and the
//! `X-Amz-Target` routing are exercised rather than bypassed. The scopes are seeded over
//! the wire through `PutApplicationAccessScope`, so nothing here reaches into the store.
//! The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{assert_refused, call, router, seeded_instance_arn};

/// The OAuth 2.0 application provider a customer managed application is created under.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue
/// `ListApplicationProviders`/`DescribeApplicationProvider` report, whose `custom` entry
/// federates over `OAUTH`.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// An `ApplicationArn` of the modelled form that no create minted, for the not-found rows.
///
/// SOURCE of the form: `smithy_query(query="operation",operation="ListApplicationAccessScopes")`
/// — `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
const UNMINTED_APPLICATION_ARN: &str =
    "arn:aws:sso::123456789012:application/ssoins-0000000000000000/apl-0000000000000000";

/// `MaxResults`' published upper bound for this operation, which is also the upper bound
/// of `Scopes`' own `length(min: 0, max: 10)`: the most scopes one page can report.
///
/// SOURCE: `smithy_query(query="operation",operation="ListApplicationAccessScopes")` —
/// `MaxResults` is `range(min: 1, max: 10)` with `default: 10`.
const MAX_RESULTS_MAX: i64 = 10;

/// The `MaxResults` values the pagination rows drive: absent (so the modelled default
/// stands in), the published lower bound, one inside the range, and the published upper
/// bound.
const PAGE_SIZES: &[Option<i64>] = &[None, Some(1), Some(3), Some(MAX_RESULTS_MAX)];

/// The `Scope` names the rows are built from, in the order the puts are made — NOT in
/// ascending order, so the page order asserted below is the rule's own rather than the
/// order the scopes were stored in.
///
/// They span the forms the model's `Scope` pattern admits
/// (`smithy_query(query="shape",shape="Scope")`:
/// `^([A-Za-z0-9_]{1,50})(:[A-Za-z0-9_]{1,50}){0,1}(:[A-Za-z0-9_]{1,50}){0,1}$`) — one
/// segment, two, the three it allows at most, the full `[A-Za-z0-9_]` charset, a
/// one-character segment, and a segment at the published 50-character bound — and are
/// twelve in number, more than the ten `Scopes` can report in one page.
fn scope_catalog() -> Vec<String> {
    let longest_segment = "s".repeat(50);
    [
        "sso:account:access",
        "profile",
        "openid",
        "aws_Z9_0",
        "a",
        longest_segment.as_str(),
        "ab:cd",
        "Z9_:a:B",
        "email",
        "user_impersonation",
        "Read",
        "write_0",
    ]
    .iter()
    .map(|scope| (*scope).to_string())
    .collect()
}

/// A router over a fresh engine and the ARN of the instance it seeded.
async fn fixture() -> (MultiServiceRouter, String) {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    (router, instance_arn)
}

/// Create one customer managed application over the wire and return its minted ARN.
async fn create_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApplication {name:?}: expected 200, got {status} {created}"
    );
    created["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication {name:?} minted no ARN: {created}"))
        .to_string()
}

/// `PutApplicationAccessScope`: `targets` is `None` for a put that sends no
/// `AuthorizedTargets` at all.
async fn put_scope(
    router: &MultiServiceRouter,
    application_arn: &str,
    scope: &str,
    targets: Option<&[String]>,
) {
    let mut request = json!({"ApplicationArn": application_arn, "Scope": scope});
    if let Some(targets) = targets {
        request["AuthorizedTargets"] = json!(targets);
    }
    let (status, body) = call(router, "PutApplicationAccessScope", &request).await;
    assert_eq!(
        status, 200,
        "the put of {scope:?} must answer 200, got {status} {body}"
    );
}

/// One `ListApplicationAccessScopes` call, as `(status, body)`.
async fn list_page(
    router: &MultiServiceRouter,
    application_arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"ApplicationArn": application_arn});
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListApplicationAccessScopes", &request).await
}

/// Walk the whole enumeration, following `NextToken`, and return every `ScopeDetails`
/// returned together with the number of pages it took.
async fn list_all(
    router: &MultiServiceRouter,
    application_arn: &str,
    max_results: Option<i64>,
    case: &str,
) -> (Vec<Value>, usize) {
    // Absent `MaxResults` is the modelled `default: 10`, so that is the page bound the
    // walk holds the service to in every row.
    let limit = max_results.unwrap_or(MAX_RESULTS_MAX);
    let mut collected: Vec<Value> = Vec::new();
    let mut pages = 0usize;
    let mut next_token: Option<String> = None;
    loop {
        let (status, body) = list_page(router, application_arn, max_results, next_token.as_deref())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplicationAccessScopes expected 200, got {status} {body}"
        );
        let page = body["Scopes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Scopes must be a list, got {body}"))
            .clone();
        assert!(
            page.len() as i64 <= limit,
            "case {case:?}: a page must hold at most MaxResults {limit} entries, got {} ({body})",
            page.len()
        );
        pages += 1;
        collected.extend(page);
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
        assert!(
            pages <= 64,
            "case {case:?}: the enumeration must terminate, got {pages} pages"
        );
    }
    (collected, pages)
}

/// The `ScopeDetails` the rule requires for a scope stored with `targets`.
fn expected_entry(scope: &str, targets: Option<&[String]>) -> Value {
    match targets {
        // `ScopeTargets` is `length 1..10`: a scope put without authorized targets is
        // reported with the member ABSENT, not with an empty list.
        None => json!({"Scope": scope}),
        Some(targets) => json!({"Scope": scope, "AuthorizedTargets": targets}),
    }
}

/// The rows of one application's scopes: the first `count` names of the catalog, each
/// with a different number of authorized targets drawn from `pool` (none, one, two,
/// three), so no row shares its target shape with its neighbours.
fn scope_rows(count: usize, pool: &[String]) -> Vec<(String, Option<Vec<String>>)> {
    scope_catalog()
        .into_iter()
        .take(count)
        .enumerate()
        .map(|(index, scope)| {
            let targets = index % 4;
            let targets = (targets > 0).then(|| pool[..targets].to_vec());
            (scope, targets)
        })
        .collect()
}

/// The entries the enumeration must report for `rows`: ascending by `Scope`, which is the
/// key the scopes are stored under.
fn expected_entries(rows: &[(String, Option<Vec<String>>)]) -> Vec<Value> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    sorted
        .iter()
        .map(|(scope, targets)| expected_entry(scope, targets.as_deref()))
        .collect()
}

/// A pool of `ScopeTarget` values of the modelled form, built from resources this account
/// really holds: the instance itself and applications created here.
///
/// SOURCE of the form: `smithy_query(query="shape",shape="ScopeTarget")` —
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::(\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}|:instance/(sso)?ins-[a-zA-Z0-9-.]{16})$`.
async fn target_pool(router: &MultiServiceRouter, instance_arn: &str, label: &str) -> Vec<String> {
    vec![
        instance_arn.to_string(),
        create_application(router, instance_arn, &format!("{label}-target-a")).await,
        create_application(router, instance_arn, &format!("{label}-target-b")).await,
    ]
}

/// THE RULE: every scope the application carries is reported exactly once, with the
/// targets its last put stored, in ascending `Scope` order — for every collection size
/// and every page size, and with the scopes of another application never leaking in.
#[tokio::test]
async fn the_enumeration_reports_exactly_the_scopes_the_application_carries() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "reports").await;

    // The sizes the rule generalises over: none, one, several, the ten `Scopes` can
    // report in one page, and more than one page's worth.
    for count in [0usize, 1, 3, MAX_RESULTS_MAX as usize, 12] {
        let application_arn =
            create_application(&router, &instance_arn, &format!("scopes-{count}")).await;
        // A neighbour application carrying a scope of its own: a scope belongs to ONE
        // application, so this one must appear in no page of the application under test.
        let other_arn =
            create_application(&router, &instance_arn, &format!("other-{count}")).await;
        put_scope(&router, &other_arn, "openid", Some(&pool[..1])).await;

        let rows = scope_rows(count, &pool);
        for (scope, targets) in &rows {
            put_scope(&router, &application_arn, scope, targets.as_deref()).await;
        }
        let expected = expected_entries(&rows);

        for max_results in PAGE_SIZES {
            let case = format!("{count} scopes, MaxResults {max_results:?}");
            let (listed, pages) = list_all(&router, &application_arn, *max_results, &case).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: the enumeration must report exactly the stored scopes in \
                 Scope order, got {listed:?}"
            );

            // One page per call: the walk takes exactly as many calls as the page size
            // divides the collection into, and an empty collection is one empty page.
            let limit = max_results.unwrap_or(MAX_RESULTS_MAX) as usize;
            let expected_pages = if count == 0 {
                1
            } else {
                count.div_ceil(limit)
            };
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages at page size {limit}, got {pages}"
            );
        }
    }
}

/// THE RULE's token half: a page carries a `NextToken` only while scopes are left over,
/// and the token resumes AFTER the entries that page already reported.
#[tokio::test]
async fn a_token_is_issued_only_while_scopes_are_left_over() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "token").await;

    // One row per (collection size, page size) pair where the sizes differ in how the
    // page divides the collection: exactly, with a remainder, and not at all.
    for count in [1usize, 3, MAX_RESULTS_MAX as usize, 12] {
        let application_arn =
            create_application(&router, &instance_arn, &format!("token-{count}")).await;
        let rows = scope_rows(count, &pool);
        for (scope, targets) in &rows {
            put_scope(&router, &application_arn, scope, targets.as_deref()).await;
        }
        let expected = expected_entries(&rows);

        for max_results in PAGE_SIZES {
            let case = format!("{count} scopes, MaxResults {max_results:?}");
            let limit = max_results.unwrap_or(MAX_RESULTS_MAX) as usize;
            let (status, body) = list_page(&router, &application_arn, *max_results, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            let first = body["Scopes"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: Scopes must be a list, got {body}"))
                .clone();
            let reported = first.len();
            assert_eq!(
                first,
                expected[..count.min(limit)].to_vec(),
                "case {case:?}: the first page must be the first {} entries, got {first:?}",
                count.min(limit)
            );

            match body["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        reported < count,
                        "case {case:?}: a token was issued though the page reported all \
                         {count} scopes, so nothing was left over: got {body}"
                    );
                    // The token stays inside the model's `Token` pattern
                    // `^[-a-zA-Z0-9+=/_]*$` and its 0..2048 length, so a client can send it
                    // back verbatim.
                    assert!(
                        !token.is_empty()
                            && token.len() <= 2048
                            && token.chars().all(|c| c.is_ascii_alphanumeric()
                                || matches!(c, '-' | '+' | '=' | '/' | '_')),
                        "case {case:?}: the token must satisfy Token's pattern and length, got \
                         {token:?}"
                    );
                    let (status, next) =
                        list_page(&router, &application_arn, *max_results, Some(token)).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the page after the token expected 200, got {status} {next}"
                    );
                    let after = next["Scopes"].as_array().cloned().unwrap_or_default();
                    assert_eq!(
                        after,
                        expected[reported..(reported + limit).min(count)].to_vec(),
                        "case {case:?}: the token must resume after the entries already \
                         reported, got {after:?}"
                    );
                }
                None => assert_eq!(
                    reported, count,
                    "case {case:?}: no token was issued though the page reported only \
                     {reported} of {count} scopes, so some were left over: got {body}"
                ),
            }
        }
    }
}

/// An application that carries NO scope is an empty, token-less page — never the
/// not-found an `ApplicationArn` nothing holds earns.
#[tokio::test]
async fn an_application_with_no_scope_lists_an_empty_page() {
    let (router, instance_arn) = fixture().await;
    let application_arn = create_application(&router, &instance_arn, "empty-app").await;

    for max_results in PAGE_SIZES {
        let case = format!("MaxResults {max_results:?}");
        let (status, body) = list_page(&router, &application_arn, *max_results, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["Scopes"],
            json!([]),
            "case {case:?}: an application carrying no scope lists none, got {body}"
        );
        assert_eq!(
            body["NextToken"],
            Value::Null,
            "case {case:?}: with nothing returned there is no token, got {body}"
        );
    }
}

/// An `ApplicationArn` this account holds no application for is refused
/// `ResourceNotFoundException`: the rows differ in how they miss a stored application.
#[tokio::test]
async fn an_application_arn_no_create_minted_is_refused_not_found() {
    let (router, instance_arn) = fixture().await;
    let stored = create_application(&router, &instance_arn, "stored-app").await;
    put_scope(&router, &stored, "openid", None).await;
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    // Each row is pattern-valid and names no stored application: a never-minted ARN, the
    // stored ARN with another application id, and one under another instance.
    let stored_suffix = stored.rsplit('/').next().unwrap_or_default().to_string();
    let rows: Vec<(&str, String)> = vec![
        ("an ARN no create minted", UNMINTED_APPLICATION_ARN.to_string()),
        (
            "the stored instance with another application id",
            format!("arn:aws:sso::123456789012:application/{instance_id}/apl-1111111111111111"),
        ),
        (
            "the stored application id under another instance",
            format!(
                "arn:aws:sso::123456789012:application/ssoins-9999999999999999/{stored_suffix}"
            ),
        ),
    ];

    for (case, arn) in &rows {
        assert_ne!(
            arn, &stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = list_page(&router, arn, None, None).await;
        assert_refused(
            case,
            "the enumeration of an unminted ApplicationArn",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // The refusals changed nothing: the stored application still lists its one scope.
    let (listed, pages) = list_all(&router, &stored, None, "after the refusals").await;
    assert_eq!(
        listed,
        vec![expected_entry("openid", None)],
        "the refusals must leave the stored scope listed, got {listed:?}"
    );
    assert_eq!(pages, 1, "one page holds the one stored scope, got {pages}");
}

/// A `NextToken` this service did not issue FOR THIS enumeration is a bad request, not an
/// empty page: the rows span a token issued for ANOTHER application's enumeration, a
/// value this service never issued, and the empty token `Token`'s 0..2048 length admits.
#[tokio::test]
async fn a_token_this_enumeration_did_not_issue_is_refused() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "cursor").await;
    let application_arn = create_application(&router, &instance_arn, "cursor-app").await;
    let other_arn = create_application(&router, &instance_arn, "cursor-other").await;
    for (scope, targets) in scope_rows(3, &pool) {
        put_scope(&router, &application_arn, &scope, targets.as_deref()).await;
        put_scope(&router, &other_arn, &scope, targets.as_deref()).await;
    }

    // A real token, issued for the OTHER application's enumeration of the same operation.
    let (status, body) = list_page(&router, &other_arn, Some(1), None).await;
    assert_eq!(
        status, 200,
        "the other application's first page expected 200, got {status} {body}"
    );
    let foreign_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the other application's first page carries a token: {body}"))
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("a token issued for another application", foreign_token),
        ("a value this service never issued", "not-a-token".to_string()),
        (
            "hex that decodes to another enumeration's scope",
            "73736f61646d696e3a76313a696e7374616e6365733a78".to_string(),
        ),
        ("the empty token Token's 0..2048 length admits", String::new()),
    ];
    for (case, token) in &rows {
        let (status, body) = list_page(&router, &application_arn, None, Some(token)).await;
        assert_refused(
            case,
            "a NextToken this enumeration did not issue",
            "ValidationException",
            status,
            &body,
        );
    }
}

/// A `MaxResults` outside the range this operation's own member declares
/// (`range(min: 1, max: 10)`, narrower than the `MaxResults` shape's 1..100) is refused
/// `ValidationException` naming the member and the bound it broke; both published bounds
/// are served.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let (router, instance_arn) = fixture().await;
    let application_arn = create_application(&router, &instance_arn, "range-app").await;
    put_scope(&router, &application_arn, "openid", None).await;

    // One past each published bound, and the clause this service states for it — the
    // wording `validation_message_wording.rs` owns the rule for.
    for (max_results, requirement) in [
        (0, "Member must have value greater than or equal to 1"),
        (
            MAX_RESULTS_MAX + 1,
            "Member must have value less than or equal to 10",
        ),
    ] {
        let case = format!("MaxResults {max_results}");
        let (status, body) = list_page(&router, &application_arn, Some(max_results), None).await;
        assert_refused(
            &case,
            "a MaxResults past a published bound",
            "ValidationException",
            status,
            &body,
        );
        let expected =
            format!("Value of input 'maxResults' failed to satisfy constraint: {requirement}");
        assert_eq!(
            body["Message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected Message {expected:?}, got {body}"
        );
    }

    for max_results in [1, MAX_RESULTS_MAX] {
        let case = format!("MaxResults {max_results}");
        let (status, body) = list_page(&router, &application_arn, Some(max_results), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a MaxResults inside the modelled range must be served, got \
             {status} {body}"
        );
    }
}

/// The enumeration is a READ: repeating it answers identically, and what it reports is
/// what `GetApplicationAccessScope` reports for the same scope.
#[tokio::test]
async fn the_enumeration_changes_nothing_a_later_caller_sees() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "read").await;
    let application_arn = create_application(&router, &instance_arn, "read-app").await;
    let rows = scope_rows(5, &pool);
    for (scope, targets) in &rows {
        put_scope(&router, &application_arn, scope, targets.as_deref()).await;
    }

    let (first, first_pages) = list_all(&router, &application_arn, Some(2), "the first walk").await;
    let (second, second_pages) =
        list_all(&router, &application_arn, Some(2), "the second walk").await;
    assert_eq!(
        (first.clone(), first_pages),
        (second, second_pages),
        "repeating the enumeration must answer identically, got {first:?}"
    );

    // And every entry it reported is what the single-scope read reports for that scope.
    for (scope, targets) in &rows {
        let (status, body) = call(
            &router,
            "GetApplicationAccessScope",
            &json!({"ApplicationArn": application_arn, "Scope": scope}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {scope:?}: the get expected 200, got {status} {body}"
        );
        assert_eq!(
            body,
            expected_entry(scope, targets.as_deref()),
            "case {scope:?}: the get must report what the enumeration listed, got {body}"
        );
        assert!(
            first.contains(&expected_entry(scope, targets.as_deref())),
            "case {scope:?}: the enumeration must have listed this scope, got {first:?}"
        );
    }
}
