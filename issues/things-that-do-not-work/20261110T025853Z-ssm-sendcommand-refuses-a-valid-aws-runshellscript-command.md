Issue-Source: trace-sweep-divergence
Issue-Trace: s3files-nfs-mount-write-lands-in-bucket
Issue-Step: task#28
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3files-nfs-mount-write-lands-in-bucket
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: ssm|SendCommand|2xx->4xx|$.Command
Issue-Related: 20261109T173225Z-ssm-run-command-is-unusable-end-to-end.md
Issue-Commit: 109411407d03b40f255761f7a04c49ae72d75d8c

# ssm SendCommand refuses a valid AWS-RunShellScript command with HTTP 400 instead of accepting it

> _Note: this issue was identified against the code base at commit `109411407d03b40f255761f7a04c49ae72d75d8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
Cause 1 of 1: ssm.SendCommand against the fixture's target managed node with the built-in document AWS-RunShellScript is refused with HTTP 400 where AWS accepts it with HTTP 200 and a full Command payload, so no command is ever created and nothing can be executed on the node (fixture s3files-nfs-mount-write-lands-in-bucket, step task#28).
AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models SendCommand with input InstanceIds (InstanceIdList, @length 0..50), DocumentName required (@pattern ^[a-zA-Z0-9_\-.:/]{3,128}$), Parameters, Comment (@length 0..100), MaxConcurrency/MaxErrors, TimeoutSeconds (30..2592000); output `Command`; client errors InvalidInstanceId (400 — documented for a node the caller cannot access, whose SSM Agent is not running, that is not registered with the SSM endpoint, or that is not in a valid state), InvalidDocument, InvalidDocumentVersion, InvalidParameters, DuplicateInstanceId, InvalidRole, InvalidOutputFolder, UnsupportedPlatformType, plus InternalServerError (500). Recorded behavior: with the target node under management, SendCommand is accepted with HTTP 200 and a `Command` structure that always carries the full projection — CommandId, DocumentName ('AWS-RunShellScript'), DocumentVersion ('$DEFAULT'), Parameters echoed from the request, InstanceIds echoed from the request, Status/StatusDetails 'Pending' for a freshly issued command, TargetCount 1, CompletedCount/ErrorCount/DeliveryTimedOutCount 0, MaxConcurrency '50', MaxErrors '0', TimeoutSeconds 600, OutputS3Region = the request's region, RequestedDateTime and ExpiresAfter timestamps, and the members AWS emits even when unset: NotificationConfig {NotificationArn '', NotificationEvents [], NotificationType ''}, CloudWatchOutputConfig {CloudWatchLogGroupName '', CloudWatchOutputEnabled false}, AlarmConfiguration {Alarms [], IgnorePollAlarmFailure false}, TriggeredAlarms [], Targets [], ClientName/ClientSourceId/Comment/OutputS3BucketName/OutputS3KeyPrefix/ServiceRole as empty strings, HasCancelCommandSignature/HasSendCommandSignature/Interactive false. The CommandId and the timestamps are per-run and are not the requirement; the 200, the always-present member set with those defaults, and the echo of DocumentName/InstanceIds/Parameters are.
Emulator: the step answers HTTP 400 and no `$.Command` member at all (the replay reports `status: expected 200, got 400` and `$.Command: missing`). Hypothesis: the refusal is InvalidInstanceId, because the emulator's managed-node state holds no row for the target instance id (the preceding DescribeInstanceInformation step in this same fixture, setup#22, serves an empty InstanceInformationList for 180 s); the replay does not expose the error body, so the emitted code and message are not characterised beyond the status.
Why: Run Command is the only way a client executes anything on a managed node. A caller that targets an instance AWS accepts gets a hard, non-retryable 400, so no automation, patch run, mount/bootstrap script or operator one-liner can run, and nothing downstream of command execution can be exercised — in this fixture the command that mounts the s3files file system and writes the round-trip object never runs at all. Because AWS's 400 class here means "node not accessible / agent not registered", a client has no way to distinguish a genuinely bad instance id from the emulator's missing state.
Fix: the SendCommand handler's target-resolution path in services/service-com-amazonaws-ssm-amazonssm/src/ — it must accept an instance id present in the managed-node state and create the command in the Run Command state (with the Command projection above) instead of refusing; this depends on the managed-node registration path populating InstanceInformation. NOTE: the open issue 20261109T173225Z-ssm-run-command-is-unusable-end-to-end.md reports the same family of fault but is bound to fixture ssm-list-commands-instanceid-filter-and-maxresults (steps setup#8..task#16) and carries NO replay key for this fixture's step, and the replay explicitly does not mark this cluster as owned — so this step must be re-replayed here. (This fixture's DescribeInstanceInformation cluster at setup#22 IS owned by that issue and is deliberately not filed again.)
Traces: s3files-nfs-mount-write-lands-in-bucket#task#28.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture s3files-nfs-mount-write-lands-in-bucket, step task#28, SendCommand [mismatch, status 200 -> 400]:
  status: expected 200, got 400
  $.Command: missing — expected the full Command projection, e.g. {AlarmConfiguration {Alarms [], IgnorePollAlarmFailure false}, ClientName '', ClientSourceId '', CloudWatchOutputConfig {CloudWatchLogGroupName '', CloudWatchOutputEnabled false}, CommandId <per-run>, Comment '', CompletedCount 0, DeliveryTimedOutCount 0, DocumentName 'AWS-RunShellScript', DocumentVersion '$DEFAULT', ErrorCount 0, ExpiresAfter <timestamp>, HasCancelCommandSignature false, HasSendCommandSignature false, InstanceIds [<the target node id from the request>], Interactive false, MaxConcurrency '50', MaxErrors '0', NotificationConfig {NotificationArn '', NotificationEvents [], NotificationType ''}, OutputS3BucketName '', OutputS3KeyPrefix '', OutputS3Region <the request's region>, Parameters {commands: [<the shell script from the request>]}, RequestedDateTime <timestamp>, ServiceRole '', Status 'Pending', StatusDetails 'Pending', TargetCount 1, Targets [], TimeoutSeconds 600, TriggeredAlarms []}, got None.

The request in this step targets a single instance id with DocumentName 'AWS-RunShellScript' and a Parameters.commands script that installs the mount helper, mounts the s3files file system, writes a file into the mount, reads it back and unmounts. Command id, timestamps and the recorded region are per-run values, not requirements.

Correlated context in the same fixture (already owned, not re-filed): step setup#22 DescribeInstanceInformation answers HTTP 200 with {'InstanceInformationList': []} on every poll for 180 s (19 attempts), so PingStatus never reaches 'Online' — consistent with the SendCommand target being absent from managed-node state.

Model: SendCommand input/output/errors per smithy_query on services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), fixture s3files-nfs-mount-write-lands-in-bucket.
2. Step task#28: call ssm.SendCommand(InstanceIds=[<the fixture's target instance id>], DocumentName='AWS-RunShellScript', Parameters={'commands': ['<the mount/write/read script>']}, TimeoutSeconds=600).
3. Observe HTTP 400 with no Command in the body. AWS answers HTTP 200 with the Command projection described above (Status/StatusDetails 'Pending', TargetCount 1, InstanceIds/DocumentName/Parameters echoed, and the always-present NotificationConfig/CloudWatchOutputConfig/AlarmConfiguration/TriggeredAlarms/Targets defaults).
4. Inspect the 400 body to confirm the refusal class (expected to be InvalidInstanceId given the node is absent from the emulator's managed-node state).
