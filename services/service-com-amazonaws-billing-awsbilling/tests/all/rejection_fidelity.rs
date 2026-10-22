// SPDX-License-Identifier: CC0-1.0

//! How this service REFUSES a billing-view request, end to end over the
//! awsJson1_0 wire.
//!
//! Each test here pins one rule, over a table that spans the class of inputs the
//! rule generalises over rather than one request:
//!
//! 1. A constraint refusal states the member PATH and the violated constraint —
//!    for a `@pattern`, the expression without the `^`/`$` anchors the model
//!    spells it with — and never the value that was rejected.
//! 2. An operation that identifies a billing view by ARN enforces the account
//!    boundary BEFORE any existence lookup, so an ARN outside the caller's own
//!    account is `AccessDeniedException` (403) whatever is stored.
//! 3. Creating a `CUSTOM` billing view is an organization-management-account
//!    action: any other caller is refused with `ValidationException` (400) and
//!    nothing is stored.
//! 4. A filter that names resources by identifier resolves every entry: one
//!    that names no view of this account refuses the whole request (400)
//!    instead of silently dropping out of the result set.
//! 5. Only a `CUSTOM` view may be edited or deleted, and eligibility is decided
//!    before existence: an absent ARN and the account's `PRIMARY` view are the
//!    same `ValidationException` (400).
//!
//! The harness (`call`, `emulator`, …) is the one `billing_views.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;

use crate::billing_views::{
    call, emulator, error_code, standalone_emulator_with_engine, ABSENT_ARN, ACCOUNT_ID,
    PRIMARY_ARN,
};

/// The `@pattern` of `BillingViewArn`, as the model declares it minus its
/// anchors. Source: `smithy_query(query="shape", shape="com.amazonaws.billing#BillingViewArn")`.
const ARN_PATTERN: &str =
    r"arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]";

/// The `@pattern` of `EnterpriseSupportBillingMonth`, minus its anchors.
/// Source: `smithy_query(query="operation", operation="GetEnterpriseSupportChargeSummary")`.
const MONTH_PATTERN: &str = r"\d{4}-(0[1-9]|1[0-2])";

/// The `@pattern` of `BillingViewName` (a `@sensitive` member), minus its
/// anchors. Source: `smithy_query(query="operation", operation="CreateBillingView")`.
const NAME_PATTERN: &str = r"[ a-zA-Z0-9_\+=\.\-@]+";

/// The `@pattern` of `BillingViewDescription` (a `@sensitive` member), minus its
/// anchors. Source: `smithy_query(query="operation", operation="CreateBillingView")`.
const DESCRIPTION_PATTERN: &str = r"([ a-zA-Z0-9_\+=\.\-@]+)?";

/// The `@pattern` of `ClientToken`, minus its anchors. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.billing#ClientToken")`.
const CLIENT_TOKEN_PATTERN: &str = r"[a-zA-Z0-9-]+";

/// The `@pattern` of `PageToken`, minus its anchors. Source:
/// `smithy_query(query="operation", operation="ListBillingViews")`.
const PAGE_TOKEN_PATTERN: &str = r"[-a-zA-Z0-9+=/_]+";

/// The `@pattern` of `AccountId`, minus its anchors. Source:
/// `smithy_query(query="operation", operation="ListBillingViews")`.
const ACCOUNT_ID_PATTERN: &str = r"[0-9]{12}";

/// How one row of the account-boundary table builds its request body from the
/// ARN under test.
type BodyOf = fn(&str) -> Value;

