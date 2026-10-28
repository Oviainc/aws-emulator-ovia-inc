Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.missing-required-parameter-family
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.missing-required-parameter-family|run#3|AssertionError|assert error == ("InvalidParameterValue", "EMPTY_RULE_NAMES: RuleNames cannot be an empty array."), f"DeleteInsightRules: RuleNames=[] gave {error!r}"
Issue-Commit: 58bb21045c140f54eac821540018fc7201e16d37

# Implement cloudwatch DeleteInsightRules: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `58bb21045c140f54eac821540018fc7201e16d37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInsightRules

## Description
`DeleteInsightRules` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/delete_insight_rules.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteInsightRules` call, so each one unblocks only when `DeleteInsightRules` works: `cloudwatch.missing-required-parameter-family`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/delete_insight_rules.rs`. Replaying the corpus, the emulator refused the `DeleteInsightRules` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudwatch_missing_required_parameter_family.py", line 36, in run
    assert error == ("InvalidParameterValue", "EMPTY_RULE_NAMES: RuleNames cannot be an empty array."), f"DeleteInsightRules: RuleNames=[] gave {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: DeleteInsightRules: RuleNames=[] gave ('NotImplemented', 'operation DeleteInsightRules is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cloudwatch emulator
Call DeleteInsightRules and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
