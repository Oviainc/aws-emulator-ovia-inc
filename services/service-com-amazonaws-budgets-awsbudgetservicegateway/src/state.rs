// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-budgets-awsbudgetservicegateway service.
//!
//! The resource this service is built around is the **Budget**. AWS gives a budget
//! its identity from the account that owns it plus its name — `Budget$BudgetName`
//! is documented as "The name must be unique within an account" — so the store is
//! keyed by that pair. A budget OWNS the notifications, their subscribers and the
//! resource tags created with it ("After the end date, Amazon Web Services deletes
//! the budget and all the associated notifications and subscribers"), so they are
//! held inside the stored budget and go away with it.
//!
//! The helpers below (ARN rendering, the opaque pagination token, the cross-member
//! rules `CreateBudget`/`UpdateBudget` document) are shared by several operation
//! handlers, so they live here rather than being copied into each `src/ops` file —
//! a new file under `src/ops/` would be picked up by `build.rs` as an operation.

use crate::ops::types::{Budget, Notification, ResourceTag, Subscriber};
use std::collections::HashMap;

/// Per-service state for com-amazonaws-budgets-awsbudgetservicegateway. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBudgetsAwsbudgetservicegatewayState {
    /// Every budget this emulator holds, keyed by `(AccountId, BudgetName)`.
    pub budgets: HashMap<(String, String), StoredBudget>,
}

/// One persisted budget: the modelled shape exactly as the caller last supplied it
/// (plus the `LastUpdatedTime` the service stamps), and the notifications and tags
/// created alongside it.
///
/// No ARN is stored: nothing in this slice reports one. The operations that resolve a
/// budget by ARN (`TagResource` / `UntagResource` / `ListTagsForResource`) are still
/// 501 and filed as their own issues, and they should build it through the runtime's
/// `engine::emulator_arn` rather than a string this module formats.
#[derive(Debug, Clone)]
pub struct StoredBudget {
    /// The budget as `DescribeBudget` / `DescribeBudgets` read it back.
    pub budget: Budget,
    /// The notifications created with this budget, in the order they were supplied.
    pub notifications: Vec<StoredNotification>,
    /// `ResourceTags` supplied at creation.
    pub tags: Vec<ResourceTag>,
}

/// A notification plus the subscribers attached to it. A notification cannot exist
/// without at least one subscriber (`Subscribers` is `@length(min: 1)`), so the two
/// are stored together.
#[derive(Debug, Clone)]
pub struct StoredNotification {
    pub notification: Notification,
    pub subscribers: Vec<Subscriber>,
}

/// Marker the opaque pagination token carries, so a token minted by another
/// service (or invented by a caller) does not decode here.
const NEXT_TOKEN_PREFIX: &str = "budgets:offset:";

/// Encode an offset into the opaque `NextToken` a paginated operation hands back.
pub fn encode_next_token(offset: usize) -> String {
    hex::encode(format!("{NEXT_TOKEN_PREFIX}{offset}"))
}

/// Decode a `NextToken` minted by [`encode_next_token`].
///
/// `None` for anything else — the caller answers that with
/// `InvalidNextTokenException` ("The pagination token is invalid").
pub fn decode_next_token(token: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(NEXT_TOKEN_PREFIX)?.parse().ok()
}

/// One page of `items`, plus the token for the page after it.
///
/// `max_results` is the modelled `@paginated` page size (already range-checked at
/// the boundary); absent means "every remaining item". `next_token` is the token a
/// previous page returned. `None` is returned when `next_token` does not decode, or
/// decodes to an offset this list never had a page at — which the caller reports as
/// `InvalidNextTokenException`.
pub fn page<T: Clone>(
    items: &[T],
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match next_token {
        Some(token) => {
            let offset = decode_next_token(token)?;
            // A token is only minted while items remain, so its offset was below the
            // length of the list it was minted for. An offset at or past the end was
            // never minted for THIS list: it is stale or tampered with, not a last page.
            if offset >= items.len() {
                return None;
            }
            offset
        }
        None => 0,
    };
    let page_size = match max_results {
        // At least one item per page: a zero page would hand back a token pointing at
        // the same offset, which is a caller loop that never terminates. The modelled
        // `@range(min: 1)` already makes this unreachable from the wire.
        Some(max) => usize::try_from(max).unwrap_or(1).max(1),
        None => items.len(),
    };
    let end = start.saturating_add(page_size).min(items.len());
    let next = (end < items.len()).then(|| encode_next_token(end));
    Some((items[start..end].to_vec(), next))
}

