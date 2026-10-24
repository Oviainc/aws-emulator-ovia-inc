// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListOrganizationInsights`.
//!
//! Every test drives the service the way a client does — a buffered HTTP request through
//! the same [`smithy_server_runtime::MultiServiceRouter`] the emulator boots — so the
//! assertions are about wire behaviour, not handler internals. The harness is the
//! `configuration_lifecycle` one and the seeded insights are the `insight_listing`
//! fixture, so the organization-wide listing is asserted over exactly the store the
//! account-scoped `ListInsights` suite uses.
//!
//! DevOps Guru MINTS an insight itself, from anomalies in the covered resources; no
//! modelled operation creates one. So a test that needs insights seeds the store the
//! analysis would write (`state.insights`), exactly as the handler reads it, and then
//! asks the service over HTTP.
//!
//! RULES under test:
//!
//! 1. An organization-wide insight listing is a view of an organization's member
//!    accounts, which belongs to the account that administers DevOps Guru for that
//!    organization, so any other caller is refused with the operation's HTTP 403
//!    `AccessDeniedException` — which is every caller a client of this emulator can
//!    present, because nothing registers an administrator.
//! 2. A well-formed request names exactly ONE of `StatusFilter`'s `Ongoing`, `Closed`
//!    and `Any` selectors; any other count is refused with HTTP 400
//!    `ValidationException`.
//! 3. For the administering caller the listing reports exactly the stored insights that
//!    one selector selects, each on the list of its OWN insight type and stamped with
//!    the member account whose store holds it — the same insights `ListInsights` reports
//!    over the same store.
//! 4. `AccountIds` and `OrganizationalUnitIds` NARROW the listing to the member accounts
//!    they name; a filter naming none restricts nothing.
//! 5. The listing pages at the `MaxResults` the request asks for, its cursor resolves
//!    only against the selection and account scope it was issued for, and reading
//!    changes nothing a later caller sees.

use serde_json::{json, Value};

use crate::configuration_lifecycle::{Emulator, Reply};
use crate::insight_listing::{
    epoch_seconds, insight, selection_fixture, BASE_MILLIS, MINUTE_MILLIS,
};

/// The account this emulator serves, which is the one member account whose insights the
/// store holds. `AwsAccountId` is modelled `^\d{12}$`.
fn served_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A twelve-digit account id that is NOT the served account — `AwsAccountId`'s pattern,
/// from `smithy_query(query="shape",shape="ListInsightsAccountIdList")`.
const OTHER_ACCOUNT: &str = "210987654321";

/// An organizational unit id matching `OrganizationalUnitId`'s pattern
/// `^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`, from
/// `smithy_query(query="shape",shape="ListInsightsOrganizationalUnitIdList")`.
const ORGANIZATIONAL_UNIT: &str = "ou-abc1-12345678";

/// Both response members a listed insight can be reported on, so a refusal can be
/// asserted to carry neither.
const INSIGHT_LISTS: [&str; 2] = ["/ProactiveInsights", "/ReactiveInsights"];

/// An instant of the fixture's grid, `minutes` after [`BASE_MILLIS`].
fn at(minutes: i64) -> i64 {
    BASE_MILLIS + minutes * MINUTE_MILLIS
}

/// `ListOrganizationInsights`'s HTTP binding, from
/// `smithy_query(query="operation",operation="ListOrganizationInsights")`:
/// `POST /organization/insights`, with every member in the body.
async fn list_organization_insights(emulator: &Emulator, body: &Value) -> Reply {
    emulator
        .call("POST", "/organization/insights", Some(body))
        .await
}

/// `ListInsights`'s binding — `POST /insights` — so a row can compare the
/// organization-wide listing with the account-scoped one over the same store.
async fn list_insights(emulator: &Emulator, body: &Value) -> Reply {
    emulator.call("POST", "/insights", Some(body)).await
}

