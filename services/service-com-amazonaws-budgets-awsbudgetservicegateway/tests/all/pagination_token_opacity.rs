// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the OPACITY of this service's pagination token, driven over the
//! wire through the same `MultiServiceRouter` the production router serves from (the
//! harness is the lifecycle suite's, so every suite in this target drives the service
//! identically).
//!
//! The RULE these tables assert: a `NextToken` is OPAQUE, so a paginated read continues
//! only from a token the service itself handed back — the token is honoured byte for
//! byte, and any other string, including a different spelling of a position inside the
//! very list being read, is refused with the modelled `InvalidNextTokenException` (HTTP
//! 400) instead of being served as a page.
//!
//! Each table spans that class rather than one request: the rows differ along the
//! paginated operation (every one of this service's eight), the offset the token
//! addresses (every position each list hands a token out at), and the form the
//! never-handed-out spelling takes — a signed decimal, a zero-padded decimal,
//! surrounding whitespace, and the same hex payload written in another letter case.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::encode_next_token;

use super::budget_lifecycle::{call, caller_account, civil_secs, is_error, router};

/// A budget start far enough back that the primary budget's performance history spans
/// every period its time unit retains, so that read pages too — written as the date it
/// is rather than as an epoch literal.
const LONG_RUNNING_START: &str = "2000-01-01T00:00:00Z";

/// The budgets this suite seeds, and how many notifications each one carries. Two of
/// them carry notifications, so the per-account notification listing pages over more
/// than one budget; the third carries none, so it is reported only by the budget
/// listing.
const SEEDED_BUDGETS: [(&str, usize); 3] = [
    ("tokens-primary", 3),
    ("tokens-second", 1),
    ("tokens-third", 0),
];

/// How many actions the primary budget carries, and how many records the history of its
/// first action holds: the create event plus one record per update call.
const SEEDED_ACTIONS: usize = 2;
const SEEDED_HISTORY_UPDATES: usize = 2;

/// The notifications a seeded budget carries, differing along every member a
/// notification's identity is made of — the notification type, the comparison operator,
/// the threshold and the threshold type
/// (`smithy_query(query="shape",shape="Notification")`).
fn notification_catalog() -> Vec<Value> {
    vec![
        json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "GREATER_THAN",
            "Threshold": 10.0,
            "ThresholdType": "PERCENTAGE"
        }),
        json!({
            "NotificationType": "FORECASTED",
            "ComparisonOperator": "EQUAL_TO",
            "Threshold": 50.0,
            "ThresholdType": "PERCENTAGE"
        }),
        json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "LESS_THAN",
            "Threshold": 90.0,
            "ThresholdType": "ABSOLUTE_VALUE"
        }),
    ]
}

/// Two subscribers for one notification, so the subscriber listing has a position to
/// hand a token out at.
fn subscriber_set(budget_name: &str, index: usize) -> Value {
    json!([
        {"SubscriptionType": "EMAIL", "Address": format!("{budget_name}-{index}-a@example.com")},
        {"SubscriptionType": "EMAIL", "Address": format!("{budget_name}-{index}-b@example.com")},
    ])
}

/// An `ExecutionRoleArn` satisfying `RoleArn`'s modelled `@pattern` and `@length(32, 618)`
/// in the account the action lives in: "roles and actions must be in the same account"
/// (`Action$ExecutionRoleArn`).
fn execution_role(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/budget-action-role")
}

/// Create one budget whose performance history spans every period its time unit retains.
async fn create_budget(router: &MultiServiceRouter, account_id: &str, budget_name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
                "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {budget_name}: expected CreateBudget to answer 200, got {status} {body}"
    );
}

/// Attach one notification, with the subscribers the listing pages over.
async fn create_notification(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    index: usize,
    notification: &Value,
) {
    let (status, body) = call(
        router,
        "CreateNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification,
            "Subscribers": subscriber_set(budget_name, index),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {budget_name} notification {index}: expected CreateNotification to answer 200, \
         got {status} {body}"
    );
}

