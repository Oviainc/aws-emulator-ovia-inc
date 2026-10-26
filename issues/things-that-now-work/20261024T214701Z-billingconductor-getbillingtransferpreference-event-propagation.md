Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# Billing transfers are never sourced from Organizations: any well-formed transfer ARN under the payer account answers 200 with the default preference instead of 404

_Filed: 2026-10-24T21:47:01Z_
_Operation: GetBillingTransferPreference | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A ResponsibilityTransferArn names an AWS Organizations resource (the modelled pattern is arn:<partition>:organizations::<12-digit management account>:transfer/o-<org id>/billing/(inbound|outbound)/rt-<id>) that this API never creates. GetBillingTransferPreference declares ResourceNotFoundException (HTTP 404) precisely for a transfer that does not exist.

Expected: a ResponsibilityTransferArn that names no billing transfer of the organization — a transfer id nobody ever created, a different organization id, or the wrong direction — is answered HTTP 404 ResourceNotFoundException. Only a transfer the organization actually has is answered 200, with the documented default (Enabled false, no PricingPlanArn, empty LastModifiedTime) when no preference was ever set on it.

Actual: the existence decision is crate::state::transfer_belongs_to_this_organization, which compares ONLY the account segment of the ARN with state.payer_account_id. Every other segment — the organization id (o-…), the direction (inbound/outbound) and the transfer id (rt-…) — is ignored, so any pattern-conforming ARN carrying the payer's account id is reported as an existing transfer: HTTP 200 with Enabled=false and no LastModifiedTime. A client cannot tell a real transfer with no preference from an ARN it invented. The same predicate gates UpdateBillingTransferPreference, so a preference can also be SET on a transfer that exists nowhere, and that invented transfer then blocks DeletePricingPlan with PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT — a 409 on a plan held back by a resource that does not exist. The missing piece is the propagation: the set of billing transfers (and their organization and direction) has to come from the Organizations state the emulator already holds, the same way the payer designation must; a store that knows no transfers must answer 404 rather than invent one.

## Evidence

Spec: GetBillingTransferPreference and UpdateBillingTransferPreference both declare ResourceNotFoundException (HTTP 404) "The request references a resource that doesn't exist" (OPERATIONS.md, and smithy_query query="operation"), and ResponsibilityTransferArn's pattern spells the transfer as an AWS Organizations resource with an organization id, a billing direction and a transfer id — identity the service must resolve, not just an account segment. GetBillingTransferPreferenceOutput.LastModifiedTime is documented "empty if the preference has never been set for the billing transfer", which presupposes a transfer that exists and has no preference — a case distinct from a transfer that does not exist.

Handler/state: src/ops/get_billing_transfer_preference.rs lines ~177-203 — the only not-found path is `if !state.transfer_belongs_to_this_organization(&transfer_arn)`, then the answer is built from `state.billing_transfer_preference(&transfer_arn)` (None => documented default). src/state.rs transfer_belongs_to_this_organization (lines ~934-942) matches `payer_account_id` against `responsibility_transfer_owner_account(arn)`, which is just `arn.split(':').nth(4)` (lines ~694-696) — nothing reads the o-… / rt-… / direction segments, and no store of transfers exists (billing_transfer_preferences is keyed by the caller-supplied ARN). src/ops/update_billing_transfer_preference.rs lines ~193-202 uses the same predicate, and src/ops/delete_pricing_plan.rs lines ~231-246 turns such a preference into a 409 ConflictException.

Tests: tests/all/billing_transfer_preference.rs exercises the get/update round trip with ARNs the test composes from the payer account id and asserts the 404 only for ARNs carrying a DIFFERENT account — no test covers an unknown transfer id of the payer's own organization, so nothing pins the 200. The existing open Organizations-propagation issue (…-createbillinggroup-event-propagation.md) is about payer_account_id alone and says nothing about the transfer resource universe.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. POST /get-billing-transfer-preference with {"ResponsibilityTransferArn":"arn:aws:organizations::<emulated account id>:transfer/o-zzzzzzzzzz/billing/inbound/rt-zzzzzzzzzzzzzzzz"} — a transfer id and organization nobody ever created.
3. Observe HTTP 200 with AutoBillingTransferBillingGroupCreation.Enabled=false and no LastModifiedTime, where the real service answers 404 ResourceNotFoundException for a transfer that does not exist.
4. POST /create-pricing-plan with {"Name":"Plan1"} and keep the Arn.
5. PUT /update-billing-transfer-preference with {"ResponsibilityTransferArn":"<the same invented ARN>","AutoBillingTransferBillingGroupCreation":{"Enabled":true,"PricingPlanArn":"<plan arn>"}} and observe HTTP 200 — a preference stored against a transfer that exists nowhere.
6. POST /delete-pricing-plan with {"Arn":"<plan arn>"} and observe 409 ConflictException / PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT, i.e. the plan is held back by the invented transfer.
