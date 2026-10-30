Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-group-resources-type-mismatch-roundtrip
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-group-resources-type-mismatch-roundtrip resource-groups-search-resources-and-list-groups
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|GroupResources|2xx->2xx|$.Failed[*]
Issue-Key: resource-groups|GroupResources|2xx->2xx|$.Succeeded[*]
Issue-Key: resource-groups|ListGroupResources|2xx->2xx|$.QueryErrors
Issue-Key: resource-groups|ListGroupResources|2xx->2xx|$.ResourceIdentifiers[*]
Issue-Key: resource-groups|ListGroupResources|2xx->2xx|$.Resources[*]
Issue-Key: resource-groups|UngroupResources|2xx->2xx|$.Failed[*]
Issue-Key: resource-groups|UngroupResources|2xx->2xx|$.Succeeded[*]
Issue-Key: resource-groups|SearchResources|2xx->2xx|$.QueryErrors
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Unsupported resource types are grouped and listed instead of failing validation

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GroupResources

## Description
This issue bundles 4 causes with one root: the emulator does not validate a resource ARN against the group's configuration type before admitting it, and it omits the per-request QueryErrors list from the two query-evaluating responses. All four must be closed before this issue is submitted.

Cause 1 of 4: GroupResources admits an ARN the group's configuration does not support.
AWS: Smithy: GroupResourcesOutput = {Succeeded: list of ResourceArn, Failed: list of FailedResource{ResourceArn, ErrorMessage (1..1024), ErrorCode (1..128)}, Pending}. Recorded response: HTTP 200 where the ARN whose resource type is not allowed by the target group's configuration appears in Failed with ErrorCode 'ResourceArnValidationException' and ErrorMessage 'Unsupported resource ARN', and does NOT appear in Succeeded. The ErrorCode and ErrorMessage are fixed text for this error class; the ResourceArn is the one from the request.
Emulator: HTTP 200 with the same ARN in $.Succeeded[0] and no $.Failed entry at all — the type mismatch is not detected and the membership is accepted.
Why: a client that checks Failed/Succeeded to learn whether a resource may join a capacity-reservation-pool style group is told the resource was added; it then expects the resource to be a member and to be subject to the group's constraints, and its negative-path handling for unsupported types is never exercised.
Fix: the GroupResources handler's per-ARN validation against the group's configuration type, and the Failed-list construction in its response.
Traces: resource-groups-group-resources-type-mismatch-roundtrip#task#3.

Cause 2 of 4: ListGroupResources reports the wrongly admitted resource and omits QueryErrors.
AWS: Smithy: ListGroupResourcesOutput = {Resources, ResourceIdentifiers (deprecated), NextToken, QueryErrors: list of QueryError{ErrorCode (enum CLOUDFORMATION_STACK_INACTIVE | CLOUDFORMATION_STACK_NOT_EXISTING | CLOUDFORMATION_STACK_UNASSUMABLE_ROLE | RESOURCE_TYPE_NOT_SUPPORTED), Message}}. Recorded response: HTTP 200 with no members for the group (because the unsupported ARN was rejected at GroupResources time) and QueryErrors present as an empty list.
Emulator: HTTP 200 listing the resource in $.Resources[0] and $.ResourceIdentifiers[0] (a membership that should never have existed, see cause 1) and omitting $.QueryErrors entirely instead of serializing [].
Why: a client enumerating members sees a resource the real service would never have admitted, and a client that reads QueryErrors without a null check (the member is always present on the wire) fails on the missing key.
Fix: ListGroupResources' response serializer (always emit QueryErrors, empty list when there are none); the phantom membership follows from cause 1.
Traces: resource-groups-group-resources-type-mismatch-roundtrip#task#4.

Cause 3 of 4: UngroupResources reports success for an ARN that is not a valid member.
AWS: Smithy: UngroupResourcesOutput = {Succeeded, Failed: list of FailedResource, Pending}. Recorded response: HTTP 200 with the ARN in Failed carrying ErrorCode 'ResourceArnValidationException' and ErrorMessage 'Unsupported resource ARN' (fixed text for the class; ResourceArn taken from the request), and nothing in Succeeded.
Emulator: HTTP 200 with the ARN in $.Succeeded[0] and no $.Failed entry.
Why: the remove path mirrors the add path, so a client cannot distinguish 'removed' from 'was never removable'.
Fix: the UngroupResources handler's per-ARN validation and Failed-list construction. Related but not covering: open issue 20261029T013351Z-resource-groups-ungroupresources-wrong-behavior--r2 concerns an empty Succeeded list the model does not admit, not the misclassification of an unsupported ARN.
Traces: resource-groups-group-resources-type-mismatch-roundtrip#task#5.

Cause 4 of 4: SearchResources omits QueryErrors.
AWS: Smithy: SearchResourcesOutput = {ResourceIdentifiers, NextToken, QueryErrors: list of QueryError}. Recorded response: HTTP 200 with QueryErrors present as an empty list when the query raised no errors.
Emulator: HTTP 200 with $.QueryErrors absent.
Why: same wire-shape breakage as cause 2 — a client that iterates the QueryErrors member on every search response hits a missing key rather than an empty list.
Fix: SearchResources' response serializer (always emit QueryErrors).
Traces: resource-groups-search-resources-and-list-groups#task#3.

Related but not covering: open issue 20261024T072908Z-resource-groups-listgroupresources-wrong-behavior concerns ResourceQuery evaluation for membership, not resource-type validation or the QueryErrors member. The ARNs and group names in the recordings are per-run values.

## Evidence
Replay of simparity-wire-aws-v1 (all verdict=mismatch, expected_status == actual_status == 200):
- resource-groups-group-resources-type-mismatch-roundtrip#task#3 GroupResources: $.Succeeded[0] extra (got 'arn:aws:sqs:us-west-2:000000000000:sim2real-probe-rg-grm-queue-2fcdc6b5'), $.Failed[0] missing (expected {'ResourceArn': <that arn>, 'ErrorMessage': 'Unsupported resource ARN', 'ErrorCode': 'ResourceArnValidationException'}).
- resource-groups-group-resources-type-mismatch-roundtrip#task#4 ListGroupResources: $.Resources[0] extra ({'Identifier': {'ResourceArn': <that arn>, 'ResourceType': 'AWS::SQS::Queue'}}), $.ResourceIdentifiers[0] extra, $.QueryErrors missing (expected []).
- resource-groups-group-resources-type-mismatch-roundtrip#task#5 UngroupResources: $.Succeeded[0] extra (<that arn>), $.Failed[0] missing (expected ErrorCode 'ResourceArnValidationException', ErrorMessage 'Unsupported resource ARN').
- resource-groups-search-resources-and-list-groups#task#3 SearchResources: $.QueryErrors missing (expected []).

## Steps to Reproduce
1. Replay fixture resource-groups-group-resources-type-mismatch-roundtrip: a group whose configuration does not allow SQS queues is created, then GroupResources is called with a queue ARN (task#3) — the queue must come back under Failed with ErrorCode ResourceArnValidationException and ErrorMessage 'Unsupported resource ARN', not under Succeeded.
2. Step task#4: ListGroupResources on that group must list no members and must include QueryErrors: [].
3. Step task#5: UngroupResources with the same ARN must likewise report it under Failed.
4. Replay fixture resource-groups-search-resources-and-list-groups step task#3: the SearchResources 200 body must include QueryErrors: [].
