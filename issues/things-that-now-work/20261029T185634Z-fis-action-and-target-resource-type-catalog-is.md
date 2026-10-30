Issue-Source: trace-sweep-divergence
Issue-Trace: fis-action-and-resource-type-catalog
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-action-and-resource-type-catalog fis-action-pagination-single-page-boundary fis-catalog-list-actions fis-get-action-not-found-error fis-list-and-get-action fis-catalog-list-target-resource-types fis-list-target-resource-types-max-results-validation-error fis-list-and-get-target-resource-type
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|GetAction|2xx->2xx|$.action.arn
Issue-Key: fis|GetAction|2xx->2xx|$.action.description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn
Issue-Key: fis|GetAction|2xx->4xx|$.action
Issue-Key: fis|GetAction|2xx->4xx|status
Issue-Key: fis|GetAction|2xx->2xx|$.action.parameters.availabilityZoneIdentifier
Issue-Key: fis|GetAction|2xx->2xx|$.action.parameters.duration
Issue-Key: fis|GetAction|2xx->2xx|$.action.targets.ManagedResources
Issue-Key: fis|ListTargetResourceTypes|2xx->2xx|$.targetResourceTypes[*]
Issue-Key: fis|GetTargetResourceType|2xx->2xx|$.targetResourceType.description
Issue-Key: fis|GetTargetResourceType|2xx->2xx|$.targetResourceType.parameters.managedResourceTypes
Issue-Key: fis|GetTargetResourceType|2xx->2xx|$.targetResourceType.parameters.zonalAutoshiftStatus
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# FIS action and target-resource-type catalog is incomplete and ARNs omit the account id

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAction, ListActions, GetTargetResourceType, ListTargetResourceTypes

## Description
The emulator's built-in FIS action catalog and target-resource-type catalog are not the catalogs FIS publishes: entries are missing, the descriptions of the entries that exist are reworded, and the ARNs the catalog emits have an empty account-id segment. Because both catalogs are read-only reference data served identically on every call, each divergence below is a defect in that seeded data (and in the ARN formatter that renders it), not in one recording. 7 causes.

Cause 1 of 7 (GetAction, action ARN has no account id, and the aws:ec2:reboot-instances description is reworded).
AWS: Smithy: GetAction -> GetActionResponse.action : Action; Action.arn targets ResourceArn (@length 20..2048, @pattern ^[\S]+$) and Action.description targets ActionDescription. An action ARN is a full FIS ARN whose account-id field is the account serving the request, i.e. arn:aws:fis:<region>:<account-id>:action/<actionId>; the recorded response shows the account-id field populated. Action.description is the published catalog text for that action, which for aws:ec2:reboot-instances reads "Reboot the specified EC2 instances."
Emulator: renders $.action.arn with the account-id field EMPTY — arn:aws:fis:<region>::action/<actionId> (recorded: expected 'arn:aws:fis:us-west-2:000000000000:action/aws:ec2:reboot-instances', got 'arn:aws:fis:us-west-2::action/aws:ec2:reboot-instances'; here 'aws:ec2:reboot-instances' is the requested id and the region/account are environment values, the requirement is that the account field is non-empty and holds the serving account) — and returns the description 'Reboot the target EC2 instances.' instead of 'Reboot the specified EC2 instances.' (fixed catalog text, no variable part).
Why: a client that parses the ARN to recover the owning account reads an empty account and either fails ARN validation (the field is required by ARN syntax) or attributes the action to no account; a client or snapshot test that asserts on the published action description sees text FIS never emits.
Fix: the action-catalog seed data (description strings) and the ARN construction used by the action serializer, which must interpolate the serving account id.
Traces: fis-action-and-resource-type-catalog#task#2.

