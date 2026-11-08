// SPDX-License-Identifier: CC0-1.0

//! How this service answers a FAILED call: the HTTP status it pairs with each error
//! shape, and the envelope a request-validation refusal carries.
//!
//! Two rules, each asserted over a table that spans its class rather than the one
//! request the issue recorded:
//!
//! 1. A refusal of the request — its values, the authorization it carries, the state of
//!    the resource it names, a quota — is answered with HTTP 400 whatever 4xx its error
//!    shape's `@httpError` declares. The `__type` and the members stay the model's.
//! 2. A value outside a constraint the MODEL declares is refused at request
//!    validation: HTTP 400, error code `ValidationException`, and a message built from
//!    the request that names the failing member path and the constraint it violated.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`) is the lifecycle
//! suite's, so both suites drive the service exactly the same way.

use serde_json::{json, Value};

use super::budget_lifecycle::{caller_account, call, is_error, router};

/// A budget that satisfies every modelled constraint and every documented
/// cross-member rule, so a row can vary ONE thing and know that is what was refused.
fn valid_budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "1", "Unit": "USD"}
    })
}

/// The `message` an error body carries, or `""` when it carries none.
///
/// A MODELLED error names the member `Message`, as the Budgets model spells it
/// (`smithy_query(query="shape",shape="NotFoundException")` — `Message` targeting
/// `errorMessage`); the framework's own `ValidationException` is not in the model and
/// names it lowercase `message`. Both are read here so one helper covers either.
///
/// `pub(crate)` so a sibling suite reads a refusal's message through the same helper.
pub(crate) fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the status table: the call, the error shape it must name, and the 4xx
/// that shape's `@httpError` declares — the status the emulator used to answer with.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    request: Value,
    exception: &'static str,
    /// `@httpError` on the error shape
    /// (`smithy_query(query="shape",shape="<Exception>")`).
    modeled_status: u16,
}

/// RULE 1: every refusal of the request is answered with HTTP 400, whatever 4xx its
/// error shape declares; the error code stays the shape the model names.
///
/// The rows differ along the error shape and so along the modelled status folded in
/// (404, 409, 403) as well as the statuses already at 400 (400 shapes, and the
/// framework's own `ValidationException`), and along the operation that answers it.
#[tokio::test]
async fn every_request_refusal_is_answered_at_http_400() {
    let router = router();
    let account_id = caller_account();

    // One stored budget, so the duplicate row has something to collide with and the
    // not-found rows name something that really is absent.
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({"AccountId": &account_id, "Budget": valid_budget("stored")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget to answer 200, got {status} {body}"
    );

    let rows = vec![
        // NotFoundException — "We can't locate the resource that you specified",
        // @httpError 404. Every operation that resolves a budget by name.
        Refusal {
            case: "DescribeBudget on a budget that does not exist",
            operation: "DescribeBudget",
            request: json!({"AccountId": &account_id, "BudgetName": "absent"}),
            exception: "NotFoundException",
            modeled_status: 404,
        },
        Refusal {
            case: "DeleteBudget on a budget that does not exist",
            operation: "DeleteBudget",
            request: json!({"AccountId": &account_id, "BudgetName": "absent"}),
            exception: "NotFoundException",
            modeled_status: 404,
        },
        Refusal {
            case: "DescribeNotificationsForBudget on a budget that does not exist",
            operation: "DescribeNotificationsForBudget",
            request: json!({"AccountId": &account_id, "BudgetName": "absent"}),
            exception: "NotFoundException",
            modeled_status: 404,
        },
        Refusal {
            case: "UpdateBudget on a budget that does not exist",
            operation: "UpdateBudget",
            request: json!({"AccountId": &account_id, "NewBudget": valid_budget("absent")}),
            exception: "NotFoundException",
            modeled_status: 404,
        },
        Refusal {
            case: "DescribeBudget for a name no budget was stored under",
            operation: "DescribeBudget",
            request: json!({"AccountId": &account_id, "BudgetName": "stored-elsewhere"}),
            exception: "NotFoundException",
            modeled_status: 404,
        },
        // DuplicateRecordException — "The budget name already exists", @httpError 409.
        Refusal {
            case: "CreateBudget for a name the account already holds",
            operation: "CreateBudget",
            request: json!({"AccountId": &account_id, "Budget": valid_budget("stored")}),
            exception: "DuplicateRecordException",
            modeled_status: 409,
        },
        // AccessDeniedException — "You are not authorized to use this operation with the
        // given parameters", @httpError 403. Every operation is scoped to the account the
        // caller signed with (`account_scoping.rs`), so an `AccountId` naming another one
        // is this refusal, folded in like the rest.
        Refusal {
            case: "DescribeBudget for an account that is not the caller's",
            operation: "DescribeBudget",
            request: json!({"AccountId": "210987654321", "BudgetName": "stored"}),
            exception: "AccessDeniedException",
            modeled_status: 403,
        },
        // InvalidParameterException — @httpError 400 already, so the mapping must
        // leave it alone: a documented cross-member rule, not a modelled constraint.
        Refusal {
            case: "CreateBudget with BudgetLimit and PlannedBudgetLimits together",
            operation: "CreateBudget",
            request: json!({
                "AccountId": &account_id,
                "Budget": {
                    "BudgetName": "exclusive",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "1", "Unit": "USD"},
                    "PlannedBudgetLimits": {"1704067200": {"Amount": "1", "Unit": "USD"}}
                }
            }),
            exception: "InvalidParameterException",
            modeled_status: 400,
        },
        // InvalidNextTokenException — "The pagination token is invalid", @httpError 400.
        Refusal {
            case: "DescribeBudgets with a token this service never minted",
            operation: "DescribeBudgets",
            request: json!({"AccountId": &account_id, "NextToken": "not-a-token"}),
            exception: "InvalidNextTokenException",
            modeled_status: 400,
        },
        // The framework's request-validation refusal, which is a 400 by construction.
        Refusal {
            case: "CreateBudget with an AccountId outside its pattern",
            operation: "CreateBudget",
            request: json!({"AccountId": "not-an-account", "Budget": valid_budget("x")}),
            exception: "ValidationException",
            modeled_status: 400,
        },
    ];

    for row in rows {
        let Refusal {
            case,
            operation,
            request,
            exception,
            modeled_status,
        } = row;
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to answer 400 \
             (the shape's @httpError is {modeled_status}), got {status} {body}"
        );
        assert!(
            is_error(&body, exception),
            "case {case:?}: expected {exception}, got {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: expected a message a caller can read, got {body}"
        );
    }
}