/// One row of the `arns`-filter table: the case, the filter it sends, and the
/// ARNs the call must select — `None` when the request must be refused.
type FilterRow = (&'static str, Vec<String>, Option<Vec<String>>);

/// A create request body that satisfies every modeled constraint.
fn create_body(name: &str, token: &str) -> Value {
    json!({ "name": name, "sourceViews": [PRIMARY_ARN], "clientToken": token })
}

/// The `message` an error body carries.
pub(crate) fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create one `CUSTOM` view through the API and return its ARN.
async fn create_view(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(router, "CreateBillingView", &create_body(name, name)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the harness create must answer 200, got {status} {created:?}"
    );
    created["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_constraint_refusal_names_the_member_and_the_constraint_and_never_the_value() {
    let router = emulator();

    // (case, operation, request, the offending value, the clause after
    // "Value … at '<member>' failed to satisfy constraint: ")
    //
    // The rows span every constraint class the model puts on this service's
    // members (pattern, length minimum and maximum, range, required) across six
    // operations and nine members, including the two `@sensitive` ones.
    let over_long_name = "n".repeat(129);
    let rows: Vec<(&str, &str, Value, &str, String, &str)> = vec![
        (
            "GetBillingView arn outside its pattern",
            "GetBillingView",
            json!({ "arn": "not-an-arn" }),
            "not-an-arn",
            format!("Member must satisfy regular expression pattern: {ARN_PATTERN}"),
            "arn",
        ),
        (
            "DeleteBillingView arn outside its pattern",
            "DeleteBillingView",
            json!({ "arn": "not-an-arn-at-all" }),
            "not-an-arn-at-all",
            format!("Member must satisfy regular expression pattern: {ARN_PATTERN}"),
            "arn",
        ),
        (
            "UpdateBillingView arn outside its pattern",
            "UpdateBillingView",
            json!({ "arn": "arn:aws:billing::000000000000:not-a-billing-view", "name": "fine" }),
            "not-a-billing-view",
            format!("Member must satisfy regular expression pattern: {ARN_PATTERN}"),
            "arn",
        ),
        (
            "a billingMonth with a month number no year has",
            "GetEnterpriseSupportChargeSummary",
            json!({ "billingMonth": "2024-13" }),
            "2024-13",
            format!("Member must satisfy regular expression pattern: {MONTH_PATTERN}"),
            "billingMonth",
        ),
        (
            "a billingMonth that is not a month at all",
            "GetEnterpriseSupportChargeSummary",
            json!({ "billingMonth": "last-july" }),
            "last-july",
            format!("Member must satisfy regular expression pattern: {MONTH_PATTERN}"),
            "billingMonth",
        ),
        (
            "a sensitive name outside its pattern",
            "CreateBillingView",
            json!({ "name": "bad/name!", "sourceViews": [PRIMARY_ARN] }),
            "bad/name!",
            format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            "name",
        ),
        (
            "a sensitive description outside its pattern",
            "CreateBillingView",
            json!({ "name": "fine", "description": "no semicolons; please", "sourceViews": [PRIMARY_ARN] }),
            "no semicolons; please",
            format!("Member must satisfy regular expression pattern: {DESCRIPTION_PATTERN}"),
            "description",
        ),
        (
            "a clientToken outside its pattern",
            "CreateBillingView",
            json!({ "name": "fine", "sourceViews": [PRIMARY_ARN], "clientToken": "not a token" }),
            "not a token",
            format!("Member must satisfy regular expression pattern: {CLIENT_TOKEN_PATTERN}"),
            "clientToken",
        ),
        (
            "a nextToken outside its pattern",
            "ListBillingViews",
            json!({ "nextToken": "token with spaces" }),
            "token with spaces",
            format!("Member must satisfy regular expression pattern: {PAGE_TOKEN_PATTERN}"),
            "nextToken",
        ),
        (
            "an ownerAccountId that is not twelve digits",
            "ListBillingViews",
            json!({ "ownerAccountId": "12345" }),
            "12345",
            format!("Member must satisfy regular expression pattern: {ACCOUNT_ID_PATTERN}"),
            "ownerAccountId",
        ),
        (
            "a name over its length maximum",
            "CreateBillingView",
            json!({ "name": over_long_name.clone(), "sourceViews": [PRIMARY_ARN] }),
            &over_long_name,
            "Member must have length less than or equal to 128".to_string(),
            "name",
        ),
        (
            "sourceViews under its length minimum",
            "CreateBillingView",
            json!({ "name": "fine", "sourceViews": [] }),
            "[]",
            "Member must have length greater than or equal to 1".to_string(),
            "sourceViews",
        ),
        (
            "maxResults above its range maximum",
            "ListBillingViews",
            json!({ "maxResults": 101 }),
            "101",
            "Member must have value less than or equal to 100".to_string(),
            "maxResults",
        ),
        (
            "maxResults below its range minimum",
            "ListBillingViews",
            json!({ "maxResults": 0 }),
            "0",
            "Member must have value greater than or equal to 1".to_string(),
            "maxResults",
        ),
    ];

    for (case, operation, request, offending, clause, member) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body:?}"
        );
        let stated = message(&body);
        let expected =
            format!("1 validation error detected: Value at '{member}' failed to satisfy constraint: {clause}");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(offending),
            "case {case:?}: the rejected value must not be echoed back, got {stated:?}"
        );
        assert!(
            !stated.contains('^') && !stated.contains('$'),
            "case {case:?}: the stated pattern carries no ^/$ anchors, got {stated:?}"
        );
    }

    // A required member that is absent has no submitted value at all, so it is
    // reported as the null it was.
    let (status, body) = call(&router, "GetBillingView", &json!({})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"arn absent\": expected 400 ValidationException, got {status} {body:?}"
    );
    assert_eq!(
        message(&body),
        "1 validation error detected: Value null at 'arn' failed to satisfy constraint: \
         Member must not be null",
        "case \"arn absent\": got {:?}",
        message(&body)
    );
}