/// Create one budget action and return the `ActionId` the service minted for it.
async fn create_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    index: usize,
) -> String {
    let (status, body) = call(
        router,
        "CreateBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "NotificationType": "ACTUAL",
            "ActionType": "APPLY_IAM_POLICY",
            "ActionThreshold": {
                "ActionThresholdValue": 40.0 + index as f64,
                "ActionThresholdType": "PERCENTAGE",
            },
            "Definition": {
                "IamActionDefinition": {
                    "PolicyArn": "arn:aws:iam::123456789012:policy/budget-restrict",
                    "Roles": ["BudgetRestrictedRole"],
                }
            },
            "ExecutionRoleArn": execution_role(account_id),
            "ApprovalModel": "AUTOMATIC",
            "Subscribers": [
                {"SubscriptionType": "EMAIL", "Address": format!("action-{index}@example.com")}
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup action {index}: expected CreateBudgetAction to answer 200, got {status} {body}"
    );
    body.get("ActionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup action {index}: expected a minted ActionId, got {body}"))
        .to_string()
}

/// Append one record to an action's history: a call that names no member still records
/// the event it was.
async fn update_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    index: usize,
) {
    let (status, body) = call(
        router,
        "UpdateBudgetAction",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "ActionId": action_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup update {index}: expected UpdateBudgetAction to answer 200, got {status} {body}"
    );
}

/// What this suite pages over: an account holding three budgets, the first of them
/// carrying three notifications (each with two subscribers), two actions, and a history
/// of several records on the first of those actions — so every paginated read of the
/// service has a list long enough to hand a token out at least once.
struct Fixture {
    router: MultiServiceRouter,
    account_id: String,
    budget_name: String,
    action_id: String,
    notification: Value,
}

impl Fixture {
    async fn seed() -> Self {
        let router = router();
        let account_id = caller_account();
        let catalog = notification_catalog();
        for (budget_name, held) in SEEDED_BUDGETS {
            create_budget(&router, &account_id, budget_name).await;
            for (index, notification) in catalog.iter().take(held).enumerate() {
                create_notification(&router, &account_id, budget_name, index, notification).await;
            }
        }
        let budget_name = SEEDED_BUDGETS[0].0.to_string();
        let mut action_id = String::new();
        for index in 0..SEEDED_ACTIONS {
            let created = create_action(&router, &account_id, &budget_name, index).await;
            if index == 0 {
                action_id = created;
            }
        }
        for index in 0..SEEDED_HISTORY_UPDATES {
            update_action(&router, &account_id, &budget_name, &action_id, index).await;
        }
        Self {
            router,
            account_id,
            budget_name,
            action_id,
            notification: catalog[0].clone(),
        }
    }

    /// Every paginated read of this service, with the request that addresses the seeded
    /// state and the member path its page of items sits at in the response.
    ///
    /// The operations and the member each one pages are the model's own
    /// (`smithy_query(query="operation",operation=...)`: each carries a `@paginated`
    /// trait naming its `items`), so the table cannot drift from the service surface.
    fn paginated(&self) -> Vec<Paginated> {
        vec![
            Paginated {
                operation: "DescribeBudgets",
                items: &["Budgets"],
                request: json!({"AccountId": &self.account_id}),
            },
            Paginated {
                operation: "DescribeBudgetNotificationsForAccount",
                items: &["BudgetNotificationsForAccount"],
                request: json!({"AccountId": &self.account_id}),
            },
            Paginated {
                operation: "DescribeNotificationsForBudget",
                items: &["Notifications"],
                request: json!({
                    "AccountId": &self.account_id,
                    "BudgetName": &self.budget_name,
                }),
            },
            Paginated {
                operation: "DescribeSubscribersForNotification",
                items: &["Subscribers"],
                request: json!({
                    "AccountId": &self.account_id,
                    "BudgetName": &self.budget_name,
                    "Notification": &self.notification,
                }),
            },
            Paginated {
                operation: "DescribeBudgetActionsForAccount",
                items: &["Actions"],
                request: json!({"AccountId": &self.account_id}),
            },
            Paginated {
                operation: "DescribeBudgetActionsForBudget",
                items: &["Actions"],
                request: json!({
                    "AccountId": &self.account_id,
                    "BudgetName": &self.budget_name,
                }),
            },
            Paginated {
                operation: "DescribeBudgetActionHistories",
                items: &["ActionHistories"],
                request: json!({
                    "AccountId": &self.account_id,
                    "BudgetName": &self.budget_name,
                    "ActionId": &self.action_id,
                }),
            },
            Paginated {
                // The one read whose paged list is nested inside the record it reports.
                operation: "DescribeBudgetPerformanceHistory",
                items: &["BudgetPerformanceHistory", "BudgetedAndActualAmountsList"],
                request: json!({
                    "AccountId": &self.account_id,
                    "BudgetName": &self.budget_name,
                }),
            },
        ]
    }
}