/// The simplest well-formed request: one status selector and no filter.
fn request(status_filter: &Value) -> Value {
    json!({ "StatusFilter": status_filter })
}

/// One listed summary, as a client reads it off the wire.
#[derive(Debug, PartialEq, Eq)]
struct Summary {
    /// The response member it was listed on.
    list: &'static str,
    id: String,
    account_id: Option<String>,
    organizational_unit_id: Option<String>,
    status: Option<String>,
}

/// Every summary one reply listed, proactive insights first.
fn listed(reply: &Reply) -> Vec<Summary> {
    INSIGHT_LISTS
        .iter()
        .flat_map(|list| {
            reply
                .at(list)
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(move |item| {
                    let member = |name: &str| {
                        item.get(name)
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    };
                    Summary {
                        list,
                        id: member("Id").unwrap_or_default(),
                        account_id: member("AccountId"),
                        organizational_unit_id: member("OrganizationalUnitId"),
                        status: member("Status"),
                    }
                })
                .collect::<Vec<Summary>>()
        })
        .collect()
}

/// The `Id`s one reply listed, in the order it listed them.
fn listed_ids(reply: &Reply) -> Vec<String> {
    listed(reply).into_iter().map(|summary| summary.id).collect()
}

/// The cursor a reply carries, if it has one.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .at("/NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

/// An administered emulator holding the `insight_listing` fixture: both insight types,
/// both statuses, and instants spread over a known grid.
fn seeded_emulator() -> Emulator {
    let emulator = administered_emulator();
    emulator.seed_insights(selection_fixture());
    emulator
}

/// The status selectors the fixture is asserted against, each with the ids it must
/// select, in the store's stable (`InsightId`-sorted) order.
///
/// The rows differ in WHICH selector is named, the insight type it names, whether the
/// time range is bounded on one side, both or neither, and whether it selects many
/// insights, one, or none.
fn selection_rows() -> Vec<(&'static str, Value, Vec<&'static str>)> {
    vec![
        (
            "Ongoing PROACTIVE",
            json!({ "Ongoing": { "Type": "PROACTIVE" } }),
            vec!["ongoing-proactive-early", "ongoing-proactive-late"],
        ),
        (
            "Ongoing REACTIVE",
            json!({ "Ongoing": { "Type": "REACTIVE" } }),
            vec!["ongoing-reactive", "ongoing-reactive-latest"],
        ),
        (
            "Closed PROACTIVE in a bounded end-time range",
            json!({ "Closed": {
                "Type": "PROACTIVE",
                "EndTimeRange": { "FromTime": epoch_seconds(at(0)), "ToTime": epoch_seconds(at(10)) },
            } }),
            vec!["closed-proactive-early"],
        ),
        (
            "Closed PROACTIVE in an unbounded end-time range",
            json!({ "Closed": { "Type": "PROACTIVE", "EndTimeRange": {} } }),
            vec!["closed-proactive-early", "closed-proactive-late"],
        ),
        (
            "Closed REACTIVE from an inclusive lower bound",
            json!({ "Closed": {
                "Type": "REACTIVE",
                "EndTimeRange": { "FromTime": epoch_seconds(at(15)) },
            } }),
            vec!["closed-reactive"],
        ),
        (
            "Any PROACTIVE started at or after a lower bound",
            json!({ "Any": {
                "Type": "PROACTIVE",
                "StartTimeRange": { "FromTime": epoch_seconds(at(10)) },
            } }),
            vec!["closed-proactive-late", "ongoing-proactive-late"],
        ),
        (
            "Any REACTIVE over an unbounded start-time range",
            json!({ "Any": { "Type": "REACTIVE", "StartTimeRange": {} } }),
            vec!["closed-reactive", "ongoing-reactive", "ongoing-reactive-latest"],
        ),
        (
            "Any PROACTIVE started after every seeded insight",
            json!({ "Any": {
                "Type": "PROACTIVE",
                "StartTimeRange": { "FromTime": epoch_seconds(at(1_000)) },
            } }),
            vec![],
        ),
    ]
}