/// Every budget stored for `account_id`, ordered by `BudgetName`.
///
/// The order is the one the pagination token indexes into, so it must not depend
/// on `HashMap` iteration order. Only the `Budget` is cloned — the notifications,
/// subscribers and tags hanging off a stored budget are not reported by a list call.
pub fn budgets_for_account(
    state: &ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    account_id: &str,
) -> Vec<Budget> {
    let mut found: Vec<&StoredBudget> = state
        .budgets
        .iter()
        .filter(|((account, _), _)| account == account_id)
        .map(|(_, stored)| stored)
        .collect();
    found.sort_by(|a, b| a.budget.budget_name.cmp(&b.budget.budget_name));
    found
        .into_iter()
        .map(|stored| stored.budget.clone())
        .collect()
}

/// How many budgets `account_id` already holds — what the per-account quota below is
/// checked against.
pub fn budget_count_for_account(
    state: &ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    account_id: &str,
) -> usize {
    state
        .budgets
        .keys()
        .filter(|(account, _)| account == account_id)
        .count()
}

/// AWS Budgets' documented default service quota: "Budgets: 20,000 per account"
/// (Billing and Cost Management service quotas). `CreateBudget` answers
/// `ServiceQuotaExceededException` ("You've reached a Service Quota limit on this
/// resource", HTTP 402) once an account holds this many.
pub const MAX_BUDGETS_PER_ACCOUNT: usize = 20_000;

/// The whole store lives in memory, and both halves of a budget's key come from the
/// request, so the number of ACCOUNTS a caller can name is unbounded too. This is the
/// emulator's own ceiling on the total number of budgets it will hold — not an AWS
/// quota — so a client cannot grow the process without limit by varying `AccountId`.
/// It is answered with the same modelled `ServiceQuotaExceededException`.
pub const MAX_BUDGETS_TOTAL: usize = 100_000;

/// `BudgetName`'s modelled `@pattern`, verbatim from the model
/// (`smithy_query(query="shape",shape="BudgetName")`). The rule
/// [`budget_name_matches_pattern`] enforces, in one place, so the code and the
/// comment above it cannot drift from the model.
pub const BUDGET_NAME_PATTERN: &str =
    r"^(?![^:\\]*/action/|(?i).*<script>.*</script>.*)[^:\\]+$";

/// Whether `name` satisfies [`BUDGET_NAME_PATTERN`] — documented on `BudgetName` as
/// "The ':' and '\' characters, and the '/action/' substring, aren't allowed", with
/// names "validated for content".
///
/// Checked here rather than left to the boundary validator because that pattern uses
/// a negative look-ahead, which the framework's regex engine cannot compile, and an
/// uncompilable `@pattern` is treated as matching ("we never reject a value because of
/// an unparseable model pattern"). The `@length(1, 100)` half of the constraint IS
/// enforced upstream, so it is not repeated here.
fn budget_name_matches_pattern(name: &str) -> bool {
    // `[^:\\]+`: at least one character, and no ':' or '\' anywhere. Any name holding
    // one of those two also fails the look-ahead's `[^:\\]*/action/` alternative, so
    // rejecting them here covers "/action/" with a prefix of other characters too.
    if name.is_empty() || name.contains(':') || name.contains('\\') {
        return false;
    }
    if name.contains("/action/") {
        return false;
    }
    // `(?i).*<script>.*</script>.*` — the case-insensitive half of the look-ahead.
    let lowered = name.to_lowercase();
    !lowered
        .split_once("<script>")
        .is_some_and(|(_, rest)| rest.contains("</script>"))
}

