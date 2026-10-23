Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.create-profiling-group-invalid-name-pattern-rejected codeguruprofiler.create-profiling-group-invalid-name-rejected codeguruprofiler.configure-agent-fleet-instance-id-too-long-rejected codeguruprofiler.add-notification-channels-invalid-event-publisher-rejected codeguruprofiler.list-profile-times-invalid-period-rejected codeguruprofiler.put-permission-invalid-action-group-rejected codeguruprofiler.describe-nonexistent-profiling-group-not-found codeguruprofiler.delete-profiling-group-then-resource-not-found codeguruprofiler.delete-profiling-group-removes-visibility codeguruprofiler.create-profiling-group-lifecycle codeguruprofiler.configure-agent-nonexistent-group-not-found codeguruprofiler.create-profiling-group-duplicate-name-rejected codeguruprofiler.create-profiling-group-duplicate-name-conflict-rejected codeguruprofiler.get-profile-no-data-rejected codeguruprofiler.get-profile-without-data-not-found codeguruprofiler.get-profile-no-aggregated-data-yet
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.create-profiling-group-invalid-name-pattern-rejected|run#6|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.create-profiling-group-invalid-name-rejected|run#6|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.configure-agent-fleet-instance-id-too-long-rejected|run#12|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.add-notification-channels-invalid-event-publisher-rejected|run#12|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.list-profile-times-invalid-period-rejected|run#10|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.put-permission-invalid-action-group-rejected|run#9|AssertionError|assert err["Message"] == expected, (err["Message"], expected)
Issue-Key: codeguruprofiler.describe-nonexistent-profiling-group-not-found|run#11|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.delete-profiling-group-then-resource-not-found|run#10|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.delete-profiling-group-removes-visibility|run#8|AssertionError|assert describe_message == expected_describe_message, (describe_message, expected_describe_message)
Issue-Key: codeguruprofiler.create-profiling-group-lifecycle|run#23|AssertionError|assert last_message == expected_describe_message, (last_message, expected_describe_message)
Issue-Key: codeguruprofiler.configure-agent-nonexistent-group-not-found|run#12|AssertionError|assert error.get("Message") == expected_message, (error.get("Message"), expected_message)
Issue-Key: codeguruprofiler.create-profiling-group-duplicate-name-rejected|run#8|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler.create-profiling-group-duplicate-name-conflict-rejected|run#8|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Key: codeguruprofiler|GetProfile|ResourceNotFoundException
Issue-Key: codeguruprofiler.get-profile-without-data-not-found|run#9|AssertionError|assert message == "No profiles found for the requested time range!", message
Issue-Key: codeguruprofiler.get-profile-no-aggregated-data-yet|run#7|AssertionError|assert message == "No profiles found for the requested time range!", message
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# CodeGuru Profiler error-message construction diverges from AWS: anchored @pattern, truncated value, wrong list-member path, unsorted/incomplete enum sets, and emulator-authored not-found/conflict texts (9 causes)

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfilingGroup, ConfigureAgent, DescribeProfilingGroup, DeleteProfilingGroup, GetProfile, ListProfileTimes, PutPermission, AddNotificationChannels

## Description
Nine distinct root causes, all in how codeguruprofiler builds the `message` of a refusal. In every case the HTTP status and error code already match AWS; what diverges is the message, and for each of these the rule that produces the message IS known — it is either a constraint-violation message built from the request (member path + constraint, so the rendering rule is the requirement) or a service text built from the request's own identifiers. Each cause must be closed before this issue is resubmitted.

=== Cause 1 of 9: @pattern constraint messages keep the model's regex anchors ===
AWS: Smithy ProfilingGroupName is `pattern: "^[\\w-]+$"`, length 1..255; CreateProfilingGroup models ValidationException (400). Recorded behavior: AWS renders the constraint as the UNANCHORED expression — "1 validation error detected: Value '<v>' at 'profilingGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+".
Emulator: emits the model string verbatim, "... pattern: ^[\\w-]+$".
Why: clients and test suites that match the documented constraint text (or re-derive the accepted alphabet from it) see a pattern AWS never emits; the anchors are an artifact of the model, not of the service's message. Precedent for the same rendering bug elsewhere in this repo: the open bcm-pricing-calculator and bedrock pattern-anchor issues.
Fix: the shared request-validation layer that renders @pattern violations — strip leading ^ / trailing $ when formatting the pattern into the message.
Traces: codeguruprofiler.create-profiling-group-invalid-name-pattern-rejected, codeguruprofiler.create-profiling-group-invalid-name-rejected.

