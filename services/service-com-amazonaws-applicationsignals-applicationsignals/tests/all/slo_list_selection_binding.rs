// SPDX-License-Identifier: CC0-1.0

//! The page cursor of the objective listing is bound to the selection it was counted in.
//!
//! The rule under test: a `NextToken` is a boundary within ONE ordered set, so every request
//! member that picks out which stored objectives `ListServiceLevelObjectives` reports travels
//! inside the token, and a token replayed under another selection is refused with the modelled
//! `ValidationException` (HTTP 400, naming `NextToken`) rather than honoured as a boundary into
//! a set it was never counted in — which would report only the objectives sorting after it and
//! tell the caller the listing had ended. A member that picks out no set of its own — a second
//! spelling of one selection, or one that changes nothing about which objectives match — leaves
//! the token honoured.
//!
//! `paged_cursor_selection_binding` states the same rule for the reads whose cursor is an
//! offset into a derived projection; this suite is the objective listing's row of it, whose
//! cursor is a key of the store. Each row of the first table differs along WHICH of the
//! selectors varies (a key-attribute map, an operation name, a dependency, a metric source by
//! its key attributes, the same metric source by the additional attributes beside them, a list
//! of metric source types), and every row is arranged so that the replayed selection holds a
//! matching objective sorting at or before the token's cursor: the entry a honoured token would
//! silently drop. The rows of the second table differ along WHY the two requests state one
//! selection (a list reordered and repeated, the linked-account flag, this account named
//! explicitly).
//!
//! The harness is `slo_lifecycle`'s, so this suite speaks to the same mounted router as every
//! other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, router, ACCOUNT};

/// The environment every seeded entity is hosted in.
const ENVIRONMENT: &str = "prod";

/// The `Type` key attribute of a synthetics canary, whose last `::` segment is the
/// `MetricSourceType` the service derives for it (`Canary`).
const CANARY_TYPE: &str = "AWS::Synthetics::Canary";

/// The `Type` key attribute of a RUM app monitor, whose derived `MetricSourceType` is
/// `AppMonitor`.
const APP_MONITOR_TYPE: &str = "AWS::RUM::AppMonitor";

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": ENVIRONMENT })
}

/// One `DependencyConfig`: the dependency's own entity and the operation called on it.
fn dependency(name: &str, operation: &str) -> Value {
    json!({
        "DependencyKeyAttributes": {
            "Type": "RemoteService",
            "Name": name,
            "Environment": ENVIRONMENT
        },
        "DependencyOperationName": operation
    })
}

/// One `MetricSource`, naming a resource outside Application Signals.
fn metric_source(kind: &str, name: &str) -> Value {
    json!({ "MetricSourceKeyAttributes": { "Type": kind, "Name": name } })
}

/// [`metric_source`] with the additional attributes beside its identity, which narrow the
/// selection as its key attributes do.
fn detailed_metric_source(kind: &str, name: &str, additional: Value) -> Value {
    let mut source = metric_source(kind, name);
    source["MetricSourceAttributes"] = additional;
    source
}

/// An SLI metric config on an Application Signals service operation, optionally through a
/// dependency.
fn entity_metric(service: &str, operation: &str, dependency_config: Option<Value>) -> Value {
    let mut metric = json!({
        "KeyAttributes": service_entity(service),
        "OperationName": operation,
        "MetricType": "LATENCY",
        "PeriodSeconds": 60
    });
    if let Some(dependency_config) = dependency_config {
        metric["DependencyConfig"] = dependency_config;
    }
    metric
}