/// The refusal a `BudgetName` outside its modelled `@pattern` takes, or `None` when
/// the name satisfies it.
///
/// A value outside a MODELLED constraint is refused at request validation, before the
/// operation runs — the same class of refusal as a bad `AccountId` or an out-of-range
/// `MaxResults`, which the boundary validator answers with HTTP 400
/// `ValidationException` and a message naming the failing member path and the
/// constraint it violated. This pattern reaches a handler only because the framework's
/// regex engine cannot compile a negative look-ahead, so the refusal states the same
/// two facts, in the clause shape the framework's own renderer uses
/// (`N validation error(s) detected: Value … at '<path>' failed to satisfy constraint:
/// <requirement>` — [`::aws_smithy_schema_server::Violation::message`]).
///
/// It states them WITHOUT the rejected name and without the pattern, which is where it
/// departs from that renderer. `BudgetName` is a content rule, not a syntax rule: the
/// model documents that "Budget names are validated for content. Names that contain
/// phone numbers, URLs, or email addresses combined with certain terms may be
/// rejected", and the pattern itself refuses `<script>…</script>`. Echoing a name back
/// because it was refused for its content reflects exactly the content the rule exists
/// to keep out of this service, and restating the filter tells a caller how to get
/// around it. The member path and the constraint are what a caller needs to fix the
/// request, and they are what AWS's recorded refusal states.
///
/// `member_path` is the trail of Smithy member names from the operation input inwards
/// (`["Budget", "BudgetName"]` for `CreateBudget`, `["BudgetName"]` for the
/// operations that take the name directly); each segment is lowerCamel-ed by the
/// framework's own path builder.
pub fn budget_name_violation(member_path: &[&str], name: &str) -> Option<String> {
    if budget_name_matches_pattern(name) {
        return None;
    }
    let path: Vec<String> = member_path
        .iter()
        .map(|segment| ::aws_smithy_schema_server::constraints::path::member(segment))
        .collect();
    // Every segment is a Smithy member name chosen by a call site in this crate, so
    // nothing a caller sent reaches the sentence below.
    let path = ::aws_smithy_schema_server::constraints::path::join(&path);
    Some(format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern"
    ))
}

/// The cross-member rules `CreateBudget` and `UpdateBudget` document for the
/// `Budget` they accept, none of which a Smithy constraint trait can express.
///
/// A rule the model DOES express is not checked here: a value outside a modelled
/// constraint is a request-validation refusal, answered by
/// [`budget_name_violation`]'s renderers, not by this service's own
/// `InvalidParameterException`.
///
/// `Err` carries the sentence that goes in `InvalidParameterException$Message`
/// ("An error on the client occurred. Typically, the cause is an invalid input
/// value."). Every rule here is quoted from the model's own documentation:
///
/// * "You can't use `BudgetLimit` with `PlannedBudgetLimits` for `CreateBudget`
///   and `UpdateBudget` actions." (`Budget$BudgetLimit`)
/// * "only one set of filter and metric selections can be present in the syntax at
///   one time. Either `FilterExpression` and `Metrics` or `CostFilters` and
///   `CostTypes`, not both or a different combination." (`CreateBudget`)
/// * "The start date must come before the end date." (`Budget$TimePeriod`)
pub fn validate_budget(budget: &Budget) -> Result<(), String> {
    if budget.budget_limit.is_some() && budget.planned_budget_limits.is_some() {
        return Err(
            "Unable to create/update budget: BudgetLimit can't be used with \
             PlannedBudgetLimits."
                .to_string(),
        );
    }
    let legacy_filters = budget.cost_filters.is_some() || budget.cost_types.is_some();
    let expression_filters = budget.filter_expression.is_some() || budget.metrics.is_some();
    if legacy_filters && expression_filters {
        return Err(
            "Unable to create/update budget: FilterExpression and Metrics can't be used with \
             CostFilters and CostTypes."
                .to_string(),
        );
    }
    if let Some(period) = &budget.time_period {
        if let (Some(start), Some(end)) = (&period.start, &period.end) {
            let start_before_end = (start.secs(), start.subsec_nanos())
                < (end.secs(), end.subsec_nanos());
            if !start_before_end {
                return Err(
                    "Unable to create/update budget: TimePeriod Start must come before End."
                        .to_string(),
                );
            }
        }
    }
    Ok(())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBudgetsAwsbudgetservicegatewayState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>)
}