/// One paginated read, and where its page of items sits in the response.
struct Paginated {
    operation: &'static str,
    items: &'static [&'static str],
    request: Value,
}

impl Paginated {
    /// One call of this read, with `MaxResults` and the token the row supplies.
    async fn call(
        &self,
        router: &MultiServiceRouter,
        max_results: Option<usize>,
        next_token: Option<&str>,
    ) -> (u16, Value) {
        let mut request = self.request.clone();
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token {
            request["NextToken"] = json!(token);
        }
        call(router, self.operation, &request).await
    }

    /// The page of items `body` reports, at the member path this read pages.
    fn page(&self, body: &Value) -> Vec<Value> {
        let mut cursor = body;
        for member in self.items {
            cursor = cursor
                .get(member)
                .unwrap_or_else(|| panic!("case {}: expected {member} in {body}", self.operation));
        }
        cursor
            .as_array()
            .unwrap_or_else(|| panic!("case {}: expected a list in {body}", self.operation))
            .clone()
    }

    /// Walk this read one item at a time, returning the items it reports and the tokens
    /// it handed out along the way (one per page but the last).
    async fn walk(&self, router: &MultiServiceRouter) -> (Vec<Value>, Vec<String>) {
        let mut items = Vec::new();
        let mut tokens: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (status, body) = self.call(router, Some(1), token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {} token={token:?}: expected 200, got {status} {body}",
                self.operation
            );
            items.extend(self.page(&body));
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => {
                    tokens.push(next.to_string());
                    token = Some(next.to_string());
                }
                None => break,
            }
            assert!(
                tokens.len() <= items.len(),
                "case {}: the walk handed out more tokens than items ({} tokens, {} items)",
                self.operation,
                tokens.len(),
                items.len()
            );
        }
        (items, tokens)
    }
}

/// The same hex payload with its first letter in the other case: `hex::encode` writes
/// lower-case digits only, so this is a spelling the service never hands out even though
/// `hex::decode` reads it.
fn other_letter_case(token: &str) -> String {
    let mut flipped = false;
    token
        .chars()
        .map(|ch| {
            if !flipped && ch.is_ascii_alphabetic() {
                flipped = true;
                ch.to_ascii_uppercase()
            } else {
                ch
            }
        })
        .collect()
}

/// The decimal part of a minted token rewritten as `decimal`, and nothing else changed:
/// a token is the hex of an opaque marker followed by the offset, so each row below
/// differs from the token the service handed out in the spelling of that offset alone.
fn respelled(token: &str, decimal: &str) -> String {
    let payload = String::from_utf8(hex::decode(token).expect("a minted token is hex"))
        .expect("a minted token's payload is text");
    let (marker, _) = payload
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("a minted token carries a marker and an offset: {payload:?}"));
    hex::encode(format!("{marker}:{decimal}"))
}