/// Every objective this suite stores, as (name, SLI metric config).
///
/// The names are ordered by the number in them, which is the order the listing reports them
/// in, so each row of the tables below can state which selection's first match sorts first.
/// Every selection a row pages holds at least two matching objectives, which is the smallest
/// set a cursor can divide.
fn seeds() -> Vec<(&'static str, Value)> {
    vec![
        (
            "sel-02-canary-ping",
            json!({ "MetricSource": metric_source(CANARY_TYPE, "ping") }),
        ),
        (
            "sel-04-canary-ping-latency",
            json!({ "MetricSource": metric_source(CANARY_TYPE, "ping") }),
        ),
        ("sel-10-alpha-read", entity_metric("alpha", "Read", None)),
        ("sel-20-beta-read", entity_metric("beta", "Read", None)),
        ("sel-30-alpha-write", entity_metric("alpha", "Write", None)),
        ("sel-40-beta-write", entity_metric("beta", "Write", None)),
        (
            "sel-50-alpha-ledger",
            entity_metric("alpha", "Read", Some(dependency("ledger", "Charge"))),
        ),
        (
            "sel-60-beta-ledger",
            entity_metric("beta", "Read", Some(dependency("ledger", "Charge"))),
        ),
        (
            "sel-70-alpha-vault",
            entity_metric("alpha", "Write", Some(dependency("vault", "Store"))),
        ),
        (
            "sel-80-beta-vault",
            entity_metric("beta", "Write", Some(dependency("vault", "Store"))),
        ),
        (
            "sel-90-rum-web",
            json!({ "MetricSource": metric_source(APP_MONITOR_TYPE, "web") }),
        ),
        (
            "sel-92-rum-web-errors",
            json!({ "MetricSource": metric_source(APP_MONITOR_TYPE, "web") }),
        ),
        (
            "sel-93-rum-web-cart",
            json!({
                "MetricSource": detailed_metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    json!({ "Page": "cart" }),
                )
            }),
        ),
        (
            "sel-94-rum-web-checkout",
            json!({
                "MetricSource": detailed_metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    json!({ "Page": "checkout" }),
                )
            }),
        ),
        (
            "sel-95-rum-web-cart-latency",
            json!({
                "MetricSource": detailed_metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    json!({ "Page": "cart" }),
                )
            }),
        ),
        (
            "sel-96-rum-web-checkout-latency",
            json!({
                "MetricSource": detailed_metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    json!({ "Page": "checkout" }),
                )
            }),
        ),
    ]
}

