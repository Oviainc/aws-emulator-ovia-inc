Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-error-config-query-conflict
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-error-config-query-conflict resource-groups-group-resources-wrong-target-rejected
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|GetGroupConfiguration|4xx->4xx|$.Message
Issue-Key: resource-groups|PutGroupConfiguration|4xx->4xx|$.Message
Issue-Key: resource-groups|GetGroupQuery|4xx->4xx|$.Message
Issue-Key: resource-groups|UpdateGroupQuery|4xx->4xx|$.Message
Issue-Key: resource-groups|GroupResources|4xx->4xx|$.Message
Issue-Key: resource-groups|UngroupResources|4xx->4xx|$.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# GroupResources and config/query conflict errors reply with non-AWS Message text

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGroupConfiguration

## Description
This issue bundles 6 causes. All six are the SAME defect in a different handler: when an operation is applied to a group whose shape does not support it (a configuration-only group asked for its query, a query-only group asked for its configuration, or GroupResources/UngroupResources pointed at a group whose configuration type does not allow explicit membership), AWS answers 400 BadRequestException with the single fixed sentence 'This operation does not support the target'. The emulator answers 400 BadRequestException too — status and error code are correct in every case — but substitutes its own hand-written diagnostic prose for the Message. The Message is part of the error contract, so each of the six must be made to emit AWS's sentence. All six must be closed before this issue is submitted.

Cause 1 of 6: GetGroupConfiguration on a group that has only a ResourceQuery.
AWS: Smithy: GetGroupConfiguration -> BadRequestException (httpError 400, member Message targeting ErrorMessage, length 1..1024). Recorded response: HTTP 400 with $.Message exactly 'This operation does not support the target'. The whole string is FIXED text — it contains no group name, no ARN and no variable part at all.
Emulator: HTTP 400 (correct) but $.Message is 'the group <group-name> has no service configuration' — invented prose that additionally interpolates the group name, a value AWS never puts in this message.
Why: a client that matches the documented AWS message (log scrapers, retry/branch logic, assertion-based tests, and SDK-level error presentation) sees an unrecognised string, and in a tool that compares error text the emulator looks like a different service.
Fix: the GetGroupConfiguration handler's not-supported-target branch, where the BadRequestException Message is constructed.
Traces: resource-groups-error-config-query-conflict#task#3.

Cause 2 of 6: PutGroupConfiguration on a group that is driven by a ResourceQuery.
AWS: Smithy: PutGroupConfiguration -> BadRequestException (400). Recorded response: HTTP 400, $.Message exactly 'This operation does not support the target' (entirely fixed text).
Emulator: HTTP 400 with $.Message 'a resource group can contain either a Configuration or a ResourceQuery, but not both, and the group <group-name> is queried by <query-type>' — invented prose interpolating the group name and the query type.
Why: same as cause 1; the reason for the rejection is correct but the wire contract is not what a client reads.
Fix: the PutGroupConfiguration handler's configuration/query conflict branch.
Traces: resource-groups-error-config-query-conflict#task#4.

Cause 3 of 6: GetGroupQuery on a group that has only a Configuration.
AWS: Smithy: GetGroupQuery -> BadRequestException (400). Recorded response: HTTP 400, $.Message exactly 'This operation does not support the target'.
Emulator: HTTP 400 with $.Message 'the group <group-name> has no resource query'.
Why: same as cause 1.
Fix: the GetGroupQuery handler's not-supported-target branch.
Traces: resource-groups-error-config-query-conflict#task#5.

Cause 4 of 6: UpdateGroupQuery on a group that has only a Configuration.
AWS: Smithy: UpdateGroupQuery -> BadRequestException (400). Recorded response: HTTP 400, $.Message exactly 'This operation does not support the target'.
Emulator: HTTP 400 with $.Message 'a resource group can contain either a ResourceQuery or a Configuration, but not both, and the group <group-name> is configured as <configuration-type>'.
Why: same as cause 1.
Fix: the UpdateGroupQuery handler's configuration/query conflict branch.
Traces: resource-groups-error-config-query-conflict#task#6.

