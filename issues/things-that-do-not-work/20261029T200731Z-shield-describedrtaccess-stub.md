Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: shield.describe-drt-access-absent-without-association
Issue-Shape-Id: com.amazonaws.shield#AWSShield_20160616
Issue-Key: shield.describe-drt-access-absent-without-association|run#3|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r} msg={message!r}"
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Implement shield DescribeDRTAccess: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDRTAccess

## Description
`DescribeDRTAccess` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_drt_access.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeDRTAccess` call, so each one unblocks only when `DescribeDRTAccess` works: `shield.describe-drt-access-absent-without-association`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_drt_access.rs`. Replaying the corpus, the emulator refused the `DescribeDRTAccess` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_shield_describe_drt_access_absent_without_association.py", line 21, in run
    assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r} msg={message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException, got 'NotImplemented' msg='operation DescribeDRTAccess is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the shield emulator
Call DescribeDRTAccess and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
