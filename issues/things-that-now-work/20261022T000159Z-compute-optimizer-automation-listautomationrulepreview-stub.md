Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer-automation.list-automation-rule-preview-structure
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation|ListAutomationRulePreview|NotImplemented
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Implement compute-optimizer-automation ListAutomationRulePreview: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAutomationRulePreview

## Description
`ListAutomationRulePreview` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/list_automation_rule_preview.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListAutomationRulePreview` call, so each one unblocks only when `ListAutomationRulePreview` works: `compute-optimizer-automation.list-automation-rule-preview-structure`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/list_automation_rule_preview.rs`. Replaying the corpus, the emulator refused the `ListAutomationRulePreview` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListAutomationRulePreview operation: operation ListAutomationRulePreview is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the compute-optimizer-automation emulator
Call ListAutomationRulePreview and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
