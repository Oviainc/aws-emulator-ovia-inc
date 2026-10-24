Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreateBillingGroup enforces no cap on how many billing groups may have AutoAssociate enabled

_Filed: 2026-10-24T02:49:10Z_
_Operation: CreateBillingGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: automatic account association is limited to a small number of billing groups per consolidated billing family (AWS allows it on only one group), and the model names the refusal: `ValidationExceptionReason` enumerates `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`. A create that would exceed the cap is refused 400 with that reason.

Actual: the handler persists `account_grouping.auto_associate` verbatim with no cross-group check, so an unbounded number of groups can be created with `AutoAssociate = true` and `ListBillingGroups`'s `AutoAssociate` filter reports them all. No request ever earns the `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS` refusal the model declares.

## Evidence

Smithy model: `ValidationExceptionReason` includes `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`; `AccountGrouping.AutoAssociate` is documented as "Specifies if this billing group will automatically associate newly added Amazon Web Services accounts that join your consolidated billing family". Handler: `src/ops/create_billing_group.rs` lines 297-310 store `auto_associate: input.account_grouping.auto_associate` after checks for name conflict, missing pricing plan, duplicate accounts, already-associated accounts and the group-count cap — none of which looks at other groups' `auto_associate`. Test coverage: `tests/all/billing_group_core_loop.rs` creates AutoAssociate groups and filters on the flag but asserts no cap.

## Steps to Reproduce

1. Authorize as the payer account and POST /create-pricing-plan to get a plan ARN.
2. POST /create-billing-group with {"Name":"Auto1","AccountGrouping":{"LinkedAccountIds":[],"AutoAssociate":true},"ComputationPreference":{"PricingPlanArn":"<plan>"},"PrimaryAccountId":"111111111111"} — succeeds.
3. Repeat with Name="Auto2" (and "Auto3"), also with AutoAssociate true.
4. Observe every call answering HTTP 200, and POST /list-billing-groups with {"Filters":{"AutoAssociate":true}} reporting all of them, where the modelled refusal TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS is expected once the cap is exceeded.
