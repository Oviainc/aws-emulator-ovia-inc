Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the billingconductor emulator

_Filed: 2026-10-21T21:19:04Z_
_Operation: service-com-amazonaws-billingconductor-awsbillingconductor | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 34 operations in `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST, SMALL stateful slice: the pricing-plan + billing-group core loop, which is the primary resource lifecycle of AWS Billing Conductor (protocol `restJson1`, sigv4 name `billingconductor`).

### Happy path to implement (in order)

1. **CreatePricingPlan** — `POST /create-pricing-plan`. Input: `Name` (required, 1..128, `^[a-zA-Z0-9_\+=\.\-@]+$`), `Description` (0..1024), `PricingRuleArns` (0..30), `Tags`, `ClientToken` (`X-Amzn-Client-Token`, @idempotencyToken). Output: `Arn` — must be a freshly minted, stored ARN of the modeled form `arn:aws:billingconductor::<account>:pricingplan/<10 alnum>`; the plan record must be persisted so step 2 can validate against it.
2. **CreateBillingGroup** — `POST /create-billing-group`. Input: `Name` (required), `AccountGrouping` (required; `LinkedAccountIds` list 0..30, `AutoAssociate`), `ComputationPreference` (required; `PricingPlanArn` required and must match `^arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})$`), `PrimaryAccountId` (`^[0-9]{12}$`), `Description`, `Tags`, `ClientToken`. Output: `Arn` — minted `arn:aws:billingconductor::<account>:billinggroup/<10-12 alnum>` and stored with `Name`, `Description`, `PrimaryAccountId`, `ComputationPreference`, `AccountGrouping`, `Size` = number of linked accounts, `Status` = `ACTIVE` (or `PRIMARY_ACCOUNT_MISSING` when no primary account and no `ResponsibilityTransferArn`), `CreationTime`/`LastModifiedTime` epoch millis. A `ComputationPreference.PricingPlanArn` that names no stored plan must fail with `ResourceNotFoundException` (404); a duplicate `Name` with a different payload must fail `ConflictException` (409); the same `ClientToken` replayed must return the SAME `Arn` instead of creating a second group.
3. **AssociateAccounts** — `POST /associate-accounts`. Input: `Arn` (required, billing group), `AccountIds` (required, 0..30). Must append the account IDs to the stored group's `AccountGrouping.LinkedAccountIds`, bump `Size` and `LastModifiedTime`, and return the group `Arn`. Unknown group `Arn` → `ResourceNotFoundException` (404); an account already associated to a DIFFERENT group → `ConflictException` (409), since an account may belong to only one billing group.
4. **ListBillingGroups** — `POST /list-billing-groups`. Input: `BillingPeriod` (`^\d{4}-(0?[1-9]|1[012])$`), `MaxResults` (1..100), `NextToken`, `Filters` (at minimum honour `Arns`, `Names`, `PricingPlan`). Output: `BillingGroups` as `BillingGroupListElement` records read back from state (`Name`, `Arn`, `Description`, `PrimaryAccountId`, `ComputationPreference`, `Size`, `CreationTime`, `LastModifiedTime`, `Status`, `AccountGrouping`) plus `NextToken` when truncated by `MaxResults`. The group created in step 2 with the accounts from step 3 must appear with the correct `Size`.
5. **ListAccountAssociations** — `POST /list-account-associations`. Input: `BillingPeriod`, `Filters` (`Association` = a billing group ARN / `MONITORED` / `UNMONITORED`, `AccountId`, `AccountIds`), `NextToken`. Output: `LinkedAccounts` as `AccountAssociationsListElement` (`AccountId`, `BillingGroupArn`, `AccountName`, `AccountEmail`) derived from the association map written in steps 2-3 — each account associated in step 3 must come back carrying the group ARN, and must be reported as `MONITORED`.
6. **DeleteBillingGroup** — `POST /delete-billing-group`. Input: `Arn` (required). Must remove the group and its account associations from state and echo the deleted `Arn`. After the delete, ListBillingGroups must no longer return it and its accounts must list as `UNMONITORED`.

Every one of these six must compute its response from persisted state. No canned bodies, no echoing the request back, no minting an ARN that is never stored: a create followed by a list/association read must return exactly what was created, and a delete must be observable by the next caller.

## State Model

Add to `ComAmazonawsBillingconductorAwsbillingconductorState` in `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`, mutated under the lock — never a fresh engine):

- `pricing_plans: HashMap<String /* plan ARN */, PricingPlan { arn, name, description, pricing_rule_arns: Vec<String>, tags: HashMap<String,String>, creation_time_millis: i64, last_modified_time_millis: i64 }>`
- `billing_groups: HashMap<String /* group ARN */, BillingGroup { arn, name, description, primary_account_id: Option<String>, pricing_plan_arn: String, linked_account_ids: Vec<String>, auto_associate: Option<bool>, responsibility_transfer_arn: Option<String>, status: BillingGroupStatus, status_reason: Option<String>, tags: HashMap<String,String>, creation_time_millis: i64, last_modified_time_millis: i64 }` — `size` is DERIVED from `linked_account_ids.len()`, not stored independently
- `account_associations: HashMap<String /* 12-digit account id */, String /* group ARN */>` — the uniqueness index that makes the ConflictException in step 3 and the MONITORED/UNMONITORED filter in step 5 computable
- `billing_group_names: HashMap<String /* name */, String /* group ARN */>` and `idempotency_tokens: HashMap<String /* ClientToken */, String /* minted ARN */>` for the 409 / idempotent-replay behaviour
- a monotonic `id_counter` (or seeded RNG) used to mint the 10-12 alphanumeric resource ids so ARNs satisfy the modeled patterns

Each handler must delete its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line when implemented.

### Explicitly out of scope for this first slice

Custom line items (Create/Update/Delete/List/BatchAssociate…), pricing rules (Create/Update/Delete/List, Associate/DisassociatePricingRules), cost reports (GetBillingGroupCostReport, ListBillingGroupCostReports), tagging (TagResource/UntagResource/ListTagsForResource), billing transfer preferences, UpdateBillingGroup and DisassociateAccounts stay at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-billingconductor-awsbillingconductor/OPERATIONS.md` lists all 34 operations with routes, input/output members and errors (CreatePricingPlan line 197, CreateBillingGroup line 86, AssociateAccounts line 118, ListBillingGroups line 110, ListAccountAssociations line 41, DeleteBillingGroup line 102) and gives the wire protocol `restJson1` / sdk id `billingconductor`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 34 operation files (every file in `src/ops/` except `mod.rs`), so no operation has behaviour today — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. Field shapes cited from the Smithy model: `ComputationPreference.PricingPlanArn` is `@required` with pattern `^arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})$`; `AccountGrouping` has `LinkedAccountIds` (0..30, default []), `AutoAssociate`, `ResponsibilityTransferArn`; `BillingGroupListElement` carries Name/Arn/Description/PrimaryAccountId/ComputationPreference/Size/CreationTime/LastModifiedTime/Status/StatusReason/AccountGrouping; `AccountAssociationsListElement` carries AccountId/BillingGroupArn/AccountName/AccountEmail; `ListAccountAssociationsFilter.Association` documents the `MONITORED` / `UNMONITORED` / billing-group-ARN values. The one-account-to-one-billing-group rule is documented on `AccountGrouping.LinkedAccountIds` ("not associated with another billing group"). No tests exist for these operations beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `billingconductor` client at it.
2. `CreatePricingPlan --name DemoPlan --description 'demo'` → expect HTTP 200 with a stored `Arn` matching `arn:aws:billingconductor::<account>:pricingplan/<10 alnum>`; today: HTTP 501 NotImplemented.
3. `CreateBillingGroup --name DemoGroup --primary-account-id 111122223333 --account-grouping LinkedAccountIds=111122223333 --computation-preference PricingPlanArn=<plan ARN from step 2>` → expect 200 with a billing group `Arn`; today: 501.
4. Repeat the same CreateBillingGroup call with the same `X-Amzn-Client-Token` → expect the SAME `Arn` back and no second group created.
5. `CreateBillingGroup` with `ComputationPreference.PricingPlanArn` naming a plan that was never created → expect `ResourceNotFoundException` (HTTP 404).
6. `AssociateAccounts --arn <group ARN> --account-ids 444455556666 777788889999` → expect 200 echoing the group ARN; today: 501.
7. `AssociateAccounts` with an unknown group ARN → expect `ResourceNotFoundException` (404); re-associating 444455556666 to a second billing group → expect `ConflictException` (409).
8. `ListBillingGroups` → expect the group in `BillingGroups` with `Name=DemoGroup`, the step-3 `ComputationPreference.PricingPlanArn`, `Size=3`, `Status=ACTIVE`, and non-zero `CreationTime`/`LastModifiedTime`; today: 501.
9. `ListBillingGroups --max-results 1` with two groups created → expect exactly 1 element plus a `NextToken` that returns the remaining group when passed back.
10. `ListAccountAssociations --filters Association=<group ARN>` → expect `LinkedAccounts` containing 111122223333, 444455556666 and 777788889999 each with `BillingGroupArn` = the group ARN; `--filters Association=UNMONITORED` must NOT list them; today: 501.
11. `DeleteBillingGroup --arn <group ARN>` → expect 200 echoing the ARN; today: 501.
12. `ListBillingGroups` again → the deleted group must be gone, and `ListAccountAssociations --filters Association=UNMONITORED` must now list the three accounts.