#[tokio::test]
async fn a_constraint_refusal_on_a_list_entry_states_the_same_rule() {
    let router = emulator();

    // The member path of a list entry is the framework's, so these rows assert
    // the RULE rather than a spelled-out path: the envelope, the constraint, no
    // anchors and no echoed value.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "an arns entry outside the ARN pattern",
            "ListBillingViews",
            json!({ "arns": ["not-an-arn"] }),
            "not-an-arn",
            ARN_PATTERN,
        ),
        (
            "a sourceViews entry outside the ARN pattern",
            "CreateBillingView",
            json!({ "name": "fine", "sourceViews": ["billingview/primary"] }),
            "billingview/primary",
            ARN_PATTERN,
        ),
    ];

    for (case, operation, request, offending, pattern) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body:?}"
        );
        let stated = message(&body);
        assert!(
            stated.starts_with("1 validation error detected: Value at '"),
            "case {case:?}: expected the AWS envelope naming the member, got {stated:?}"
        );
        assert!(
            stated.contains(&format!(
                "failed to satisfy constraint: Member must satisfy regular expression pattern: \
                 {pattern}"
            )),
            "case {case:?}: expected the unanchored pattern {pattern:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(offending),
            "case {case:?}: the rejected value must not be echoed back, got {stated:?}"
        );
        assert!(
            !stated.contains('^') && !stated.contains('$'),
            "case {case:?}: the stated pattern carries no ^/$ anchors, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_outside_the_callers_account_is_refused_before_any_lookup() {
    let router = emulator();
    let custom_arn = create_view(&router, "boundary-view").await;

    // The rows vary the foreign account id, the partition the ARN names and the
    // resource id — including `primary`, which DOES exist in the caller's own
    // account, so the answer cannot be coming from a store lookup.
    let foreign_arns = [
        "arn:aws:billing::111122223333:billingview/primary",
        "arn:aws:billing::123456789012:billingview/does-not-exist",
        "arn:aws-us-gov:billing::111122223333:billingview/primary",
        "arn:aws-cn:billing::999999999999:billingview/custom-1",
    ];
    // Every implemented operation that identifies a billing view by ARN; the
    // last one reaches the ARN through the `arns` filter.
    let operations: [(&str, BodyOf); 4] = [
        ("GetBillingView", |arn| json!({ "arn": arn })),
        ("UpdateBillingView", |arn| json!({ "arn": arn, "name": "renamed" })),
        ("DeleteBillingView", |arn| json!({ "arn": arn, "force": true })),
        ("ListBillingViews", |arn| json!({ "arns": [arn] })),
    ];

    for arn in foreign_arns {
        for (operation, body_of) in operations {
            let case = format!("{operation} on {arn}");
            let (status, body) = call(&router, operation, &body_of(arn)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected 403 AccessDeniedException, got {status} {body:?}"
            );
            assert!(
                message(&body).contains(arn),
                "case {case:?}: the refusal is built from the requested ARN, got {:?}",
                message(&body)
            );
        }
    }

    // The positive control: the caller's own views answer as before, and the
    // refusals above changed nothing.
    for (case, arn) in [("own PRIMARY", PRIMARY_ARN), ("own CUSTOM", &custom_arn)] {
        let (status, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ARN in the caller's own account resolves, got {status} {got:?}"
        );
    }
    let (_, listed) = call(&router, "ListBillingViews", &json!({})).await;
    let arns: Vec<&str> = listed["billingViews"]
        .as_array()
        .expect("billingViews is a list")
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        arns,
        vec![custom_arn.as_str(), PRIMARY_ARN],
        "case \"after the refusals\": a refused cross-account call changes nothing, \
         listed {arns:?}"
    );
}

