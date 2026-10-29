// SPDX-License-Identifier: CC0-1.0

//! What a page token binds itself to where the walk reads a query member by
//! MEMBERSHIP: the SET that member denotes, never the order or the repeats the
//! request spelled it with.
//!
//! The rule under test: every paginated listing of this service selects the
//! sequence a page is a window onto with membership tests — an action type is
//! previewed when the requested list CONTAINS it, an account is in scope when
//! the scope list holds it anywhere, a filter holds when the item's value is
//! among that filter's values and every filter of the list has to hold — and
//! serves the selected items in the store's record order. So neither the order
//! of such a list nor an entry repeated in it can change which items a walk
//! serves or the order it serves them in: two such spellings name ONE selection
//! and continue ONE walk, while a selection that really is different is still
//! refused the token.
//!
//! The table spans the class rather than one request: the member re-spelled (a
//! preview's `recommendedActionTypes`, its `organizationScope.accountIds`, the
//! filter list of each filtered listing), the kind of re-spelling (a permuted
//! list, a repeated entry, reordered filters, a filter spelled twice) and the
//! operation it is driven through — every paginated listing that takes such a
//! member, the plain ones and the ones that GROUP before paging alike. Each row
//! is driven the way a client drives it: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::caller_account_id;
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{call, call_ok, is_next_token};
use crate::page_token_refusals::{assert_token_refused, paginated_operations};
use crate::shared_page_walk::{emulator_with_a_fixture, member_account_ids};

/// The two `RecommendedActionType` values the model declares
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), both of which
/// the fixture's inventory holds actions of.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// The model's only `ResourceType` value
/// (`smithy_query(query="shape",shape="ResourceType")`).
const EBS_VOLUME: &str = "EbsVolume";

/// Every paginated preview operation, as `(operation, the output member its page
/// lands in)`. Both are driven for every preview row, so the two cannot drift
/// apart over which spellings continue one walk.
const PREVIEWS: [(&str, &str); 2] = [
    ("ListAutomationRulePreview", "previewResults"),
    ("ListAutomationRulePreviewSummaries", "previewResultSummaries"),
];

/// The page size every walk below is taken at: the smallest one, so a selection
/// of more than one item leaves a token on its first page.
const ONE_ITEM: i32 = 1;

/// A page size above every total this fixture leaves, inside the modelled
/// `maxResults` range (`range(min: 1, max: 100)`), so one request serves a whole
/// selection.
const WHOLE_SELECTION: i32 = 100;

/// A ceiling on the pages of one walk, so a non-advancing token fails a case
/// instead of hanging it.
const MAX_PAGES: usize = 50;

/// One row: two requests naming ONE selection, and the listing they are driven
/// through.
struct Row {
    case: String,
    operation: &'static str,
    items: &'static str,
    one: Value,
    other: Value,
}

/// An account-scoped preview naming `action_types`.
fn account_preview(action_types: Value) -> Value {
    json!({ "ruleType": "AccountRule", "recommendedActionTypes": action_types })
}

/// An organization-wide preview of both action types, scoped to `account_ids`.
fn organization_preview(account_ids: Value) -> Value {
    json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        "organizationScope": { "accountIds": account_ids },
    })
}

/// A filtered listing's request.
fn filtered(filters: Value) -> Value {
    json!({ "filters": filters })
}

/// The spellings a PREVIEW selection is re-spelled with, as `(case, one
/// spelling, the other)`: the two membership-read list members the selection
/// carries, each permuted and each with an entry repeated.
fn preview_spellings(member_account: &str) -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "the action types permuted",
            account_preview(json!([UPGRADE, SNAPSHOT_AND_DELETE])),
            account_preview(json!([SNAPSHOT_AND_DELETE, UPGRADE])),
        ),
        (
            "an action type repeated",
            account_preview(json!([UPGRADE, SNAPSHOT_AND_DELETE])),
            account_preview(json!([
                SNAPSHOT_AND_DELETE,
                UPGRADE,
                UPGRADE,
                SNAPSHOT_AND_DELETE
            ])),
        ),
        (
            "the scope account ids permuted",
            organization_preview(json!([caller_account_id(), member_account])),
            organization_preview(json!([member_account, caller_account_id()])),
        ),
        (
            "a scope account id repeated",
            organization_preview(json!([caller_account_id(), member_account])),
            organization_preview(json!([
                member_account,
                member_account,
                caller_account_id()
            ])),
        ),
    ]
}

