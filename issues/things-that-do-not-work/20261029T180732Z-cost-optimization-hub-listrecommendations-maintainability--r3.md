Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# Collapse the duplicated organization-view permission gate in state.rs

_Filed: 2026-10-29T18:07:32Z_
_Operation: ListRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The "a read that reaches past the caller's own account needs the organization's view, which only the management account has" gate is implemented TWICE in this crate's own state module, and the two copies have already drifted in their predicate.

`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::addressed_accounts_for_read` (used by the `ListEnrollmentStatuses` handler) asks the question through `ReadScope::reaches_past`, which treats a named scope holding only the caller's own account as NOT reaching past the caller. `recommendation_view` (used by the `ListRecommendations` and `ListRecommendationSummaries` handlers) needs the same gate but does not go through either `ReadScope` or that helper: it inlines its own `!named.is_empty() && !self.is_management_account(..)` check and only then calls `addressed_accounts` for the scope. So the gate predicate exists in two spellings that must be kept in step by hand, even though the `ReadScope` doc comment above them claims the question is answered in one place.

This is a duplication finding only: both behaviours are pinned by tests and must stay exactly as they are. The de-duplication is response-preserving — route the recommendation readers' gate through the same `ReadScope`-based entry point while keeping its "any populated accountIds" predicate (e.g. a second question on `ReadScope` beside `reaches_past`), so the permission rule lives in one place and every status code and body stays identical.

## Evidence

Copy 1 — `services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/state.rs:1087-1098`:
```
    pub fn addressed_accounts_for_read(
        &self,
        caller: &str,
        named: &[String],
        whole_organization: bool,
    ) -> Result<Vec<String>, OrganizationViewRequired> {
        let scope = ReadScope::of(named, whole_organization);
        if scope.reaches_past(caller) && !self.is_management_account(caller) {
            return Err(OrganizationViewRequired);
        }
        Ok(self.accounts_in(scope, caller))
    }
```

Copy 2 — `src/state.rs:1181-1194` (inside `recommendation_view`, the helper both recommendation readers call):
```
        let named = filter
            .and_then(|filter| filter.account_ids.as_deref())
            .unwrap_or_default();
        // Refused for ANY populated `accountIds`, because what the caller lacks
        // is the view, not one account in it.
        if !named.is_empty() && !self.is_management_account(account_id) {
            return Err(OrganizationViewRequired);
        }
...
        let addressed = self.addressed_accounts(account_id, named, false);
```

The drift between the two predicates — `src/state.rs:1281-1287`:
```
    fn reaches_past(&self, caller: &str) -> bool {
        match self {
            Self::Named(named) => named.iter().any(|wanted| wanted != caller),
```
versus copy 2's `!named.is_empty()`.

The crate states the rule is written once — `src/state.rs:1246-1253`: "That precedence — a named account beats the organization's view, so naming the caller's own account is a read of that one record however the organization member is set — is written once here, because both the accounts a reader scopes its rows to (`...::addressed_accounts`) and whether its caller needs the organization's view at all (`...::addressed_accounts_for_read`) are questions about this one scope." `recommendation_view` answers the second question without building a `ReadScope` at all.

Callers of the two copies: `src/ops/list_enrollment_statuses.rs:195-203` (`store.addressed_accounts_for_read(caller, named, input.include_organization_info)`) and `src/ops/list_recommendations.rs:221-224` / `src/ops/list_recommendation_summaries.rs:274-277` (`state.lock().recommendation_view(crate::state::caller_account_id(), input.filter.as_ref())`).

Existing tests that pin both behaviours, and which must keep passing unchanged: `tests/all/enrollment_read_scope_gate.rs:279` ("Naming the caller's own account answers the caller's own record whatever …" — admitted, HTTP 200), `tests/all/shared_recommendation_view.rs:213-236` ("The organization gate is one rule: a populated `filter.accountIds` is refused", whose first row is `("the caller's own account", json!({"accountIds": [account]}))` — refused), `tests/all/management_account_gating.rs:57-85` and `tests/all/account_scoped_reads.rs:197-313`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/state.rs` and read `addressed_accounts_for_read` (lines 1087-1098) and `recommendation_view` (lines 1176-1195): both answer "does this read need the organization's view?" and both return `OrganizationViewRequired`, but only the first asks `ReadScope::reaches_past`.
2. Compare the predicates: `reaches_past` (line 1283) excludes a named scope that names only the caller, while the inline check at line 1186 refuses any non-empty `accountIds` — the two copies have already diverged.
3. Note the `ReadScope` doc at lines 1246-1253 claiming the gate question is "written once here", which the second copy bypasses.
4. Confirm the behavioural difference is intentional and test-pinned (`tests/all/enrollment_read_scope_gate.rs:279` admits `accountId` = caller; `tests/all/shared_recommendation_view.rs:225` refuses `filter.accountIds` = [caller]), so the fix must keep every response and test unchanged while leaving one implementation of the gate.
