Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-list-commands-instanceid-filter-and-maxresults
Issue-Step: setup#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-list-commands-instanceid-filter-and-maxresults
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DescribeInstanceInformation|2xx->2xx|$
Issue-Key: ssm|DescribeInstanceInformation|2xx->2xx|$.InstanceInformationList[*].PingStatus
Issue-Key: ssm|SendCommand|2xx->4xx|status
Issue-Key: ssm|ListCommands|2xx->2xx|$.Commands[*]
Issue-Key: ssm|ListCommands|2xx->2xx|$.NextToken
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# ssm Run Command is unusable end to end: no node ever reports Online, SendCommand is refused, and ListCommands serves nothing

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation, SendCommand, ListCommands

## Description
Four net-new root causes along SSM's Run Command path, all in the `ssm` service: a node that should become a managed node never appears in DescribeInstanceInformation, SendCommand against that node id is refused with 400 where AWS accepts it, ListCommands then serves an empty Commands list where AWS returns the issued commands with their full projection, and the same ListCommands call omits NextToken when the result is page-limited. Causes 2-4 sit downstream of Cause 1 but are each their own divergence cluster with its own replay binding, so all four must be re-replayed and closed before submitting.

Cause 1 of 4: a node registered in the fixture's setup never becomes an Online managed node — DescribeInstanceInformation serves an empty list indefinitely.
AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models DescribeInstanceInformation with output `InstanceInformationList` (InstanceInformationList of InstanceInformation) + NextToken, paginated, errors InvalidInstanceId/InvalidFilterKey/InvalidInstanceInformationFilterValue/InvalidNextToken/InternalServerError. InstanceInformation carries `InstanceId` (@pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$) and `PingStatus` (PingStatus enum, doc "Connection status of SSM Agent"), plus LastPingDateTime/AgentVersion/PlatformType/ResourceType and friends. Recorded behavior: once the node under test is registered with Systems Manager, AWS lists exactly one InstanceInformation row for it and that row's PingStatus reaches `Online` within the fixture's polling window; the subsequent Run Command steps depend on it.
Emulator: DescribeInstanceInformation keeps answering HTTP 200 with `{"InstanceInformationList": []}` — the replay polled 19 times and gave up at the 180 s cap with the list still empty, so PingStatus never reaches Online because no row is ever served. The operation answers, it just never reflects the registered node.
Why: DescribeInstanceInformation is the inventory front door for Systems Manager. Every client that waits for a node to come under management before acting — Run Command, Session Manager, State Manager, patching, any operator script that polls for Online — waits forever and then times out, and a fleet listing reports zero managed nodes where AWS reports the registered one. Nothing downstream of node management can be exercised.
Fix: the managed-node registration/state path for SSM — whatever registers a node with Systems Manager must write an InstanceInformation row into the SSM instance-information state with PingStatus transitioning to Online, and DescribeInstanceInformation's handler must project rows from that state (services/service-com-amazonaws-ssm-amazonssm/src/, the describe_instance_information handler and the instance-information state module it reads). Related open issues report the same family of fault from behavioral and CloudFormation-driven suites — e.g. 20261109T142303Z-ssm-5-net-new-causes-managed-nodes-never.md, 20261108T104938Z-ec2-ssm-a-profile-bearing-ec2-instance-never.md and 20261107T112846Z-cloudformation-ec2-ssm-a-profile-bearing-ec2-instance.md — but none of them carries this fixture's wire replay key, so this step must be re-replayed here.
Traces: ssm-list-commands-instanceid-filter-and-maxresults#setup#8.

Cause 2 of 4: SendCommand against the node id is refused with HTTP 400 where AWS accepts the command and returns 200.
AWS: Smithy: SendCommand input `InstanceIds` (InstanceIdList @length 0..50), `DocumentName` required (@pattern ^[a-zA-Z0-9_\-.:/]{3,128}$), Parameters, Comment (@length 0..100), MaxConcurrency/MaxErrors etc.; output `Command`; client errors include InvalidInstanceId (400, doc: raised when the caller lacks access to the node, SSM Agent is not running, the agent is not registered with the SSM endpoint, or the node is not in a valid state), InvalidDocument, InvalidParameters. Recorded behavior: with the target node under management and a built-in document (AWS-RunShellScript), SendCommand is accepted with HTTP 200 and a Command payload; the fixture issues three such commands in setup.
Emulator: all three SendCommand steps answer HTTP 400 — the replay reports only `status: expected 200, got 400`, so no command is ever created. Hypothesis: the refusal is InvalidInstanceId, raised because the node from Cause 1 is absent from the emulator's managed-node state; the replay does not expose the body, so the code and message are not asserted here beyond the status.
Why: Run Command is how clients execute anything on a managed node. A caller that targets a node AWS accepts gets a hard 400, so no automation, patch script, or operator one-liner can run; and because the refusal class in AWS means "agent not registered / node not accessible", a client has no signal distinguishing a genuinely bad id from the emulator's missing state.
Fix: once Cause 1 registers the node, the SendCommand handler's target-resolution path must accept an instance id present in the managed-node state and create the command in the Run Command state rather than refusing it (the send_command handler plus the managed-node lookup it performs).
Traces: ssm-list-commands-instanceid-filter-and-maxresults#setup#9 (representative); same divergence at setup#10 and setup#11, the other two commands.

