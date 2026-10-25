// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SearchOrganizationInsights`.
//!
//! Every test drives the service the way a client does — a buffered HTTP request through
//! the same [`smithy_server_runtime::MultiServiceRouter`] the emulator boots — so the
//! assertions are about wire behaviour, not handler internals. The harness is the
//! `configuration_lifecycle` one and the seeded insights are the `insight_search`
//! fixture, so the organization-wide search is asserted over exactly the store the
//! account-scoped `SearchInsights` suite uses.
//!
//! DevOps Guru MINTS an insight itself, from anomalies in the covered resources; no
//! modelled operation creates one. So a test that needs insights seeds the store the
//! analysis would write (`state.insights`), exactly as the handler reads it, and then
//! asks the service over HTTP.
//!
//! RULES under test:
//!
//! 1. An organization-wide insight search is a view of an organization's member accounts,
//!    which belongs to the account that administers DevOps Guru for that organization, so
//!    any other caller is refused with the operation's HTTP 403 `AccessDeniedException` —
//!    which is every caller a client of this emulator can present, because nothing
//!    registers an administrator.
//! 2. For the administering caller the search reports the stored insights of the ONE
//!    `InsightType` it names whose behaviour STARTED inside the required `StartTimeRange`
//!    (both bounds inclusive, an absent bound unbounded), each on the response list of its
//!    own type — the same insights `SearchInsights` reports over the same store.
//! 3. Every filter type the request carries NARROWS that selection — an insight is
//!    reported only while each of them admits it (`AND` across filter types), and a filter
//!    is satisfied by ANY ONE of the values it names (`OR` within one). A filter naming no
//!    value restricts nothing.
//! 4. The required `AccountIds` narrows the search to the member accounts it names; a
//!    member account the organization does not hold leaves none in scope. A value outside
//!    the enum, range or length the model declares is a client error, not an empty answer.
//! 5. The search pages at the `MaxResults` the request asks for, its cursor resolves only
//!    against the selection and account scope it was issued for, and reading changes
//!    nothing a later caller sees.

use serde_json::{json, Value};
use std::collections::BTreeSet;

use crate::configuration_lifecycle::{Emulator, Reply};
use crate::insight_listing::{epoch_seconds, BASE_MILLIS, MINUTE_MILLIS};
use crate::insight_search::{search_fixture, APP_BOUNDARY_KEY};

/// `SearchOrganizationInsights`'s HTTP binding, from
/// `smithy_query(query="operation",operation="SearchOrganizationInsights")`:
/// `POST /organization/insights/search`, with every member in the body.
const SEARCH_PATH: &str = "/organization/insights/search";

/// `SearchInsights`'s binding — `POST /insights/search` — so a row can compare the
/// organization-wide search with the account-scoped one over the same store, and show
/// that one's cursor does not resolve against the other's.
const ACCOUNT_SEARCH_PATH: &str = "/insights/search";

/// Both response members a searched insight can be reported on.
const INSIGHT_LISTS: [&str; 2] = ["/ProactiveInsights", "/ReactiveInsights"];

/// The account this emulator serves, which is the one member account whose insights the
/// store holds. `AwsAccountId` is modelled `^\d{12}$`.
fn served_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A twelve-digit account id that is NOT the served account — `AwsAccountId`'s pattern,
/// from `smithy_query(query="shape",shape="SearchInsightsAccountIdList")`.
const OTHER_ACCOUNT: &str = "210987654321";

/// An instant of the fixture's grid, `minutes` from [`BASE_MILLIS`].
fn at(minutes: i64) -> i64 {
    BASE_MILLIS + minutes * MINUTE_MILLIS
}

/// A well-formed request body: the three required members
/// (`AccountIds`, modelled len 1..1, naming the served member account; `StartTimeRange`;
/// and `Type`), plus whatever else a row wants.
fn request(insight_type: &str, from: Option<i64>, to: Option<i64>) -> Value {
    let mut range = json!({});
    if let Some(from) = from {
        range["FromTime"] = json!(epoch_seconds(from));
    }
    if let Some(to) = to {
        range["ToTime"] = json!(epoch_seconds(to));
    }
    json!({
        "AccountIds": [served_account()],
        "Type": insight_type,
        "StartTimeRange": range,
    })
}

