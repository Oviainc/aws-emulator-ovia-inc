Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.test-authorization-policy-evaluation
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|TestAuthorization|NotImplemented
Issue-Commit: cf75889d0041250f9d4c4a6a69f11963f546e0f7

# Implement iot TestAuthorization: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cf75889d0041250f9d4c4a6a69f11963f546e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestAuthorization

## Description
`TestAuthorization` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/test_authorization.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `TestAuthorization` call, so each one unblocks only when `TestAuthorization` works: `iot.test-authorization-policy-evaluation`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/test_authorization.rs`. Replaying the corpus, the emulator refused the `TestAuthorization` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestAuthorization operation: operation TestAuthorization is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot emulator
Call TestAuthorization and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