// =============================================================================
// 1. The organization-wide view belongs to the administering account
// =============================================================================

/// RULE 1: an organization-wide insight listing is a view of an organization's member
/// accounts, which belongs to the account administering DevOps Guru for that
/// organization. No account administers one here, so every caller is refused with the
/// operation's HTTP 403 `AccessDeniedException` — not a 200 with an empty listing, and
/// not the scaffold's 501.
///
/// The rows span every status selector and, for each, the filter forms a client can
/// send: none, a named account, a named organizational unit, and a chosen page size.
#[tokio::test]
async fn an_organization_insight_listing_is_denied_while_no_account_administers_devops_guru() {
    let emulator = Emulator::new();
    // Insights exist, so the refusal is not just an empty store answering.
    emulator.seed_insights(selection_fixture());

    for (case, status_filter, _) in selection_rows() {
        let filters: [(&str, Value); 4] = [
            ("no filter", request(&status_filter)),
            (
                "the served account named",
                json!({ "StatusFilter": status_filter, "AccountIds": [served_account()] }),
            ),
            (
                "an organizational unit named",
                json!({ "StatusFilter": status_filter, "OrganizationalUnitIds": [ORGANIZATIONAL_UNIT] }),
            ),
            (
                "a chosen page size",
                json!({ "StatusFilter": status_filter, "MaxResults": 1 }),
            ),
        ];
        for (filter, body) in filters {
            let reply = list_organization_insights(&emulator, &body).await;
            assert_eq!(
                reply.status, 403,
                "case {case:?}/{filter:?}: expected HTTP 403, got {} in {}",
                reply.status, reply.raw
            );
            assert!(
                reply.names_error("AccessDeniedException"),
                "case {case:?}/{filter:?}: expected an AccessDeniedException, got {}",
                reply.raw
            );
            for list in INSIGHT_LISTS {
                assert!(
                    reply.at(list).is_none(),
                    "case {case:?}/{filter:?}: a refusal must list no {list}, got {}",
                    reply.raw
                );
            }
        }
    }
}

// =============================================================================
// 2. Exactly one status selector
// =============================================================================

/// RULE 2: the listing answers a request whose `StatusFilter` names exactly ONE of
/// `Ongoing`, `Closed` and `Any`, and refuses any other count with HTTP 400
/// `ValidationException` — `StatusFilter` documents "You can specify one status filter",
/// and the model, which marks all three members optional, cannot express it.
///
/// The rows differ in WHICH selectors are named and in how many: none, each one alone,
/// each pair, and all three.
#[tokio::test]
async fn the_status_filter_must_name_exactly_one_selector() {
    let emulator = seeded_emulator();
    let ongoing = json!({ "Type": "REACTIVE" });
    let closed = json!({
        "Type": "REACTIVE",
        "EndTimeRange": { "FromTime": epoch_seconds(at(0)), "ToTime": epoch_seconds(at(60)) },
    });
    let any = json!({ "Type": "PROACTIVE", "StartTimeRange": { "FromTime": epoch_seconds(at(0)) } });

    let cases: [(&str, Value, bool); 8] = [
        ("no selector", json!({}), false),
        ("Ongoing alone", json!({ "Ongoing": ongoing }), true),
        ("Closed alone", json!({ "Closed": closed }), true),
        ("Any alone", json!({ "Any": any }), true),
        (
            "Ongoing + Closed",
            json!({ "Ongoing": ongoing, "Closed": closed }),
            false,
        ),
        (
            "Ongoing + Any",
            json!({ "Ongoing": ongoing, "Any": any }),
            false,
        ),
        ("Closed + Any", json!({ "Closed": closed, "Any": any }), false),
        (
            "all three",
            json!({ "Ongoing": ongoing, "Closed": closed, "Any": any }),
            false,
        ),
    ];

    for (case, status_filter, accepted) in cases {
        let reply = list_organization_insights(&emulator, &request(&status_filter)).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: one selector must be accepted, got {}",
                reply.raw
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a StatusFilter naming {} selectors must be refused, got {}",
                status_filter.as_object().map_or(0, serde_json::Map::len),
                reply.raw
            );
            assert!(
                reply.names_error("ValidationException"),
                "case {case:?}: the refusal must be a ValidationException, got {}",
                reply.raw
            );
        }
    }
}