/// The simplest well-formed request for `insight_type`: an unbounded start-time range.
fn every_instant(insight_type: &str) -> Value {
    request(insight_type, None, None)
}

async fn search(emulator: &Emulator, body: &Value) -> Reply {
    emulator.call("POST", SEARCH_PATH, Some(body)).await
}

async fn search_account(emulator: &Emulator, body: &Value) -> Reply {
    emulator.call("POST", ACCOUNT_SEARCH_PATH, Some(body)).await
}

/// The `Id`s one reply reported, proactive insights first.
fn reported_ids(reply: &Reply) -> Vec<String> {
    INSIGHT_LISTS
        .iter()
        .flat_map(|list| {
            reply
                .at(list)
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect::<Vec<String>>()
        })
        .collect()
}

/// The cursor a reply carries, if it has one.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .at("/NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// `expected`, in the ascending `InsightId` order the store keeps and therefore the order
/// a page reports.
fn in_store_order(expected: &[&str]) -> Vec<String> {
    expected
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// An emulator in which the served account administers DevOps Guru for the organization,
/// as the Organizations-side registration no operation of this service performs would.
/// Without it every organization-wide read is refused — rule 1 — so this is what makes
/// the reporting path reachable at all.
fn administered_emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator.delegate_organization_administration(&served_account());
    emulator
}

/// An administered emulator holding the `insight_search` fixture: both insight types, both
/// statuses, all three severities, start instants spread over a known grid, and resource
/// collections that are a stack, a tag pair, or nothing at all.
fn seeded_emulator() -> Emulator {
    let emulator = administered_emulator();
    emulator.seed_insights(search_fixture());
    emulator
}

/// Assert one row: the request selects exactly `expected`, and carries no cursor because
/// the selection fits one page.
async fn assert_selects(emulator: &Emulator, case: &str, body: &Value, expected: &[&str]) {
    let reply = search(emulator, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a well-formed organization search must succeed, got HTTP {} {}",
        reply.status, reply.raw
    );
    assert_eq!(
        reported_ids(&reply),
        in_store_order(expected),
        "case {case:?}: the search must report exactly {expected:?}, got {}",
        reply.raw
    );
    assert!(
        next_token(&reply).is_none(),
        "case {case:?}: a selection that fits one page carries no NextToken, got {}",
        reply.raw
    );
}

/// Every PROACTIVE insight the fixture holds, in the store's order.
const EVERY_PROACTIVE: [&str; 5] = [
    "proactive-early-low-ongoing-gamma",
    "proactive-high-closed-late",
    "proactive-high-ongoing-alpha",
    "proactive-low-closed-beta",
    "proactive-medium-ongoing-payments",
];

/// Every REACTIVE insight the fixture holds, in the store's order.
const EVERY_REACTIVE: [&str; 3] = [
    "reactive-high-ongoing-alpha",
    "reactive-low-ongoing-bare",
    "reactive-medium-closed-checkout",
];

// =============================================================================
// 1. The organization-wide view belongs to the administering account
// =============================================================================

/// RULE 1: an organization-wide insight search is a view of an organization's member
/// accounts, which belongs to the account administering DevOps Guru for that organization.
/// No account administers one here, so every caller is refused with the operation's HTTP
/// 403 `AccessDeniedException` — not a 200 with an empty answer, and not the scaffold's
/// 501.
///
/// The rows span both insight types, bounded and unbounded windows, the filter forms a
/// client can send, the served and another member account, and a chosen page size.
#[tokio::test]
async fn an_organization_insight_search_is_denied_while_no_account_administers_devops_guru() {
    let emulator = Emulator::new();
    // Insights exist, so the refusal is not just an empty store answering.
    emulator.seed_insights(search_fixture());

    let rows: Vec<(&str, Value)> = vec![
        ("PROACTIVE, no filter", every_instant("PROACTIVE")),
        ("REACTIVE, no filter", every_instant("REACTIVE")),
        (
            "PROACTIVE, a bounded window",
            request("PROACTIVE", Some(at(0)), Some(at(60))),
        ),
        ("another member account named", {
            let mut body = every_instant("PROACTIVE");
            body["AccountIds"] = json!([OTHER_ACCOUNT]);
            body
        }),
        ("every filter type named", {
            let mut body = every_instant("PROACTIVE");
            body["Filters"] = json!({
                "Severities": ["HIGH"],
                "Statuses": ["ONGOING"],
                "ServiceCollection": { "ServiceNames": ["LAMBDA"] },
                "ResourceCollection": { "CloudFormation": { "StackNames": ["alpha"] } },
            });
            body
        }),
        ("a chosen page size", {
            let mut body = every_instant("REACTIVE");
            body["MaxResults"] = json!(1);
            body
        }),
    ];

    for (case, body) in rows {
        let reply = search(&emulator, &body).await;
        assert_eq!(
            reply.status, 403,
            "case {case:?}: a caller that administers no organization must be refused, got HTTP {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("AccessDeniedException"),
            "case {case:?}: the refusal must be an AccessDeniedException, got {}",
            reply.raw
        );
        for list in INSIGHT_LISTS {
            assert!(
                reply.at(list).is_none(),
                "case {case:?}: a refusal reports no insight on {list}, got {}",
                reply.raw
            );
        }
    }
}