Cause 3 of 4: ListCommands returns an empty Commands list, and the Command projection AWS returns is never produced.
AWS: Smithy: ListCommands input CommandId(@length 36..36)/InstanceId(@pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$)/MaxResults(1..50)/NextToken/Filters, output `Commands` (CommandList of Command) + NextToken, paginated{items Commands, pageSize MaxResults}. Recorded behavior: after the commands of Cause 2 complete, ListCommands scoped by InstanceId returns one Command entry per issued command, most-recently-requested first, and each entry carries the full Command projection — CommandId, DocumentName, DocumentVersion, Comment, ExpiresAfter, Parameters, InstanceIds, Targets, RequestedDateTime, Status and StatusDetails, TargetCount/CompletedCount/ErrorCount/DeliveryTimedOutCount, MaxConcurrency/MaxErrors, TimeoutSeconds, ServiceRole, plus the members AWS always emits even when unset: `NotificationConfig` {NotificationArn, NotificationEvents [], NotificationType} with empty strings, `CloudWatchOutputConfig` {CloudWatchLogGroupName '', CloudWatchOutputEnabled false}, `AlarmConfiguration` {Alarms [], IgnorePollAlarmFailure false}, `TriggeredAlarms` [], `Targets` [], ClientName/ClientSourceId/OutputS3BucketName/OutputS3KeyPrefix as empty strings, and HasCancelCommandSignature/HasSendCommandSignature/Interactive as false. The command ids, timestamps and the bucket/region values in the recording are per-run and are not the requirement; the entry count, the ordering and the always-present member set are.
Emulator: HTTP 200 with no Commands entries at all — the replay reports every expected `$.Commands[0..2]` as missing. No command exists to list because Cause 2 refused them all, and consequently none of the projection above is exercised.
Why: ListCommands is the audit/history view of Run Command. A client that issues a command and then lists it to track status or to find its CommandId gets nothing back, so status polling, retry-on-failure and any command-history UI are empty; this is silent (HTTP 200) rather than an error, so callers conclude no command was ever sent.
Fix: the ListCommands handler must read the Run Command state populated by SendCommand, return entries newest-first honouring the InstanceId/CommandId/Filters scoping, and its serializer must project the always-present Command members above with AWS's empty-string/empty-list/false defaults.
Traces: ssm-list-commands-instanceid-filter-and-maxresults#task#15.

Cause 4 of 4: a page-limited ListCommands omits NextToken.
AWS: Smithy: ListCommands is @paginated with inputToken NextToken, outputToken NextToken, items Commands, pageSize MaxResults(range 1..50). Recorded behavior: when MaxResults truncates the result set, AWS returns the page of Commands together with a non-empty `NextToken` that the caller passes back to fetch the remainder; the token's value is opaque and per-run, its PRESENCE is the requirement.
Emulator: the same call answers HTTP 200 with `NextToken` absent, i.e. it signals "last page" on a truncated result.
Why: a paginating client (every SDK paginator, `aws ssm list-commands` with --max-items) stops after the first page and silently loses the rest of the command history; there is no error to detect, just missing data.
Fix: the ListCommands handler's pagination logic and output serializer — when more entries remain beyond MaxResults, emit a NextToken that the handler accepts on the following call.
Traces: ssm-list-commands-instanceid-filter-and-maxresults#task#16.

## Evidence
Cause 1 — fixture ssm-list-commands-instanceid-filter-and-maxresults, step setup#8, DescribeInstanceInformation [mismatch, status 200 -> 200]:
  $: poll capped at 180s — got {'InstanceInformationList': []}
  $.InstanceInformationList[0].PingStatus: poll: not in ['Online'] after 19 attempt(s)
  The list never contains a row for the node, so PingStatus never reaches Online.

Cause 2 — same fixture, step setup#9, SendCommand [mismatch]:
  status: expected 200, got 400
  Repeats identically at setup#10 and setup#11 (affected_count 3) — the three commands the fixture issues with DocumentName AWS-RunShellScript against the single target node id. The replay asserts only the status for these steps, so the emulator's error code/message are not characterised here beyond the 400.