// =============================================================================
// 3. What the listing reports
// =============================================================================

/// RULE 3: for the administering caller the listing reports exactly the stored insights
/// its ONE status selector selects, each on the list of its own insight type and stamped
/// with the member account whose store holds it; and those are the same insights
/// `ListInsights` reports over the same store, so the account-scoped and the
/// organization-scoped listing cannot disagree.
///
/// The rows are [`selection_rows`]: every selector, over bounded, half-bounded and
/// unbounded time ranges, selecting many insights, one, and none.
#[tokio::test]
async fn the_listing_reports_the_insights_the_status_filter_selects() {
    let emulator = seeded_emulator();

    for (case, status_filter, expected_ids) in selection_rows() {
        let reply = list_organization_insights(&emulator, &request(&status_filter)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the listing must succeed, got {}",
            reply.raw
        );
        assert_eq!(
            listed_ids(&reply),
            expected_ids,
            "case {case:?}: unexpected insights listed, got {}",
            reply.raw
        );

        // The same store, read through the account-scoped listing with the same filter:
        // one selector, one selection.
        let account_scoped = list_insights(&emulator, &request(&status_filter)).await;
        assert_eq!(
            account_scoped.status, 200,
            "case {case:?}: the account-scoped listing must succeed, got {}",
            account_scoped.raw
        );
        assert_eq!(
            listed_ids(&account_scoped),
            expected_ids,
            "case {case:?}: ListInsights must select the same insights, got {}",
            account_scoped.raw
        );

        // Every summary names the member account that holds it, carries no
        // organizational unit (the store records no membership), reports the status its
        // record holds, and sits on the list of its own type.
        let wanted_list = if status_filter.to_string().contains("\"Type\":\"PROACTIVE\"") {
            "/ProactiveInsights"
        } else {
            "/ReactiveInsights"
        };
        for summary in listed(&reply) {
            assert_eq!(
                summary.list, wanted_list,
                "case {case:?}: insight {:?} must be listed on {wanted_list}, got {}",
                summary.id, reply.raw
            );
            assert_eq!(
                summary.account_id,
                Some(served_account()),
                "case {case:?}: insight {:?} must name the member account holding it, got {:?}",
                summary.id,
                summary.account_id
            );
            assert_eq!(
                summary.organizational_unit_id, None,
                "case {case:?}: insight {:?} must report no OrganizationalUnitId, got {:?}",
                summary.id, summary.organizational_unit_id
            );
            assert!(
                summary
                    .status
                    .as_deref()
                    .is_some_and(|status| ["ONGOING", "CLOSED"].contains(&status)),
                "case {case:?}: insight {:?} must report an InsightStatus, got {:?}",
                summary.id,
                summary.status
            );
        }
        assert!(
            reply
                .at(if wanted_list == "/ProactiveInsights" {
                    "/ReactiveInsights"
                } else {
                    "/ProactiveInsights"
                })
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty),
            "case {case:?}: the list for the other insight type must be empty, got {}",
            reply.raw
        );
    }
}

/// RULE 3's empty case: an organization whose analysis produced no insight lists none,
/// and that is not an error. The rows are every selector.
#[tokio::test]
async fn an_organization_without_insights_lists_none() {
    let emulator = administered_emulator();

    for (case, status_filter, _) in selection_rows() {
        let reply = list_organization_insights(&emulator, &request(&status_filter)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: an empty store must still answer, got {}",
            reply.raw
        );
        assert!(
            listed_ids(&reply).is_empty(),
            "case {case:?}: no insight can be listed, got {}",
            reply.raw
        );
        assert!(
            next_token(&reply).is_none(),
            "case {case:?}: a single page carries no NextToken, got {}",
            reply.raw
        );
    }
}