// =============================================================================
// 2. Type and the required start-time range
// =============================================================================

/// RULE 2: for the administering caller the search reports the stored insights of the
/// `InsightType` it names whose `InsightTimeRange.StartTime` lies in the required
/// `StartTimeRange` — "a time range used to specify when the behavior of an insight or
/// anomaly started" — with both bounds inclusive and an absent bound unbounded.
///
/// The rows differ in the type searched, in which side of the range is bounded, in how
/// wide the window is (every insight, some, exactly one, none), and in whether the window
/// is inverted — which holds no instant and so selects nothing.
#[tokio::test]
async fn type_and_start_time_range_select_the_insights_searched_for() {
    let emulator = seeded_emulator();

    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "PROACTIVE, unbounded window",
            every_instant("PROACTIVE"),
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "PROACTIVE, bounded below only",
            request("PROACTIVE", Some(at(0)), None),
            vec![
                "proactive-high-closed-late",
                "proactive-high-ongoing-alpha",
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "PROACTIVE, bounded above only",
            request("PROACTIVE", None, Some(at(0))),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-high-ongoing-alpha",
            ],
        ),
        (
            "PROACTIVE, both bounds",
            request("PROACTIVE", Some(at(0)), Some(at(20))),
            vec![
                "proactive-high-ongoing-alpha",
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "PROACTIVE, a window of one instant, inclusive on both sides",
            request("PROACTIVE", Some(at(10)), Some(at(10))),
            vec!["proactive-low-closed-beta"],
        ),
        (
            "PROACTIVE, inverted window",
            request("PROACTIVE", Some(at(60)), Some(at(0))),
            vec![],
        ),
        (
            "REACTIVE, unbounded window",
            every_instant("REACTIVE"),
            EVERY_REACTIVE.to_vec(),
        ),
        (
            "REACTIVE, both bounds",
            request("REACTIVE", Some(at(0)), Some(at(30))),
            vec![
                "reactive-high-ongoing-alpha",
                "reactive-medium-closed-checkout",
            ],
        ),
        (
            "REACTIVE, a window after every stored insight",
            request("REACTIVE", Some(at(1_000)), None),
            vec![],
        ),
    ];

    for (case, body, expected) in rows {
        assert_selects(&emulator, case, &body, &expected).await;
    }
}

