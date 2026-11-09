Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kms.xsvc-dynamodb-1-cmk-encrypted-tables-share-one-key-round-trip dynamodb__kms.xsvc-dynamodb-1-create-table-with-nonexistent-cmk-rejected dynamodb__kms.xsvc-dynamodb-1-disabled-cmk-reads-and-writes-rejected dynamodb__kms.xsvc-dynamodb-1-pending-deletion-cmk-reads-and-writes-rejected
Issue-Key: dynamodb__kms|CreateTable|NotImplemented
Issue-Key: dynamodb__kms.xsvc-dynamodb-1-create-table-with-nonexistent-cmk-rejected|run#9|AssertionError|assert code == ERR, (
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# DynamoDB CreateTable with an SSESpecification is an unimplemented scaffold branch, so no CMK-encrypted table can exist and no KMS key-state rejection can be observed (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Cause 1 of 1 — CreateTable refuses any request that carries SSESpecification.
AWS: CreateTable with SSESpecification{Enabled:true, SSEType:'KMS', KMSMasterKeyId:<cmk>} creates the table and its description carries SSEDescription{Status:'ENABLED', SSEType:'KMS', KMSMasterKeyArn:<the key's ARN>} (Smithy: CreateTableInput.SSESpecification members Enabled/SSEType(enum KMS)/KMSMasterKeyId; TableDescription.SSEDescription members Status/SSEType/KMSMasterKeyArn/InaccessibleEncryptionDateTime). Downstream, DynamoDB wraps the KMS fault rather than surfacing it directly: a nonexistent CMK is refused at CreateTable with ValidationException whose message names com.amazonaws.services.kms.model.NotFoundException and no table is created; GetItem/PutItem/Scan on a table whose CMK is Disabled fail ValidationException naming DisabledException; with the CMK PendingDeletion they fail ValidationException naming KMSInvalidStateException; after EnableKey / CancelKeyDeletion+EnableKey the same reads and writes succeed again and items written before the key change survive.
Emulator: every one of those requests dies at the first call — HTTP 501 'An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator'. The same stub is also what the nonexistent-key check receives where it expects ValidationException.
Why: encryption-at-rest with a customer managed key is unusable on DynamoDB — a table cannot be created at all, SSEDescription can never be read back, and the entire KMS key-state failure surface (the production incident where a disabled or pending-deletion CMK silently breaks a table) cannot be reproduced or tested.
Fix: the CreateTable handler's SSESpecification branch plus the KMS key-state gate on the GetItem/PutItem/Scan data path (ValidationException wrapping the KMS cause, not the KMS error code itself).
Traces: dynamodb__kms.xsvc-dynamodb-1-cmk-encrypted-tables-share-one-key-round-trip, dynamodb__kms.xsvc-dynamodb-1-create-table-with-nonexistent-cmk-rejected, dynamodb__kms.xsvc-dynamodb-1-disabled-cmk-reads-and-writes-rejected, dynamodb__kms.xsvc-dynamodb-1-pending-deletion-cmk-reads-and-writes-rejected.

## Evidence
Three checks error identically: 'botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator' (HTTP 501). The fourth asserts the rejection contract and reports: "AssertionError: CreateTable with nonexistent KMS key 'arn:aws:kms:us-west-2:000000000000:key/10bb1288-7b9e-4e40-8f0f-d32b2e908e86' must fail with 'ValidationException', got 'NotImplemented': 'operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator'".
Expected: HTTP 200 and TableDescription.SSEDescription{Status:'ENABLED', SSEType:'KMS', KMSMasterKeyArn:<key arn>} for the valid case; HTTP 400 ValidationException (message naming the wrapped KMS NotFoundException/DisabledException/KMSInvalidStateException) for the three negative cases.

## Steps to Reproduce
1. kms:CreateKey (symmetric). 2. dynamodb:CreateTable(TableName=..., KeySchema=[pk], SSESpecification={'Enabled': True, 'SSEType': 'KMS', 'KMSMasterKeyId': <key arn>}) — observe 501 NotImplemented. 3. Repeat with a well-formed but nonexistent key ARN — observe 501 instead of ValidationException. 4. (Blocked by 2) DisableKey / ScheduleKeyDeletion, then GetItem and PutItem on the encrypted table and confirm ValidationException naming the KMS cause, then EnableKey / CancelKeyDeletion+EnableKey and confirm reads and writes recover with pre-existing items intact.
