Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# AssociateAccounts answers 409 ConflictException for an account already in another billing group instead of 400 ValidationException (ACCOUNTS_ALREADY_ASSOCIATED)

_Filed: 2026-10-24T02:49:10Z_
_Operation: AssociateAccounts | Class: error-response_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: associating an account that is already a member of a different billing group is an input-validation failure the model names explicitly — `ValidationExceptionReason` enumerates `ACCOUNTS_ALREADY_ASSOCIATED` (and `ACCOUNTS_NOT_ASSOCIATED` for the mirror case on disassociate) — so the refusal is HTTP 400 `ValidationException` with that reason and the offending member (`AccountIds`) in `Fields`.

Actual: the handler returns HTTP 409 `ConflictException` with `ResourceType="account"` and NO `Reason` at all. A client that keys retry/branching off the status code or off the error code sees a different error class than the real service, and the modelled `Reason` member is absent because — as the handler's own comment concedes — no value of `ConflictExceptionReason` describes this case.

## Evidence

Smithy model: `ValidationExceptionReason` includes `ACCOUNTS_ALREADY_ASSOCIATED`; `ConflictExceptionReason` enumerates only `RESOURCE_NAME_CONFLICT`, `PRICING_RULE_IN_PRICING_PLAN_CONFLICT`, `PRICING_PLAN_ATTACHED_TO_BILLING_GROUP_DELETE_CONFLICT`, `PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT`, `PRICING_RULE_ATTACHED_TO_PRICING_PLAN_DELETE_CONFLICT`, `WRITE_CONFLICT_RETRY` — none of which covers an account owned by another billing group, and `ConflictException` is documented as "You can cause an inconsistent state by updating or deleting a resource". Handler: `src/ops/associate_accounts.rs` lines 205-226 returns `ConflictException { resource_type: "account", reason: None }`; the in-code comment at lines 215-221 states that no ConflictExceptionReason fits. Test coverage: no test asserts the status/code for this path.

## Steps to Reproduce

1. Authorize as the payer account and POST /create-billing-group twice to create groups G1 and G2 (both referencing an existing pricing plan).
2. POST /associate-accounts with {"Arn":"<G1>","AccountIds":["111111111111"]} — succeeds.
3. POST /associate-accounts with {"Arn":"<G2>","AccountIds":["111111111111"]}.
4. Observe HTTP 409 with error type ConflictException and no `Reason`, where the modelled refusal for this case is HTTP 400 ValidationException with reason ACCOUNTS_ALREADY_ASSOCIATED.
