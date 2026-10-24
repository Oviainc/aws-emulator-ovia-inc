Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreateBillingGroup answers 409 ConflictException when a linked account belongs to another group instead of 400 ValidationException (ACCOUNTS_ALREADY_ASSOCIATED)

_Filed: 2026-10-24T02:49:10Z_
_Operation: CreateBillingGroup | Class: error-response_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: creating a billing group whose `AccountGrouping.LinkedAccountIds` contains an account already associated with another billing group is refused as an input-validation failure — HTTP 400 `ValidationException` with the modelled reason `ACCOUNTS_ALREADY_ASSOCIATED` and `AccountGrouping.LinkedAccountIds` named in `Fields` (the same shape the handler already uses for its sibling `DUPLICATE_ACCOUNT` refusal).

Actual: the handler returns HTTP 409 `ConflictException` with `ResourceType="account"` and `Reason` omitted, so the error class, status code and reason all differ from the modelled refusal for this case.

## Evidence

Smithy model: `ValidationExceptionReason` includes `ACCOUNTS_ALREADY_ASSOCIATED` and `DUPLICATE_ACCOUNT`; no `ConflictExceptionReason` value describes an account owned by another billing group. Handler: `src/ops/create_billing_group.rs` lines 247-270 — the duplicate-account case immediately above (lines 233-239) is correctly refused with `ValidationException(DUPLICATE_ACCOUNT)`, while the already-associated case returns `ConflictException { resource_type: "account", reason: None }`. Test coverage: no test pins the status or code of this path.

## Steps to Reproduce

1. Authorize as the payer account and POST /create-pricing-plan to get a plan ARN.
2. POST /create-billing-group with {"Name":"G1","AccountGrouping":{"LinkedAccountIds":["111111111111"]},"ComputationPreference":{"PricingPlanArn":"<plan>"}} — succeeds.
3. POST /create-billing-group with {"Name":"G2","AccountGrouping":{"LinkedAccountIds":["111111111111"]},"ComputationPreference":{"PricingPlanArn":"<plan>"}}.
4. Observe HTTP 409 ConflictException without a `Reason` instead of HTTP 400 ValidationException with reason ACCOUNTS_ALREADY_ASSOCIATED.
