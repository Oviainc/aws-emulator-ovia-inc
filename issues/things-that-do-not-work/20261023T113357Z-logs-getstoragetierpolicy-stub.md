Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.get-storage-tier-policy-access-denied-without-permission logs.get-storage-tier-policy-no-spurious-not-found logs.get-storage-tier-policy-reflects-put-storage-tier-policy logs.put-storage-tier-policy-invalid-tier-rejected logs.put-storage-tier-policy-lifecycle logs.storage-tier-policy-lifecycle logs.storage-tier-policy-lifecycle-transitions logs.storage-tier-policy-put-get-reflects-setting logs.storage-tier-policy-put-then-get
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|GetStorageTierPolicy|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement logs GetStorageTierPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStorageTierPolicy

## Description
`GetStorageTierPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_storage_tier_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `GetStorageTierPolicy` call, so each one unblocks only when `GetStorageTierPolicy` works: `logs.get-storage-tier-policy-access-denied-without-permission`, `logs.get-storage-tier-policy-no-spurious-not-found`, `logs.get-storage-tier-policy-reflects-put-storage-tier-policy`, `logs.put-storage-tier-policy-invalid-tier-rejected`, `logs.put-storage-tier-policy-lifecycle`, `logs.storage-tier-policy-lifecycle`, `logs.storage-tier-policy-lifecycle-transitions`, `logs.storage-tier-policy-put-get-reflects-setting`, `logs.storage-tier-policy-put-then-get`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_storage_tier_policy.rs`. Replaying the corpus, the emulator refused the `GetStorageTierPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetStorageTierPolicy operation: operation GetStorageTierPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call GetStorageTierPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