#[tokio::test]
async fn creating_a_view_is_refused_unless_the_caller_is_the_management_account() {
    let (router, engine) = standalone_emulator_with_engine();

    // Five concurrent creates, differing in everything the request can differ
    // in: name shape, whether a description is carried, how many source views
    // are named, how many tags are attached, and whether a client token is
    // sent. The gate is about the CALLER, so every one of them is refused with
    // the same code, and none of them stores anything.
    let bodies = [
        json!({ "name": "race-0", "sourceViews": [PRIMARY_ARN] }),
        json!({ "name": "race 1", "sourceViews": [PRIMARY_ARN], "clientToken": "race-1" }),
        json!({
            "name": "race-2",
            "description": "a described view",
            "sourceViews": [PRIMARY_ARN, format!("arn:aws:billing::{ACCOUNT_ID}:billingview/other")],
        }),
        json!({
            "name": "race_3",
            "sourceViews": [PRIMARY_ARN],
            "resourceTags": [{ "key": "team", "value": "finance" }],
        }),
        json!({
            "name": "race.4",
            "sourceViews": [PRIMARY_ARN],
            "dataFilterExpression": { "dimensions": { "key": "LINKED_ACCOUNT", "values": [ACCOUNT_ID] } },
        }),
    ];
    let (first, second, third, fourth, fifth) = tokio::join!(
        call(&router, "CreateBillingView", &bodies[0]),
        call(&router, "CreateBillingView", &bodies[1]),
        call(&router, "CreateBillingView", &bodies[2]),
        call(&router, "CreateBillingView", &bodies[3]),
        call(&router, "CreateBillingView", &bodies[4]),
    );

    let mut messages = std::collections::BTreeSet::new();
    for (index, (status, body)) in [first, second, third, fourth, fifth].iter().enumerate() {
        let case = format!("concurrent create {index}");
        assert_eq!(
            (*status, error_code(body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a non-management account creates nothing, got {status} {body:?}"
        );
        messages.insert(message(body));
    }
    assert_eq!(
        messages.len(),
        1,
        "case \"deterministic refusal\": every refusal is identical, got {messages:?}"
    );

    let (_, listed) = call(&router, "ListBillingViews", &json!({})).await;
    let arns: Vec<&str> = listed["billingViews"]
        .as_array()
        .expect("billingViews is a list")
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        arns,
        vec![PRIMARY_ARN],
        "case \"after the refusals\": only the account's PRIMARY view exists, listed {arns:?}"
    );
    assert_eq!(
        engine
            .state::<ComAmazonawsBillingAwsbillingState>()
            .lock()
            .billing_views
            .len(),
        1,
        "case \"after the refusals\": a refused create stores nothing"
    );

    // The rule is about the caller, not a blanket refusal: the management
    // account of the family creates the same view, and it reads back.
    engine
        .state::<ComAmazonawsBillingAwsbillingState>()
        .lock()
        .organization_management_account_id = Some(ACCOUNT_ID.to_string());
    let (status, created) = call(&router, "CreateBillingView", &bodies[0]).await;
    assert_eq!(
        status, 200,
        "case \"management account\": expected the create to go through, got {status} {created:?}"
    );
    let arn = created["arn"].as_str().unwrap_or_default().to_string();
    let (status, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        (status, got["billingView"]["name"].as_str()),
        (200, Some("race-0")),
        "case \"management account\": the created view reads back, got {status} {got:?}"
    );
}

#[tokio::test]
async fn the_arns_filter_resolves_every_entry_it_names() {
    let router = emulator();
    let first = create_view(&router, "filter-one").await;
    let second = create_view(&router, "filter-two").await;
    let unknown = ABSENT_ARN.to_string();
    let other_unknown = format!("arn:aws:billing::{ACCOUNT_ID}:billingview/also-absent");

    // (case, the arns filter, the ARNs the call must select — or None when the
    // request must be refused)
    let rows: Vec<FilterRow> = vec![
        ("one entry that names no view", vec![unknown.clone()], None),
        (
            "an unknown entry beside a known one",
            vec![first.clone(), unknown.clone()],
            None,
        ),
        (
            "a known entry after an unknown one",
            vec![unknown.clone(), first.clone()],
            None,
        ),
        (
            "two entries that name no view",
            vec![unknown.clone(), other_unknown.clone()],
            None,
        ),
        (
            "one entry that names a view",
            vec![first.clone()],
            Some(vec![first.clone()]),
        ),
        (
            "every entry names a view",
            vec![second.clone(), first.clone()],
            Some({
                let mut both = vec![first.clone(), second.clone()];
                both.sort();
                both
            }),
        ),
        (
            "the PRIMARY view by ARN",
            vec![PRIMARY_ARN.to_string()],
            Some(vec![PRIMARY_ARN.to_string()]),
        ),
        ("an empty filter names nothing", vec![], {
            let mut all = vec![first.clone(), second.clone(), PRIMARY_ARN.to_string()];
            all.sort();
            Some(all)
        }),
    ];

    for (case, arns, expected) in rows {
        let (status, body) = call(&router, "ListBillingViews", &json!({ "arns": arns })).await;
        match expected {
            None => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: an unknown ARN refuses the request, got {status} {body:?}"
                );
                let stated = message(&body);
                assert!(
                    arns.iter().any(|arn| stated.contains(arn)),
                    "case {case:?}: the refusal names the offending ARN, got {stated:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body:?}"
                );
                let got: Vec<String> = body["billingViews"]
                    .as_array()
                    .expect("billingViews is a list")
                    .iter()
                    .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
                    .collect();
                assert_eq!(got, expected, "case {case:?}: expected {expected:?}, got {got:?}");
            }
        }
    }
}

