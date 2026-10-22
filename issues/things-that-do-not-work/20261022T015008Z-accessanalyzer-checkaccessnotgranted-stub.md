Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.check-access-not-granted-fail-when-access-present accessanalyzer.check-access-not-granted-pass-when-access-absent
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer|CheckAccessNotGranted|NotImplemented
Issue-Commit: c6f9974c04d0d99d1dbb9b00d2d222f6d449381b

# Implement accessanalyzer CheckAccessNotGranted: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c6f9974c04d0d99d1dbb9b00d2d222f6d449381b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckAccessNotGranted

## Description
`CheckAccessNotGranted` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/check_access_not_granted.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CheckAccessNotGranted` call, so each one unblocks only when `CheckAccessNotGranted` works: `accessanalyzer.check-access-not-granted-fail-when-access-present`, `accessanalyzer.check-access-not-granted-pass-when-access-absent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/check_access_not_granted.rs`. Replaying the corpus, the emulator refused the `CheckAccessNotGranted` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CheckAccessNotGranted operation: operation CheckAccessNotGranted is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the accessanalyzer emulator
Call CheckAccessNotGranted and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