=== Cause 2 of 9: @length constraint message truncates the offending value ===
AWS: Smithy ConfigureAgent.fleetInstanceId targets FleetInstanceId, length 1..255; ConfigureAgent models ValidationException (400). Recorded behavior: the message echoes the rejected value IN FULL — "1 validation error detected: Value '<the entire 302-character value>' at 'fleetInstanceId' failed to satisfy constraint: Member must have length less than or equal to 255".
Emulator: elides the middle of the value and appends a byte-count marker, e.g. "Value 'i-0000…000... [302 bytes]' at 'fleetInstanceId' ...".
Why: the echoed value is part of the message AWS builds from the request; a client that parses the offending value out of the message, or compares messages, gets an emulator-authored summary instead. The "[N bytes]" marker is not an AWS concept.
Fix: the shared validation-message formatter for @length violations — echo the rejected value as received, with no truncation or byte-count annotation.
Traces: codeguruprofiler.configure-agent-fleet-instance-id-too-long-rejected.

=== Cause 3 of 9: enum violation inside a nested list reports the element path instead of the list-level nested constraint ===
AWS: AddNotificationChannels.channels targets Channels (length 1..2, uniqueItems) whose members carry eventPublishers of EventPublisher (enum: AnomalyDetection); ValidationException (400) is modeled. Recorded behavior: the failure is reported against the LIST member with the nested constraint wrapped — "1 validation error detected: Value '[NotARealPublisher]' at 'channels.1.member.eventPublishers' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [AnomalyDetection]]".
Emulator: reports the individual ELEMENT path and the inner constraint unwrapped — "Value 'NotARealPublisher' at 'channels.1.member.eventPublishers.1.member' failed to satisfy constraint: Member must satisfy enum value set: [AnomalyDetection]".
Why: the member path in a constraint message is request-derived and is how clients locate the bad field; pointing at a synthetic `.N.member` leaf instead of the list member (and dropping the nested-constraint wrapper) misreports which input member AWS blames. Precedent: the open bedrock issue "mis-shape list-member failures".
Fix: the shared validation layer's path/rendering for constraint violations on list members — aggregate element failures onto the enclosing list member and wrap the inner constraint as "Member must satisfy constraint: [...]".
Traces: codeguruprofiler.add-notification-channels-invalid-event-publisher-rejected.

=== Cause 4 of 9: enum value set is rendered in model-declaration order, not AWS's order ===
AWS: Smithy AggregationPeriod declares values in the order PT5M, PT1H, P1D; ListProfileTimes models ValidationException (400). Recorded behavior: AWS lists the allowed set as "[P1D, PT1H, PT5M]".
Emulator: "[PT5M, PT1H, P1D]" — the model's declaration order.
Why: the allowed-value list is the actionable part of the message; a client (or doc/test fixture) comparing the set text diverges even though the code is right. Precedent: the open Trusted Advisor issue "wrong enum value-set order".
Fix: the shared validation-message formatter for enum violations — emit the value set in the order AWS reports it (sorted) rather than model order.
Traces: codeguruprofiler.list-profile-times-invalid-period-rejected.

=== Cause 5 of 9: ActionGroup enum value set in the PutPermission refusal is missing a value AWS lists ===
AWS: recorded behavior only — the on-disk Smithy model's ActionGroup enum contains just `agentPermissions`, but the service's own ValidationException for an invalid actionGroup reports "Member must satisfy enum value set: [internalAgentPermissions, agentPermissions]". Status 400 / code ValidationException already match.
Emulator: reports "[agentPermissions]".
Why: a client reading the refusal is told a narrower set than the service actually accepts. Hypothesis: AWS validates against an internal enum that includes an unpublished `internalAgentPermissions` value which the public model omits.
Fix: the PutPermission actionGroup validation / the enum value set it renders — report the set AWS reports for this member. Lowest-value cause in this bundle (message-set content only, sourced from the recording rather than the model).
Traces: codeguruprofiler.put-permission-invalid-action-group-rejected.