/// RULE 2: each selected insight is reported on the list of its OWN type, so a search for
/// one type reports an empty list for the other rather than omitting it.
#[tokio::test]
async fn each_type_is_reported_on_its_own_list() {
    let emulator = seeded_emulator();

    for (case, insight_type, own, other, expected) in [
        (
            "PROACTIVE",
            "PROACTIVE",
            "/ProactiveInsights",
            "/ReactiveInsights",
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "REACTIVE",
            "REACTIVE",
            "/ReactiveInsights",
            "/ProactiveInsights",
            EVERY_REACTIVE.to_vec(),
        ),
    ] {
        let reply = search(&emulator, &every_instant(insight_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a well-formed organization search must succeed, got HTTP {} {}",
            reply.status, reply.raw
        );
        let listed: Vec<String> = reply
            .at(own)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.get("Id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed,
            in_store_order(&expected),
            "case {case:?}: every selected insight belongs on {own}, got {}",
            reply.raw
        );
        assert_eq!(
            reply.at(other).and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {case:?}: the other type's list must be present and empty, got {}",
            reply.raw
        );
    }
}

/// RULE 2's cross-check: the organization-wide search and the account-scoped
/// `SearchInsights` read ONE store under the same selection rule, so for the one member
/// account in scope they report the same insights.
///
/// The rows are both insight types over bounded, half-bounded and unbounded windows, and a
/// filtered selection.
#[tokio::test]
async fn the_organization_search_agrees_with_the_account_scoped_search() {
    let emulator = seeded_emulator();

    let rows: Vec<(&str, Value)> = vec![
        ("PROACTIVE, unbounded window", every_instant("PROACTIVE")),
        ("REACTIVE, unbounded window", every_instant("REACTIVE")),
        (
            "PROACTIVE, bounded below only",
            request("PROACTIVE", Some(at(10)), None),
        ),
        (
            "REACTIVE, both bounds",
            request("REACTIVE", Some(at(0)), Some(at(30))),
        ),
        ("PROACTIVE, filtered by severity and status", {
            let mut body = every_instant("PROACTIVE");
            body["Filters"] = json!({ "Severities": ["LOW", "MEDIUM"], "Statuses": ["ONGOING"] });
            body
        }),
    ];

    for (case, body) in rows {
        let organization = search(&emulator, &body).await;
        // The account-scoped search models no `AccountIds`, so the same body minus that
        // member is the same selection asked of one account's own insights.
        let mut account_body = body.clone();
        account_body
            .as_object_mut()
            .expect("the request body is an object")
            .remove("AccountIds");
        let account = search_account(&emulator, &account_body).await;
        assert_eq!(
            (organization.status, reported_ids(&organization)),
            (account.status, reported_ids(&account)),
            "case {case:?}: the two searches of one store must agree, got {} and {}",
            organization.raw,
            account.raw
        );
    }
}

// =============================================================================
// 3. Each filter type narrows the selection
// =============================================================================

/// RULE 3: every filter type the request carries narrows the selection — an insight is
/// reported only while each of them admits it — and a filter is satisfied by any one of
/// the values it names. A filter naming no value restricts nothing.
///
/// The rows differ in WHICH filter type is sent, in how many values it names (none, one,
/// several), in whether a named value matches no insight, and in how many filter types are
/// combined.
#[tokio::test]
async fn each_filter_type_narrows_the_search() {
    let emulator = seeded_emulator();

    let rows: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        (
            "no Filters member at all",
            "PROACTIVE",
            Value::Null,
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "an empty Filters structure",
            "PROACTIVE",
            json!({}),
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "Severities names no value",
            "PROACTIVE",
            json!({ "Severities": [] }),
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "Severities names one value",
            "PROACTIVE",
            json!({ "Severities": ["LOW"] }),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-low-closed-beta",
            ],
        ),
        (
            "Severities names all three values",
            "PROACTIVE",
            json!({ "Severities": ["LOW", "MEDIUM", "HIGH"] }),
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "Severities names a severity no REACTIVE insight carries",
            "REACTIVE",
            json!({ "Severities": ["MEDIUM"] }),
            vec!["reactive-medium-closed-checkout"],
        ),
        (
            "Statuses names one value",
            "PROACTIVE",
            json!({ "Statuses": ["CLOSED"] }),
            vec!["proactive-high-closed-late", "proactive-low-closed-beta"],
        ),
        (
            "Statuses names both values",
            "REACTIVE",
            json!({ "Statuses": ["ONGOING", "CLOSED"] }),
            EVERY_REACTIVE.to_vec(),
        ),
        (
            "ServiceCollection names one service",
            "PROACTIVE",
            json!({ "ServiceCollection": { "ServiceNames": ["LAMBDA"] } }),
            vec![
                "proactive-high-ongoing-alpha",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "ServiceCollection names a service no insight of this type is about",
            "PROACTIVE",
            json!({ "ServiceCollection": { "ServiceNames": ["EC2"] } }),
            vec![],
        ),
        (
            "ResourceCollection names one stack",
            "REACTIVE",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["alpha"] } } }),
            vec!["reactive-high-ongoing-alpha"],
        ),
        (
            "ResourceCollection names two stacks",
            "PROACTIVE",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["beta", "gamma"] } } }),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-low-closed-beta",
            ],
        ),
        (
            "ResourceCollection names a stack no insight is about",
            "PROACTIVE",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["delta"] } } }),
            vec![],
        ),
        (
            "ResourceCollection names one tag pair",
            "PROACTIVE",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": ["payments"] }],
            } }),
            vec!["proactive-medium-ongoing-payments"],
        ),
        (
            "ResourceCollection names a tag pair under an uncovered boundary key",
            "PROACTIVE",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": "another-boundary", "TagValues": ["payments"] }],
            } }),
            vec![],
        ),
        (
            "Severities AND Statuses",
            "PROACTIVE",
            json!({ "Severities": ["HIGH"], "Statuses": ["ONGOING"] }),
            vec!["proactive-high-ongoing-alpha"],
        ),
        (
            "Severities AND a ResourceCollection no insight of that severity is about",
            "PROACTIVE",
            json!({
                "Severities": ["LOW"],
                "ResourceCollection": { "CloudFormation": { "StackNames": ["alpha"] } },
            }),
            vec![],
        ),
        (
            "all four filter types at once",
            "PROACTIVE",
            json!({
                "Severities": ["MEDIUM", "HIGH"],
                "Statuses": ["ONGOING"],
                "ServiceCollection": { "ServiceNames": ["LAMBDA"] },
                "ResourceCollection": {
                    "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": ["payments"] }],
                },
            }),
            vec!["proactive-medium-ongoing-payments"],
        ),
    ];

    for (case, insight_type, filters, expected) in rows {
        let mut body = every_instant(insight_type);
        if !filters.is_null() {
            body["Filters"] = filters;
        }
        assert_selects(&emulator, case, &body, &expected).await;
    }
}