// =============================================================================
// 4. The account filters narrow the listing
// =============================================================================

/// RULE 4: `AccountIds` and `OrganizationalUnitIds` narrow the listing to the member
/// accounts they name — a filter naming none restricts nothing, a filter naming an
/// account the organization does not hold leaves none in scope, and an organizational
/// unit no member account belongs to leaves none either.
///
/// The rows differ in which filter is sent, whether it names the served member account
/// or another, and whether the list is empty (`ListInsightsAccountIdList` and
/// `ListInsightsOrganizationalUnitIdList` are both modelled len 0..1, so an empty list is
/// a well-formed request and one id is the most either can name).
#[tokio::test]
async fn the_account_filters_narrow_the_listing() {
    let emulator = seeded_emulator();
    let status_filter = json!({ "Any": { "Type": "REACTIVE", "StartTimeRange": {} } });
    let every_reactive = vec![
        "closed-reactive".to_string(),
        "ongoing-reactive".to_string(),
        "ongoing-reactive-latest".to_string(),
    ];

    let cases: [(&str, Value, Vec<String>); 6] = [
        ("no filter", request(&status_filter), every_reactive.clone()),
        (
            "an empty AccountIds restricts nothing",
            json!({ "StatusFilter": status_filter, "AccountIds": [] }),
            every_reactive.clone(),
        ),
        (
            "the served member account named",
            json!({ "StatusFilter": status_filter, "AccountIds": [served_account()] }),
            every_reactive.clone(),
        ),
        (
            "another account named",
            json!({ "StatusFilter": status_filter, "AccountIds": [OTHER_ACCOUNT] }),
            Vec::new(),
        ),
        (
            "an empty OrganizationalUnitIds restricts nothing",
            json!({ "StatusFilter": status_filter, "OrganizationalUnitIds": [] }),
            every_reactive.clone(),
        ),
        (
            "an organizational unit named",
            json!({ "StatusFilter": status_filter, "OrganizationalUnitIds": [ORGANIZATIONAL_UNIT] }),
            Vec::new(),
        ),
    ];

    for (case, body, expected_ids) in cases {
        let reply = list_organization_insights(&emulator, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the listing must succeed, got {}",
            reply.raw
        );
        assert_eq!(
            listed_ids(&reply),
            expected_ids,
            "case {case:?}: unexpected insights listed, got {}",
            reply.raw
        );
    }
}

// =============================================================================
// 5. Paging, cursor scope, and reading without changing anything
// =============================================================================

/// Walk every page of one listing at `page_size` per call, asserting each page respects
/// it, and return the ids in the order they were walked.
async fn walk(emulator: &Emulator, body: &Value, page_size: usize) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = body.clone();
        if let Some(cursor) = token.as_deref() {
            request["NextToken"] = json!(cursor);
        }
        let reply = list_organization_insights(emulator, &request).await;
        assert_eq!(
            reply.status, 200,
            "walking {request} at {page_size} per page must succeed, got {}",
            reply.raw
        );
        let page = listed_ids(&reply);
        assert!(
            page.len() <= page_size,
            "walking {request}: a page must carry at most {page_size} insights, got {}",
            page.len()
        );
        walked.extend(page);
        match next_token(&reply) {
            Some(cursor) => token = Some(cursor),
            None => return walked,
        }
    }
}

