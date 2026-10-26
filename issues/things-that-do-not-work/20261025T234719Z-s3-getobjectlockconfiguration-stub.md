Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObjectLockConfiguration|NotImplemented
Issue-Commit: d9d92e1bb4d26644359d4701433c452d79a4d930

# Implement s3 GetObjectLockConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d9d92e1bb4d26644359d4701433c452d79a4d930`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObjectLockConfiguration

## Description
`GetObjectLockConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/get_object_lock_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetObjectLockConfiguration` call, so each one unblocks only when `GetObjectLockConfiguration` works: `s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/get_object_lock_configuration.rs`. Replaying the corpus, the emulator refused the `GetObjectLockConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetObjectLockConfiguration operation: operation GetObjectLockConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3 emulator
Call GetObjectLockConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Update
`GetObjectLockConfiguration` is implemented, and its `//! SCAFFOLDED, NOT IMPLEMENTED:`
header line deleted, by the fix for
`20261025T032458Z-s3-putobjectlockconfiguration-stub.md`: the write half needed a reader to
prove its round trip, so the read half was implemented in that same change. It reports the
bucket's stored `ObjectLockEnabled` and the `Rule`'s `DefaultRetention`, answers 404
`NoSuchBucket` for a bucket the emulator does not hold and 404
`ObjectLockConfigurationNotFoundError` for one carrying no configuration, and is covered by
`services/service-com-amazonaws-s3-amazons3/tests/all/bucket_object_lock_configuration.rs`.
Whoever picks this issue up should check the unit it names
(`s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can`), which also depends
on the Object Lock ENFORCEMENT causes tracked in
`20261025T234719Z-s3-object-lock-is-not-enforced-retained-and.md`, rather than re-implementing
the handler.
