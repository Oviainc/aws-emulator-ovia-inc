Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.delete-active-receipt-rule-set-fails
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.delete-active-receipt-rule-set-fails|run#8|AssertionError|assert err == "CannotDelete", (
Issue-Commit: 4bb3c01901698ef95426c2799d1398daad63aada

# Implement ses DeleteReceiptRuleSet: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4bb3c01901698ef95426c2799d1398daad63aada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteReceiptRuleSet

## Description
`DeleteReceiptRuleSet` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/delete_receipt_rule_set.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteReceiptRuleSet` call, so each one unblocks only when `DeleteReceiptRuleSet` works: `ses.delete-active-receipt-rule-set-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/delete_receipt_rule_set.rs`. Replaying the corpus, the emulator refused the `DeleteReceiptRuleSet` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ses_delete_active_receipt_rule_set_fails.py", line 71, in run
    assert err == "CannotDelete", (
           ^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected CannotDelete deleting the active rule set, got 'NotImplemented': 'operation DeleteReceiptRuleSet is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ses emulator
Call DeleteReceiptRuleSet and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
