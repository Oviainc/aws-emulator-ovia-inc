Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.storage-lens-group-update-and-arn-tagging-roundtrip
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3-control|UpdateStorageLensGroup|NotImplemented
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# Implement s3control UpdateStorageLensGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStorageLensGroup

## Description
`UpdateStorageLensGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/update_storage_lens_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateStorageLensGroup` call, so each one unblocks only when `UpdateStorageLensGroup` works: `s3control.storage-lens-group-update-and-arn-tagging-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/update_storage_lens_group.rs`. Replaying the corpus, the emulator refused the `UpdateStorageLensGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateStorageLensGroup operation: operation UpdateStorageLensGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3control emulator
Call UpdateStorageLensGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
