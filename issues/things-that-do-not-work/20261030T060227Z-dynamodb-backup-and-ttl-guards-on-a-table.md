Issue-Source: trace-sweep-divergence
Issue-Trace: ddbctl-backup-on-creating-table-and-restore-name-collision
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbctl-backup-on-creating-table-and-restore-name-collision ddbctl-ttl-reenable-and-toggle-rejected dynamodb-ttl-contributor-insights-and-replica-autoscaling-config
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateBackup|4xx->4xx|$.__type
Issue-Key: dynamodb|CreateBackup|4xx->4xx|$.message
Issue-Key: dynamodb|UpdateContinuousBackups|4xx->2xx|$.__type
Issue-Key: dynamodb|UpdateContinuousBackups|4xx->2xx|$.message
Issue-Key: dynamodb|UpdateContinuousBackups|4xx->2xx|status
Issue-Key: dynamodb|UpdateTimeToLive|4xx->2xx|$.__type
Issue-Key: dynamodb|UpdateTimeToLive|4xx->2xx|$.message
Issue-Key: dynamodb|UpdateTimeToLive|4xx->2xx|status
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB backup and TTL guards on a table that is not ready answer the wrong error or none

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackup

## Description
Three refusals that guard operations against a table whose state does not permit them: two pick the wrong error for a table that is not yet usable, and one rate guard is missing entirely. 3 causes, all dynamodb.

Cause 1 of 3: CreateBackup against a table that is still being created answers TableInUseException where AWS answers TableNotFoundException.
AWS: the recording shows HTTP 400 with __type 'com.amazonaws.dynamodb.v20120810#TableNotFoundException' and the message 'Table not found: <table name>' — the fixed text is 'Table not found: ' and the variable part is the table name from the request. AWS treats a table that has not finished creating as not found for the backup path.
Emulator: HTTP 400 with __type 'com.amazonaws.dynamodb.v20120810#TableInUseException' and 'Table is being created or deleted: <table name>'.
Why: the two errors mean different things to a client. TableNotFoundException is a terminal 'the resource is not there' that drives a create-then-retry path; TableInUseException reads as a transient conflict that callers retry in place. SDK error-type matching therefore routes this refusal into the wrong recovery branch.
Fix: the CreateBackup handler's table-state guard — raise TableNotFoundException with the 'Table not found: <name>' message for a table that is not yet usable.
Traces: ddbctl-backup-on-creating-table-and-restore-name-collision#task#3.

Cause 2 of 3: UpdateContinuousBackups against the same not-yet-usable table is accepted with 200 instead of being refused.
AWS: Smithy: UpdateContinuousBackups' modelled errors include TableNotFoundException (client, 400) and ContinuousBackupsUnavailableException (client, 400). The recording shows HTTP 400 with __type 'com.amazonaws.dynamodb.v20120810#TableNotFoundException' and 'Table not found: <table name>'.
Emulator: HTTP 200 with no __type and no message — the enable is accepted and, as the status-settling issue shows, is then reported back as ENABLED.
Why: a client gets a success for a PITR enable that AWS rejects, so it proceeds believing continuous backups are configured on a table that has none; the error it was written to handle never arrives.
Fix: the UpdateContinuousBackups handler — gate the call on the table being usable and raise TableNotFoundException otherwise.
Traces: ddbctl-backup-on-creating-table-and-restore-name-collision#task#4.

Cause 3 of 3: repeated UpdateTimeToLive calls within AWS's cooldown are accepted instead of refused.
AWS: Smithy: UpdateTimeToLive's modelled errors include LimitExceededException and ResourceInUseException; the recording shows HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and the message 'Time to live has been modified multiple times within a fixed interval' — the whole sentence is fixed text, nothing is request-derived. DynamoDB rate-limits TTL changes on a table.
Emulator: HTTP 200 with no __type and no message — any number of TTL toggles is accepted.
Why: the guard is the reason a client must not treat TTL as a freely togglable switch; with it missing, code that flips TTL in a loop passes against the emulator and then fails in production on the first repeat, and the refusal path is never exercised.
Fix: the UpdateTimeToLive validation plus the TTL state it reads — record the last TTL modification time per table and refuse a second change inside the interval with ValidationException (400).
Traces: ddbctl-ttl-reenable-and-toggle-rejected#task#9 (cluster of 2, also dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#15).

## Evidence
Divergence 1 (CreateBackup, 400->400): $.__type expected 'com.amazonaws.dynamodb.v20120810#TableNotFoundException', got 'com.amazonaws.dynamodb.v20120810#TableInUseException'; $.message expected 'Table not found: <table>', got 'Table is being created or deleted: <table>'.
Divergence 2 (UpdateContinuousBackups, 400->200): status expected 400 got 200; $.__type expected 'com.amazonaws.dynamodb.v20120810#TableNotFoundException', got None; $.message expected 'Table not found: <table>', got None.
Divergence 12 (UpdateTimeToLive, 400->200): status expected 400 got 200; $.__type expected 'com.amazon.coral.validate#ValidationException', got None; $.message expected 'Time to live has been modified multiple times within a fixed interval', got None (cluster of 2).
Model: smithy_query(operation=UpdateContinuousBackups) -> errors TableNotFoundException(400), ContinuousBackupsUnavailableException(400), InvalidEndpointException(421), InternalServerError(500); operation=UpdateTimeToLive -> errors LimitExceededException(400), ResourceInUseException(400), ResourceNotFoundException(400), InvalidEndpointException(421), InternalServerError(500).

## Steps to Reproduce
1. Replay ddbctl-backup-on-creating-table-and-restore-name-collision#task#3: CreateBackup on a table that is still creating -> TableInUseException instead of TableNotFoundException.
2. Replay the same fixture's #task#4: UpdateContinuousBackups on that table -> 200 instead of 400 TableNotFoundException.
3. Replay ddbctl-ttl-reenable-and-toggle-rejected#task#9: UpdateTimeToLive twice in quick succession -> the second call answers 200 instead of 400 ValidationException 'Time to live has been modified multiple times within a fixed interval'.
