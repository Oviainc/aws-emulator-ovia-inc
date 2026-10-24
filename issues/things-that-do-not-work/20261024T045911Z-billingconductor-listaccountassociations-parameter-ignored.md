Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# ListAccountAssociations ignores BillingPeriod and always reports the current associations

_Filed: 2026-10-24T04:59:11Z_
_Operation: ListAccountAssociations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `BillingPeriod` selects the month the associations are reported for — "list linked accounts that are linked to the payer account for the specified time period. If no information is provided, the current billing period is used" (operation documentation). An account associated with a billing group only from 2026-03 onwards is therefore reported with that `BillingGroupArn` for 2026-03 and later, and as UNMONITORED (no `BillingGroupArn`) for an earlier period; the same holds for a group that was deleted.

Actual: the handler reads `input.billing_period` only in a comment and never uses it — every billing period, past or future, is answered with the store's CURRENT association index. A client asking about a closed billing period gets today's membership, so period-over-period reporting (the purpose of this API) cannot be distinguished from a single snapshot, and an obviously wrong period is accepted instead of being judged (the model enumerates `ILLEGAL_BILLING_PERIOD` / `INVALID_BILLING_PERIOD_FOR_OPERATION` reasons for `ValidationException`).

The same simplification is stated in the sibling list handlers (`list_billing_groups.rs`, `list_pricing_rules.rs`, `list_pricing_rules_associated_to_pricing_plan.rs`), but it is most visible here, where the association-by-period IS the resource being listed.

## Evidence

Spec: `smithy_query(query="doc",shape="ListAccountAssociations")` — "...for the specified time period. If no information is provided, the current billing period is used"; input `BillingPeriod` (pattern ^\\d{4}-(0?[1-9]|1[012])$); `ValidationExceptionReason` enumerates `ILLEGAL_BILLING_PERIOD` and `INVALID_BILLING_PERIOD_FOR_OPERATION`.

Handler: `src/ops/list_account_associations.rs:180-182` — "`BillingPeriod` selects the month to report for; this emulator keeps no per-period history, so every period reports the current associations"; `input.billing_period` is never read again, and the store keeps no history (`src/state.rs` holds one `account_associations` map with no period dimension, unlike `ResourceAssociationRecord`, which does carry a billing-period range and IS honoured by `ListResourcesAssociatedToCustomLineItem`).

Tests: no test passes `BillingPeriod` to this operation and asserts a period-specific answer.

## Steps to Reproduce

1. Boot the emulator (payer account designated) and create a billing group with LinkedAccountIds=["111122223333"]
2. POST /list-account-associations with {"BillingPeriod":"2020-01"} — a period long before the group existed
3. Observe the account reported with the new group's BillingGroupArn instead of as UNMONITORED for that period
4. Delete the billing group, then POST /list-account-associations with the CURRENT billing period and with the period during which the group existed — both answer identically, showing the parameter has no effect
