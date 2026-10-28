Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.snapshot-copy-ops-not-found-semantics
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DisableSnapshotCopy|NotImplemented
Issue-Commit: 723f6765f7182e095db8b4d143563d5119d6f14e

# Implement redshift DisableSnapshotCopy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `723f6765f7182e095db8b4d143563d5119d6f14e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableSnapshotCopy

## Description
`DisableSnapshotCopy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/disable_snapshot_copy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DisableSnapshotCopy` call, so each one unblocks only when `DisableSnapshotCopy` works: `redshift.snapshot-copy-ops-not-found-semantics`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/disable_snapshot_copy.rs`. Replaying the corpus, the emulator refused the `DisableSnapshotCopy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DisableSnapshotCopy operation: operation DisableSnapshotCopy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the redshift emulator
Call DisableSnapshotCopy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