/// The spellings a FILTERED listing's filter list is re-spelled with, as `(case,
/// operation, the output member its page lands in, one spelling, the other)`.
///
/// Every filtered listing of this service is a row — the three kinds of filter
/// the model declares (of a rule, of a recommended action, of an automation
/// event) and the summaries that group before paging — and the re-spellings span
/// a permuted value list, a reordered filter list and a filter spelled twice.
fn filter_spellings() -> Vec<(&'static str, &'static str, &'static str, Value, Value)> {
    let action_types = |first: &str, second: &str| {
        json!([{ "name": "RecommendedActionType", "values": [first, second] }])
    };
    let event_statuses = |values: Value| json!([{ "name": "EventStatus", "values": values }]);
    vec![
        (
            "the values of one filter permuted",
            "ListRecommendedActions",
            "recommendedActions",
            filtered(action_types(UPGRADE, SNAPSHOT_AND_DELETE)),
            filtered(action_types(SNAPSHOT_AND_DELETE, UPGRADE)),
        ),
        (
            "the filters reordered",
            "ListRecommendedActions",
            "recommendedActions",
            filtered(json!([
                { "name": "RecommendedActionType", "values": [UPGRADE, SNAPSHOT_AND_DELETE] },
                { "name": "ResourceType", "values": [EBS_VOLUME] },
            ])),
            filtered(json!([
                { "name": "ResourceType", "values": [EBS_VOLUME] },
                { "name": "RecommendedActionType", "values": [SNAPSHOT_AND_DELETE, UPGRADE] },
            ])),
        ),
        (
            "one filter spelled twice",
            "ListRecommendedActions",
            "recommendedActions",
            filtered(json!([{ "name": "ResourceType", "values": [EBS_VOLUME] }])),
            filtered(json!([
                { "name": "ResourceType", "values": [EBS_VOLUME] },
                { "name": "ResourceType", "values": [EBS_VOLUME] },
            ])),
        ),
        (
            "the values of one filter permuted",
            "ListRecommendedActionSummaries",
            "recommendedActionSummaries",
            filtered(action_types(UPGRADE, SNAPSHOT_AND_DELETE)),
            filtered(action_types(SNAPSHOT_AND_DELETE, UPGRADE)),
        ),
        (
            "the values of one filter permuted",
            "ListAutomationRules",
            "automationRules",
            filtered(json!([{ "name": "Status", "values": ["Active", "Inactive"] }])),
            filtered(json!([{ "name": "Status", "values": ["Inactive", "Active"] }])),
        ),
        (
            "the filters reordered",
            "ListAutomationRules",
            "automationRules",
            filtered(json!([
                { "name": "Status", "values": ["Active"] },
                { "name": "RuleType", "values": ["AccountRule"] },
            ])),
            filtered(json!([
                { "name": "RuleType", "values": ["AccountRule"] },
                { "name": "Status", "values": ["Active"] },
            ])),
        ),
        (
            "the values of one filter permuted",
            "ListAutomationEvents",
            "automationEvents",
            filtered(event_statuses(
                json!(["Complete", "Failed", "RollbackComplete"]),
            )),
            filtered(event_statuses(
                json!(["RollbackComplete", "Complete", "Failed"]),
            )),
        ),
        (
            "the filters reordered, one of them spelled twice",
            "ListAutomationEvents",
            "automationEvents",
            filtered(json!([
                { "name": "EventStatus", "values": ["Complete", "Failed", "RollbackComplete"] },
                { "name": "EventType", "values": [SNAPSHOT_AND_DELETE] },
            ])),
            filtered(json!([
                { "name": "EventType", "values": [SNAPSHOT_AND_DELETE] },
                { "name": "EventStatus", "values": ["Failed", "RollbackComplete", "Complete"] },
                { "name": "EventType", "values": [SNAPSHOT_AND_DELETE] },
            ])),
        ),
        (
            "the values of one filter permuted",
            "ListAutomationEventSummaries",
            "automationEventSummaries",
            filtered(event_statuses(
                json!(["Complete", "Failed", "RollbackComplete"]),
            )),
            filtered(event_statuses(
                json!(["RollbackComplete", "Failed", "Complete"]),
            )),
        ),
    ]
}