/// RULE 5: the listing pages at the `MaxResults` the request asks for, and walking every
/// page yields exactly the selection once, in the store's stable order — whatever the page
/// size, and whether or not it divides the selection.
///
/// The rows are the page sizes 1 through 4 (`ListInsightsMaxResults` is modelled range
/// 1..100, so 1 is its lower bound), the modelled maximum 100, and no `MaxResults` at all,
/// each over a selection of three insights and one of five.
#[tokio::test]
async fn the_listing_pages_at_the_requested_size() {
    let emulator = seeded_emulator();
    let selections: [(&str, Value, Vec<&str>); 2] = [
        (
            "three reactive insights",
            json!({ "Any": { "Type": "REACTIVE", "StartTimeRange": {} } }),
            vec!["closed-reactive", "ongoing-reactive", "ongoing-reactive-latest"],
        ),
        (
            "five proactive insights",
            json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": {} } }),
            vec![
                "closed-proactive-early",
                "closed-proactive-late",
                "closed-proactive-open-ended",
                "ongoing-proactive-early",
                "ongoing-proactive-late",
            ],
        ),
    ];

    for (case, status_filter, expected_ids) in selections {
        for max_results in [Some(1), Some(2), Some(3), Some(4), Some(100), None] {
            let mut body = request(&status_filter);
            if let Some(size) = max_results {
                body["MaxResults"] = json!(size);
            }
            // With no `MaxResults` the service pages at its own size, which is at least
            // the modelled maximum, so one page carries the whole selection.
            let page_size = max_results.unwrap_or(100) as usize;
            let walked = walk(&emulator, &body, page_size).await;
            assert_eq!(
                walked, expected_ids,
                "case {case:?} at MaxResults={max_results:?}: walking the pages must yield the selection once, got {walked:?}"
            );
        }
    }
}

