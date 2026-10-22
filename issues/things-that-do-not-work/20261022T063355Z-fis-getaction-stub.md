Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.e2e-experiment-create-start-observe-stop-teardown fis.fis-injected-api-internal-error-fails-a-role-scoped-ec2-call-and-spares-a-control-caller
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|GetAction|NotImplemented
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Implement fis GetAction: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAction

## Description
`GetAction` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/get_action.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetAction` call, so each one unblocks only when `GetAction` works: `fis.e2e-experiment-create-start-observe-stop-teardown`, `fis.fis-injected-api-internal-error-fails-a-role-scoped-ec2-call-and-spares-a-control-caller`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/get_action.rs`. Replaying the corpus, the emulator refused the `GetAction` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAction operation: operation GetAction is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the fis emulator
Call GetAction and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