=== Cause 6 of 9: ResourceNotFoundException text for group reads/deletes is emulator-authored ===
AWS: Smithy ResourceNotFoundException (HTTP 404, required `message`) is modeled on DescribeProfilingGroup/DeleteProfilingGroup. Recorded behavior: the message is the service's own text naming the requested group — "CodeGuru Profiler can\u2019t complete the operation because <profilingGroupName> was not found." (note the typographic apostrophe), built from the request's profilingGroupName.
Emulator: "no profiling group named <profilingGroupName>".
Why: the 404 already classifies correctly, but a client surfacing or matching the service text sees emulator prose; the text is request-derived, so it is a reproducible rule, not an environment artifact.
Fix: the not-found path of the profiling-group read/delete handlers (and the shared helper they use to build the message).
Traces: codeguruprofiler.describe-nonexistent-profiling-group-not-found, codeguruprofiler.delete-profiling-group-then-resource-not-found, codeguruprofiler.delete-profiling-group-removes-visibility, codeguruprofiler.create-profiling-group-lifecycle.

=== Cause 7 of 9: ConfigureAgent's not-found text uses a different AWS format that the emulator does not reproduce ===
AWS: ConfigureAgent models ResourceNotFoundException (404). Recorded behavior: for this operation the text is account-qualified — "Profiling group for <accountId>/<profilingGroupName> does not exist!" — i.e. the agent plane uses its own message format, distinct from Cause 6's text.
Emulator: the same generic "no profiling group named <profilingGroupName>" as every other operation.
Why: the emulator collapses two distinct, request-derived AWS message formats into one, so neither operation matches; an agent-side client keying on the ConfigureAgent text never sees it.
Fix: ConfigureAgent's not-found path — build the account-qualified message from the caller's account id and the requested group name instead of the shared generic text.
Traces: codeguruprofiler.configure-agent-nonexistent-group-not-found.

=== Cause 8 of 9: ConflictException text for a duplicate profiling group is emulator-authored ===
AWS: CreateProfilingGroup models ConflictException (HTTP 409, required `message`). Recorded behavior: the text is built from the request and the caller's account — "ProfilingGroup (name: <profilingGroupName>, accountId: <accountId>) already exists."
Emulator: "a profiling group named <profilingGroupName> already exists".
Why: 409/ConflictException classify correctly, but the identifying detail AWS includes (the account id alongside the name, telling the caller WHICH group collided) is absent. Precedent for the same class of defect: the open verifiedpermissions issue on emulator-authored ConflictException messages.
Fix: CreateProfilingGroup's duplicate-name conflict path — render the name/accountId message format.
Traces: codeguruprofiler.create-profiling-group-duplicate-name-rejected, codeguruprofiler.create-profiling-group-duplicate-name-conflict-rejected.

=== Cause 9 of 9: GetProfile's no-data refusal does not use the service's fixed message ===
AWS: GetProfile models ResourceNotFoundException (404). Recorded behavior: when the group exists but no aggregated profile covers the requested range, the message is the service's FIXED constant, independent of the request — "No profiles found for the requested time range!" — and it is the same string whether the group has never received data or has data that is not yet aggregated.
Emulator: a per-request sentence instead — "no profile has been posted to profiling group <name> in the requested time range".
Why: the code is right (404 ResourceNotFoundException), but the constant AWS returns is what distinguishes "no profile data in range" from "group not found" for a client that cannot tell the two 404s apart by code alone; the emulator's text makes the two indistinguishable in a different way and never emits the constant.
Fix: GetProfile's no-aggregated-data path — return the fixed service message for an empty time range.
Traces: codeguruprofiler.get-profile-no-data-rejected, codeguruprofiler.get-profile-without-data-not-found, codeguruprofiler.get-profile-no-aggregated-data-yet.

Shared pointers for all causes: constraint-violation rendering lives in the service's shared validate step (Causes 1-5); the per-operation texts live in the handlers under services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/ (e.g. the not-found message built at post_agent_profile.rs:172-182 is the same "no profiling group named {}" template as Causes 6-7) and in crate::state.