/// RULE 5's cursor half: a `NextToken` names a page of ONE selection under ONE account
/// scope, so a cursor from another selector, another account filter, the account-scoped
/// `ListInsights` walk, or a selection that has since been emptied names no page and is
/// refused with HTTP 400 `ValidationException` rather than silently seeking elsewhere.
#[tokio::test]
async fn a_cursor_resolves_only_against_the_selection_it_was_issued_for() {
    let emulator = seeded_emulator();
    let reactive = json!({ "Any": { "Type": "REACTIVE", "StartTimeRange": {} } });
    let proactive = json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": {} } });

    let first = list_organization_insights(
        &emulator,
        &json!({ "StatusFilter": reactive, "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first page must succeed, got {}",
        first.raw
    );
    let cursor = next_token(&first).expect("a selection of three insights paged at 1 has a cursor");

    // The cursor this selection issued resolves against it.
    let second = list_organization_insights(
        &emulator,
        &json!({ "StatusFilter": reactive, "MaxResults": 1, "NextToken": cursor }),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "the cursor this listing issued must resolve, got {}",
        second.raw
    );
    assert_eq!(
        listed_ids(&second),
        vec!["ongoing-reactive".to_string()],
        "the second page must carry the second insight of the selection, got {}",
        second.raw
    );

    // A cursor of the account-scoped walk of the SAME filter, which is a different
    // collection: it must not resolve here.
    let account_scoped =
        list_insights(&emulator, &json!({ "StatusFilter": reactive, "MaxResults": 1 })).await;
    assert_eq!(
        account_scoped.status, 200,
        "the account-scoped first page must succeed, got {}",
        account_scoped.raw
    );
    let account_cursor =
        next_token(&account_scoped).expect("the account-scoped walk also issues a cursor");

    let foreign: [(&str, Value); 4] = [
        (
            "another selector",
            json!({ "StatusFilter": proactive, "MaxResults": 1, "NextToken": cursor }),
        ),
        (
            "another account filter",
            json!({
                "StatusFilter": reactive,
                "MaxResults": 1,
                "AccountIds": [served_account()],
                "NextToken": cursor,
            }),
        ),
        (
            "the account-scoped listing's cursor",
            json!({ "StatusFilter": reactive, "MaxResults": 1, "NextToken": account_cursor }),
        ),
        (
            "a cursor this service never issued",
            json!({
                "StatusFilter": reactive,
                "MaxResults": 1,
                "NextToken": "ffffffff-ffff-ffff-ffff-ffffffffffff",
            }),
        ),
    ];
    for (case, body) in foreign {
        let reply = list_organization_insights(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a cursor naming no page of this listing must be refused, got {}",
            reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {}",
            reply.raw
        );
    }

    // A cursor into a selection that has since shrunk away names no page either.
    let emptied = administered_emulator();
    emptied.seed_insights(vec![
        insight("a", "REACTIVE", "ONGOING", at(0), None),
        insight("b", "REACTIVE", "ONGOING", at(1), None),
    ]);
    let page = list_organization_insights(
        &emptied,
        &json!({ "StatusFilter": reactive, "MaxResults": 1 }),
    )
    .await;
    let stale_cursor = next_token(&page).expect("two insights paged at 1 have a cursor");
    // Re-seed both insights as CLOSED, so the ONGOING selection the cursor was issued
    // for holds nothing.
    emptied.seed_insights(vec![
        insight("a", "REACTIVE", "CLOSED", at(0), Some(at(2))),
        insight("b", "REACTIVE", "CLOSED", at(1), Some(at(3))),
    ]);
    let stale = list_organization_insights(
        &emptied,
        &json!({
            "StatusFilter": { "Ongoing": { "Type": "REACTIVE" } },
            "MaxResults": 1,
            "NextToken": stale_cursor,
        }),
    )
    .await;
    assert_eq!(
        stale.status, 400,
        "a cursor into an emptied selection must be refused, got {}",
        stale.raw
    );
    assert!(
        stale.names_error("ValidationException"),
        "the stale-cursor refusal must be a ValidationException, got {}",
        stale.raw
    );
}

/// RULE 5's read half: a read must not change what a later caller sees — neither the
/// listing itself, nor the insights it is derived from, nor the refusal a
/// non-administering caller gets.
///
/// The rows are every selector, read repeatedly and in turn, in an emulator where the
/// listing reports and in one where it refuses.
#[tokio::test]
async fn listing_organization_insights_leaves_the_store_unchanged() {
    for administered in [false, true] {
        let emulator = if administered {
            seeded_emulator()
        } else {
            let emulator = Emulator::new();
            emulator.seed_insights(selection_fixture());
            emulator
        };

        let mut first_bodies: Vec<(u16, String)> = Vec::new();
        for (case, status_filter, _) in selection_rows() {
            let reply = list_organization_insights(&emulator, &request(&status_filter)).await;
            assert_eq!(
                reply.status,
                if administered { 200 } else { 403 },
                "case administered={administered}/{case:?}: unexpected status, got {}",
                reply.raw
            );
            first_bodies.push((reply.status, reply.body.to_string()));
        }

        for round in 1..=3 {
            for (index, (case, status_filter, _)) in selection_rows().into_iter().enumerate() {
                let reply = list_organization_insights(&emulator, &request(&status_filter)).await;
                assert_eq!(
                    (reply.status, reply.body.to_string()),
                    first_bodies[index],
                    "case administered={administered}/{case:?} round {round}: a repeated read must answer the same, got {}",
                    reply.raw
                );
            }
        }

        // The insights the organization listing reads are still all there, as the
        // account-scoped listing sees them.
        let every_insight = json!({ "Any": { "Type": "PROACTIVE", "StartTimeRange": {} } });
        let reply = list_insights(&emulator, &request(&every_insight)).await;
        assert_eq!(
            listed_ids(&reply),
            vec![
                "closed-proactive-early".to_string(),
                "closed-proactive-late".to_string(),
                "closed-proactive-open-ended".to_string(),
                "ongoing-proactive-early".to_string(),
                "ongoing-proactive-late".to_string(),
            ],
            "case administered={administered}: the organization listing must not change the insight store, got {}",
            reply.raw
        );
    }
}