/// Every spelling of `offset` that is NOT the token the service handed out for it:
/// rows differ along the form — a sign, zero padding, surrounding whitespace, and the
/// letter case of the hex itself.
fn never_handed_out(token: &str, offset: usize) -> Vec<(String, String)> {
    let rows = vec![
        (
            format!("a signed decimal (+{offset})"),
            respelled(token, &format!("+{offset}")),
        ),
        (
            format!("one leading zero (0{offset})"),
            respelled(token, &format!("0{offset}")),
        ),
        (
            format!("four leading zeros (0000{offset})"),
            respelled(token, &format!("0000{offset}")),
        ),
        (
            format!("a leading space ( {offset})"),
            respelled(token, &format!(" {offset}")),
        ),
        (
            format!("a trailing space ({offset} )"),
            respelled(token, &format!("{offset} ")),
        ),
        (
            format!("a trailing newline ({offset}\\n)"),
            respelled(token, &format!("{offset}\n")),
        ),
        ("upper-case hex".to_string(), token.to_uppercase()),
        ("mixed-case hex".to_string(), other_letter_case(token)),
    ];
    for (case, respelling) in &rows {
        assert_ne!(
            respelling, token,
            "case {case}: the row must differ from the token the service handed out, \
             got {respelling:?}"
        );
    }
    rows
}

/// RULE, the half that must keep working: a read continues from the token it handed out,
/// and walking a list one item at a time through those tokens reports every item of it
/// exactly once, in order.
#[tokio::test]
async fn a_read_continues_from_the_token_it_handed_out() {
    let fixture = Fixture::seed().await;
    for row in fixture.paginated() {
        let (status, body) = row.call(&fixture.router, None, None).await;
        assert_eq!(
            status, 200,
            "case {}: expected the unpaged read to answer 200, got {status} {body}",
            row.operation
        );
        let whole = row.page(&body);
        assert!(
            whole.len() >= 2,
            "case {}: the fixture must give this read a list to page, got {whole:?}",
            row.operation
        );
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {}: expected no NextToken when the page held the whole list, got {body}",
            row.operation
        );

        let (walked, tokens) = row.walk(&fixture.router).await;
        assert_eq!(
            walked, whole,
            "case {}: expected the walk to report the whole list in order {whole:?}, \
             got {walked:?}",
            row.operation
        );
        assert_eq!(
            tokens.len(),
            whole.len() - 1,
            "case {}: expected one token per page but the last, got {tokens:?}",
            row.operation
        );
        // The one spelling the service hands out: the hex of the offset the next page
        // starts at, as `encode_next_token` writes it.
        for (index, token) in tokens.iter().enumerate() {
            let offset = index + 1;
            assert_eq!(
                token,
                &encode_next_token(offset),
                "case {} page {index}: expected the token minted for offset {offset}, \
                 got {token:?}",
                row.operation
            );
        }
    }
}

/// RULE: a token the service never handed out does not continue a read, however close
/// its spelling is to one that was — it is refused with the modelled
/// `InvalidNextTokenException` (HTTP 400), never served as the page at that position.
#[tokio::test]
async fn a_token_the_service_never_handed_out_is_refused() {
    let fixture = Fixture::seed().await;
    for row in fixture.paginated() {
        let (_, tokens) = row.walk(&fixture.router).await;
        for (index, token) in tokens.iter().enumerate() {
            let offset = index + 1;
            // The token itself still continues the read, so the refusals below are
            // about the SPELLING and not about the position.
            let (status, body) = row.call(&fixture.router, Some(1), Some(token)).await;
            assert_eq!(
                status, 200,
                "case {} offset {offset}: expected the handed-out token to answer 200, \
                 got {status} {body}",
                row.operation
            );
            assert_eq!(
                row.page(&body).len(),
                1,
                "case {} offset {offset}: expected one item for the page at that offset, \
                 got {body}",
                row.operation
            );

            for (case, respelling) in never_handed_out(token, offset) {
                let (status, body) = row
                    .call(&fixture.router, Some(1), Some(&respelling))
                    .await;
                assert_eq!(
                    status, 400,
                    "case {} offset {offset} {case}: expected the refusal to answer 400, \
                     got {status} {body}",
                    row.operation
                );
                assert!(
                    is_error(&body, "InvalidNextTokenException"),
                    "case {} offset {offset} {case}: expected InvalidNextTokenException, \
                     got {body}",
                    row.operation
                );
            }
        }
    }
}