/// RULE 3 over the start-time range as well: a filter narrows the window's selection
/// rather than replacing it, so an insight outside the window is not reported however the
/// filters are set.
#[tokio::test]
async fn filters_narrow_the_time_window_rather_than_replacing_it() {
    let emulator = seeded_emulator();

    for (case, from, to, expected) in [
        (
            "LOW inside a window the earliest LOW insight is outside of",
            at(0),
            at(20),
            vec!["proactive-low-closed-beta"],
        ),
        (
            "LOW inside a window that reaches the earliest LOW insight",
            at(-60),
            at(20),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-low-closed-beta",
            ],
        ),
    ] {
        let mut body = request("PROACTIVE", Some(from), Some(to));
        body["Filters"] = json!({ "Severities": ["LOW"] });
        assert_selects(&emulator, case, &body, &expected).await;
    }
}

// =============================================================================
// 4. The account scope, and refusing a value the model does not admit
// =============================================================================

/// RULE 4: `AccountIds` narrows the search to the member accounts it names, so naming the
/// member account whose insights the organization holds reports them and naming any other
/// member account leaves none in scope — a 200 reporting nothing, because an account the
/// organization does not hold insights for is not an error.
///
/// The rows differ in the insight type, in whether the named account is the served member
/// or another, and in whether a filter narrows the selection as well.
#[tokio::test]
async fn the_account_filter_narrows_the_search() {
    let emulator = seeded_emulator();

    let served = served_account();
    let rows: Vec<(&str, &str, &str, Value, Vec<&str>)> = vec![
        (
            "the served member account, PROACTIVE",
            &served,
            "PROACTIVE",
            Value::Null,
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "the served member account, REACTIVE",
            &served,
            "REACTIVE",
            Value::Null,
            EVERY_REACTIVE.to_vec(),
        ),
        (
            "the served member account, filtered",
            &served,
            "PROACTIVE",
            json!({ "Statuses": ["CLOSED"] }),
            vec!["proactive-high-closed-late", "proactive-low-closed-beta"],
        ),
        (
            "another member account, PROACTIVE",
            OTHER_ACCOUNT,
            "PROACTIVE",
            Value::Null,
            vec![],
        ),
        (
            "another member account, REACTIVE",
            OTHER_ACCOUNT,
            "REACTIVE",
            Value::Null,
            vec![],
        ),
        (
            "another member account, filtered",
            OTHER_ACCOUNT,
            "PROACTIVE",
            json!({ "Statuses": ["CLOSED"] }),
            vec![],
        ),
    ];

    for (case, account_id, insight_type, filters, expected) in rows {
        let mut body = every_instant(insight_type);
        body["AccountIds"] = json!([account_id]);
        if !filters.is_null() {
            body["Filters"] = filters;
        }
        assert_selects(&emulator, case, &body, &expected).await;
    }
}