/// RULE 1, the other side: folding a refusal to 400 must not flatten a status that is
/// NOT a refusal of the request. HTTP 501 `NotImplemented` means "nobody has written
/// that operation yet", so an operation that IS written never answers it — and this
/// service has no scaffold left, the account-wide action list having been the last one.
/// The rows are the requests that used to reach a scaffold, each satisfying every
/// modelled constraint on its input so that it reaches the handler rather than the
/// validation layer in front of it.
#[tokio::test]
async fn no_operation_of_this_service_answers_501_any_more() {
    let router = router();
    let account_id = caller_account();
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({"AccountId": &account_id, "Budget": valid_budget("stored")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget to answer 200, got {status} {body}"
    );

    for (operation, request) in [
        (
            "DescribeBudgetActionsForAccount",
            json!({"AccountId": &account_id}),
        ),
        (
            "DescribeBudgetActionsForBudget",
            json!({"AccountId": &account_id, "BudgetName": "stored"}),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {operation}: an implemented operation must serve the request, \
             got {status} {body}"
        );
        assert!(
            !is_error(&body, "NotImplemented"),
            "case {operation}: expected no NotImplemented, got {body}"
        );
    }
}

/// One row of the validation table: the call, the member path its message must name,
/// and the constraint clause it must state.
struct Rejected {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The dotted, lowerCamel member path the message states.
    path: &'static str,
    /// The requirement clause, as the framework's validation renderer words it.
    constraint: &'static str,
}

/// RULE 2: a value outside a constraint the model declares is refused at request
/// validation — HTTP 400, error code `ValidationException`, and a message that names
/// the failing member path and the constraint it violated.
///
/// The rows differ along the constraint kind (`@pattern`, `@length` at each published
/// bound, `@range` at each published bound, `@enum`, `@required`), the depth of the
/// member path (a top-level member, a member of a nested structure) and the operation
/// — including the `BudgetName` `@pattern`, which reaches a handler because the
/// framework's regex engine cannot compile its negative look-ahead.
#[tokio::test]
async fn a_modelled_constraint_violation_names_its_member_and_constraint() {
    let router = router();
    let account_id = caller_account();
    // The constraint clauses, as `aws_smithy_schema_server`'s validation renderer
    // words them; the bounds are the modelled ones
    // (`smithy_query(query="shape",shape="BudgetName,MaxResults,Notification")`).
    let pattern = "Member must satisfy regular expression pattern";
    let rows = vec![
        // Budget$BudgetName: @pattern ^(?![^:\]*/action/|(?i).*<script>.*</script>.*)[^:\]+$
        Rejected {
            case: "budget name with a colon",
            operation: "CreateBudget",
            request: json!({"AccountId": &account_id, "Budget": valid_budget("a:b")}),
            path: "budget.budgetName",
            constraint: pattern,
        },
        Rejected {
            case: "budget name with a backslash",
            operation: "CreateBudget",
            request: json!({"AccountId": &account_id, "Budget": valid_budget("a\\b")}),
            path: "budget.budgetName",
            constraint: pattern,
        },
        Rejected {
            case: "budget name holding the /action/ substring",
            operation: "CreateBudget",
            request: json!({"AccountId": &account_id, "Budget": valid_budget("a/action/b")}),
            path: "budget.budgetName",
            constraint: pattern,
        },
        Rejected {
            case: "budget name holding a script element",
            operation: "CreateBudget",
            request: json!({
                "AccountId": &account_id,
                "Budget": valid_budget("a<SCRIPT>alert(1)</script>b")
            }),
            path: "budget.budgetName",
            constraint: pattern,
        },
        // The same constraint, reached through a different member path.
        Rejected {
            case: "the name rule on UpdateBudget's NewBudget",
            operation: "UpdateBudget",
            request: json!({"AccountId": &account_id, "NewBudget": valid_budget("a:b")}),
            path: "newBudget.budgetName",
            constraint: pattern,
        },
        Rejected {
            case: "the name rule on DescribeBudget's BudgetName",
            operation: "DescribeBudget",
            request: json!({"AccountId": &account_id, "BudgetName": "a:b"}),
            path: "budgetName",
            constraint: pattern,
        },
        Rejected {
            case: "the name rule on DeleteBudget's BudgetName",
            operation: "DeleteBudget",
            request: json!({"AccountId": &account_id, "BudgetName": "a/action/b"}),
            path: "budgetName",
            constraint: pattern,
        },
        Rejected {
            case: "the name rule on DescribeNotificationsForBudget's BudgetName",
            operation: "DescribeNotificationsForBudget",
            request: json!({"AccountId": &account_id, "BudgetName": "a\\b"}),
            path: "budgetName",
            constraint: pattern,
        },
        // AccountId: @length(12, 12) + @pattern ^\d{12}$.
        Rejected {
            case: "account id outside its pattern",
            operation: "CreateBudget",
            request: json!({"AccountId": "abcdefghijkl", "Budget": valid_budget("ok")}),
            path: "accountId",
            constraint: pattern,
        },
        // BudgetName: @length(1, 100) — one past each published bound.
        Rejected {
            case: "budget name shorter than the published minimum",
            operation: "CreateBudget",
            request: json!({"AccountId": &account_id, "Budget": valid_budget("")}),
            path: "budget.budgetName",
            constraint: "Member must have length greater than or equal to 1",
        },
        Rejected {
            case: "budget name longer than the published maximum",
            operation: "CreateBudget",
            request: json!({
                "AccountId": &account_id,
                "Budget": valid_budget(LONG_NAME)
            }),
            path: "budget.budgetName",
            constraint: "Member must have length less than or equal to 100",
        },
        // DescribeBudgets$MaxResults: @range(1, 1000) — one past each bound.
        Rejected {
            case: "max results below the published minimum",
            operation: "DescribeBudgets",
            request: json!({"AccountId": &account_id, "MaxResults": 0}),
            path: "maxResults",
            constraint: "Member must have value greater than or equal to 1",
        },
        Rejected {
            case: "max results above the published maximum",
            operation: "DescribeBudgets",
            request: json!({"AccountId": &account_id, "MaxResults": 1001}),
            path: "maxResults",
            constraint: "Member must have value less than or equal to 1000",
        },
        // An enumerated member accepts only the values the model lists.
        Rejected {
            case: "time unit outside the enum",
            operation: "CreateBudget",
            request: json!({
                "AccountId": &account_id,
                "Budget": {"BudgetName": "ok", "TimeUnit": "WEEKLY", "BudgetType": "COST"}
            }),
            path: "budget.timeUnit",
            constraint: "Member must satisfy enum value set",
        },
        // A @required member absent from the request is the same class of refusal.
        Rejected {
            case: "account id absent",
            operation: "CreateBudget",
            request: json!({"Budget": valid_budget("ok")}),
            path: "accountId",
            constraint: "Member must not be null",
        },
    ];

    for row in rows {
        let Rejected {
            case,
            operation,
            request,
            path,
            constraint,
        } = row;
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let text = message(&body);
        assert!(
            text.contains("validation error"),
            "case {case:?}: expected a validation-error envelope, got {text:?}"
        );
        assert!(
            text.contains(&format!("'{path}'")),
            "case {case:?}: expected the message to name '{path}', got {text:?}"
        );
        assert!(
            text.contains(constraint),
            "case {case:?}: expected the message to state {constraint:?}, got {text:?}"
        );
    }

    // Not one of them reached the store.
    let (_, body) = call(
        &router,
        "DescribeBudgets",
        &json!({"AccountId": &account_id}),
    )
    .await;
    assert_eq!(
        body["Budgets"].as_array().map(Vec::len),
        Some(0),
        "expected a refused request to store nothing, got {body}"
    );
}

/// 101 characters: one past `BudgetName`'s `@length` maximum of 100.
const LONG_NAME: &str = "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn";

/// RULE 2, for a member whose constraint is a CONTENT rule: `BudgetName` is documented
/// as "validated for content. Names that contain phone numbers, URLs, or email
/// addresses combined with certain terms may be rejected", and its pattern refuses
/// `<script>…</script>`, so the refusal states the member and the constraint WITHOUT
/// echoing the rejected name back — a name refused for its content is not reflected
/// into the response — and without restating the filter it failed.
///
/// The rows differ along the content that got the name rejected: a script element, a
/// name carrying an email address, one carrying a URL, and the two forbidden
/// characters.
#[tokio::test]
async fn a_budget_name_refused_for_its_content_is_not_echoed_back() {
    let router = router();
    let account_id = caller_account();
    for (case, name) in [
        ("a script element", "<script>alert(1)</script>"),
        (
            "a script element around an email address",
            "<script>mail:nobody@example.com</script>",
        ),
        ("a url", "https://example.com/action/pay"),
        ("a colon", "ops:oncall"),
        ("a backslash", "ops\\oncall"),
    ] {
        for (operation, request) in [
            (
                "CreateBudget",
                json!({"AccountId": &account_id, "Budget": valid_budget(name)}),
            ),
            (
                "DescribeBudget",
                json!({"AccountId": &account_id, "BudgetName": name}),
            ),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} via {operation}: expected 400, got {status} {body}"
            );
            let text = message(&body);
            assert!(
                !text.contains(name),
                "case {case:?} via {operation}: expected the rejected name not to be \
                 echoed, got {text:?}"
            );
            // Nor the characters the name was rejected for, which only the name could
            // have put there.
            for forbidden in ["<script>", "@example.com", "example.com"] {
                assert!(
                    !text.contains(forbidden),
                    "case {case:?} via {operation}: expected {forbidden:?} not to appear \
                     in the message, got {text:?}"
                );
            }
            assert!(
                text.contains("Member must satisfy regular expression pattern")
                    && !text.contains("^(?!"),
                "case {case:?} via {operation}: expected the constraint to be named \
                 without restating the pattern, got {text:?}"
            );
        }
    }
}