/// Every row of the table: the preview spellings through both preview
/// operations, and each filtered listing's own.
fn rows(member_account: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (case, one, other) in preview_spellings(member_account) {
        for (operation, items) in PREVIEWS {
            rows.push(Row {
                case: format!("{operation}: {case}"),
                operation,
                items,
                one: one.clone(),
                other: other.clone(),
            });
        }
    }
    for (case, operation, items, one, other) in filter_spellings() {
        rows.push(Row {
            case: format!("{operation}: {case}"),
            operation,
            items,
            one,
            other,
        });
    }
    rows
}

/// Requests whose selections really DIFFER, as `(case, operation, the request a
/// token is taken under, the request it is then presented to)`.
///
/// The counterpart of the table above: normalising a membership-read list to the
/// set it denotes must not collapse two selections that are not one. The rows
/// span a narrowed value list, a narrowed account scope, and — the case the
/// normalisation could most easily lose — one filter offering two alternatives
/// against two filters of the same name requiring both, which select
/// differently.
fn different_selections(member_account: &str) -> Vec<(&'static str, &'static str, Value, Value)> {
    vec![
        (
            "an action type dropped",
            "ListAutomationRulePreview",
            account_preview(json!([UPGRADE, SNAPSHOT_AND_DELETE])),
            account_preview(json!([UPGRADE])),
        ),
        (
            "an action type dropped",
            "ListAutomationRulePreviewSummaries",
            account_preview(json!([UPGRADE, SNAPSHOT_AND_DELETE])),
            account_preview(json!([SNAPSHOT_AND_DELETE])),
        ),
        (
            "an account dropped from the scope",
            "ListAutomationRulePreview",
            organization_preview(json!([caller_account_id(), member_account])),
            organization_preview(json!([member_account])),
        ),
        (
            "a filter value dropped",
            "ListRecommendedActions",
            filtered(
                json!([{ "name": "RecommendedActionType", "values": [UPGRADE, SNAPSHOT_AND_DELETE] }]),
            ),
            filtered(json!([{ "name": "RecommendedActionType", "values": [UPGRADE] }])),
        ),
        (
            "two alternatives of one filter against two filters of that name",
            "ListAutomationRules",
            filtered(json!([{ "name": "Status", "values": ["Active", "Inactive"] }])),
            filtered(json!([
                { "name": "Status", "values": ["Active"] },
                { "name": "Status", "values": ["Inactive"] },
            ])),
        ),
        (
            "a filter value dropped",
            "ListAutomationEvents",
            filtered(json!([{ "name": "EventStatus", "values": ["Complete", "Failed"] }])),
            filtered(json!([{ "name": "EventStatus", "values": ["Complete"] }])),
        ),
        (
            "a filter value dropped",
            "ListAutomationEventSummaries",
            filtered(json!([{ "name": "EventStatus", "values": ["Complete", "Failed"] }])),
            filtered(json!([{ "name": "EventStatus", "values": ["Failed"] }])),
        ),
    ]
}

/// The requirement a listing reports for a token not taken for this call's
/// query, from the one table of this service's paginated operations.
fn token_requirement(operation: &str) -> &'static str {
    // The id only fills the step listing's request, which no row below drives;
    // it has the modelled `EventId` form (`^[0-9A-Za-z]{16}$`).
    paginated_operations("Ev0000000000000a")
        .into_iter()
        .find(|(name, _, _)| *name == operation)
        .map(|(_, _, requirement)| requirement)
        .unwrap_or_else(|| panic!("{operation:?} must be a paginated operation of this service"))
}

/// The items one response reports under `member_name`, as the walks below
/// compare them: everything each item carries except the window a summary says
/// it was computed over, whose open end is the instant of the CALL and so
/// differs between two requests by design.
fn reported_items(body: &Value, member_name: &str) -> Vec<Value> {
    body.get(member_name)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let mut item = item.clone();
            if let Some(members) = item.as_object_mut() {
                members.remove("timePeriod");
            }
            item
        })
        .collect()
}

/// The `nextToken` a response carries, asserting its modelled form.
fn token_of(body: &Value, case: &str) -> Option<String> {
    let token = body.get("nextToken").and_then(Value::as_str)?;
    assert!(
        is_next_token(token),
        "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
    );
    Some(token.to_string())
}