/// RULE 4: a `Type`, severity, status or service name outside the enum the model declares
/// is a client error (HTTP 400 `ValidationException`), not an empty answer; so is a
/// `MaxResults` outside the modelled range, a `NextToken` outside the modelled pattern,
/// and an `AccountIds` outside its modelled length or its member's pattern.
///
/// The rows differ in WHICH member carries the bad value and in the FORM of the wrongness
/// (another enum's value, a lowercased value, an unknown value, a value past a numeric
/// bound, a list of the wrong length, a value the member's pattern refuses).
#[tokio::test]
async fn a_value_the_model_does_not_admit_is_refused() {
    let emulator = seeded_emulator();

    let rows: Vec<(&str, Value)> = vec![
        ("Type outside InsightType", json!({ "Type": "PREDICTIVE" })),
        ("Type lowercased", json!({ "Type": "proactive" })),
        ("Type empty", json!({ "Type": "" })),
        (
            "Severities carries another enum's value",
            json!({ "Filters": { "Severities": ["ONGOING"] } }),
        ),
        (
            "Severities carries a lowercased value beside a good one",
            json!({ "Filters": { "Severities": ["HIGH", "high"] } }),
        ),
        (
            "Statuses carries a value InsightStatus does not admit",
            json!({ "Filters": { "Statuses": ["ANY"] } }),
        ),
        (
            "ServiceNames carries an unknown service",
            json!({ "Filters": { "ServiceCollection": { "ServiceNames": ["EC3"] } } }),
        ),
        // `SearchOrganizationInsightsMaxResults` is modelled range 1..100.
        ("MaxResults below the modelled range", json!({ "MaxResults": 0 })),
        ("MaxResults above the modelled range", json!({ "MaxResults": 101 })),
        // `UuidNextToken` is modelled len 36..36 with an 8-4-4-4-12 lowercase-hex pattern.
        (
            "NextToken outside the modelled pattern",
            json!({ "NextToken": "not-a-page-token-at-all" }),
        ),
        // `SearchInsightsAccountIdList` is modelled len 1..1, over `AwsAccountId`
        // (len 12..12, pattern ^\d{12}$).
        ("AccountIds empty", json!({ "AccountIds": [] })),
        (
            "AccountIds names two accounts",
            json!({ "AccountIds": [served_account(), OTHER_ACCOUNT] }),
        ),
        (
            "AccountIds names a value AwsAccountId's pattern refuses",
            json!({ "AccountIds": ["not-an-account"] }),
        ),
    ];

    for (case, overrides) in rows {
        let mut body = every_instant("PROACTIVE");
        for (member, value) in overrides.as_object().expect("the row must be an object") {
            body[member.as_str()] = value.clone();
        }
        let reply = search(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: the request must be refused, got HTTP {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {}",
            reply.raw
        );
        for list in INSIGHT_LISTS {
            assert!(
                reply.at(list).is_none(),
                "case {case:?}: a refusal reports no insight on {list}, got {}",
                reply.raw
            );
        }
    }
}

/// RULE 4's siblings: `AccountIds`, `Type` and `StartTimeRange` are all modelled
/// **required**, so a request missing any of them is refused rather than answered over a
/// default.
#[tokio::test]
async fn a_request_missing_a_required_member_is_refused() {
    let emulator = seeded_emulator();

    for (case, body) in [
        (
            "no AccountIds",
            json!({ "Type": "PROACTIVE", "StartTimeRange": {} }),
        ),
        (
            "no Type",
            json!({ "AccountIds": [served_account()], "StartTimeRange": {} }),
        ),
        (
            "no StartTimeRange",
            json!({ "AccountIds": [served_account()], "Type": "PROACTIVE" }),
        ),
        ("no required member at all", json!({})),
    ] {
        let reply = search(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a request missing a required member must be refused, got HTTP {} {}",
            reply.status, reply.raw
        );
    }
}

// =============================================================================
// 5. Paging, cursor scope, and reading without changing anything
// =============================================================================

/// Every id a search walks at `max_results` per page, following its cursor to the end.
async fn walk(emulator: &Emulator, body: &Value, max_results: Option<i32>) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page = body.clone();
        if let Some(max_results) = max_results {
            page["MaxResults"] = json!(max_results);
        }
        if let Some(token) = token.as_deref() {
            page["NextToken"] = json!(token);
        }
        let reply = search(emulator, &page).await;
        assert_eq!(
            reply.status, 200,
            "walking at {max_results:?} per page must succeed, got HTTP {} {}",
            reply.status, reply.raw
        );
        let ids = reported_ids(&reply);
        assert!(
            ids.len() <= max_results.unwrap_or(i32::MAX) as usize,
            "walking at {max_results:?} per page: a page must hold at most the requested \
             MaxResults, got {}",
            reply.raw
        );
        walked.extend(ids);
        match next_token(&reply) {
            Some(next) => token = Some(next),
            None => return walked,
        }
    }
}

