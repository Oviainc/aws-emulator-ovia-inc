Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: freetier.upgrade-account-plan-idempotent-when-already-paid freetier.upgrade-account-plan-invalid-plan-type-error freetier.upgrade-account-plan-invalid-plan-type-rejected
Issue-Shape-Id: com.amazonaws.freetier#AWSFreeTierService
Issue-Key: freetier.upgrade-account-plan-idempotent-when-already-paid|run#4|AssertionError|raise AssertionError(
Issue-Key: freetier.upgrade-account-plan-invalid-plan-type-error|run#6|AssertionError|assert error["Message"] == (
Issue-Key: freetier.upgrade-account-plan-invalid-plan-type-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# freetier UpgradeAccountPlan performs the FREE->PAID upgrade with no payment-instrument vetting, and its enum refusal lists the plan types in the wrong order (2 causes)

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpgradeAccountPlan

## Description
TWO distinct causes in UpgradeAccountPlan: a mutation AWS refuses that the emulator performs, and a validation message whose enum listing is in the wrong order.

Cause 1 of 2: UpgradeAccountPlan(accountPlanType="PAID") succeeds where AWS refuses it for an account with no vetted payment instrument.
AWS: Smithy: UpgradeAccountPlanRequest.accountPlanType is required and targets the enum AccountPlanType [FREE, PAID]; the operation models AccessDeniedException and ResourceNotFoundException, and ValidationException is service-common. The model says nothing about payment-instrument vetting, so this is recorded behavior only: on an account that has no vetted payment instrument, AWS refuses the FREE->PAID upgrade with ValidationException (HTTP 400) and the fixed message "Account plan upgrade failed PI vet failure" — no variable parts. The refusal is idempotent: a second identical call fails the same way, and the account's plan-state record is left exactly as it was (here, still never provisioned, so GetAccountPlanState keeps answering ResourceNotFoundException "Missing data for account: <accountId>").
Emulator: the first UpgradeAccountPlan(accountPlanType="PAID") returns success, so there is no vetting gate at all — the emulator treats the paid upgrade as unconditionally available.
Why: this is the service's only mutation and it is irreversible (there is no downgrade API). A client developed against the emulator believes the upgrade path is open and its success branch reachable; against AWS the identical call fails with a ValidationException the client never handles, and its post-upgrade logic (reading back accountPlanType=PAID) is dead code. Conversely, a client that must surface "add a payment method first" to a user has no way to exercise or test that branch here.
Fix: the plan-upgrade precondition in this operation's handler (services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/upgrade_account_plan.rs) together with the plan-state record in crate::state — a PAID upgrade must be gated on a vetted payment instrument and refused with the ValidationException above when there is none, without touching the stored plan state.
Note on scope: the required refusal is account-conditional (an account WITH a vetted payment instrument does upgrade). The emulator's accounts carry no payment instrument of any kind, so the no-instrument branch is the one it should model; it currently models neither branch's gate.
Traces: freetier.upgrade-account-plan-idempotent-when-already-paid.

Cause 2 of 2: the accountPlanType enum-value-set listing in the validation message is in the wrong order.
AWS: Smithy: AccountPlanType is an enum declaring [FREE, PAID]; an out-of-enum value is refused with ValidationException (HTTP 400). Recorded behavior: AWS renders the permitted set as "[PAID, FREE]" — the reverse of the model's declaration order — in the fixed text "1 validation error detected: Value '<value>' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [PAID, FREE]", whose only variable part is the rejected value. Hypothesis: as with this service's languageCode message, the order is AWS's validator's own set iteration order rather than the model's; it is stable across the recordings.
Emulator: emits "... Member must satisfy enum value set: [FREE, PAID]" — model-declaration order. The code (ValidationException) and the rest of the text match, and the rejected call correctly leaves the plan state untouched.
Why: anything that parses the permitted-value list out of the message, or asserts the message text, diverges from AWS.
Fix: the enum-validation message builder for this operation (services/.../src/ops/upgrade_account_plan.rs, or the shared validation helper it calls) must render AWS's order for AccountPlanType.
Traces: freetier.upgrade-account-plan-invalid-plan-type-error, freetier.upgrade-account-plan-invalid-plan-type-rejected.

Both checks behind cause 2 pass their effect-absence legs (the plan snapshot is identical before and after the rejected call), so cause 2 is a message-text defect only. Cause 1 is a missing refusal and the reason this issue is HIGH. Related but distinct: the fixed issue 20261024T000352Z-freetier-upgradeaccountplan-wrong-behavior.md covered UpgradeAccountPlan(FREE) reactivating an expired free plan; neither cause here is that one, and the FREE path is not what these checks exercise.

## Evidence
Cause 1 — UpgradeAccountPlan(accountPlanType="PAID"), twice in a row, on an account whose GetAccountPlanState answers ResourceNotFoundException "Missing data for account: <accountId>":
  expected (both attempts): ClientError Code=ValidationException, Message="Account plan upgrade failed PI vet failure", and the plan-state snapshot unchanged across both calls.
  actual: the FIRST call returned success. The check's own guard fired: "first UpgradeAccountPlan(PAID) unexpectedly succeeded; expected a payment-instrument-vetting refusal on this account". The positive control (GetAccountPlanState answering either a FREE/PAID state or ResourceNotFoundException with AWS's exact "Missing data for account: <accountId>" text) passed on the emulator, so the service was reachable and correctly worded on that path — only the upgrade gate is absent.

Cause 2 — UpgradeAccountPlan(accountPlanType="GOLD"), Code=ValidationException on both sides:
  expected Message: "1 validation error detected: Value 'GOLD' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [PAID, FREE]"
  actual   Message: "1 validation error detected: Value 'GOLD' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [FREE, PAID]"
  Only the bracketed set's order differs; 'GOLD' is the request value filling the template. Reproduced identically by two independent checks. The GetAccountPlanState snapshot taken before and after is unchanged, so the rejection has no side effect.

## Steps to Reproduce
1. Boot the emulator with the freetier service mounted and build a botocore freetier client against it.
2. Cause 2 (non-destructive, do this first): call UpgradeAccountPlan(accountPlanType="GOLD") and compare Error.Message's bracketed enum set with AWS's "[PAID, FREE]". Take a GetAccountPlanState snapshot either side to confirm the rejection is still side-effect free.
3. Cause 1: on an account with no vetted payment instrument, call UpgradeAccountPlan(accountPlanType="PAID"). Observe that it SUCCEEDS; AWS refuses with ValidationException "Account plan upgrade failed PI vet failure".
4. Call UpgradeAccountPlan(accountPlanType="PAID") a second time and confirm the refusal is identical and idempotent once the gate exists.
5. Call GetAccountPlanState before and after and confirm the refused upgrade leaves the plan-state record exactly as it was (for a never-provisioned account, ResourceNotFoundException "Missing data for account: <accountId>").
6. Re-replay the checks in trace_ids to confirm both causes are closed.
