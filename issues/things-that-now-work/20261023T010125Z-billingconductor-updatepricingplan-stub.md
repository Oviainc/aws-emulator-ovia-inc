Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billingconductor.update-pricing-plan-and-list-pricing-plans-require-payer-account
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor.update-pricing-plan-and-list-pricing-plans-require-payer-account|run#8|AssertionError|assert code_a == "AccessDeniedException", f"UpdatePricingPlan: unexpected code: {code_a} ({msg_a})"
Issue-Commit: ee002e577bc0f1571995f44867241dcee6b1ff11

# Implement billingconductor UpdatePricingPlan: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ee002e577bc0f1571995f44867241dcee6b1ff11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePricingPlan

## Description
`UpdatePricingPlan` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/update_pricing_plan.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdatePricingPlan` call, so each one unblocks only when `UpdatePricingPlan` works: `billingconductor.update-pricing-plan-and-list-pricing-plans-require-payer-account`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/update_pricing_plan.rs`. Replaying the corpus, the emulator refused the `UpdatePricingPlan` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_billingconductor_update_pricing_plan_and_list_pricing_plans_require_payer_account.py", line 45, in run
    assert code_a == "AccessDeniedException", f"UpdatePricingPlan: unexpected code: {code_a} ({msg_a})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: UpdatePricingPlan: unexpected code: NotImplemented (operation UpdatePricingPlan is scaffolded but not implemented in this emulator)

## Steps to Reproduce
Boot the billingconductor emulator
Call UpdatePricingPlan and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