Cause 3 — same fixture, step task#15, ListCommands [mismatch, status 200 -> 200]:
  $.Commands[0], $.Commands[1], $.Commands[2]: all expected, all missing (the response carried no entries).
  Each expected entry is a full Command projection, e.g. {AlarmConfiguration {Alarms [], IgnorePollAlarmFailure false}, ClientName '', ClientSourceId '', CloudWatchOutputConfig {CloudWatchLogGroupName '', CloudWatchOutputEnabled false}, CommandId <per-run>, Comment <the per-command comment from the request>, CompletedCount 1, DeliveryTimedOutCount 0, DocumentName 'AWS-RunShellScript', DocumentVersion '$DEFAULT', ErrorCount 0, ExpiresAfter <timestamp>, HasCancelCommandSignature false, HasSendCommandSignature false, InstanceIds [<the target node id from the request>], Interactive false, MaxConcurrency '50', MaxErrors '0', NotificationConfig {NotificationArn '', NotificationEvents [], NotificationType ''}, OutputS3BucketName '', OutputS3KeyPrefix '', OutputS3Region <region>, Parameters <from the request>, RequestedDateTime <timestamp>, ServiceRole '', Status 'Success', StatusDetails 'Success', TargetCount 1, Targets [], TimeoutSeconds 600, TriggeredAlarms []}. Entries are ordered most-recent-first. Command ids, timestamps, region and bucket values are per-run and not the requirement; the entry count, the ordering and the always-present member set with those defaults are.

Cause 4 — same fixture, step task#16, ListCommands with MaxResults [mismatch, status 200 -> 200]:
  $.NextToken: expected present (opaque, shaped-not-asserted), got missing.

Model citations (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): DescribeInstanceInformation output InstanceInformationList+NextToken, paginated, errors InvalidInstanceId/InvalidFilterKey/InvalidInstanceInformationFilterValue/InvalidNextToken/InternalServerError; InstanceInformation members InstanceId(@pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$)/PingStatus(PingStatus enum)/LastPingDateTime/AgentVersion/PlatformType/ResourceType/ActivationId/IamRole/RegistrationDate/... ; SendCommand input InstanceIds(@length 0..50)+DocumentName(required)+Parameters+Comment(0..100)+MaxConcurrency+MaxErrors+TimeoutSeconds(30..2592000), output Command, errors InvalidInstanceId(400)/InvalidDocument/InvalidDocumentVersion/InvalidParameters/DuplicateInstanceId/InvalidRole/InvalidNotificationConfig/InvalidOutputFolder/MaxDocumentSizeExceeded/UnsupportedPlatformType/InternalServerError; ListCommands input CommandId(36..36)/InstanceId/MaxResults(1..50)/NextToken/Filters(1..5), output Commands(CommandList)+NextToken, paginated{inputToken NextToken, outputToken NextToken, items Commands, pageSize MaxResults}; Command members CommandId/DocumentName/DocumentVersion/Comment/ExpiresAfter/Parameters/InstanceIds/Targets/RequestedDateTime/Status/StatusDetails/OutputS3*/MaxConcurrency/MaxErrors/TargetCount/CompletedCount/ErrorCount/DeliveryTimedOutCount/ServiceRole/NotificationConfig/CloudWatchOutputConfig/TimeoutSeconds/AlarmConfiguration/TriggeredAlarms.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), fixture ssm-list-commands-instanceid-filter-and-maxresults.
2. Cause 1 — step setup#8: after the fixture's setup brings up the node under test, poll ssm.DescribeInstanceInformation. Observe HTTP 200 with InstanceInformationList == [] on every poll for 180 s; AWS lists the node with PingStatus reaching 'Online'.
3. Cause 2 — steps setup#9..setup#11: call ssm.SendCommand(InstanceIds=[<the node id>], DocumentName='AWS-RunShellScript', Parameters={'commands': ['true']}, Comment='<per-command comment>'). Observe HTTP 400; AWS answers 200 with a Command payload. Inspect the body to confirm the refusal class (expected to be InvalidInstanceId given the node is absent from state).
4. Cause 3 — step task#15: call ssm.ListCommands(InstanceId=<the node id>). Observe HTTP 200 with an empty Commands list; AWS returns one entry per issued command, newest first, each carrying the full Command projection including the always-present NotificationConfig/CloudWatchOutputConfig/AlarmConfiguration/TriggeredAlarms/Targets defaults.
5. Cause 4 — step task#16: call ssm.ListCommands(InstanceId=<the node id>, MaxResults=<fewer than the number of commands>). Observe that NextToken is absent from the response, so a paginator stops after the first page; AWS returns an opaque NextToken whenever entries remain.
