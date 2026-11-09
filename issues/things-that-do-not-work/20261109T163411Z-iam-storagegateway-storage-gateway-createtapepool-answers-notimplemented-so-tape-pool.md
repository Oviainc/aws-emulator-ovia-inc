Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__storagegateway.least-privilege-delete-tape-pool-denied-then-allowed
Issue-Key: iam__storagegateway.least-privilege-delete-tape-pool-denied-then-allowed|run#17|AssertionError|assert last_code == "SUCCESS", (
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# Storage Gateway CreateTapePool answers NotImplemented, so tape-pool least-privilege checks can never converge

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTapePool

## Description
Cause 1 of 1: the CreateTapePool handler is an unimplemented scaffold stub, so even a correctly granted principal cannot create a tape pool.

AWS: CreateTapePool creates a custom tape pool and answers 200 with PoolARN; a principal granted storagegateway:CreateTapePool succeeds, and only a principal without the grant is refused with AccessDeniedException. Smithy: storagegateway#CreateTapePool takes PoolName, StorageClass, RetentionLockType/RetentionLockTimeInDays and Tags, returns CreateTapePoolOutput{PoolARN} and declares InvalidGatewayRequestException and InternalServerError. `NotImplemented` is not among its modelled errors.

Emulator: CreateTapePool answers the non-AWS error code `NotImplemented`, so the granted principal's call never converges to SUCCESS (the check's last observed code is 'NotImplemented').

Why: a client cannot create a tape pool at all, and the failure is indistinguishable from a permission problem: the error code is outside the operation's modelled error set, so SDK error handling cannot classify it and a least-privilege test cannot tell an ungranted call from an unimplemented one. The paired DeleteTapePool permission check is unreachable.

Fix: implement the CreateTapePool handler and its tape-pool state (and the matching DeleteTapePool path) in the Storage Gateway service.

Traces: iam__storagegateway.least-privilege-delete-tape-pool-denied-then-allowed.

## Evidence
Expected: CreateTapePool returns 200 with PoolARN for the granted principal (code SUCCESS).
Actual: AssertionError: the granted CreateTapePool never converged for the restricted principal; last code 'NotImplemented'

## Steps to Reproduce
1. Create a role granted storagegateway:CreateTapePool (and DeleteTapePool); AssumeRole.
2. Call CreateTapePool with PoolName and StorageClass.
3. Observe the NotImplemented error instead of a PoolARN.