#[tokio::test]
async fn only_a_custom_view_may_be_edited_or_deleted() {
    let router = emulator();

    // (case, operation, the ARN, the request members beside `arn`, expected
    // status, expected code). The two refused kinds — an ARN no view is stored
    // under, and the account's PRIMARY view — are ONE answer, so a caller
    // cannot tell them apart.
    let rows: Vec<(&str, &str, String, Value, u16, &str)> = vec![
        (
            "update an absent ARN",
            "UpdateBillingView",
            ABSENT_ARN.to_string(),
            json!({ "name": "renamed" }),
            400,
            "ValidationException",
        ),
        (
            "update the PRIMARY view",
            "UpdateBillingView",
            PRIMARY_ARN.to_string(),
            json!({ "name": "renamed" }),
            400,
            "ValidationException",
        ),
        (
            "update the PRIMARY view's description only",
            "UpdateBillingView",
            PRIMARY_ARN.to_string(),
            json!({ "description": "mine now" }),
            400,
            "ValidationException",
        ),
        (
            "delete an absent ARN",
            "DeleteBillingView",
            ABSENT_ARN.to_string(),
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "delete the PRIMARY view",
            "DeleteBillingView",
            PRIMARY_ARN.to_string(),
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "delete the PRIMARY view with force",
            "DeleteBillingView",
            PRIMARY_ARN.to_string(),
            json!({ "force": true }),
            400,
            "ValidationException",
        ),
    ];

    for (case, operation, arn, members, expected_status, expected_code) in rows {
        let mut request = members;
        request["arn"] = json!(arn);
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {body:?}"
        );
        assert!(
            message(&body).contains("Custom Billing View"),
            "case {case:?}: the refusal states the eligibility rule, got {:?}",
            message(&body)
        );
    }

    // The PRIMARY view came through every refusal untouched, and a CUSTOM view
    // takes both calls.
    let (status, primary) = call(&router, "GetBillingView", &json!({ "arn": PRIMARY_ARN })).await;
    assert_eq!(
        (status, primary["billingView"]["name"].as_str()),
        (200, Some("Primary View")),
        "case \"PRIMARY after the refusals\": it is unchanged, got {status} {primary:?}"
    );

    let custom_arn = create_view(&router, "editable-view").await;
    let (status, updated) = call(
        &router,
        "UpdateBillingView",
        &json!({ "arn": custom_arn, "name": "edited" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"update a CUSTOM view\": got {status} {updated:?}"
    );
    let (status, deleted) =
        call(&router, "DeleteBillingView", &json!({ "arn": custom_arn })).await;
    assert_eq!(
        status, 200,
        "case \"delete a CUSTOM view\": got {status} {deleted:?}"
    );
}
