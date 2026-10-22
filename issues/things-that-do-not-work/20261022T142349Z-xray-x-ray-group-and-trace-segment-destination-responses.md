Issue-Source: trace-sweep-divergence
Issue-Trace: xray-group-lifecycle-with-tags
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-group-lifecycle-with-tags xray-insights-investigation xray-trace-segment-destination-toggle
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|CreateGroup|2xx->2xx|$.Group.InsightsConfiguration
Issue-Key: xray|GetGroup|2xx->2xx|$.Group.InsightsConfiguration
Issue-Key: xray|UpdateGroup|2xx->2xx|$.Group.InsightsConfiguration
Issue-Key: xray|CreateGroup|2xx->2xx|$.Group.InsightsConfiguration.NotificationsEnabled
Issue-Key: xray|GetGroups|2xx->2xx|$.Groups[*]
Issue-Key: xray|UpdateTraceSegmentDestination|4xx->2xx|$.Message
Issue-Key: xray|UpdateTraceSegmentDestination|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: xray|UpdateTraceSegmentDestination|4xx->2xx|status
Issue-Key: xray|GetTraceSegmentDestination|2xx->2xx|$.Destination
Issue-Key: xray|GetTraceSegmentDestination|2xx->2xx|$.Status
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# X-Ray group and trace-segment-destination responses: InsightsConfiguration omitted, Default group ARN suffixed, destination switch unguarded and stuck PENDING

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Five distinct root causes in the X-Ray group control plane and the account's trace-segment-destination singleton of services/service-com-amazonaws-xray-awsxray, all from wire fixture replay (simparity-wire-aws-v1, state_reset_failures=0). None is marked ALREADY FILED or OWNED in the replay report. The resolved issue 20261022T013648Z-xray-x-ray-group-control-plane-no-implicit-default.md added the implicit Default group (it is now returned), but not with AWS's ARN form — Cause 2 below. The open issue 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md describes the same destination permissiveness behaviourally (its Causes 5 and 6) but carries NO wire replay key, so Causes 3-5 here are unowned on the wire and are filed with their fixture steps.

Cause 1 of 5: InsightsConfiguration is omitted from every serialized Group/GroupSummary.
AWS: a group always reports its insights configuration, defaulting to {InsightsEnabled: false, NotificationsEnabled: false} when the group was created without one; it is present in CreateGroup, GetGroup, UpdateGroup and GetGroups responses alike. Smithy: Group and GroupSummary both have member InsightsConfiguration -> InsightsConfiguration{InsightsEnabled, NotificationsEnabled} (NullableBoolean), and CreateGroup accepts InsightsConfiguration on input.
Emulator: $.Group.InsightsConfiguration is missing (null) on CreateGroup, GetGroup and UpdateGroup, and absent from each entry of GetGroups.Groups; HTTP 200 and the other Group members match. Where the fixture asserts the nested member directly, NotificationsEnabled is likewise missing.
Why: insights enablement becomes invisible — a client cannot read back whether insights/notifications are on for a group it just created with InsightsConfiguration set, and strongly-typed SDK consumers get null where AWS always supplies the structure.
Fix: the Group/GroupSummary serializer plus the group state's default — store and emit the insights configuration (defaulting both flags to false) for every group, including the Default group.
Traces: xray-group-lifecycle-with-tags#task#1 (CreateGroup), #task#3 and #task#10 (GetGroup), #task#9 (UpdateGroup), #task#4 (GetGroups), xray-insights-investigation#setup#1 (CreateGroup, nested NotificationsEnabled).

