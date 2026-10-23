Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-region-switch.decline-approval-step-cancels-execution arc-region-switch.get-plan-in-region-reflects-plan arc-region-switch.update-plan-execution-resume-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch|GetPlanInRegion|NotImplemented
Issue-Commit: 1d15d4851343e3a31be69cd8cea36918aab666fe

# Implement arc-region-switch GetPlanInRegion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1d15d4851343e3a31be69cd8cea36918aab666fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPlanInRegion

## Description
`GetPlanInRegion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/get_plan_in_region.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `GetPlanInRegion` call, so each one unblocks only when `GetPlanInRegion` works: `arc-region-switch.decline-approval-step-cancels-execution`, `arc-region-switch.get-plan-in-region-reflects-plan`, `arc-region-switch.update-plan-execution-resume-wrong-state-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/get_plan_in_region.rs`. Replaying the corpus, the emulator refused the `GetPlanInRegion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetPlanInRegion operation: operation GetPlanInRegion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the arc-region-switch emulator
Call GetPlanInRegion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