/// Store every entry of [`seeds`], so each selection below has a set to page.
async fn seed(router: &MultiServiceRouter) {
    for (name, metric) in seeds() {
        let (status, created) = create(
            router,
            json!({
                "Name": name,
                "SliConfig": {
                    "SliMetricConfig": metric,
                    "MetricThreshold": 100.0,
                    "ComparisonOperator": "LessThan"
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the setup create to answer 200, got {status} with {created}"
        );
    }
}

/// One listing request, with the whole selection already spelled into its query string and its
/// body: only the page bound and the cursor vary from call to call.
#[derive(Clone)]
struct Selection {
    /// How this request picks its set out, for the assertion messages.
    label: &'static str,
    /// The selectors this request spells as query members, with no leading separator.
    query: String,
    /// The selectors this request spells in the JSON body.
    body: Value,
}

/// A selection stated entirely in the body.
fn body_selection(label: &'static str, body: Value) -> Selection {
    Selection {
        label,
        query: String::new(),
        body,
    }
}

/// One page of `selection`, bounded by `max_results` and resuming from `token` when given.
async fn page(
    router: &MultiServiceRouter,
    selection: &Selection,
    max_results: usize,
    token: Option<&str>,
) -> (u16, Value) {
    let mut query = format!("MaxResults={max_results}");
    if !selection.query.is_empty() {
        query.push('&');
        query.push_str(&selection.query);
    }
    if let Some(token) = token {
        query.push_str(&format!("&NextToken={token}"));
    }
    call(
        router,
        "POST",
        &format!("/slos?{query}"),
        Some(selection.body.clone()),
    )
    .await
}

/// The objective names one page reports, in order.
fn names(answer: &Value) -> Vec<String> {
    answer["SloSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The cursor a page handed back, or a panic naming the case: every selection below holds more
/// objectives than the page bound, so each first page carries one.
fn cursor_of(label: &str, answer: &Value) -> String {
    answer["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {label:?}: expected a NextToken on a page of one of several matching objectives, got {answer}")
        })
        .to_string()
}

/// Every objective `selection` reports, read a page of `max_results` at a time until the
/// listing ends.
async fn walked(
    router: &MultiServiceRouter,
    selection: &Selection,
    max_results: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(
            pages <= seeds().len() + 2,
            "case {:?}: paging did not terminate after {pages} pages, seen {seen:?}",
            selection.label
        );
        let (status, answer) = page(router, selection, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 while paging, got {status} with {answer}",
            selection.label
        );
        let reported = names(&answer);
        assert!(
            reported.len() <= max_results,
            "case {:?}: expected at most {max_results} summaries, got {reported:?}",
            selection.label
        );
        seen.extend(reported);
        token = answer["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
    }
}

/// One pair of selections of this read that differ in exactly ONE selector.
struct Case {
    /// The selector that differs, named in every assertion.
    facet: &'static str,
    /// The selection whose page issues the token.
    issuer: Selection,
    /// The selection it is replayed against, whose own first match sorts at or before the
    /// issuing page's cursor — so a honoured token would drop it.
    other: Selection,
}

/// Every row of the rule: one selector varied, with the replayed selection holding an entry a
/// honoured cursor would skip.
fn varied_selections() -> Vec<Case> {
    vec![
        Case {
            facet: "the key attributes it names",
            issuer: body_selection(
                "the objectives of beta",
                json!({ "KeyAttributes": { "Name": "beta" } }),
            ),
            other: body_selection(
                "the objectives of alpha",
                json!({ "KeyAttributes": { "Name": "alpha" } }),
            ),
        },
        Case {
            facet: "the operation it names",
            issuer: body_selection("the Write objectives", json!({ "OperationName": "Write" })),
            other: body_selection("the Read objectives", json!({ "OperationName": "Read" })),
        },
        Case {
            facet: "the dependency it names",
            issuer: body_selection(
                "the objectives calling vault",
                json!({ "DependencyConfig": dependency("vault", "Store") }),
            ),
            other: body_selection(
                "the objectives calling ledger",
                json!({ "DependencyConfig": dependency("ledger", "Charge") }),
            ),
        },
        Case {
            facet: "the metric source it names",
            issuer: body_selection(
                "the objectives on the app monitor",
                json!({ "MetricSource": metric_source(APP_MONITOR_TYPE, "web") }),
            ),
            other: body_selection(
                "the objectives on the canary",
                json!({ "MetricSource": metric_source(CANARY_TYPE, "ping") }),
            ),
        },
        Case {
            facet: "the additional attributes of the metric source it names",
            issuer: body_selection(
                "the objectives on the app monitor's checkout page",
                json!({
                    "MetricSource": detailed_metric_source(
                        APP_MONITOR_TYPE,
                        "web",
                        json!({ "Page": "checkout" }),
                    )
                }),
            ),
            other: body_selection(
                "the objectives on the app monitor's cart page",
                json!({
                    "MetricSource": detailed_metric_source(
                        APP_MONITOR_TYPE,
                        "web",
                        json!({ "Page": "cart" }),
                    )
                }),
            ),
        },
        Case {
            facet: "the metric source types it admits",
            issuer: body_selection(
                "the dependency objectives",
                json!({ "MetricSourceTypes": ["ServiceDependency"] }),
            ),
            other: body_selection(
                "the operation objectives",
                json!({ "MetricSourceTypes": ["ServiceOperation"] }),
            ),
        },
    ]
}

/// One selection stated two ways: the token one spelling issues must resume the other.
struct Equivalent {
    /// What differs between the two requests, which is not part of the selection.
    difference: &'static str,
    issuer: Selection,
    resumed: Selection,
}

/// Every row of the other half of the rule: ONE selection, stated two ways.
fn equivalent_spellings() -> Vec<Equivalent> {
    vec![
        Equivalent {
            // An objective is reported when its own type is ANY of the listed ones, so neither
            // the order of the list nor a type named twice picks out another set.
            difference: "its metric source types reordered, one of them named twice",
            issuer: body_selection(
                "two metric source types",
                json!({ "MetricSourceTypes": ["ServiceDependency", "ServiceOperation"] }),
            ),
            resumed: body_selection(
                "the same two types reordered and repeated",
                json!({
                    "MetricSourceTypes": ["ServiceOperation", "ServiceDependency", "ServiceOperation"]
                }),
            ),
        },
        Equivalent {
            // There is no second account whose objectives it could add, so the flag selects
            // nothing extra.
            difference: "the linked-account flag, which adds no account's objectives",
            issuer: body_selection(
                "the Read objectives",
                json!({ "OperationName": "Read" }),
            ),
            resumed: Selection {
                label: "the same objectives with linked accounts included",
                query: "IncludeLinkedAccounts=true".to_string(),
                body: json!({ "OperationName": "Read" }),
            },
        },
        Equivalent {
            // An account id naming THIS account selects the same objectives as no id at all.
            difference: "this account named explicitly",
            issuer: body_selection(
                "the objectives of alpha",
                json!({ "KeyAttributes": { "Name": "alpha" } }),
            ),
            resumed: Selection {
                label: "the objectives of alpha in this account",
                query: format!("SloOwnerAwsAccountId={ACCOUNT}"),
                body: json!({ "KeyAttributes": { "Name": "alpha" } }),
            },
        },
    ]
}

/// A token issued under one selection is not a boundary in another: replaying it is refused,
/// the other selection's own page is left as it was, and the token still resumes the selection
/// it was issued under — which pages every one of its objectives exactly once.
#[tokio::test]
async fn a_token_issued_under_one_selection_is_refused_under_another() {
    let router = router();
    seed(&router).await;

    for case in varied_selections() {
        let Case {
            facet,
            issuer,
            other,
        } = &case;
        let label = format!(
            "differing in {facet}: the token of {:?} on {:?}",
            issuer.label, other.label
        );

        let (status, first) = page(&router, issuer, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first page to succeed, got {status} with {first}"
        );
        let token = cursor_of(&label, &first);
        let issued_at = names(&first);

        let (status, before) = page(&router, other, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the other selection's own first page to succeed, got {status} with {before}"
        );
        let other_first = names(&before);
        assert_eq!(
            other_first.len(),
            1,
            "case {label:?}: expected the other selection to hold a first page of its own, got {before}"
        );
        // What a honoured cursor would cost: the other selection's own first match sorts at or
        // before the cursor, so resuming there would never report it.
        assert!(
            other_first[0] <= issued_at[0],
            "case {label:?}: expected the other selection to hold a match at or before the cursor {:?}, got {other_first:?}",
            issued_at[0]
        );

        let (status, refusal) = page(&router, other, 1, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a token issued under another selection, got {status} with {refusal}"
        );
        assert_eq!(
            refusal["__type"]
                .as_str()
                .map(|kind| kind.rsplit('#').next_back().unwrap_or(kind)),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {refusal}"
        );
        assert!(
            refusal["message"]
                .as_str()
                .is_some_and(|message| message.contains("NextToken")),
            "case {label:?}: expected the refusal to name NextToken, got {refusal}"
        );
        assert!(
            refusal.get("SloSummaries").is_none() && refusal.get("NextToken").is_none(),
            "case {label:?}: a refused cursor carries no page and no cursor, got {refusal}"
        );

        let (status, after) = page(&router, other, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the read after the refusal to succeed, got {status} with {after}"
        );
        assert_eq!(
            names(&after), other_first,
            "case {label:?}: expected the refused token to leave the other selection's page as it was, got {after}"
        );

        // The same token still resumes the set it WAS issued under, so the refusal above is
        // earned by the selection it was replayed under and by nothing else.
        let (status, resumed) = page(&router, issuer, 1, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the issuing selection to honour its own token, got {status} with {resumed}"
        );
        let resumed_names = names(&resumed);
        assert_eq!(
            resumed_names.len(),
            1,
            "case {label:?}: expected the resumed page to carry the objective after the first, got {resumed}"
        );
        assert!(
            resumed_names[0] > issued_at[0],
            "case {label:?}: expected the resumed page to advance past {:?}, got {resumed_names:?}",
            issued_at[0]
        );

        // Paging the issuing selection one objective at a time still reports each of its
        // matches exactly once, in name order.
        let (status, whole) = page(&router, issuer, 50, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the unpaged listing to succeed, got {status} with {whole}"
        );
        let expected = names(&whole);
        assert!(
            expected.len() >= 2,
            "case {label:?}: expected the selection to hold a set a cursor can divide, got {expected:?}"
        );
        assert!(
            whole["NextToken"].is_null(),
            "case {label:?}: expected no cursor on a page holding every match, got {whole}"
        );
        assert_eq!(
            walked(&router, issuer, 1).await,
            expected,
            "case {label:?}: expected paging to report every match exactly once in name order"
        );
    }
}

/// A token resumes the selection it was issued under however the resuming request SPELLS that
/// selection, and whatever it says about members the selection does not depend on.
#[tokio::test]
async fn a_token_resumes_every_spelling_of_the_selection_it_was_issued_under() {
    let router = router();
    seed(&router).await;

    for case in equivalent_spellings() {
        let Equivalent {
            difference,
            issuer,
            resumed,
        } = &case;
        let label = format!("resumed with {difference}");

        let (status, first) = page(&router, issuer, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first page to succeed, got {status} with {first}"
        );
        let token = cursor_of(&label, &first);

        let (issuer_status, by_issuer) = page(&router, issuer, 1, Some(&token)).await;
        let (status, by_other_spelling) = page(&router, resumed, 1, Some(&token)).await;
        assert_eq!(
            status, issuer_status,
            "case {label:?}: expected the same status for either spelling, got {status} and {issuer_status}"
        );
        assert_eq!(
            status, 200,
            "case {label:?}: expected the resumed read to succeed, got {status} with {by_other_spelling}"
        );
        assert_eq!(
            names(&by_other_spelling),
            names(&by_issuer),
            "case {label:?}: expected the token to resume the same page for either spelling, got {by_other_spelling} and {by_issuer}"
        );
        assert_ne!(
            names(&by_other_spelling),
            names(&first),
            "case {label:?}: expected the resumed page to advance past the first objective, got {by_other_spelling}"
        );
        // One selection, so either spelling reports the same objectives from the start too.
        assert_eq!(
            walked(&router, resumed, 1).await,
            walked(&router, issuer, 1).await,
            "case {label:?}: expected either spelling to report one set"
        );
    }
}
