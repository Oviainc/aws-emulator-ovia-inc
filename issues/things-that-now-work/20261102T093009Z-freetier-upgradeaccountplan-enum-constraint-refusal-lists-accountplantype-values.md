Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: freetier.upgrade-account-plan-invalid-type-rejected
Issue-Shape-Id: com.amazonaws.freetier#AWSFreeTierService
Issue-Key: freetier.upgrade-account-plan-invalid-type-rejected|run#5|AssertionError|assert message == (
Issue-Commit: ebea418ed7238d7557ed4699ab6be023240b4505

# freetier UpgradeAccountPlan enum-constraint refusal lists accountPlanType values in model order [FREE, PAID] instead of AWS's [PAID, FREE]

> _Note: this issue was identified against the code base at commit `ebea418ed7238d7557ed4699ab6be023240b4505`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpgradeAccountPlan

## Description
ONE net-new root cause, grounded in the surviving failing check freetier.upgrade-account-plan-invalid-type-rejected.

Cause 1 of 1: the accountPlanType enum-value-set listing inside UpgradeAccountPlan's ValidationException message is rendered in the Smithy declaration order rather than the order AWS renders.

AWS: Smithy (com.amazonaws.freetier): UpgradeAccountPlanRequest.accountPlanType is @required and targets the enum AccountPlanType, whose declared values are [FREE, PAID]; ValidationException is service-common for this operation. A value outside the enum is refused before any mutation with HTTP 400, Code=ValidationException. Recorded behavior: AWS words the message with the fixed text "1 validation error detected: Value '<value>' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [PAID, FREE]". The only variable part of that template is <value>, the rejected input the client sent (in this check, 'PREMIUM'); the member path 'accountPlanType' and the bracketed set "[PAID, FREE]" are fixed text. Hypothesis: the bracketed order is AWS's validator's own set iteration order, not the model's declaration order; it is stable across the recordings for this service, exactly as it is for this service's languageCode enum.

Emulator: returns the correct status and the correct Code=ValidationException, and correctly leaves the plan state untouched (the check's before/after GetAccountPlanState legs both passed), but emits the set in model-declaration order: "1 validation error detected: Value 'PREMIUM' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [FREE, PAID]". Only the bracketed set's ordering diverges.

Why: a client that asserts on, logs, or parses the permitted-value list out of this message (a common way to surface "valid plan types are ..." to a user, and the way contract tests pin error wording) gets a different string from the emulator than from AWS. The error message is a requirement of the error class, so the emulator's refusal is not byte-faithful even though its code and effect-absence are correct.

Fix: the enum-validation message builder used by this operation's input validation — services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/upgrade_account_plan.rs, or the shared validation/enum-rendering helper it calls — must render AWS's order for AccountPlanType rather than iterating the model's declared order. Note the check sends 'PREMIUM' while the already-open issue's recordings sent 'GOLD': the rejected value is a template slot, so the fix must not special-case either literal.

Related open issue, which does NOT cover this check: 20261029T183000Z-freetier-upgradeaccountplan-performs-the-free-paid-upgrade-with.md describes the same enum-ordering behavior as its Cause 2, but its Issue-Trace-Ids / Issue-Key list only freetier.upgrade-account-plan-invalid-plan-type-error, freetier.upgrade-account-plan-invalid-plan-type-rejected and freetier.upgrade-account-plan-idempotent-when-already-paid; the check filed here (freetier.upgrade-account-plan-invalid-type-rejected) carries no replay key in that issue, so it would be left unreplayed. Fixing the shared ordering should close both.

Traces: freetier.upgrade-account-plan-invalid-type-rejected.

## Evidence
Check freetier.upgrade-account-plan-invalid-type-rejected (group overly-permissive), call UpgradeAccountPlan(accountPlanType="PREMIUM"):

  Code:     ValidationException on both sides — matches.
  expected Message: "1 validation error detected: Value 'PREMIUM' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [PAID, FREE]"
  actual   Message: "1 validation error detected: Value 'PREMIUM' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [FREE, PAID]"

Failure site: check_freetier_upgrade_account_plan_invalid_type_rejected.py line 26, `assert message == (...)`, AssertionError reporting the actual message above. The assertion on Code (line 21) passed, and the check's GetAccountPlanState snapshot taken before and after the rejected call compared equal, so the refusal has no side effect — the divergence is confined to the bracketed enum set's ordering in the message text.

Fixed text vs values: "1 validation error detected: Value '" / "' at 'accountPlanType' failed to satisfy constraint: Member must satisfy enum value set: [PAID, FREE]" is fixed; 'PREMIUM' is the value taken from the request.

## Steps to Reproduce
1. Boot the emulator with the freetier service mounted and build a botocore freetier client against it.
2. Call GetAccountPlanState and record the answer (a (accountPlanType, accountPlanStatus) pair, or ResourceNotFoundException for an account with no Free Tier plan).
3. Call UpgradeAccountPlan(accountPlanType="PREMIUM") and catch the ClientError.
4. Assert Error.Code == "ValidationException" (passes today) and compare Error.Message with AWS's text, whose bracketed set is "[PAID, FREE]". Observe the emulator emits "[FREE, PAID]".
5. Call GetAccountPlanState again and confirm it is unchanged (passes today) — the rejection is correctly side-effect free.
6. Re-replay freetier.upgrade-account-plan-invalid-type-rejected to confirm the fix, and re-replay freetier.upgrade-account-plan-invalid-plan-type-error / -rejected from the related open issue, which share this message builder.
