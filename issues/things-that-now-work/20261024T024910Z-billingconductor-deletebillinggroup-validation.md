Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# DeleteBillingGroup deletes a billing group that has AutoAssociate enabled instead of refusing it

_Filed: 2026-10-24T02:49:10Z_
_Operation: DeleteBillingGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a billing group with automatic account association turned on (`AccountGrouping.AutoAssociate = true`) cannot be deleted while that setting is on — the model enumerates `CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP` in `ValidationExceptionReason`, and `DeleteBillingGroup` declares `ValidationException` (400) as its only client-input error, so that reason can only belong to this operation. The caller must first turn `AutoAssociate` off (UpdateBillingGroup) and then delete.

Actual: the handler removes any group its `Arn` resolves, regardless of `auto_associate`, and answers 200 with the ARN. The group, its custom line items and its account associations are all destroyed on a call the real service refuses — unrecoverable state loss against a request that should have failed.

## Evidence

Smithy model: `ValidationExceptionReason` includes `CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP` (and `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`); `DeleteBillingGroup` errors = AccessDenied, InternalServer, Throttling, ValidationException. Handler: `src/ops/delete_billing_group.rs` lines 155-177 — the only refusal is `INVALID_BILLING_GROUP` for a group that does not exist; `auto_associate` is never consulted even though it is persisted on `BillingGroupRecord` (`src/state.rs` line ~335) and reported by `ListBillingGroups`. Test coverage: `tests/all/billing_group_core_loop.rs` creates groups with `"AutoAssociate": true` (lines 334, 913) and filters on it (lines 1013-1019) but no test exercises deleting one.

## Steps to Reproduce

1. Authorize as the payer account and POST /create-pricing-plan to get a plan ARN.
2. POST /create-billing-group with {"Name":"AutoG","AccountGrouping":{"LinkedAccountIds":["111111111111"],"AutoAssociate":true},"ComputationPreference":{"PricingPlanArn":"<plan>"}}.
3. POST /delete-billing-group with {"Arn":"<group>"}.
4. Observe HTTP 200 and the group gone from POST /list-billing-groups, where the modelled refusal is 400 ValidationException with reason CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP and the group still present.