/// RULE 5: the search pages at the `MaxResults` the request asks for, and walking every
/// page yields exactly the selection once, in the store's stable order — whatever the page
/// size, and whether or not it divides the selection.
///
/// The rows are the page sizes 1 through 3 (`SearchOrganizationInsightsMaxResults` is
/// modelled range 1..100, so 1 is its lower bound), a size the selection fits inside, the
/// modelled maximum 100, and no `MaxResults` at all, each over a selection of five
/// insights and one of three.
#[tokio::test]
async fn a_walk_reports_each_selected_insight_exactly_once() {
    let emulator = seeded_emulator();

    for (case, body, expected) in [
        (
            "five proactive insights",
            every_instant("PROACTIVE"),
            EVERY_PROACTIVE.to_vec(),
        ),
        (
            "three reactive insights",
            every_instant("REACTIVE"),
            EVERY_REACTIVE.to_vec(),
        ),
    ] {
        let expected = in_store_order(&expected);
        for max_results in [Some(1), Some(2), Some(3), Some(5), Some(100), None] {
            let walked = walk(&emulator, &body, max_results).await;
            assert_eq!(
                walked, expected,
                "case {case:?} at {max_results:?} per page: the walk must report the selection \
                 exactly once, got {walked:?}"
            );
        }
    }
}

/// RULE 5's cursor half: a `NextToken` names a page of ONE selection under ONE account
/// scope, so a cursor from another insight type, another filter set, or the account-scoped
/// `SearchInsights` walk names no page of this search and is refused with HTTP 400
/// `ValidationException` rather than silently seeking elsewhere.
#[tokio::test]
async fn a_cursor_resolves_only_against_the_selection_it_was_issued_for() {
    let emulator = seeded_emulator();

    let first_page = |body: &Value| {
        let mut body = body.clone();
        body["MaxResults"] = json!(1);
        body
    };

    // A cursor of the organization search over every PROACTIVE insight.
    let proactive = every_instant("PROACTIVE");
    let organization_cursor = next_token(&search(&emulator, &first_page(&proactive)).await)
        .expect("five insights paged at 1 have a cursor");
    // A cursor of the organization search over the HIGH PROACTIVE insights alone.
    let mut filtered = every_instant("PROACTIVE");
    filtered["Filters"] = json!({ "Severities": ["HIGH"] });
    let filtered_cursor = next_token(&search(&emulator, &first_page(&filtered)).await)
        .expect("two HIGH insights paged at 1 have a cursor");
    // A cursor of the ACCOUNT-scoped search over the same insights.
    let mut account_body = first_page(&proactive);
    account_body
        .as_object_mut()
        .expect("the request body is an object")
        .remove("AccountIds");
    let account_cursor = next_token(&search_account(&emulator, &account_body).await)
        .expect("the account-scoped search pages the same five insights");

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a PROACTIVE cursor against the REACTIVE selection",
            every_instant("REACTIVE"),
            organization_cursor.clone(),
        ),
        (
            "an unfiltered cursor against a filtered selection",
            filtered.clone(),
            organization_cursor.clone(),
        ),
        (
            "a filtered cursor against the unfiltered selection",
            proactive.clone(),
            filtered_cursor,
        ),
        (
            "the account-scoped search's cursor against the organization search",
            proactive.clone(),
            account_cursor,
        ),
        (
            "the organization search's cursor against another account scope",
            {
                let mut body = every_instant("PROACTIVE");
                body["AccountIds"] = json!([OTHER_ACCOUNT]);
                body
            },
            organization_cursor.clone(),
        ),
    ];

    for (case, mut body, cursor) in rows {
        body["NextToken"] = json!(cursor);
        let reply = search(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a cursor of another selection names no page, got HTTP {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {}",
            reply.raw
        );
    }

    // The organization search's own cursor still resolves against its own selection, so
    // the rows above refuse because of the scope and not because every cursor is refused.
    let mut resumed = first_page(&proactive);
    resumed["NextToken"] = json!(organization_cursor);
    let reply = search(&emulator, &resumed).await;
    assert_eq!(
        reply.status, 200,
        "a cursor must resolve against the selection it was issued for, got HTTP {} {}",
        reply.status, reply.raw
    );
    assert_eq!(
        reported_ids(&reply),
        vec![EVERY_PROACTIVE[1].to_string()],
        "the second page must hold the second selected insight, got {}",
        reply.raw
    );
}

