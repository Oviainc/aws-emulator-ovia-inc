Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# UpdateBillingGroup enables AutoAssociate on any number of billing groups, enforcing no cap

_Filed: 2026-10-24T15:03:06Z_
_Operation: UpdateBillingGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: automatic account association is limited to one billing group per consolidated billing family, and the model names the refusal: `ValidationExceptionReason` enumerates `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`. An `UpdateBillingGroup` that would turn `AccountGrouping.AutoAssociate` on for a second group is refused 400 with that reason (the create path has the same rule — filed separately as CreateBillingGroup/validation).

Actual: the handler copies `AccountGrouping.AutoAssociate` from the request onto the record with no cross-group check, so a caller can create one auto-associate group and then flip every other group to `AutoAssociate: true` one `UpdateBillingGroup` call at a time; `ListBillingGroups` with `Filters.AutoAssociate=true` then reports all of them. No request on this path ever earns the modelled refusal.

## Evidence

Smithy model: `ValidationExceptionReason` includes `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`; `UpdateBillingGroupAccountGrouping$AutoAssociate` is documented as "Specifies if the billing group has automatic account association (AutoAssociate) enabled" and `AccountGrouping$AutoAssociate` as associating "newly added Amazon Web Services accounts that join your consolidated billing family". Handler: `src/ops/update_billing_group.rs` lines 308-314 set `updated.auto_associate = Some(auto_associate)` after the name-conflict, pricing-plan, responsibility-transfer and status checks — none of which inspects any other group's `auto_associate`. Test coverage: `tests/all/update_billing_group.rs` exercises the AutoAssociate round-trip but asserts no cap. The sibling gap on the create path is filed as `20261024T024910Z-billingconductor-createbillinggroup-validation` (CreateBillingGroup/validation), which does not cover this operation.

## Steps to Reproduce

1. Authorize as the payer account; POST /create-pricing-plan to get a plan ARN
2. POST /create-billing-group twice (Name="Auto1" and Name="Auto2") with {"AccountGrouping":{"LinkedAccountIds":[],"AutoAssociate":true}} for the first and AutoAssociate omitted for the second
3. POST /update-billing-group with {"Arn":"<Auto2>","AccountGrouping":{"AutoAssociate":true}}
4. Observe HTTP 200 and POST /list-billing-groups with {"Filters":{"AutoAssociate":true}} reporting both groups, where the modelled TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS refusal is expected
