Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.copy-snapshot-cross-account-without-permission-blocked ec2.copy-snapshot-malformed-presigned-url-rejected ec2.image-register-describe-deregister-lifecycle ec2.op-contract-cancel-image-launch-permission ec2.op-contract-create-snapshot ec2.op-contract-create-volume ec2.op-contract-delete-snapshot-in-use-rejected ec2.op-contract-deregister-image ec2.op-contract-describeFastSnapshotRestores ec2.op-contract-describeSnapshotAttribute ec2.op-contract-describeSnapshotTierStatus ec2.op-contract-describeSnapshots ec2.op-contract-detach-image-watermark ec2.op-contract-disable-fast-snapshot-restores ec2.op-contract-disable-image-deprecation ec2.op-contract-disable-image-deregistration-protection ec2.op-contract-enableFastLaunch ec2.op-contract-enableFastSnapshotRestores ec2.op-contract-enableImage ec2.op-contract-enableImageDeprecation ec2.op-contract-enableImageDeregistrationProtection ec2.op-contract-export-image ec2.op-contract-list-images-in-recycle-bin ec2.op-contract-list-snapshots-in-recycle-bin ec2.op-contract-lock-snapshot-blocks-deletion ec2.op-contract-modify-snapshot-attribute ec2.op-contract-modify-snapshot-tier ec2.op-contract-register-image ec2.op-contract-replace-image-instance-type-specification ec2.op-contract-reset-image-attribute ec2.op-contract-reset-snapshot-attribute ec2.op-contract-restore-snapshot-tier-not-archived-rejected ec2.op-contract-unlock-snapshot ec2.snapshot-create-from-volume-and-describe ec2.snapshot-in-use-by-registered-image-delete-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateSnapshot|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 CreateSnapshot: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshot

## Description
`CreateSnapshot` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_snapshot.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

35 recorded unit(s) fail on their `CreateSnapshot` call, so each one unblocks only when `CreateSnapshot` works: `ec2.copy-snapshot-cross-account-without-permission-blocked`, `ec2.copy-snapshot-malformed-presigned-url-rejected`, `ec2.image-register-describe-deregister-lifecycle`, `ec2.op-contract-cancel-image-launch-permission`, `ec2.op-contract-create-snapshot`, `ec2.op-contract-create-volume`, `ec2.op-contract-delete-snapshot-in-use-rejected`, `ec2.op-contract-deregister-image`, `ec2.op-contract-describeFastSnapshotRestores`, `ec2.op-contract-describeSnapshotAttribute`, `ec2.op-contract-describeSnapshotTierStatus`, `ec2.op-contract-describeSnapshots`, `ec2.op-contract-detach-image-watermark`, `ec2.op-contract-disable-fast-snapshot-restores`, `ec2.op-contract-disable-image-deprecation`, `ec2.op-contract-disable-image-deregistration-protection`, `ec2.op-contract-enableFastLaunch`, `ec2.op-contract-enableFastSnapshotRestores`, `ec2.op-contract-enableImage`, `ec2.op-contract-enableImageDeprecation` (+15 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_snapshot.rs`. Replaying the corpus, the emulator refused the `CreateSnapshot` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSnapshot operation: operation CreateSnapshot is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateSnapshot and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
