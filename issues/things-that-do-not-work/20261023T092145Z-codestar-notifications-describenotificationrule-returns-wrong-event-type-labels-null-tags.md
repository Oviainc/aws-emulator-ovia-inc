Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-notifications-create-rule-explicit-disabled-status
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-notifications-create-rule-explicit-disabled-status codestar-notifications-rule-lifecycle codestar-notifications-update-rule-empty-targets-clears-all codestar-notifications-target-subscribe-unsubscribe
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications|DescribeNotificationRule|2xx->2xx|$.EventTypes[*].EventTypeName
Issue-Key: codestar-notifications|DescribeNotificationRule|2xx->2xx|$.EventTypes[*].ResourceType
Issue-Key: codestar-notifications|DescribeNotificationRule|2xx->2xx|$.Tags
Issue-Key: codestar-notifications|DescribeNotificationRule|2xx->2xx|$.Targets[*].TargetStatus
Issue-Key: codestar-notifications|DescribeNotificationRule|2xx->2xx|$.EventTypes[*]
Issue-Key: codestar-notifications|DescribeNotificationRule|2xx->2xx|$.Targets[*]
Issue-Commit: aae04825aca42e5b620901be4ebc2f4e42bc7c15

# DescribeNotificationRule returns wrong event-type labels, null Tags and stale target state

