Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__kms.archive-kms-key-identifier-absent-then-granted-on-update
Issue-Key: events__kms.archive-kms-key-identifier-absent-then-granted-on-update|_error#15|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# events__kms: UpdateArchive accepts a KmsKeyIdentifier whose key policy does not let EventBridge call GenerateDataKey, instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateArchive

## Description
Cause 1 of 1: EventBridge never attempts the KMS call when an archive's KmsKeyIdentifier is set, so a key that events.amazonaws.com may not use is accepted and the archive is silently bound to an unusable key.
AWS: recorded behavior only — the Smithy model (com.amazonaws.eventbridge UpdateArchive) declares KmsKeyIdentifier but no KMS-specific error for it. The recording shows AWS probing the key as events.amazonaws.com and refusing the update with HTTP 400, Error.Code 'ValidationException' and the message "KMS returned an access denied exception when calling the GenerateDataKey API for the key with ARN <key ARN>. Please ensure that events.amazonaws.com has permissions to access GenerateDataKey." — fixed text apart from <key ARN>, taken from the request. The refused update leaves the archive as it was (DescribeArchive still reports no KmsKeyIdentifier), while the same call naming a key whose policy grants events.amazonaws.com kms:GenerateDataKey/Decrypt/DescribeKey succeeds and DescribeArchive then reports that key.
Emulator: the update naming the ungranted key returns HTTP 200 with {'ArchiveArn': ..., 'State': 'ENABLED', 'CreationTime': ...} — no validation, no refusal.
Why: a client that mis-scopes a key policy gets no feedback at update time; the archive reports ENABLED while its events can never be encrypted, and a test asserting the ValidationException guard passes vacuously.
Fix: the UpdateArchive (and CreateArchive) handler's KmsKeyIdentifier validation — resolve the key and authorize GenerateDataKey as the events.amazonaws.com service principal before mutating the archive, and surface the refusal as the 400 ValidationException above.
Traces: events__kms.archive-kms-key-identifier-absent-then-granted-on-update.

## Evidence
Expected ('ValidationException', 'KMS returned an access denied exception when calling the GenerateDataKey API for the key with ARN <key ARN>. Please ensure that events.amazonaws.com has permissions to access GenerateDataKey.'); actual AssertionError "expected the call to be refused, but it succeeded: {'ArchiveArn': 'arn:aws:events:<region>:<account>:archive/<name>', 'State': 'ENABLED', 'CreationTime': ..., 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".

## Steps to Reproduce
1. CreateEventBus, then CreateArchive with EventSourceArn = that bus and no KmsKeyIdentifier; DescribeArchive must report no KmsKeyIdentifier.
2. Create a KMS key whose policy grants only the account root (no statement for Service events.amazonaws.com).
3. UpdateArchive(ArchiveName=<archive>, KmsKeyIdentifier=<that key's ARN>) — expect 400 ValidationException with the message above and the archive left keyless; the emulator answers 200.
4. Create a second key that grants events.amazonaws.com kms:DescribeKey/GenerateDataKey/Decrypt/ReEncrypt*, UpdateArchive with it, and DescribeArchive must report that KmsKeyIdentifier.
