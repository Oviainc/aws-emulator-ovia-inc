Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.op-contract-set-instance-health autoscaling.unhealthy-instance-replaced
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: auto-scaling|SetInstanceHealth|NotImplemented
Issue-Commit: 38b869ac761e6c1ceaf9cdd20433d5027a2449ee

# Implement autoscaling SetInstanceHealth: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `38b869ac761e6c1ceaf9cdd20433d5027a2449ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetInstanceHealth

## Description
`SetInstanceHealth` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/set_instance_health.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `SetInstanceHealth` call, so each one unblocks only when `SetInstanceHealth` works: `autoscaling.op-contract-set-instance-health`, `autoscaling.unhealthy-instance-replaced`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/set_instance_health.rs`. Replaying the corpus, the emulator refused the `SetInstanceHealth` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SetInstanceHealth operation: operation SetInstanceHealth is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the autoscaling emulator
Call SetInstanceHealth and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