Cause 2 of 7 (ListActions omits published actions, e.g. aws:arc:start-zonal-autoshift).
AWS: Smithy: ListActions -> ListActionsResponse.actions : ActionSummaryList, paginated (items=actions). The list contains EVERY action FIS publishes; the recording includes the ARC zonal-autoshift action with its targets map ({'ManagedResources': {'resourceType': 'aws:arc:zonal-shift-managed-resource'}}) and tags {}.
Emulator: that element is absent from $.actions entirely (recorded: expected the aws:arc:start-zonal-autoshift summary, got None). The same omission shows up on the single-page boundary fixture, so it is missing from the seeded catalog rather than lost to pagination.
Why: a client enumerating available fault-injection actions cannot discover the action at all, so an experiment template that legitimately uses it can never be built from discovery; a pagination test that counts the catalog gets the wrong total.
Fix: the action-catalog seed data served by the ListActions handler.
Traces: fis-action-pagination-single-page-boundary#task#1 (also fis-action-pagination-single-page-boundary#task#2, fis-catalog-list-actions#task#1, fis-get-action-not-found-error#task#2).

Cause 3 of 7 (GetAction 404s on a published action id).
AWS: Smithy: GetAction, errors ResourceNotFoundException (404) / ValidationException (400); 200 returns the Action for any published action id. aws:arc:start-zonal-autoshift is published, with parameters availabilityZoneIdentifier (required true) and duration (required true) and the ManagedResources target.
Emulator: answers 404 with no $.action (recorded: status expected 200, got 404).
Why: a client that looks up a real action by id is told the action does not exist, so any flow gated on that lookup aborts against a service that would have succeeded. This is the same missing catalog entry as cause 2 observed through the point-read path.
Fix: the action-catalog seed data the GetAction handler looks up.
Traces: fis-get-action-not-found-error#task#3.