/// RULE 5's read half: a read must not change what a later caller sees — neither the
/// search itself, nor the insights it is derived from, nor the refusal a non-administering
/// caller gets.
///
/// The rows are both insight types, filtered and unfiltered, read repeatedly and in turn,
/// in an emulator where the search reports and in one where it refuses.
#[tokio::test]
async fn searching_organization_insights_leaves_the_store_unchanged() {
    let mut rows: Vec<(&str, Value)> = vec![
        ("PROACTIVE, unbounded window", every_instant("PROACTIVE")),
        ("REACTIVE, unbounded window", every_instant("REACTIVE")),
        (
            "PROACTIVE, bounded window",
            request("PROACTIVE", Some(at(0)), Some(at(30))),
        ),
    ];
    let mut filtered = every_instant("REACTIVE");
    filtered["Filters"] = json!({ "Severities": ["HIGH"], "Statuses": ["ONGOING"] });
    rows.push(("REACTIVE, filtered", filtered));

    for administered in [false, true] {
        let emulator = if administered {
            seeded_emulator()
        } else {
            let emulator = Emulator::new();
            emulator.seed_insights(search_fixture());
            emulator
        };

        let mut first_bodies: Vec<(u16, String)> = Vec::new();
        for (case, body) in &rows {
            let reply = search(&emulator, body).await;
            assert_eq!(
                reply.status,
                if administered { 200 } else { 403 },
                "case administered={administered}/{case:?}: unexpected status, got {}",
                reply.raw
            );
            first_bodies.push((reply.status, reply.body.to_string()));
        }

        for round in 1..=3 {
            for (index, (case, body)) in rows.iter().enumerate() {
                let reply = search(&emulator, body).await;
                assert_eq!(
                    (reply.status, reply.body.to_string()),
                    first_bodies[index],
                    "case administered={administered}/{case:?} round {round}: a repeated read \
                     must answer the same, got {}",
                    reply.raw
                );
            }
        }

        // The insights the organization search reads are still all there, as the
        // account-scoped search sees them.
        let mut account_body = every_instant("PROACTIVE");
        account_body
            .as_object_mut()
            .expect("the request body is an object")
            .remove("AccountIds");
        let reply = search_account(&emulator, &account_body).await;
        assert_eq!(
            reported_ids(&reply),
            in_store_order(&EVERY_PROACTIVE),
            "case administered={administered}: the organization search must not change the \
             insight store, got {}",
            reply.raw
        );
    }
}

/// RULE 2's empty case: an organization whose analysis produced no insight reports none,
/// on both lists, and that is not an error. The rows are both insight types.
#[tokio::test]
async fn an_organization_without_insights_reports_none() {
    let emulator = administered_emulator();

    for insight_type in ["PROACTIVE", "REACTIVE"] {
        let reply = search(&emulator, &every_instant(insight_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {insight_type:?}: an empty store must still answer, got HTTP {} {}",
            reply.status, reply.raw
        );
        for list in INSIGHT_LISTS {
            assert_eq!(
                reply.at(list).and_then(Value::as_array).map(Vec::len),
                Some(0),
                "case {insight_type:?}: {list} must be present and empty, got {}",
                reply.raw
            );
        }
        assert!(
            next_token(&reply).is_none(),
            "case {insight_type:?}: an empty answer carries no NextToken, got {}",
            reply.raw
        );
    }
}