Cause 2 of 5: the implicit Default group's ARN carries a generated id suffix.
AWS: the account's built-in group is named "Default" and its ARN has no generated id segment — arn:aws:xray:<region>:<account>:group/Default. Only user-created groups get the trailing generated id (the user group's arn:...:group/<name>/<12-hex> matched the recording exactly in the same step). Smithy: GroupSummary.GroupARN is documented as "The ARN of the group generated based on the GroupName" — recorded behavior fixes the Default group's form.
Emulator: GetGroups returns the Default group as GroupARN 'arn:aws:xray:us-west-2:000000000000:group/Default/b267df339df0' — the suffix form used for user groups.
Why: the Default group's ARN is a stable, well-known identifier that clients, IAM policies and tagging flows hard-code; a per-boot random suffix makes it unguessable and makes any ARN-keyed reference to the Default group fail.
Fix: the ARN construction for the seeded Default group in the X-Ray group state — omit the generated id segment for it.
Traces: xray-group-lifecycle-with-tags#task#4.

Cause 3 of 5: UpdateTraceSegmentDestination accepts a switch to CloudWatchLogs that AWS refuses, and mutates the account singleton.
AWS: switching the account's trace segment destination to CloudWatchLogs is refused with HTTP 403 and x-amzn-errortype AccessDeniedException while no CloudWatch Logs resource policy grants X-Ray logs:PutLogEvents on the aws/spans log group, and the stored destination is left unchanged (a subsequent GetTraceSegmentDestination still reports XRay); recorded message example "XRay does not have permission to call PutLogEvents on the aws/spans Log Group. Please verify that your CloudWatch Logs Resource Policies are configured correctly". Smithy: UpdateTraceSegmentDestination models only InvalidRequestException (400)/ThrottledException (429), so AccessDeniedException at 403 is recorded behavior only; Destination -> TraceSegmentDestination enum [XRay, CloudWatchLogs].
Emulator: answers HTTP 200 with no Message and no x-amzn-errortype, and the singleton is actually changed — the following GetTraceSegmentDestination reports Destination 'CloudWatchLogs' where AWS reports 'XRay'.
Why: a client following the Transaction Search enablement flow is told the switch succeeded, then sees no spans delivered, instead of getting the actionable permission error; worse, the emulator's account state now diverges from AWS's for every later read in the same session.
Fix: a precondition check in the UpdateTraceSegmentDestination handler against CloudWatch Logs resource-policy state before accepting Destination=CloudWatchLogs, and it must not write the singleton when it refuses.
Traces: xray-trace-segment-destination-toggle#task#2 (403 expected, 200 returned), #task#3 (GetTraceSegmentDestination Destination diverges as a result).

Cause 4 of 5: UpdateTraceSegmentDestination accepts a no-op update instead of refusing it.
AWS: re-issuing UpdateTraceSegmentDestination with the destination the account is already on is refused with InvalidRequestException at HTTP 400 (x-amzn-errortype: InvalidRequestException), message example "The destination is already set to XRay". Smithy: UpdateTraceSegmentDestination models InvalidRequestException (client, 400).
Emulator: answers HTTP 200 with no Message and no x-amzn-errortype.
Why: the destination is an asynchronously-switched account singleton; a client that relies on the rejection to detect "already there" cannot distinguish a real transition from a no-op and starts a second transition, losing the idempotency guard the toggle flow depends on.
Fix: a current-value comparison in the UpdateTraceSegmentDestination handler before it stores the new value.
Traces: xray-trace-segment-destination-toggle#task#4.

