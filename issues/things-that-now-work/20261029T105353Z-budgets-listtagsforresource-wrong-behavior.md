Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c57727bdfc46ee809d2c1cc52508769e6ed5546

# An action ARN of a budget whose name ends in "/action" resolves to the wrong budget, so that action's tags are unreachable

_Filed: 2026-10-29T10:53:53Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c57727bdfc46ee809d2c1cc52508769e6ed5546`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::parse_resource_arn` is the one place the three ARN-addressed tagging operations (`ListTagsForResource`, `TagResource`, `UntagResource`) resolve which resource a `ResourceARN` names. It splits the resource field on the FIRST occurrence of `/action/` (`resource.split_once(ARN_ACTION_INFIX)`), and the comment above that constant claims the split is safe because `BudgetName`'s `@pattern` forbids that substring inside a name: "so the split below is unambiguous however many slashes a budget name holds".

The claim does not hold. The pattern (and the crate's own re-implementation of it) forbids the substring `/action/` — with BOTH slashes — so a budget name ENDING in `/action` (e.g. `team/action`) is admitted by the model and stored by `CreateBudget` without complaint. For such a budget the action ARN the crate documents, `budget/{BudgetName}/action/{ActionId}`, renders as `budget/team/action/action/<uuid>`: the first `/action/` in it is the one formed by the end of the budget NAME plus the separator's leading slash, not the separator. `split_once` therefore yields `TaggedResource::Action { budget_name: "team", action_id: "action/<uuid>" }` — a different budget than the one that was created, and an action id no action ever carries (`ActionId` is a 36-char UUID).

Consequence: an action created on such a budget — including the `ResourceTags` `CreateBudgetAction` persists for it — can never be reached through the tag plane. `resource_tags` / `resource_tags_mut` look the misparsed pair up, find nothing, and all three handlers answer the modelled `NotFoundException` with a message naming a budget the caller did not ask about ("the budget action the ResourceARN names doesn't exist"). The tags are written once at create and are then unreadable, un-updatable and un-deletable, while the action itself still reads back normally through `DescribeBudgetAction`. Filed once against the first checklist operation whose handler calls the shared helper; `TagResource` and `UntagResource` resolve the ARN through the same call and misbehave identically.

## Evidence

The stated rule the code breaks — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:2379-2384:
    /// A budget ACTION is named beneath the budget it acts on (`Action$BudgetName`), so its
    /// ARN is the budget's with the action's own id below it:
    /// `budget/{BudgetName}/action/{ActionId}`. That is the substring `BudgetName`'s
    /// `@pattern` forbids inside a name ("the '/action/' substring aren't allowed"), so the
    /// split below is unambiguous however many slashes a budget name holds.
    const ARN_ACTION_INFIX: &str = "/action/";

The split it refers to, which takes the FIRST occurrence — src/state.rs:2440-2450:
    let resource = resource.strip_prefix(ARN_BUDGET_PREFIX).ok_or_else(malformed)?;
    match resource.split_once(ARN_ACTION_INFIX) {
        Some((budget_name, action_id)) => {
            if budget_name.is_empty() || action_id.is_empty() {
                return Err(malformed());
            }
            Ok(TaggedResource::Action {
                account_id: account_id.to_string(),
                budget_name: budget_name.to_string(),
                action_id: action_id.to_string(),
            })

The name the model admits, which makes the split ambiguous — `BudgetName` documentation (smithy_query(query="doc",shape="BudgetName")):
    "A string that represents the budget name. The \":\" and \"\\\" characters, and the \"/action/\" substring, aren't allowed."
and the modelled pattern quoted verbatim in the crate at src/state.rs:1785-1786:
    pub const BUDGET_NAME_PATTERN: &str =
        r"^(?![^:\\]*/action/|(?i).*<script>.*</script>.*)[^:\\]+$";
Neither forbids a name that merely ENDS in `/action`; the crate's own enforcement of the pattern rejects only the two-slash substring — src/state.rs:1804-1806:
    if name.contains("/action/") {
        return false;
    }
so `budget_name_violation(&["Budget", "BudgetName"], "team/action")` answers `None` and `CreateBudget` stores the budget (src/ops/create_budget.rs:201-205).

The handler that turns the misparse into the wrong refusal — src/ops/list_tags_for_resource.rs:162-184:
        let target = crate::state::parse_resource_arn(&input.resource_arn).map_err(|message| { ... })?;
        ...
        let held = crate::state::resource_tags(&store, &target).ok_or_else(|| {
            ListTagsForResourceErrorModeled::NotFoundException(NotFoundException {
                message: Some(match &target {
                    ...
                    crate::state::TaggedResource::Action { .. } => {
                        "Unable to list tags: the budget action the ResourceARN names doesn't \
                         exist."
(the same two calls appear in src/ops/tag_resource.rs:168-202 and src/ops/untag_resource.rs:154-177).

The ARN layout both sides agree on — tests/all/resource_tags.rs:60-67:
    /// The ARN of a budget action, which is named beneath the budget it acts on:
    /// `…:budget/{BudgetName}/action/{ActionId}`.
    pub(crate) fn action_arn(account_id: &str, budget_name: &str, action_id: &str) -> String {
        format!("{}/action/{action_id}", budget_arn(account_id, budget_name))

Test coverage that stops short of this shape — tests/all/resource_tag_reads.rs:219-245 states the rule this breaks ("RULE 3: a budget name `BudgetName`'s modelled `@pattern` admits is carried unchanged inside the ARN, so the budget the ARN resolves to is the one that was created") and its rows exercise slashes and the word action only through the BUDGET ARN — ("several slashes", "team/platform/finops"), ("the word action with one slash", "action/") — with no row resolving an ACTION ARN of a budget whose name ends in `/action`; tests/all/resource_tags.rs and tests/all/resource_tag_deletion.rs build action ARNs only for plain names.

## Steps to Reproduce

1. Call CreateBudget with AccountId = the emulator's account and Budget = {BudgetName: "team/action", TimeUnit: "MONTHLY", BudgetType: "COST", BudgetLimit: {Amount: "100", Unit: "USD"}} — it answers 200, because the modelled pattern and crate::state::budget_name_matches_pattern forbid only the two-slash substring "/action/".
2. Call CreateBudgetAction for that budget (BudgetName "team/action", any valid ActionType/Definition/ExecutionRoleArn/Subscribers) with ResourceTags = [{Key: "owner", Value: "finance"}]; note the minted ActionId. It answers 200 and the action reads back through DescribeBudgetAction.
3. Call ListTagsForResource with ResourceARN = "arn:aws:budgets::<account>:budget/team/action/action/<ActionId>" — the ARN layout src/state.rs:2379-2381 and tests/all/resource_tags.rs:60-67 both document for that action. Expected: the one tag the create persisted. Actual: 400 NotFoundException, because split_once takes the first "/action/" and resolves the ARN to budget "team" with action id "action/<ActionId>".
4. Repeat with TagResource and UntagResource on the same ARN: both answer NotFoundException, so the action's tag set can never be read, re-valued or deleted after creation.
5. Contrast with a budget named "team" holding an action: "arn:aws:budgets::<account>:budget/team/action/<ActionId>" resolves correctly and answers 200 — the defect is confined to names whose own text ends in "/action".
