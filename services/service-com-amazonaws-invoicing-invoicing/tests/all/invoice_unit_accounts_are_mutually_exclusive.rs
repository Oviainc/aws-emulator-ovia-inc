// SPDX-License-Identifier: CC0-1.0

//! An invoice unit's linked accounts are MUTUALLY EXCLUSIVE, driven end to end over this
//! service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson body, so what
//! these tests see is exactly what an SDK client sees.
//!
//! THE RULE: an account is a linked account of at most ONE invoice unit, and is named at
//! most once in the rule of the unit it is linked into — the model documents the resource
//! as "a set of mutually exclusive accounts that correspond to your business entity"
//! (`smithy_query(query="shape", shape="InvoiceUnit")`) and `LinkedAccounts` as "the list
//! of `LINKED_ACCOUNT` IDs where charges are included within the invoice unit"
//! (`smithy_query(query="shape", shape="InvoiceUnitRule")`). Every write of an invoice
//! unit — `CreateInvoiceUnit` and `UpdateInvoiceUnit` alike — refuses a rule that breaks
//! it with the model's own `ValidationExceptionReason$mutualExclusionError`, and writes
//! nothing.
//!
//! Exclusivity is over the units that EXIST and over their CURRENT rules: an account a
//! live unit no longer links (dropped by an update, or taken away with a deleted unit) is
//! linkable again, and `InvoiceReceiver` and `BillSourceAccounts` are not linked accounts
//! at all.
//!
//! The tables below span what the rule generalises over: where the offending account sits
//! in the request's list, how many accounts the list carries, which unit already holds it
//! and where in that unit's list, how an account became free again, and which of the two
//! write paths is being driven.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), shape name
/// only.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The explanatory text of a failure body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `InvoiceUnitArn` of a response that carries one.
fn arn_of(body: &Value) -> String {
    body.get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {body}"))
        .to_string()
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`:
/// `smithy_query(query="shape", shape="AccountIdString")`), derived from an index so each
/// one is distinct.
fn account(index: u32) -> String {
    format!("{:012}", 700 + index)
}

/// Several account ids, by index.
fn accounts(indices: &[u32]) -> Vec<String> {
    indices.iter().copied().map(account).collect()
}

/// A `CreateInvoiceUnit` body: the unit's name and the accounts its rule links.
fn create_body(name: &str, linked: &[String]) -> Value {
    json!({
        "Name": name,
        "InvoiceReceiver": account(0),
        "TaxInheritanceDisabled": false,
        "Rule": {"LinkedAccounts": linked},
    })
}

/// Create one invoice unit linking `linked`, returning its ARN.
async fn create_unit(router: &MultiServiceRouter, name: &str, linked: &[String]) -> String {
    arn_of(&ok(router, "CreateInvoiceUnit", &create_body(name, linked)).await)
}

/// The `Rule.LinkedAccounts` `GetInvoiceUnit` reports for `arn`.
async fn linked_of(router: &MultiServiceRouter, arn: &str) -> Value {
    let got = ok(router, "GetInvoiceUnit", &json!({"InvoiceUnitArn": arn})).await;
    got.get("Rule")
        .and_then(|rule| rule.get("LinkedAccounts"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// How many invoice units exist right now.
async fn live_unit_count(router: &MultiServiceRouter) -> usize {
    let listed = ok(router, "ListInvoiceUnits", &json!({})).await;
    listed["InvoiceUnits"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The refusal the mutual-exclusion rule answers an overlapping write with: the account
/// the write was refused over, and the invoice unit that already links it.
fn already_linked_message(offender: &str) -> String {
    format!("Account <{offender}> is already linked to another invoice unit")
}

/// The refusal the same rule answers a rule that names one account twice with.
fn repeated_message(offender: &str) -> String {
    format!("Account <{offender}> is listed more than once in the invoice unit rule")
}

/// Assert `body` is the modelled mutual-exclusion refusal: HTTP 400, the service's
/// `ValidationException`, the model's own reason, the stated message, and the unit that
/// already holds the account in the shape's own ARN member.
fn assert_mutual_exclusion(
    case: &str,
    status: u16,
    body: &Value,
    expected_message: &str,
    holder: Option<&str>,
) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected a ValidationException, got {body}"
    );
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("mutualExclusionError"),
        "case {case}: expected the modelled mutualExclusionError reason, got {body}"
    );
    assert_eq!(
        error_message(body),
        expected_message,
        "case {case}: expected the message {expected_message:?}, got {body}"
    );
    assert_eq!(
        body.get("resourceName").and_then(Value::as_str),
        holder,
        "case {case}: expected the holding unit {holder:?} in resourceName, got {body}"
    );
}

/// One overlap row: what the unit already in state links, what the refused create asks
/// for, and which account of it is the one reported.
#[derive(Debug)]
struct OverlapCase {
    label: &'static str,
    /// The accounts the unit already in state links.
    held: &'static [u32],
    /// The accounts the create under test asks to link.
    requested: &'static [u32],
    /// The account the refusal must name: the first one of `requested`, in request order,
    /// that `held` already links.
    offender: u32,
}

/// Rows differing along where the overlap sits: which entry of the request names the held
/// account (only, first, middle, last), how long each list is, where in the HOLDER's list
/// the account sits, and a request that overlaps on two accounts at once.
const OVERLAPS: &[OverlapCase] = &[
    OverlapCase {
        label: "one account against one",
        held: &[1],
        requested: &[1],
        offender: 1,
    },
    OverlapCase {
        label: "the first of three requested",
        held: &[1],
        requested: &[1, 2, 3],
        offender: 1,
    },
    OverlapCase {
        label: "the middle of three requested",
        held: &[2],
        requested: &[1, 2, 3],
        offender: 2,
    },
    OverlapCase {
        label: "the last of three requested",
        held: &[3],
        requested: &[1, 2, 3],
        offender: 3,
    },
    OverlapCase {
        label: "the last entry of the holder's list",
        held: &[4, 5, 6],
        requested: &[6],
        offender: 6,
    },
    OverlapCase {
        label: "the middle entry of the holder's list",
        held: &[4, 5, 6],
        requested: &[7, 5],
        offender: 5,
    },
    OverlapCase {
        label: "two overlapping accounts: the first in request order is reported",
        held: &[8, 9],
        requested: &[9, 8],
        offender: 9,
    },
];

/// THE RULE on the create path: a create naming an account a live invoice unit already
/// links is refused with the model's own `mutualExclusionError`, and stores nothing.
#[tokio::test]
async fn a_create_naming_an_account_another_unit_links_is_refused() {
    for case in OVERLAPS {
        let router = emulator();
        let holder = create_unit(&router, "holder", &accounts(case.held)).await;

        let (status, body) = call(
            &router,
            "CreateInvoiceUnit",
            &create_body("newcomer", &accounts(case.requested)),
        )
        .await;
        assert_mutual_exclusion(
            case.label,
            status,
            &body,
            &already_linked_message(&account(case.offender)),
            Some(holder.as_str()),
        );

        // The refused create stored nothing: the registry still holds the one unit, and
        // the name it asked for is free.
        assert_eq!(
            live_unit_count(&router).await,
            1,
            "case {}: the refused create must not have stored a unit",
            case.label
        );
        let listed = ok(
            &router,
            "ListInvoiceUnits",
            &json!({"Filters": {"Names": ["newcomer"]}}),
        )
        .await;
        assert_eq!(
            listed["InvoiceUnits"].as_array().map(Vec::len),
            Some(0),
            "case {}: the refused name must name no unit, got {listed}",
            case.label
        );
    }
}

/// One row of the repeated-account table: the list a rule states and the account it
/// states twice.
#[derive(Debug)]
struct RepeatCase {
    label: &'static str,
    requested: &'static [u32],
    offender: u32,
}

/// Rows differing along how the repeat is shaped: a list that is nothing but one account
/// twice, a repeat at the end, one separated by other accounts, one repeated three times,
/// and a longer list whose repeat is in the middle.
const REPEATS: &[RepeatCase] = &[
    RepeatCase {
        label: "the same account twice, alone",
        requested: &[1, 1],
        offender: 1,
    },
    RepeatCase {
        label: "a repeat at the end of the list",
        requested: &[1, 2, 2],
        offender: 2,
    },
    RepeatCase {
        label: "a repeat separated by another account",
        requested: &[1, 2, 1],
        offender: 1,
    },
    RepeatCase {
        label: "the same account three times",
        requested: &[3, 3, 3],
        offender: 3,
    },
    RepeatCase {
        label: "a repeat in the middle of a longer list",
        requested: &[1, 2, 3, 2, 4, 5],
        offender: 2,
    },
];

/// THE SAME RULE inside ONE rule: an invoice unit is a SET of accounts, so a rule naming
/// one account twice is refused with the same reason — and the refusal names no other
/// unit, because no other unit is involved.
#[tokio::test]
async fn a_rule_naming_one_account_twice_is_refused() {
    for case in REPEATS {
        let router = emulator();

        let (status, body) = call(
            &router,
            "CreateInvoiceUnit",
            &create_body("repeater", &accounts(case.requested)),
        )
        .await;
        assert_mutual_exclusion(
            case.label,
            status,
            &body,
            &repeated_message(&account(case.offender)),
            None,
        );
        assert_eq!(
            live_unit_count(&router).await,
            0,
            "case {}: the refused create must not have stored a unit",
            case.label
        );

        // The same accounts, stated once each, are accepted: what was refused is the
        // repeat and not the accounts.
        let mut distinct: Vec<u32> = case.requested.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        let arn = create_unit(&router, "repeater", &accounts(&distinct)).await;
        assert_eq!(
            linked_of(&router, &arn).await,
            json!(accounts(&distinct)),
            "case {}: the deduplicated rule must be stored as stated",
            case.label
        );
    }
}

/// One accepted row: how state is arranged, and the rule a create may then link.
#[derive(Debug, Clone, Copy)]
enum FreeAccount {
    /// An account no stored configuration names at all.
    NamedNowhere,
    /// An account another unit names as its `InvoiceReceiver` — "the account that
    /// receives invoices related to the invoice unit", which is not a linked account.
    AnotherUnitsInvoiceReceiver,
    /// An account another unit names only in `BillSourceAccounts`, which the model
    /// documents as accounts that may be "payer accounts from other organizations".
    AnotherUnitsBillSource,
    /// An account an `UpdateInvoiceUnit` dropped from the rule that linked it.
    DroppedByAnUpdate,
    /// An account whose only invoice unit was deleted.
    TakenAwayByADelete,
}

/// THE OTHER HALF of the rule: an account NO live unit's current rule links is accepted,
/// whichever way it came to be free — and the unit that was created reads back with the
/// rule it was created with.
#[tokio::test]
async fn an_account_no_live_rule_links_is_accepted() {
    let subject = account(42);
    for arrangement in [
        FreeAccount::NamedNowhere,
        FreeAccount::AnotherUnitsInvoiceReceiver,
        FreeAccount::AnotherUnitsBillSource,
        FreeAccount::DroppedByAnUpdate,
        FreeAccount::TakenAwayByADelete,
    ] {
        let router = emulator();
        match arrangement {
            FreeAccount::NamedNowhere => {}
            FreeAccount::AnotherUnitsInvoiceReceiver => {
                ok(
                    &router,
                    "CreateInvoiceUnit",
                    &json!({
                        "Name": "receiver-unit",
                        "InvoiceReceiver": subject,
                        "TaxInheritanceDisabled": false,
                        "Rule": {"LinkedAccounts": accounts(&[1])},
                    }),
                )
                .await;
            }
            FreeAccount::AnotherUnitsBillSource => {
                ok(
                    &router,
                    "CreateInvoiceUnit",
                    &json!({
                        "Name": "bill-source-unit",
                        "InvoiceReceiver": account(0),
                        "TaxInheritanceDisabled": false,
                        "Rule": {
                            "LinkedAccounts": accounts(&[1]),
                            "BillSourceAccounts": [subject.clone()],
                        },
                    }),
                )
                .await;
            }
            FreeAccount::DroppedByAnUpdate => {
                let holder =
                    create_unit(&router, "dropper", &[subject.clone(), account(1)]).await;
                ok(
                    &router,
                    "UpdateInvoiceUnit",
                    &json!({
                        "InvoiceUnitArn": holder,
                        "Rule": {"LinkedAccounts": accounts(&[1])},
                        "ClientToken": "drop-the-subject",
                    }),
                )
                .await;
            }
            FreeAccount::TakenAwayByADelete => {
                let holder = create_unit(&router, "doomed", std::slice::from_ref(&subject)).await;
                ok(
                    &router,
                    "DeleteInvoiceUnit",
                    &json!({"InvoiceUnitArn": holder, "ClientToken": "delete-the-holder"}),
                )
                .await;
            }
        }

        let linked = vec![subject.clone(), account(2)];
        let (status, body) = call(
            &router,
            "CreateInvoiceUnit",
            &create_body("newcomer", &linked),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arrangement:?}: an account no live rule links is accepted, got {status} {body}"
        );
        // Round trip: the rule the create stated is what a read reports.
        let arn = arn_of(&body);
        assert_eq!(
            linked_of(&router, &arn).await,
            json!(linked),
            "case {arrangement:?}: the created unit must report the rule it was created with"
        );
    }
}

/// A rule with NO accounts to be exclusive about is accepted in every shape the model
/// admits: an empty `LinkedAccounts` list, no `LinkedAccounts` member at all, and a rule
/// stating only `BillSourceAccounts` — whatever the other units link.
#[tokio::test]
async fn a_rule_with_no_linked_accounts_is_accepted() {
    let router = emulator();
    create_unit(&router, "holder", &accounts(&[1, 2, 3])).await;

    // (label, the `Rule` the create states, the `LinkedAccounts` a read must report)
    let cases: Vec<(&str, Value, Value)> = vec![
        ("an empty LinkedAccounts list", json!({"LinkedAccounts": []}), json!([])),
        ("no LinkedAccounts member at all", json!({}), Value::Null),
        (
            "BillSourceAccounts naming accounts another unit links",
            json!({"BillSourceAccounts": accounts(&[1, 2])}),
            Value::Null,
        ),
    ];

    for (index, (label, rule, expected)) in cases.into_iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreateInvoiceUnit",
            &json!({
                "Name": format!("accountless-{index}"),
                "InvoiceReceiver": account(0),
                "TaxInheritanceDisabled": false,
                "Rule": rule,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: expected HTTP 200, got {status} {body}"
        );
        let arn = arn_of(&body);
        assert_eq!(
            linked_of(&router, &arn).await,
            expected,
            "case {label}: expected the stored LinkedAccounts {expected}"
        );
    }
}

/// THE SAME RULE on the update path, from the one place both writes apply it: an update
/// cannot move a unit onto an account another unit links, nor leave a rule that names one
/// account twice — and a refused update leaves the stored rule exactly as it was.
#[tokio::test]
async fn an_update_holds_to_the_same_rule_as_a_create() {
    let router = emulator();
    let holder = create_unit(&router, "holder", &accounts(&[1, 2, 3])).await;
    let mover = create_unit(&router, "mover", &accounts(&[4, 5])).await;
    let stored = linked_of(&router, &mover).await;

    // (label, the Rule the update states, the refusal it earns)
    let refused: Vec<(&str, Value, String, Option<String>)> = vec![
        (
            "onto the holder's only requested account",
            json!({"LinkedAccounts": accounts(&[1])}),
            already_linked_message(&account(1)),
            Some(holder.clone()),
        ),
        (
            "onto the holder's middle account, beside its own",
            json!({"LinkedAccounts": vec![account(4), account(2)]}),
            already_linked_message(&account(2)),
            Some(holder.clone()),
        ),
        (
            "onto the holder's last account, at the end of a longer list",
            json!({"LinkedAccounts": vec![account(4), account(5), account(6), account(3)]}),
            already_linked_message(&account(3)),
            Some(holder.clone()),
        ),
        (
            "a rule naming one account twice",
            json!({"LinkedAccounts": vec![account(4), account(7), account(4)]}),
            repeated_message(&account(4)),
            None,
        ),
        (
            "a rule naming a fresh account twice",
            json!({"LinkedAccounts": vec![account(8), account(8)]}),
            repeated_message(&account(8)),
            None,
        ),
    ];

    for (index, (label, rule, message, expected_holder)) in refused.into_iter().enumerate() {
        let (status, body) = call(
            &router,
            "UpdateInvoiceUnit",
            &json!({
                "InvoiceUnitArn": mover,
                "Rule": rule,
                "ClientToken": format!("refused-update-{index}"),
            }),
        )
        .await;
        assert_mutual_exclusion(label, status, &body, &message, expected_holder.as_deref());
        assert_eq!(
            linked_of(&router, &mover).await,
            stored,
            "case {label}: a refused update must leave the stored rule as it was"
        );
    }

    // The writes the rule ALLOWS on the same unit: restating its own accounts, keeping
    // them while adding a free one, an update that carries no `Rule` at all, and the
    // holder's account once the holder no longer links it.
    let allowed: Vec<(&str, Value, Value)> = vec![
        (
            "its own accounts, restated",
            json!({"LinkedAccounts": accounts(&[4, 5])}),
            json!(accounts(&[4, 5])),
        ),
        (
            "its own accounts plus a free one",
            json!({"LinkedAccounts": accounts(&[4, 5, 9])}),
            json!(accounts(&[4, 5, 9])),
        ),
        (
            "its own accounts, reordered",
            json!({"LinkedAccounts": vec![account(9), account(5), account(4)]}),
            json!(vec![account(9), account(5), account(4)]),
        ),
    ];
    for (index, (label, rule, expected)) in allowed.into_iter().enumerate() {
        ok(
            &router,
            "UpdateInvoiceUnit",
            &json!({
                "InvoiceUnitArn": mover,
                "Rule": rule,
                "ClientToken": format!("allowed-update-{index}"),
            }),
        )
        .await;
        assert_eq!(
            linked_of(&router, &mover).await,
            expected,
            "case {label}: the update the rule allows must be applied"
        );
    }

    // An update carrying no `Rule` leaves the stored one in place rather than being
    // refused by the rule it still satisfies.
    let before = linked_of(&router, &mover).await;
    ok(
        &router,
        "UpdateInvoiceUnit",
        &json!({
            "InvoiceUnitArn": mover,
            "Description": "no rule in this request",
            "ClientToken": "no-rule-update",
        }),
    )
    .await;
    assert_eq!(
        linked_of(&router, &mover).await,
        before,
        "case an update with no Rule: the stored rule must be kept"
    );

    // Exclusivity tracks STATE: once the holder drops an account, the refused update is
    // the same request and is applied.
    ok(
        &router,
        "UpdateInvoiceUnit",
        &json!({
            "InvoiceUnitArn": holder,
            "Rule": {"LinkedAccounts": accounts(&[2, 3])},
            "ClientToken": "holder-drops-one",
        }),
    )
    .await;
    let freed = vec![account(9), account(5), account(4), account(1)];
    ok(
        &router,
        "UpdateInvoiceUnit",
        &json!({
            "InvoiceUnitArn": mover,
            "Rule": {"LinkedAccounts": freed.clone()},
            "ClientToken": "retry-after-the-drop",
        }),
    )
    .await;
    assert_eq!(
        linked_of(&router, &mover).await,
        json!(freed),
        "case the account the holder dropped: the update must be applied once it is free"
    );
}