/// Everything `request` selects, in one page.
async fn whole_selection(
    router: &MultiServiceRouter,
    operation: &str,
    items: &str,
    request: &Value,
    case: &str,
) -> Vec<Value> {
    let mut whole = request.clone();
    whole["maxResults"] = json!(WHOLE_SELECTION);
    let body = call_ok(router, operation, &whole, case).await;
    assert_eq!(
        token_of(&body, case),
        None,
        "case {case:?}: the fixture must leave this selection fewer than {WHOLE_SELECTION} items, \
         got {body}"
    );
    reported_items(&body, items)
}

/// Walk `operation` one item at a time, taking the first page with `first` and
/// every later page with `resumed_with`, and answer the items served.
async fn walk_resuming_with(
    router: &MultiServiceRouter,
    operation: &str,
    items: &str,
    first: &Value,
    resumed_with: &Value,
    case: &str,
) -> Vec<Value> {
    let mut served: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = if token.is_none() {
            first.clone()
        } else {
            resumed_with.clone()
        };
        request["maxResults"] = json!(ONE_ITEM);
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {} of the walk must be served, got {status} and {body}",
            pages + 1
        );
        served.extend(reported_items(&body, items));
        pages += 1;
        assert!(
            pages <= MAX_PAGES,
            "case {case:?}: the walk must terminate, got {} items over {pages} pages",
            served.len()
        );
        token = token_of(&body, case);
        if token.is_none() {
            break;
        }
    }
    served
}

/// A membership-read list re-spelled — permuted, or carrying a repeat — is one
/// selection: a walk that switches spelling after its first page serves exactly
/// the items the selection holds, in order, none served twice and none skipped.
#[tokio::test]
async fn one_selection_spelled_two_ways_continues_one_page_walk() {
    let (router, _) = emulator_with_a_fixture().await;
    let member_account = member_account_ids(1)
        .pop()
        .expect("the fixture enrolls at least one member account");

    for row in rows(&member_account) {
        // Both directions: either spelling may be the one that minted the token
        // the other resumes with.
        for (first, resumed_with) in [(&row.one, &row.other), (&row.other, &row.one)] {
            let case = row.case.as_str();
            let expected = whole_selection(&router, row.operation, row.items, first, case).await;
            assert!(
                expected.len() > 1,
                "case {case:?}: the fixture must leave this selection more than one item, so its \
                 first page carries a token, got {expected:?}"
            );
            let also_expected =
                whole_selection(&router, row.operation, row.items, resumed_with, case).await;
            assert_eq!(
                also_expected, expected,
                "case {case:?}: the two spellings must select the same items; expected \
                 {expected:?}, got {also_expected:?}"
            );

            let served = walk_resuming_with(
                &router,
                row.operation,
                row.items,
                first,
                resumed_with,
                case,
            )
            .await;
            assert_eq!(
                served, expected,
                "case {case:?}: a walk that switches spelling after its first page must serve the \
                 selection exactly once, in order; expected {expected:?}, got {served:?}"
            );
        }
    }
}

/// A token taken for a selection that really is different is still refused, with
/// the requirement that listing reports and without echoing the token — while
/// the selection that issued it resumes with it.
#[tokio::test]
async fn a_token_taken_for_a_different_selection_is_still_refused() {
    let (router, _) = emulator_with_a_fixture().await;
    let member_account = member_account_ids(1)
        .pop()
        .expect("the fixture enrolls at least one member account");

    for (selections, operation, issuing, other) in different_selections(&member_account) {
        let case = format!("{operation}: {selections}");
        let case = case.as_str();
        let mut first = issuing.clone();
        first["maxResults"] = json!(ONE_ITEM);
        let first_page = call_ok(&router, operation, &first, case).await;
        let token = token_of(&first_page, case).unwrap_or_else(|| {
            panic!("case {case:?}: the first page must carry a token, got {first_page}")
        });

        let mut presented = other.clone();
        presented["maxResults"] = json!(ONE_ITEM);
        let message = assert_token_refused(
            &router,
            operation,
            &presented,
            &token,
            token_requirement(operation),
            case,
        )
        .await;
        assert!(
            !message.contains(&token),
            "case {case:?}: the refusal must not echo the rejected token, got {message:?}"
        );

        // The same token on the selection that issued it is served, so the
        // refusal above is the selection check and not a blanket rejection.
        let mut resumed = first.clone();
        resumed["nextToken"] = json!(token);
        let (status, body) = call(&router, operation, &resumed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the issuing selection must resume with its own token, got {status} and \
             {body}"
        );
    }
}
