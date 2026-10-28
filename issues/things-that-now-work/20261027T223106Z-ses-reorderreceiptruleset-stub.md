Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.reorder-receipt-rule-set-duplicate-and-unknown-rule-rejected ses.reorder-receipt-rule-set-missing-rule-fails
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.reorder-receipt-rule-set-duplicate-and-unknown-rule-rejected|run#13|AssertionError|assert dup_err == expected_dup, (
Issue-Key: ses.reorder-receipt-rule-set-missing-rule-fails|run#10|AssertionError|assert err == ("InvalidParameterValue", f"Positions for rules not found: {rule2}"), (
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# Implement ses ReorderReceiptRuleSet: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReorderReceiptRuleSet

## Description
`ReorderReceiptRuleSet` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/reorder_receipt_rule_set.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ReorderReceiptRuleSet` call, so each one unblocks only when `ReorderReceiptRuleSet` works: `ses.reorder-receipt-rule-set-duplicate-and-unknown-rule-rejected`, `ses.reorder-receipt-rule-set-missing-rule-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/reorder_receipt_rule_set.rs`. Replaying the corpus, the emulator refused the `ReorderReceiptRuleSet` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ses_reorder_receipt_rule_set_duplicate_and_unknown_rule_rejected.py", line 60, in run
    assert dup_err == expected_dup, (
           ^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ('InvalidParameterValue', 'Multiple positions found for rule: rich') for a reorder list naming 'rich' twice, got ('NotImplemented', 'operation ReorderReceiptRuleSet is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ses emulator
Call ReorderReceiptRuleSet and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