Cause 4 of 7 (GetAction on the first listed action returns a different action's body; ARN again account-less).
AWS: the ListActions ordering and the GetAction body agree, so a read-after-list on actions[0] returns that action's own description, parameters and targets, with a fully qualified ARN.
Emulator: the body it returns carries neither the expected description nor the expected parameters/targets, and its ARN is again missing the account id (recorded: $.action.arn expected an account-qualified ARN, got 'arn:aws:fis:us-west-2::action/...'; $.action.description, $.action.parameters.availabilityZoneIdentifier, $.action.parameters.duration and $.action.targets.ManagedResources all diverge, the parameter and target entries being absent). Hypothesis: with the ARC entry missing from the catalog (causes 2 and 3) every positional index shifts, so the action the fixture reaches through actions[0] is not the one AWS put there.
Why: a client that lists then reads the first action gets fields belonging to another action — parameters it must supply are reported absent, so a template built from them is invalid.
Fix: the action-catalog seed data (completeness and ordering) plus the ARN construction in the action serializer.
Traces: fis-list-and-get-action#task#2.

Cause 5 of 7 (ListTargetResourceTypes omits published resource types, so the whole list is shifted).
AWS: Smithy: ListTargetResourceTypes -> targetResourceTypes : TargetResourceTypeSummaryList, paginated. The list holds every published target resource type, including aws:arc:zonal-shift-managed-resource, aws:directconnect:virtual-interface and aws:dsql:cluster, each with its published description.
Emulator: those entries are absent and every surviving entry appears at a lower index (recorded at $.targetResourceTypes[0..23]: expected 'aws:arc:zonal-shift-managed-resource' at [0], got 'aws:dynamodb:global-table'; expected 'aws:directconnect:virtual-interface' at [1], got 'aws:ec2:autoscaling-group'; expected 'aws:dsql:cluster' at [2], got 'aws:ec2:ebs-volume'; and so on through the tail of the list). Descriptions of the entries that do exist also differ in their fixed text (e.g. the DynamoDB global-table description's trailing period).
Why: a client discovering which resource types it may target cannot see several supported types, and any index- or count-based assertion over the catalog is wrong.
Fix: the target-resource-type catalog seed data served by the ListTargetResourceTypes handler.
Traces: fis-catalog-list-target-resource-types#task#1.

Cause 6 of 7 (the first target-resource-type element is the wrong type).
AWS: targetResourceTypes[0] is aws:arc:zonal-shift-managed-resource, description 'An ARC Zonal Shift Managed Resource.'
Emulator: returns aws:dynamodb:global-table at index 0 (recorded: expected the ARC entry, got {'resourceType': 'aws:dynamodb:global-table', ...}). Same missing-entry shift as cause 5, seen on the maxResults fixture's successful call.
Why: a client that reads the head of the list, or pages it, lands on a different resource type than the service would hand it.
Fix: the target-resource-type catalog seed data.
Traces: fis-list-target-resource-types-max-results-validation-error#task#2.

Cause 7 of 7 (GetTargetResourceType returns another type's body and drops its parameters).
AWS: Smithy: GetTargetResourceType -> targetResourceType : TargetResourceType. For aws:arc:zonal-shift-managed-resource the body carries description 'An ARC Zonal Shift Managed Resource.' and the parameters map managedResourceTypes (required true) and zonalAutoshiftStatus (required false).
Emulator: returns the DynamoDB global-table description and no parameters map entries (recorded: $.targetResourceType.description expected 'An ARC Zonal Shift Managed Resource.', got 'An Amazon DynamoDB global table.'; $.targetResourceType.parameters.managedResourceTypes and .zonalAutoshiftStatus expected present, got None).
Why: a client reading a target resource type after listing it gets another type's metadata and is told the type takes no parameters, so it omits parameters the real service requires.
Fix: the target-resource-type catalog seed data (entries, ordering and their parameters maps) read by the GetTargetResourceType handler.
Traces: fis-list-and-get-target-resource-type#task#2.

## Evidence
Cause 1 — fis-action-and-resource-type-catalog#task#2, GetAction 200/200: $.action.arn expected 'arn:aws:fis:us-west-2:000000000000:action/aws:ec2:reboot-instances', got 'arn:aws:fis:us-west-2::action/aws:ec2:reboot-instances'; $.action.description expected 'Reboot the specified EC2 instances.', got 'Reboot the target EC2 instances.'
Cause 2 — fis-action-pagination-single-page-boundary#task#1, ListActions 200/200: $.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:arc:start-zonal-autoshift"] expected present, got None (4 steps affected).
Cause 3 — fis-get-action-not-found-error#task#3, GetAction: status expected 200, got 404; $.action expected the aws:arc:start-zonal-autoshift Action, got None.
Cause 4 — fis-list-and-get-action#task#2, GetAction 200/200: arn account field empty; $.action.description, $.action.parameters.availabilityZoneIdentifier, $.action.parameters.duration, $.action.targets.ManagedResources all diverge (parameters/targets absent).
Cause 5 — fis-catalog-list-target-resource-types#task#1, ListTargetResourceTypes 200/200: 22 index keys diverge; expected [0] aws:arc:zonal-shift-managed-resource / [1] aws:directconnect:virtual-interface / [2] aws:dsql:cluster, got aws:dynamodb:global-table / aws:ec2:autoscaling-group / aws:ec2:ebs-volume.
Cause 6 — fis-list-target-resource-types-max-results-validation-error#task#2: $.targetResourceTypes[0] expected the ARC entry, got aws:dynamodb:global-table.
Cause 7 — fis-list-and-get-target-resource-type#task#2, GetTargetResourceType 200/200: description expected 'An ARC Zonal Shift Managed Resource.', got 'An Amazon DynamoDB global table.'; parameters.managedResourceTypes and parameters.zonalAutoshiftStatus expected present, got None.
Smithy cited: Action.arn -> ResourceArn @length 20..2048 @pattern ^[\S]+$; ListActions/ListTargetResourceTypes @paginated (items=actions / targetResourceTypes); GetAction errors ResourceNotFoundException(404), ValidationException(400).

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: GET /actions/aws:ec2:reboot-instances — inspect $.action.arn's account-id field and $.action.description (fis-action-and-resource-type-catalog#task#2).
3. Cause 2: GET /actions and look for the element with id aws:arc:start-zonal-autoshift (fis-action-pagination-single-page-boundary#task#1).
4. Cause 3: GET /actions/aws:arc:start-zonal-autoshift — expect 200, observe 404 (fis-get-action-not-found-error#task#3).
5. Cause 4: GET /actions, then GET /actions/{actions[0].id} and compare description/parameters/targets (fis-list-and-get-action#task#2).
6. Causes 5-6: GET /targetResourceTypes and compare the list head and ordering (fis-catalog-list-target-resource-types#task#1, fis-list-target-resource-types-max-results-validation-error#task#2).
7. Cause 7: GET /targetResourceTypes/aws:arc:zonal-shift-managed-resource and compare description and parameters (fis-list-and-get-target-resource-type#task#2).