> _Note: this issue was identified against the code base at commit `aae04825aca42e5b620901be4ebc2f4e42bc7c15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNotificationRule

## Description
DescribeNotificationRule answers 200 with a correctly shaped body, but several members of DescribeNotificationRuleResult carry the wrong content: the EventTypeSummary labels come from a different catalog than AWS's, the Tags map is omitted instead of serialized as an empty map, target state never leaves PENDING, and a target removed with Unsubscribe is still reported. This bundle has 4 causes; all four must be closed.

Cause 1 of 4 - event-type catalog labels, missing Tags map and TargetStatus, on a rule created with an explicit DISABLED status (fixture codestar-notifications-create-rule-explicit-disabled-status, step task#7).
AWS: Smithy: DescribeNotificationRule -> DescribeNotificationRuleResult with members EventTypes (EventTypeBatch of EventTypeSummary{EventTypeId, ServiceName, EventTypeName, ResourceType}), Tags (map Tags), Targets (TargetsBatch of TargetSummary{TargetAddress, TargetType, TargetStatus}); TargetStatus is an enum with values PENDING|ACTIVE|UNREACHABLE|INACTIVE|DEACTIVATED; ResourceType is constrained only by pattern ^([a-zA-Z0-9-])+$, so the model does not fix its casing. Recorded behavior only for the catalog values and the settled state: the recording returns EventTypeName 'Deployment: Succeeded' and ResourceType 'Application' for the CodeDeploy event type, an empty Tags map {} for an untagged rule, and TargetStatus ACTIVE for the SNS target once the rule exists.
Emulator: returns EventTypeName 'Deployment succeeded' and ResourceType 'application', omits Tags entirely (null rather than {}), and reports TargetStatus 'PENDING'.
Why: the event-type label and resource type are the values a client displays and the values it round-trips back into CreateNotificationRule/UpdateNotificationRule EventTypeIds lookups, so a client matching on AWS's catalog text finds nothing. A null Tags map makes a client that iterates the map without a null check fail on an untagged rule instead of iterating zero entries. And a target that is permanently PENDING never satisfies a client that waits for ACTIVE before treating the rule as usable, so such a client hangs or times out on a rule AWS considers ready.
Fix: the DescribeNotificationRule handler and its serializer, plus the event-type catalog the state layer seeds: the catalog entries need AWS's EventTypeName/ResourceType spelling, Tags must serialize as an empty map when the rule has no tags, and target subscription state must settle to ACTIVE rather than remain at its initial PENDING.
Traces: codestar-notifications-create-rule-explicit-disabled-status#task#7.

Cause 2 of 4 - the same three divergences on the lifecycle rule, over a CodeCommit event type (fixture codestar-notifications-rule-lifecycle, step task#5).
AWS: Smithy as in cause 1. Recorded behavior only: EventTypeSummary for EventTypeId 'codecommit-repository-comments-on-commits' is {EventTypeName 'Comments: On commits', ResourceType 'Repository', ServiceName 'CodeCommit'}, Tags is {}, and the target is ACTIVE.
Emulator: EventTypeName 'Comments on commits', ResourceType 'repository', Tags absent, TargetStatus 'PENDING'. The cluster also covers the describe after update (codestar-notifications-rule-lifecycle#task#7) and codestar-notifications-update-rule-empty-targets-clears-all#task#5, so the wrong labels persist across an update, not just at create time.
Why: same client impact as cause 1, and it shows the catalog defect is not specific to one event type - every entry in the seeded catalog is affected.
Fix: same components as cause 1 (event-type catalog seed plus DescribeNotificationRule serialization).
Traces: codestar-notifications-rule-lifecycle#task#5, codestar-notifications-rule-lifecycle#task#7, codestar-notifications-update-rule-empty-targets-clears-all#task#5.

Cause 3 of 4 - a target removed with Unsubscribe is still listed by DescribeNotificationRule (fixture codestar-notifications-target-subscribe-unsubscribe, step task#9).
AWS: Smithy: Unsubscribe(UnsubscribeRequest{Arn, TargetAddress}) -> UnsubscribeResult, documented as 'Removes an association between a notification rule and an Amazon Q Developer in chat applications topic...'. Recorded behavior: after Unsubscribe of the second SNS topic, DescribeNotificationRule returns exactly one entry in Targets.
Emulator: returns two entries - the unsubscribed SNS target is still present as {TargetType 'SNS', TargetStatus 'PENDING'} - alongside the same wrong EventTypes labels, absent Tags and PENDING status described above.
Why: Unsubscribe silently does nothing to the rule's target set, so a client that unsubscribes a topic and re-reads the rule still sees it associated; any logic that reconciles desired against actual targets loops or re-issues Unsubscribe forever, and a client reasoning about notification fan-out believes a removed destination is still attached.
Fix: the Unsubscribe handler and the rule state it mutates - the target must be detached from the rule's target list (and that removal reflected by DescribeNotificationRule/ListTargets), not merely accepted with a 200.
Traces: codestar-notifications-target-subscribe-unsubscribe#task#9.

Cause 4 of 4 - wrong event-type labels and missing Tags map after an update that clears all targets (fixture codestar-notifications-update-rule-empty-targets-clears-all, step task#7).
AWS: Smithy as in cause 1. Recorded behavior only: EventTypeSummary for 'codecommit-repository-branches-and-tags-created' is {EventTypeName 'Branches and tags: Created', ResourceType 'Repository', ServiceName 'CodeCommit'} and Tags is {}.
Emulator: EventTypeName 'Branches and tags created', ResourceType 'repository', Tags absent.
Why: as in cause 1 - the describe after a target-clearing update is the point a client confirms the rule's final shape, and it reads non-AWS catalog text and a null tag map there.
Fix: same components as cause 1; this cause exists to confirm the fix holds on the post-update describe path as well as the post-create one.
Traces: codestar-notifications-update-rule-empty-targets-clears-all#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 47.3%, 49 mismatched steps, bootstrap_signal false, state_reset_failures 0).

Cause 1 - codestar-notifications-create-rule-explicit-disabled-status#task#7, DescribeNotificationRule, status expected 200 / actual 200:
  $.EventTypes[0].EventTypeName: expected 'Deployment: Succeeded', got 'Deployment succeeded'
  $.EventTypes[0].ResourceType: expected 'Application', got 'application'
  $.Tags: expected {}, got None (member missing)
  $.Targets[0].TargetStatus: expected 'ACTIVE', got 'PENDING'

Cause 2 - codestar-notifications-rule-lifecycle#task#5, DescribeNotificationRule, 200/200:
  $.EventTypes[0]: expected {'EventTypeId': 'codecommit-repository-comments-on-commits', 'EventTypeName': 'Comments: On commits', 'ResourceType': 'Repository', 'ServiceName': 'CodeCommit'}, got {'EventTypeId': 'codecommit-repository-comments-on-commits', 'ServiceName': 'CodeCommit', 'EventTypeName': 'Comments on commits', 'ResourceType': 'repository'}
  $.Tags: expected {}, got None
  $.Targets[0].TargetStatus: expected 'ACTIVE', got 'PENDING'
  Cluster members: codestar-notifications-rule-lifecycle#task#5, codestar-notifications-rule-lifecycle#task#7, codestar-notifications-update-rule-empty-targets-clears-all#task#5.

Cause 3 - codestar-notifications-target-subscribe-unsubscribe#task#9, DescribeNotificationRule, 200/200:
  $.Targets[1]: extra (expected None, got {'TargetAddress': 'arn:aws:sns:...:<second topic>', 'TargetType': 'SNS', 'TargetStatus': 'PENDING'}) - i.e. the unsubscribed target remains
  plus $.EventTypes[0] label/casing divergence, $.Tags expected {} got None, $.Targets[0].TargetStatus expected 'ACTIVE' got 'PENDING'

Cause 4 - codestar-notifications-update-rule-empty-targets-clears-all#task#7, DescribeNotificationRule, 200/200:
  $.EventTypes[0]: expected {... 'EventTypeName': 'Branches and tags: Created', 'ResourceType': 'Repository' ...}, got {... 'EventTypeName': 'Branches and tags created', 'ResourceType': 'repository' ...}
  $.Tags: expected {}, got None

Model grounding: smithy_query operation=DescribeNotificationRule / closure on services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json - DescribeNotificationRuleResult.Tags: map Tags; TargetSummary.TargetStatus: enum TargetStatus [PENDING, ACTIVE, UNREACHABLE, INACTIVE, DEACTIVATED]; EventTypeSummary{EventTypeId, ServiceName, EventTypeName, ResourceType (pattern ^([a-zA-Z0-9-])+$)}; Unsubscribe -> UnsubscribeResult, documented as removing the rule/target association. Hosts, ports, account ids, generated rule ids/ARNs and topic names in the quoted bodies are environment-specific examples, not requirements.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus); no fixture setup beyond the corpus is needed.
3. Cause 1: replay codestar-notifications-create-rule-explicit-disabled-status and inspect step task#7 - CreateNotificationRule with a CodeDeploy event type and an SNS target, Status DISABLED, then POST /describeNotificationRule with the returned Arn. Compare EventTypes[0].EventTypeName/ResourceType, Tags and Targets[0].TargetStatus against the recorded response.
4. Cause 2: replay codestar-notifications-rule-lifecycle and inspect step task#5 (and task#7, after the update) - same comparison over the CodeCommit event type 'codecommit-repository-comments-on-commits'.
5. Cause 3: replay codestar-notifications-target-subscribe-unsubscribe and inspect step task#9 - after Subscribe of a second SNS topic and Unsubscribe of it, POST /describeNotificationRule and count Targets; the emulator returns 2, AWS returns 1.
6. Cause 4: replay codestar-notifications-update-rule-empty-targets-clears-all and inspect step task#7 - UpdateNotificationRule with an empty Targets list, then describe; compare EventTypes[0] and Tags.
7. All four steps answer 200 with the right members present, so diff the member VALUES, not the status.
