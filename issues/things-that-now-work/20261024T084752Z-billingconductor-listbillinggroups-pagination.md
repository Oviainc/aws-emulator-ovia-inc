Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# ListBillingGroups accepts any string as NextToken instead of refusing a malformed token

_Filed: 2026-10-24T08:47:52Z_
_Operation: ListBillingGroups | Class: pagination_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination token of this service is an opaque value the server mints; a caller may only echo back a token a previous page handed it. AWS refuses a token it did not mint (or one that has been corrupted) with the modelled 400 `ValidationException` rather than serving an arbitrary page.

Expected: `POST /list-billing-groups` with `NextToken: "not-a-token"` is refused 400 `ValidationException` (the operation's modelled 400; `ValidationExceptionReason` enumerates `CANNOT_PARSE` and `FIELD_VALIDATION_FAILED` for exactly this class of input fault).

Actual: the shared `crate::state::paginate` helper treats the token as a plain-text sort key and simply keeps the elements whose key is `>= token` (`items.filter(|(key, _)| key.as_str() >= start)`). Any string is therefore a legal cursor: a token the server never minted silently yields 200 with a truncated or empty `BillingGroups` list and no `NextToken`, so a client with a corrupted token is told "no more billing groups" instead of being told its token is invalid. Because the token IS the stored key (an ARN for this operation, an account id for `ListAccountAssociations`, a rule/plan ARN for the association lists), it is also not opaque and a token minted by one list operation is silently accepted by another.

## Evidence

Spec: `ListBillingGroups` is `@paginated(inputToken=NextToken, outputToken=NextToken, items=BillingGroups)` with `NextToken` targeting the opaque `Token` string, and the operation declares `ValidationException` (HTTP 400) — see OPERATIONS.md, ListBillingGroups. Well-established AWS behaviour: a `NextToken` that the service did not issue is rejected, not interpreted.

Handler/helper: `src/ops/list_billing_groups.rs` passes `input.next_token.as_deref()` straight to `crate::state::paginate`; `src/state.rs` `paginate()` does `let start = next_token.unwrap_or(""); items.filter(|(key, _)| key.as_str() >= start)` and never validates the token or reports an unknown one. The same helper backs every implemented paginated read (`ListAccountAssociations`, `ListPricingPlans`, `ListPricingRules`, `ListPricingRulesAssociatedToPricingPlan`, `ListPricingPlansAssociatedWithPricingRule`, `ListResourcesAssociatedToCustomLineItem`), so the behaviour is service-wide.

Tests: the suite only round-trips tokens the server minted (`tests/all/billing_group_core_loop.rs:851` and `:1191`, `tests/all/pricing_plan_list.rs:149`), and `tests/all/payer_account_authorization.rs:272` asserts a token is not a capability across callers. No test asserts the answer for a token the server never issued, so the refusal is unpinned.

## Steps to Reproduce

1. Make the emulated account the payer so the control plane is reachable, then create two billing groups so a listing is non-empty.
2. POST /list-billing-groups with `{"NextToken": "not-a-token"}`.
3. Observe HTTP 200 with an empty (or arbitrarily truncated) `BillingGroups` list instead of a 400 `ValidationException` for a token the service never minted.
4. POST /list-billing-groups with `{"MaxResults": 1}`, take the returned `NextToken`, and pass it to POST /list-pricing-plans: observe it is accepted as a cursor there too, showing the token is a plain-text key rather than an opaque, operation-scoped value.
