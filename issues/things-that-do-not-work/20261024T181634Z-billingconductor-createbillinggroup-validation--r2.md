Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e42d60e9c80231c2e4470b4ab9cec2534ece4a72

# CreateBillingGroup accepts a PrimaryAccountId that is not one of the group's linked accounts (and an unvalidated AccountGrouping generally)

_Filed: 2026-10-24T18:16:34Z_
_Operation: CreateBillingGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `e42d60e9c80231c2e4470b4ab9cec2534ece4a72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The billing group's AccountGrouping is persisted without checking the relationships the service's own ValidationExceptionReason values name as refusals.

Expected: a CreateBillingGroup whose PrimaryAccountId is not among AccountGrouping.LinkedAccountIds is refused HTTP 400 ValidationException with Reason PRIMARY_NOT_ASSOCIATED (the primary account is "the account ID that serves as the main account in a billing group", so it has to be an account of that group).

Actual: the handler stores the primary account as given, and when it is not a member it deliberately records it in the released/UNMONITORED tail instead of refusing — so the call answers HTTP 200, ListBillingGroups reports the group with Status ACTIVE and a PrimaryAccountId that belongs to NO billing group, and ListAccountAssociations reports that same account as UNMONITORED while it is the group's "main account". Two further symptoms of the same unvalidated-grouping class:
- AccountGrouping.LinkedAccountIds may be empty (or omitted, since the member carries @default([])), creating a billing group with Size 0; ValidationExceptionReason enumerates MISSING_LINKED_ACCOUNT_IDS (and MULTIPLE_LINKED_ACCOUNT_IDS) for exactly this input.
- AccountGrouping.ResponsibilityTransferArn is stored without checking that it names a billing transfer of this organization, even though the same store already has that predicate (crate::state::transfer_belongs_to_this_organization, which GetBillingTransferPreference and UpdateBillingTransferPreference both enforce). The result is a group reported with BillingGroupType TRANSFER_BILLING, a billing owner and Status ACTIVE on a transfer relationship that exists nowhere.

## Evidence

Spec: ValidationExceptionReason (smithy_query query="shape") enumerates PRIMARY_NOT_ASSOCIATED, MISSING_LINKED_ACCOUNT_IDS and MULTIPLE_LINKED_ACCOUNT_IDS, and CreateBillingGroup lists ValidationException (HTTP 400) among its errors; AccountGrouping$LinkedAccountIds is documented "Account IDs must be a part of the consolidated billing family, and not associated with another billing group" and PrimaryAccountId "The account ID that serves as the main account in a billing group".

Handler: services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/create_billing_group.rs — the only grouping checks are duplicate account ids, cross-group ownership and the AutoAssociate cap; lines ~310-338 persist `primary_account_id: input.primary_account_id.clone()` and then call `state.note_unassociated_account(&primary)` when the primary is not in `account_associations`, with the comment "The primary account belongs to the consolidated billing family whether or not it is a member of this group". `responsibility_transfer_arn` is copied straight from the input (line ~318) with no ownership check, while src/ops/get_billing_transfer_preference.rs and src/ops/update_billing_transfer_preference.rs both gate on `transfer_belongs_to_this_organization`.

Tests: tests/all/billing_group_core_loop.rs only creates groups whose primary IS a linked account (cases at lines ~316-341); tests/all/account_exclusivity_refusal.rs covers the already-associated/duplicate rules. No test asserts either acceptance or refusal of a non-member primary, an empty LinkedAccountIds create, or a foreign ResponsibilityTransferArn. The existing CreateBillingGroup validation issue (…-createbillinggroup-validation.md, fixed) is about the AutoAssociate cap, a different rule.

## Steps to Reproduce

1. Designate the emulated account as the payer account so the control plane is reachable (the service's payer gate otherwise answers 403 for every operation).
2. POST /create-pricing-plan with {"Name":"Plan1"} and keep the returned Arn.
3. POST /create-billing-group with {"Name":"Group1","AccountGrouping":{"LinkedAccountIds":["111122223333"]},"ComputationPreference":{"PricingPlanArn":"<plan arn>"},"PrimaryAccountId":"999988887777"} — an account that is not in LinkedAccountIds.
4. Observe HTTP 200 and a minted ARN instead of a 400 ValidationException whose Reason is PRIMARY_NOT_ASSOCIATED.
5. POST /list-billing-groups and observe the group reported with Status ACTIVE and PrimaryAccountId 999988887777.
6. POST /list-account-associations and observe 999988887777 reported with no BillingGroupArn (UNMONITORED) while it is the group's primary account.
7. Also: POST /create-billing-group with {"Name":"Group2","AccountGrouping":{"LinkedAccountIds":[]},"ComputationPreference":{"PricingPlanArn":"<plan arn>"},"PrimaryAccountId":"111122223333"} and observe HTTP 200 with a Size-0 group instead of a MISSING_LINKED_ACCOUNT_IDS refusal.
8. Also: POST /create-billing-group naming AccountGrouping.ResponsibilityTransferArn for a transfer of some other organization and observe HTTP 200 plus BillingGroupType TRANSFER_BILLING, while GET of that same transfer via POST /get-billing-transfer-preference answers 404.