Cause 5 of 6: GroupResources against a group whose configuration type does not permit explicit membership.
AWS: Smithy: GroupResources -> BadRequestException (400). Recorded response: HTTP 400, $.Message exactly 'This operation does not support the target'.
Emulator: HTTP 400 with $.Message 'GroupResources can only be used with a group whose configuration is one of AWS::EC2::CapacityReservationPool, AWS::EC2::HostManagement, AWS::ResourceGroups::ApplicationGroup; other resource group types are not supported by this operation' — invented prose that also leaks the emulator's internal allow-list.
Why: same as cause 1.
Fix: the GroupResources handler's target-eligibility check.
Traces: resource-groups-group-resources-wrong-target-rejected#task#2.

Cause 6 of 6: UngroupResources against a group whose configuration type does not permit explicit membership.
AWS: Smithy: UngroupResources -> BadRequestException (400). Recorded response: HTTP 400, $.Message exactly 'This operation does not support the target'.
Emulator: HTTP 400 with $.Message 'UngroupResources can only be used with a group whose configuration is one of AWS::EC2::CapacityReservationPool, AWS::EC2::HostManagement, AWS::ResourceGroups::ApplicationGroup; other resource group types are not supported by this operation'.
Why: same as cause 1.
Fix: the UngroupResources handler's target-eligibility check.
Traces: resource-groups-group-resources-wrong-target-rejected#task#3.

No open issue covers this: the existing resource-groups issues (20261029T013351Z-resource-groups-ungroupresources-wrong-behavior--r2, 20261029T094111Z-resource-groups-listgroupingstatuses-validation) concern response shapes and Filters bounds, not error Message text.

## Evidence
Replay of simparity-wire-aws-v1, all six steps verdict=mismatch with expected_status == actual_status == 400 and the only diverging key $.Message:
- resource-groups-error-config-query-conflict#task#3 GetGroupConfiguration: expected 'This operation does not support the target', got 'the group sim2real-probe-resource-groups-rg-querygroup-17e03429 has no service configuration'
- resource-groups-error-config-query-conflict#task#4 PutGroupConfiguration: expected 'This operation does not support the target', got 'a resource group can contain either a Configuration or a ResourceQuery, but not both, and the group ... is queried by TAG_FILTERS_1_0'
- resource-groups-error-config-query-conflict#task#5 GetGroupQuery: expected 'This operation does not support the target', got '... has no resource query'
- resource-groups-error-config-query-conflict#task#6 UpdateGroupQuery: expected 'This operation does not support the target', got 'a resource group can contain either a ResourceQuery or a Configuration, but not both, ... is configured as AWS::EC2::CapacityReservationPool'
- resource-groups-group-resources-wrong-target-rejected#task#2 GroupResources: expected 'This operation does not support the target', got 'GroupResources can only be used with a group whose configuration is one of ...'
- resource-groups-group-resources-wrong-target-rejected#task#3 UngroupResources: expected 'This operation does not support the target', got 'UngroupResources can only be used with a group whose configuration is one of ...'
The group names, query types and configuration types quoted above are per-run values from these recordings; the requirement is the fixed sentence with no interpolation at all.

## Steps to Reproduce
1. Replay fixture resource-groups-error-config-query-conflict and compare steps task#3, task#4, task#5 and task#6: each returns 400 with the correct error type but a non-AWS $.Message.
2. Replay fixture resource-groups-group-resources-wrong-target-rejected and compare steps task#2 and task#3: same pattern for GroupResources/UngroupResources.
3. Equivalently: CreateGroup a group with only a ResourceQuery, then call GetGroupConfiguration/PutGroupConfiguration on it; CreateGroup a group with only a Configuration, then call GetGroupQuery/UpdateGroupQuery on it; and call GroupResources/UngroupResources on a tag-query group. All six must answer 400 BadRequestException with $.Message == 'This operation does not support the target'.
