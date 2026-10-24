Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# Any 12-digit AccountId is served as its own budget namespace instead of AccessDeniedException for an account that is not the caller's

_Filed: 2026-10-24T15:02:47Z_
_Operation: DescribeBudgets | Class: error-response_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every budgets operation takes `AccountId` (`^\d{12}$`), documented as "The accountId that is associated with the budget", and every one of them declares `AccessDeniedException` (HTTP 403) — "You are not authorized to use this operation with the given parameters." In AWS the budgets control plane is scoped to the calling account: signing with the credentials of account A and passing `AccountId` of account B is refused with `AccessDeniedException`, not served.

The emulator instead treats the request's `AccountId` as nothing more than a partition key of its store. `DescribeBudgets` with a foreign `AccountId` answers HTTP 200 with `{"Budgets":[]}` (or with that account's budgets, if an earlier call created them under the same foreign id), and the same holds for the whole implemented surface: `CreateBudget`/`CreateNotification`/`CreateBudgetAction` happily create resources under `AccountId="000000000000"`, and the subsequent `DescribeBudget`/`DescribeNotificationsForBudget`/`DescribeBudgetAction` read them back. Nothing in the crate ever compares the supplied `AccountId` to the emulator's own account (`::engine::emulator_account_id()`), and `AccessDeniedException` is never constructed anywhere — it appears only as an unused variant of each operation's modeled error enum.

Expected: a request whose `AccountId` is not the caller's account is refused with HTTP 403 `AccessDeniedException` and nothing is created or reported. Actual: HTTP 200, with a parallel, fully functional budget namespace per arbitrary account id.

## Evidence

Spec: Smithy model — `AccountId` member doc ("The accountId that is associated with the budget") on every operation; `AccessDeniedException` (HTTP 403, "You are not authorized to use this operation with the given parameters") declared by CreateBudget, DescribeBudget, DescribeBudgets, DeleteBudget, UpdateBudget, CreateNotification, DeleteNotification, DescribeNotificationsForBudget, DescribeSubscribersForNotification, CreateBudgetAction, DescribeBudgetAction, DescribeBudgetActionHistories, DeleteBudgetAction, UpdateBudgetAction, ExecuteBudgetAction.
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/describe_budgets.rs:155-201 — `input.account_id` is passed straight to `crate::state::budgets_for_account(...)` with no caller check; services/.../src/ops/create_budget.rs:222 builds the store key as `(input.account_id, budget.budget_name)`; services/.../src/state.rs:31-33 keys `budgets` by `(AccountId, BudgetName)` and state.rs:1015-1020 explicitly contemplates "a client … varying AccountId" and bounds it with a memory ceiling rather than refusing it.
Emulator facility that exists for this: `::engine::emulator_account_id()` (vendor/server/src/lib.rs:149), used by sibling services for exactly this gate (e.g. services/service-com-amazonaws-billing-awsbilling/src/state.rs:970 `is_caller_account_arn`, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:1842).
Tests: `grep -r AccessDeniedException src/` matches only the generated error-enum variants (one per operation file); no test in tests/all/* sends an AccountId other than the canonical one, so nothing asserts the refusal.
Not a duplicate: the existing budgets issues cover per-operation validation/limits/read projection; none covers the account-scoping gate (compare the accepted precedent for other services: `account` GetAccountInformation validation, `invoicing` BatchGetInvoiceProfile error-response).

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateBudget` and body {"AccountId":"000000000000","Budget":{"BudgetName":"foreign","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} — emulator answers HTTP 200; AWS answers HTTP 403 AccessDeniedException because the signing account is not 000000000000.
2. POST / with `X-Amz-Target: AWSBudgetServiceGateway.DescribeBudgets` and body {"AccountId":"000000000000"} — emulator answers HTTP 200 and reports the budget created in step 1; AWS answers HTTP 403 AccessDeniedException.
3. Repeat DescribeBudgets with an account id no call has ever used, e.g. {"AccountId":"999999999999"} — emulator answers HTTP 200 `{"Budgets":[]}` instead of HTTP 403 AccessDeniedException.
4. Confirm the same for the other implemented operations (CreateNotification, CreateBudgetAction, DescribeBudget, DescribeBudgetAction) under the foreign AccountId: each one answers 2xx.