## Evidence
Each line below is (emulator actual, AWS expected) from the replay's assertion tuples.

Cause 1 — create-profiling-group-invalid-name-pattern-rejected / create-profiling-group-invalid-name-rejected:
  actual:   "...at 'profilingGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w-]+$"
  expected: "...at 'profilingGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+"

Cause 2 — configure-agent-fleet-instance-id-too-long-rejected:
  actual:   "1 validation error detected: Value 'i-0000…0000... [302 bytes]' at 'fleetInstanceId' failed to satisfy constraint: Member must have length less than or equal to 255"
  expected: same message with the full 302-character value echoed, no "[302 bytes]" marker

Cause 3 — add-notification-channels-invalid-event-publisher-rejected:
  actual:   "Value 'NotARealPublisher' at 'channels.1.member.eventPublishers.1.member' failed to satisfy constraint: Member must satisfy enum value set: [AnomalyDetection]"
  expected: "Value '[NotARealPublisher]' at 'channels.1.member.eventPublishers' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [AnomalyDetection]]"

Cause 4 — list-profile-times-invalid-period-rejected:
  actual:   "Value 'P2D' at 'period' ... enum value set: [PT5M, PT1H, P1D]"
  expected: "Value 'P2D' at 'period' ... enum value set: [P1D, PT1H, PT5M]"

Cause 5 — put-permission-invalid-action-group-rejected:
  actual:   "Value 'bogusPermissions' at 'actionGroup' ... enum value set: [agentPermissions]"
  expected: "Value 'bogusPermissions' at 'actionGroup' ... enum value set: [internalAgentPermissions, agentPermissions]"

Cause 6 — describe/delete not-found (4 checks):
  actual:   'no profiling group named sim2real-cgp-nosuch-4e95407142d5'
  expected: 'CodeGuru Profiler can\u2019t complete the operation because sim2real-cgp-nosuch-4e95407142d5 was not found.'

Cause 7 — configure-agent-nonexistent-group-not-found:
  actual:   'no profiling group named sim2real-cgp-noagent-962c43b82c99'
  expected: 'Profiling group for 000000000000/sim2real-cgp-noagent-962c43b82c99 does not exist!'

Cause 8 — create-profiling-group-duplicate-name(-conflict)-rejected:
  actual:   'a profiling group named sim2real-cgp-4e544744e337 already exists'
  expected: 'ProfilingGroup (name: sim2real-cgp-4e544744e337, accountId: 000000000000) already exists.'

Cause 9 — get-profile no-data (3 checks), code ResourceNotFoundException matched:
  actual:   'no profile has been posted to profiling group sim2real-cgp-2dc45fb613af in the requested time range'
  expected: 'No profiles found for the requested time range!'

## Steps to Reproduce
Boot the emulator, point a botocore codeguruprofiler client at it, and inspect Error.Message on each refusal (the Code and HTTP status already match in every case):
1. Cause 1: CreateProfilingGroup(profilingGroupName="has a space", clientToken=uuid4()) -> ValidationException whose pattern is rendered anchored.
2. Cause 2: ConfigureAgent(profilingGroupName=<existing>, fleetInstanceId="i-"+"0"*300) -> message truncates the value and appends "[302 bytes]".
3. Cause 3: AddNotificationChannels(profilingGroupName=<existing>, channels=[{"uri": <sns-arn>, "eventPublishers": ["NotARealPublisher"]}]) -> element path instead of list-member path.
4. Cause 4: ListProfileTimes(profilingGroupName=<existing>, startTime=.., endTime=.., period="P2D") -> enum set in model order.
5. Cause 5: PutPermission(profilingGroupName=<existing>, actionGroup="bogusPermissions", principals=[<arn>]) -> enum set missing internalAgentPermissions.
6. Cause 6: DescribeProfilingGroup / DeleteProfilingGroup on a never-created name.
7. Cause 7: ConfigureAgent on a never-created name.
8. Cause 8: CreateProfilingGroup twice with the same profilingGroupName.
9. Cause 9: CreateProfilingGroup, then GetProfile(profilingGroupName=name, period="PT5M").
Or replay the checks listed in trace_ids.
