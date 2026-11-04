Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.list-experiment-resolved-targets-reflects-server-side-resolution
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis.list-experiment-resolved-targets-reflects-server-side-resolution|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: ff0601f42ed824b0a3725f8a9bff62aa4ebc3901

# FIS tag-identified targets never resolve: ListExperimentResolvedTargets reports an empty resolution forever, so a resourceTags experiment's resolution never becomes observable

> _Note: this issue was identified against the code base at commit `ff0601f42ed824b0a3725f8a9bff62aa4ebc3901`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExperimentResolvedTargets

## Description
One root cause (Cause 1 of 1).

Related open issue: 20261024T000450Z-fis-listexperimentresolvedtargets-wrong-behavior.md describes the same `resolve_targets` omission, but it does NOT carry this check's replay key (the suite did not mark this check OWNED), so this failing check is filed here and must be re-replayed against any fix of that issue.

Cause 1 of 1: a target that identifies its resources by `resourceTags` (optionally narrowed by `filters`) resolves to nothing, so ListExperimentResolvedTargets answers 200 with an empty `resolvedTargets` list for the whole life of the experiment and a client that polls for the resolution never sees it appear.

AWS: Smithy (services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json) models ExperimentTemplateTarget with `resourceType`, `resourceArns`, `resourceTags` ("The tags for the target resources"), `filters` ("The filters to apply to identify target resources using specific attributes") and `selectionMode` ("Scopes the identified resources to a specific count or percentage"); ListExperimentResolvedTargets (GET /experiments/{experimentId}/resolvedTargets, 200) returns `resolvedTargets: list<ResolvedTarget{resourceType, targetName, targetInformation}>` with `nextToken` paging, errors ResourceNotFoundException (404) and ValidationException (400). Real FIS resolves a tag-identified target at experiment start against the resources in the account/region that carry those tags, narrows by `filters`, scopes by `selectionMode`, and then reports one ResolvedTarget per resolved resource: `resourceType` = the target's resource type (e.g. `aws:ec2:instance`), `targetName` = the template's target key, and `targetInformation` naming the resolved resource (its id/ARN). While that resolution is still in flight AWS refuses the read with ValidationException (HTTP 400) and the message `Target resolution is ongoing. Retry after target resolution ends.` — fixed text, no variable parts — and once resolution has ended it answers 200 with the resolved rows. Resources that do NOT carry the tag are never resolved: a same-type, same-subnet instance without the tag must be absent from the result.

Emulator: the resolution snapshot is computed once at StartExperiment by `resolve_targets`, which walks ONLY `target.resource_arns`; its own doc comment states that a target identifying resources by `resourceTags` "identifies nothing here". `ListExperimentResolvedTargets` then just filters/pages `experiment.resolved_targets`, so for a tag-identified target it returns 200 with `resolvedTargets: []` at every instant — never the in-flight ValidationException, and never a resolved row. The replayed check launches one tagged and one untagged EC2 instance, creates a template whose single target is `{resourceType: aws:ec2:instance, resourceTags: {<key>: <value>}, selectionMode: ALL}`, starts the experiment and polls the resolution for a non-empty list; because the emulator's list is permanently empty the poll never satisfies its predicate and the check is killed at the harness timeout (reported as "Check timed out after 60.0s") instead of asserting on one resolved target.

Why: a client that drives FIS the documented way — define targets by tags, start the experiment, then read ListExperimentResolvedTargets to learn WHICH resources the fault will hit — gets an empty answer that is indistinguishable from "nothing matched", and a poll loop waiting for the resolution hangs until its own deadline. Any logic that asserts the blast radius (exactly the tagged resources, never the untagged ones) before/while the experiment runs cannot work against the emulator, and the consequential `experimentOptions.emptyTargetResolutionMode` decision is taken on a resolution that is empty only because resolution was never performed.

Fix: the experiment-start target resolution in the FIS state layer — `resolve_targets` must resolve `resourceTags` (and apply `filters`, then `selectionMode`) against the emulator's own resource stores for the target's `resourceType` (the crate already reaches another service's store, it resolves the execution role against the IAM store at StartExperiment), and the read path in the ListExperimentResolvedTargets handler must report the in-flight ValidationException with AWS's wording until that resolution has ended rather than returning an empty 200.

Traces: fis.list-experiment-resolved-targets-reflects-server-side-resolution.

## Evidence
Check fis.list-experiment-resolved-targets-reflects-server-side-resolution — verdict [mismatch], "Check timed out after 60.0s".

Expected (AWS): after StartExperiment on a template whose only target is {resourceType: 'aws:ec2:instance', resourceTags: {<tagKey>: <tagValue>}, selectionMode: 'ALL'}, ListExperimentResolvedTargets eventually answers 200 with exactly ONE ResolvedTarget: resourceType == 'aws:ec2:instance', targetName == the template's target key ('taggedInstance' in the check), targetInformation referencing the tagged instance id and NOT the untagged decoy instance id. Before resolution ends the same call is refused with HTTP 400, code ValidationException, message 'Target resolution is ongoing. Retry after target resolution ends.' (entirely fixed text).

Actual (emulator): HTTP 200 with resolvedTargets == [] on every call, indefinitely — never the in-flight ValidationException and never a resolved row. The check's poll helper (_list_resolved_targets_or_pending + _poll_until, predicate len(targets) >= 1) therefore never completes and the check is terminated at the 60s harness deadline.

Code: services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs, `pub fn resolve_targets(...)` (around line 2955) iterates `target.resource_arns` only; its doc comment at lines ~2944-2947 states a target identifying resources by `resourceTags` "identifies nothing here, and nothing is reported for it". services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_experiment_resolved_targets.rs lines 169-207 only filter and page `experiment.resolved_targets`, with no in-flight state.

The instance ids, tag value, template id and experiment id in the recording are per-run values and are not part of the requirement; the requirement is: exactly the tag-matching resources resolve, the untagged decoy does not, and the pre-resolution read is the ValidationException above.

## Steps to Reproduce
1. Boot the emulator.
2. Launch two EC2 instances of the same type into the same subnet via the emulated EC2 service: one tagged {sim2real-fis-target: <value>}, one with no such tag. Wait until both report State.Name == 'running'.
3. Create an IAM role trusted by fis.amazonaws.com with ec2:RebootInstances/ec2:DescribeInstances allowed.
4. fis.create_experiment_template(stopConditions=[{source:'none'}], targets={'taggedInstance': {'resourceType': 'aws:ec2:instance', 'resourceTags': {'sim2real-fis-target': '<value>'}, 'selectionMode': 'ALL'}}, actions={'rebootAction': {'actionId': 'aws:ec2:reboot-instances', 'targets': {'Instances': 'taggedInstance'}}}, roleArn=<role arn>).
5. fis.start_experiment(experimentTemplateId=<template id>) and note the experiment id.
6. Poll fis.list_experiment_resolved_targets(experimentId=<experiment id>) — observe HTTP 200 with resolvedTargets == [] on every call, forever, instead of (a) ValidationException 'Target resolution is ongoing. Retry after target resolution ends.' while resolution is in flight and then (b) exactly one ResolvedTarget naming the tagged instance and not the decoy.
