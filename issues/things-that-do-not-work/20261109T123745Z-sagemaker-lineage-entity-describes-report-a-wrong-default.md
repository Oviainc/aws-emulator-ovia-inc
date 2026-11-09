Issue-Source: trace-sweep-divergence
Issue-Trace: api-experiment-trial-component-lifecycle
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-experiment-trial-component-lifecycle sagemaker-trial-component-metric-numeric-precision-preserved api-lineage-entities-lifecycle sagemaker-artifact-source-uri-binary-safe-round-trip sagemaker-action-lifecycle-create-describe-delete sagemaker-action-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.LineageGroupArn
Issue-Key: sagemaker|DescribeArtifact|2xx->2xx|$.LineageGroupArn
Issue-Key: sagemaker|DescribeAction|2xx->2xx|$.LineageGroupArn
Issue-Key: sagemaker|DescribeContext|2xx->2xx|$.LineageGroupArn
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# SageMaker lineage-entity describes report a wrong default lineage-group ARN

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrialComponent

## Description
4 causes, all one shared fault: every lineage-aware Describe operation renders the account's implicit default lineage group ARN from the region/account pair instead of from the group's name, so LineageGroupArn names a lineage group that does not exist. The fault belongs in whatever builds the default lineage-group ARN (shared by all four handlers / the lineage state), not in four separate serializers.

Cause 1 of 4 - DescribeTrialComponent.LineageGroupArn.
AWS: LineageGroupArn is a modeled output member (Smithy: DescribeTrialComponent output member LineageGroupArn -> LineageGroupArn, a string with @length max 256 and @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:lineage-group/). The resource segment after 'lineage-group/' is the lineage group's NAME. An entity created without an explicit lineage group is associated with the account's default lineage group, whose name is the fixed literal 'sagemaker-default-lineage-group'; the recording shows exactly that ARN shape - arn:aws:sagemaker:<region>:<account>:lineage-group/sagemaker-default-lineage-group - where region and account are taken from the request's endpoint and the caller's account and the trailing group name is fixed text.
Emulator: returns arn:aws:sagemaker:<region>:<account>:lineage-group/sagemaker-<region>-<account> - i.e. it substitutes a region-and-account-derived string for the group name. The prefix is correct and the @pattern still matches, so the defect is invisible to shape validation.
Why: a client that resolves the returned LineageGroupArn - DescribeLineageGroup, GetLineageGroupPolicy, QueryLineage, or a tag/permission operation on that ARN - is sent to a lineage group name the account does not have, and gets a ResourceNotFound for a group that does exist under its real name. Lineage traversal from any trial component therefore dead-ends.
Fix: the lineage-group ARN construction used by the lineage/experiment state (the default lineage group's name must be the fixed 'sagemaker-default-lineage-group', not a region/account-derived string); the four Describe handlers should keep reading it from there.
Traces: api-experiment-trial-component-lifecycle#task#12 (also task#14, and sagemaker-trial-component-metric-numeric-precision-preserved#task#2).

Cause 2 of 4 - DescribeArtifact.LineageGroupArn, same wrong default group name.
AWS: Smithy: DescribeArtifact output member LineageGroupArn -> LineageGroupArn (@length max 256, @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:lineage-group/); the recorded response carries the default group ARN ending in the fixed name 'sagemaker-default-lineage-group'.
Emulator: ends the ARN in the region/account-derived 'sagemaker-<region>-<account>' instead.
Why: as cause 1 - lineage traversal from an artifact resolves a non-existent group.
Fix: same shared default-lineage-group ARN construction.
Traces: api-lineage-entities-lifecycle#task#4 (also task#6, and sagemaker-artifact-source-uri-binary-safe-round-trip#task#2).

Cause 3 of 4 - DescribeAction.LineageGroupArn, same wrong default group name.
AWS: Smithy: DescribeAction output member LineageGroupArn -> LineageGroupArn (same constraints); recorded response ends in the fixed 'sagemaker-default-lineage-group'.
Emulator: ends in 'sagemaker-<region>-<account>'.
Why: as cause 1 - lineage traversal from an action resolves a non-existent group.
Fix: same shared default-lineage-group ARN construction.
Traces: api-lineage-entities-lifecycle#task#8 (also task#10, sagemaker-action-lifecycle-create-describe-delete#task#2, sagemaker-action-lifecycle-transitions#task#2).

Cause 4 of 4 - DescribeContext.LineageGroupArn, same wrong default group name.
AWS: Smithy: DescribeContext output member LineageGroupArn -> LineageGroupArn (same constraints); recorded response ends in the fixed 'sagemaker-default-lineage-group'.
Emulator: ends in 'sagemaker-<region>-<account>'.
Why: as cause 1 - lineage traversal from a context resolves a non-existent group.
Fix: same shared default-lineage-group ARN construction.
Traces: api-lineage-entities-lifecycle#task#12 (also task#14).

No open issue covers this: the nearest ones, 20261029T192819Z-sagemaker-lifecycle-identity-and-round-trip-fidelity-defects.md and 20261102T105242Z-sagemaker-describe-responses-omit-members-present-in-the.md, are about identity (CreatedBy) and about omitted members, not about the value of the default lineage-group ARN, and the replay reports these four clusters as unowned.

## Evidence
All four clusters are value_mismatch on the same JSON path with the same substitution, status 200 -> 200:

1. api-experiment-trial-component-lifecycle#task#12 DescribeTrialComponent - $.LineageGroupArn: expected 'arn:aws:sagemaker:us-west-2:000000000000:lineage-group/sagemaker-default-lineage-group', got 'arn:aws:sagemaker:us-west-2:000000000000:lineage-group/sagemaker-us-west-2-000000000000' (divergence_key service=sagemaker|DescribeTrialComponent|status=200->200|keys=$.LineageGroupArn; 3 affected steps).
2. api-lineage-entities-lifecycle#task#4 DescribeArtifact - identical expected/actual pair (divergence_key service=sagemaker|DescribeArtifact|status=200->200|keys=$.LineageGroupArn; 3 affected steps).
3. api-lineage-entities-lifecycle#task#8 DescribeAction - identical expected/actual pair (divergence_key service=sagemaker|DescribeAction|status=200->200|keys=$.LineageGroupArn; 4 affected steps).
4. api-lineage-entities-lifecycle#task#12 DescribeContext - identical expected/actual pair (divergence_key service=sagemaker|DescribeContext|status=200->200|keys=$.LineageGroupArn; 2 affected steps).

In the expected ARN, 'arn:aws:sagemaker:<region>:<account>:lineage-group/' is the modeled prefix (region and account come from the endpoint and the caller) and 'sagemaker-default-lineage-group' is FIXED text naming the account's default lineage group. The us-west-2/000000000000 values above are this run's region and account and are not themselves a requirement; the trailing group name is.

## Steps to Reproduce
1. Replay fixture api-experiment-trial-component-lifecycle. At task#12 the trial component is created without an explicit lineage group and then described; read $.LineageGroupArn - it ends in 'sagemaker-<region>-<account>' where the recording has 'sagemaker-default-lineage-group'.
2. Replay fixture api-lineage-entities-lifecycle and read $.LineageGroupArn from task#4 (DescribeArtifact), task#8 (DescribeAction) and task#12 (DescribeContext): the same substitution appears in all three.
3. Take any returned LineageGroupArn and call DescribeLineageGroup on the name after 'lineage-group/': the emulator-produced name resolves to no group, the recorded one does.
