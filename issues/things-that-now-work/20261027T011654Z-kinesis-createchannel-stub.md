Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.channel-lifecycle-create-active-delete kinesis.concurrent-channel-update-serializes-cleanly kinesis.concurrent-update-channel-lost-update-detected kinesis.create-channel-duplicate-name-rejected kinesis.create-channel-invalid-kms-key-denied kinesis.create-channel-kms-access-denied kinesis.create-channel-lifecycle kinesis.create-channel-then-describe-shows-active kinesis.create-channel-without-kms-permission-must-be-denied kinesis.delete-channel-in-use-rejected kinesis.delete-channel-limit-exceeded-fails kinesis.delete-channel-nonexistent-fails kinesis.delete-channel-then-describe-fails kinesis.delete-channel-with-dependents-blocked kinesis.describe-channel-nonexistent-fails kinesis.list-channels-contains-created-channel kinesis.list-channels-reflects-created-channel kinesis.update-channel-concurrent-modification-conflict kinesis.update-channel-persists-configuration-change kinesis.update-channel-properties-persist kinesis.update-channel-reflects-in-describe
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|CreateChannel|NotImplemented
Issue-Key: kinesis.concurrent-update-channel-lost-update-detected|_setup_channel#1|AssertionError|assert channel_arn is not None, f"CreateChannel never succeeded: {code}"
Issue-Key: kinesis.create-channel-invalid-kms-key-denied|run#33|AssertionError|assert code is not None and code.startswith("ValidationException") and "KMS" in code, code
Issue-Commit: d5c75e29b1e90f1499e967e91f7af40c6cd43d58

# Implement kinesis CreateChannel: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d5c75e29b1e90f1499e967e91f7af40c6cd43d58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
`CreateChannel` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/create_channel.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

21 recorded unit(s) fail on their `CreateChannel` call, so each one unblocks only when `CreateChannel` works: `kinesis.channel-lifecycle-create-active-delete`, `kinesis.concurrent-channel-update-serializes-cleanly`, `kinesis.concurrent-update-channel-lost-update-detected`, `kinesis.create-channel-duplicate-name-rejected`, `kinesis.create-channel-invalid-kms-key-denied`, `kinesis.create-channel-kms-access-denied`, `kinesis.create-channel-lifecycle`, `kinesis.create-channel-then-describe-shows-active`, `kinesis.create-channel-without-kms-permission-must-be-denied`, `kinesis.delete-channel-in-use-rejected`, `kinesis.delete-channel-limit-exceeded-fails`, `kinesis.delete-channel-nonexistent-fails`, `kinesis.delete-channel-then-describe-fails`, `kinesis.delete-channel-with-dependents-blocked`, `kinesis.describe-channel-nonexistent-fails`, `kinesis.list-channels-contains-created-channel`, `kinesis.list-channels-reflects-created-channel`, `kinesis.update-channel-concurrent-modification-conflict`, `kinesis.update-channel-persists-configuration-change`, `kinesis.update-channel-properties-persist` (+1 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/create_channel.rs`. Replaying the corpus, the emulator refused the `CreateChannel` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateChannel operation: operation CreateChannel is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call CreateChannel and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