Cause 5 of 5: the trace-segment-destination Status never settles to ACTIVE.
AWS: GetTraceSegmentDestination reports Status ACTIVE once the account is settled on its destination; PENDING is only the in-flight state of a switch. Smithy: TraceSegmentDestinationStatus enum [PENDING, ACTIVE]; GetTraceSegmentDestination -> {Destination, Status}.
Emulator: GetTraceSegmentDestination reports Status 'PENDING' where AWS reports 'ACTIVE' for the settled account (Destination 'XRay' matched at that step), i.e. the status is left in the transitional value.
Why: any caller that polls GetTraceSegmentDestination for Status==ACTIVE before enabling instrumentation waits forever, and automation that gates on the settled status deadlocks.
Fix: the trace-segment-destination state machine behind GetTraceSegmentDestination — transition PENDING -> ACTIVE once the destination is applied (and report ACTIVE for the account's steady-state default).
Traces: xray-trace-segment-destination-toggle#task#5.

## Evidence
Cause 1: xray-group-lifecycle-with-tags#task#1 CreateGroup 200 vs 200 — `$.Group.InsightsConfiguration: missing (expected {'InsightsEnabled': False, 'NotificationsEnabled': False}, got None)`; identical diff on GetGroup (#task#3, #task#10) and UpdateGroup (#task#9); xray-insights-investigation#setup#1 — `$.Group.InsightsConfiguration.NotificationsEnabled: missing (expected False, got None)`; xray-group-lifecycle-with-tags#task#4 GetGroups — both entries expected to carry 'InsightsConfiguration': {'InsightsEnabled': False, 'NotificationsEnabled': False}, neither did.
Cause 2: xray-group-lifecycle-with-tags#task#4 GetGroups 200 vs 200 — `$.Groups[0]: expected GroupARN 'arn:aws:xray:us-west-2:000000000000:group/Default', got 'arn:aws:xray:us-west-2:000000000000:group/Default/b267df339df0'` (the user group's ARN 'arn:...:group/sim2real-probe-xray-grp-c0945404/cc61385143d1' matched).
Cause 3: xray-trace-segment-destination-toggle#task#2 UpdateTraceSegmentDestination — `status: expected 403, got 200`; `$headers.x-amzn-errortype: missing (expected 'AccessDeniedException', got None)`; `$.Message: missing (expected 'XRay does not have permission to call PutLogEvents on the aws/spans Log Group...', got None)`. Consequent #task#3 GetTraceSegmentDestination — `$.Destination: value_mismatch (expected 'XRay', got 'CloudWatchLogs')`.
Cause 4: xray-trace-segment-destination-toggle#task#4 — `status: expected 400, got 200`; `$headers.x-amzn-errortype: missing (expected 'InvalidRequestException', got None)`; `$.Message: missing (expected 'The destination is already set to XRay', got None)`.
Cause 5: xray-trace-segment-destination-toggle#task#5 GetTraceSegmentDestination 200 vs 200 — `$.Status: value_mismatch (expected 'ACTIVE', got 'PENDING')`.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (simparity-wire-aws-v1); emulator_up true, bootstrap_signal false.
2. Cause 1: CreateGroup(GroupName=<n>, FilterExpression=...), then GetGroup, UpdateGroup and GetGroups — inspect Group/GroupSummary.InsightsConfiguration in each (expect {InsightsEnabled: false, NotificationsEnabled: false}, observe absent). Fixture xray-group-lifecycle-with-tags steps task#1, task#3, task#9, task#4; xray-insights-investigation step setup#1.
3. Cause 2: GetGroups on any account and read the Default group's GroupARN (expect arn:aws:xray:<region>:<account>:group/Default with no trailing id; observe a 12-hex suffix). Fixture xray-group-lifecycle-with-tags step task#4.
4. Cause 3: with the account on Destination=XRay and no CloudWatch Logs resource policy for xray.amazonaws.com logs:PutLogEvents on aws/spans, call UpdateTraceSegmentDestination(Destination='CloudWatchLogs') — expect 403 AccessDeniedException and GetTraceSegmentDestination still 'XRay'; observe 200 and 'CloudWatchLogs'. Fixture xray-trace-segment-destination-toggle steps task#2, task#3.
5. Cause 4: GetTraceSegmentDestination, then UpdateTraceSegmentDestination with the destination just read — expect 400 InvalidRequestException; observe 200. Fixture xray-trace-segment-destination-toggle step task#4.
6. Cause 5: with the account settled on XRay, GetTraceSegmentDestination — expect Status ACTIVE, observe PENDING. Fixture xray-trace-segment-destination-toggle step task#5.
7. Re-replay the fixtures/steps in Issue-Trace/Issue-Step and trace_ids to confirm.