/// RULE 2 does not over-reject: a name the modelled `@pattern` ALLOWS is accepted and
/// reads back, including the near misses of each half of its negative look-ahead.
#[tokio::test]
async fn names_the_modelled_pattern_allows_are_accepted() {
    let router = router();
    let account_id = caller_account();
    // Each row satisfies `^(?![^:\]*/action/|(?i).*<script>.*</script>.*)[^:\]+$` while
    // sitting as close to one of its rejections as a legal name can.
    for (case, name) in [
        ("a plain name", "monthly-cost".to_string()),
        ("a name holding spaces", "monthly cost budget".to_string()),
        ("a slash that is not /action/", "team/platform".to_string()),
        ("the word action without both slashes", "/action".to_string()),
        ("action followed by a slash only", "action/".to_string()),
        ("an opening script tag with no close", "a<script>b".to_string()),
        ("a closing script tag with no open", "a</script>b".to_string()),
        ("non-ascii characters", "予算-2026".to_string()),
        ("one character, the length minimum", "x".to_string()),
        ("exactly the length maximum", "y".repeat(100)),
    ] {
        let create = json!({"AccountId": &account_id, "Budget": valid_budget(&name)});
        let (status, body) = call(&router, "CreateBudget", &create).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the name {name:?} to be accepted, got {status} {body}"
        );
        // Round-trip: the name the service stored is the name that was sent.
        let (status, body) = call(
            &router,
            "DescribeBudget",
            &json!({"AccountId": &account_id, "BudgetName": &name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBudget to read {name:?} back, got {status} {body}"
        );
        assert_eq!(
            body["Budget"]["BudgetName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the stored name to be {name:?}, got {body}"
        );
    }
}
