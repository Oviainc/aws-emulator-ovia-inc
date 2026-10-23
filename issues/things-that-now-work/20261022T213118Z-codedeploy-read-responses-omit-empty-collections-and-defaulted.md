Issue-Source: trace-sweep-divergence
Issue-Trace: codedeploy-deployment-group-onprem-tag-filter-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codedeploy-deployment-group-onprem-tag-filter-lifecycle codedeploy-application-revision-registration-and-lookup codedeploy-get-onprem-instance-not-registered-rejected codedeploy-on-premises-instance-registration-and-tagging
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|GetApplicationRevision|2xx->2xx|$.revisionInfo.deploymentGroups
Issue-Key: codedeploy|GetOnPremisesInstance|2xx->2xx|$.instanceInfo.tags
Issue-Related: 20261022T213118Z-codedeploy-mutating-operations-do-not-update-stored-state.md
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# CodeDeploy read responses omit empty collections and defaulted members

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeploymentGroup

## Description
Three wire divergences where the emulator answers HTTP 200 with a correctly shaped but UNDER-POPULATED body: collections that AWS always serializes as an empty list are omitted, and server-side defaults AWS always materializes are missing. Three read paths, three causes.

Cause 1 of 3: GetDeploymentGroup omits every unset collection and every defaulted member of deploymentGroupInfo.
AWS: Smithy GetDeploymentGroup -> deploymentGroupInfo (DeploymentGroupInfo) declaring ec2TagFilters (EC2TagFilterList), autoScalingGroups (AutoScalingGroupList), triggerConfigurations (TriggerConfigList), deploymentStyle (DeploymentStyle) and outdatedInstancesStrategy (OutdatedInstancesStrategy, documented as behaving like UPDATE when unspecified). Recorded behavior for a Server-platform group created with none of these set: AWS returns the keys present — ec2TagFilters [], autoScalingGroups [], triggerConfigurations [], deploymentStyle {'deploymentOption': 'WITHOUT_TRAFFIC_CONTROL', 'deploymentType': 'IN_PLACE'}, outdatedInstancesStrategy 'UPDATE', bakeTime 0 and managedHookConfig {'executionRoleArn': 'NONE'}.
Emulator: all seven members are absent from deploymentGroupInfo.
Why: a client indexing info['ec2TagFilters'] or info['deploymentStyle'] raises KeyError against the emulator where AWS always supplies a value, and absent-vs-empty comparisons invert — drift detection and IaC refresh report spurious changes on every deployment group.
Fix: the GetDeploymentGroup serializer / deployment-group projection — materialize server-side defaults at create time and serialize unset collections as empty lists, services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/.
Traces: codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#5. (The same omission recurs at #task#7 of that fixture, whose own cause — an un-applied UpdateDeploymentGroup — is filed in the state issue alongside this one.)

Cause 2 of 3: GetApplicationRevision omits revisionInfo.deploymentGroups.
AWS: Smithy GetApplicationRevision -> revisionInfo (GenericRevisionInfo) declaring `deploymentGroups` (DeploymentGroupsList, "The deployment groups for which this is the current target revision"). For a freshly registered revision that is no group's target, AWS returns the key present with value [].
Emulator: the `deploymentGroups` key is absent from revisionInfo.
Why: a client that reads revisionInfo['deploymentGroups'] to decide whether a revision is in use raises KeyError instead of seeing the empty list, so "unused revision" cannot be distinguished from a malformed response.
Fix: the GetApplicationRevision serializer / revision projection (empty-list normalization for deploymentGroups).
Traces: codedeploy-application-revision-registration-and-lookup#task#4.

Cause 3 of 3: GetOnPremisesInstance omits instanceInfo.tags for an untagged instance.
AWS: Smithy GetOnPremisesInstance -> instanceInfo (InstanceInfo) declaring `tags` (TagList, "The tags currently associated with the on-premises instance"). For an instance registered with no tags, AWS returns the key present with value [].
Emulator: the `tags` key is absent from instanceInfo.
Why: tag-driven code (deployment-group target selection, cleanup sweeps) must special-case a missing key the real API never sends, and an untagged instance is indistinguishable from an unparsable response.
Fix: the GetOnPremisesInstance serializer / instance projection (empty-list normalization for tags).
Traces: codedeploy-get-onprem-instance-not-registered-rejected#task#3 (same cluster also at codedeploy-on-premises-instance-registration-and-tagging#task#2).

## Evidence
Replayed wire corpus simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0). All three clusters are HTTP 200 vs HTTP 200 — status and shape are right, the body is under-populated. Expected = recorded AWS response, Actual = emulator.

1. codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#5 GetDeploymentGroup — missing: $.deploymentGroupInfo.autoScalingGroups (expected []), .ec2TagFilters (expected []), .triggerConfigurations (expected []), .bakeTime (expected 0), .deploymentStyle (expected {'deploymentOption': 'WITHOUT_TRAFFIC_CONTROL', 'deploymentType': 'IN_PLACE'}), .managedHookConfig (expected {'executionRoleArn': 'NONE'}), .outdatedInstancesStrategy (expected 'UPDATE').
2. codedeploy-application-revision-registration-and-lookup#task#4 GetApplicationRevision — missing: $.revisionInfo.deploymentGroups (expected []).
3. codedeploy-get-onprem-instance-not-registered-rejected#task#3 GetOnPremisesInstance — missing: $.instanceInfo.tags (expected []). Same cluster at codedeploy-on-premises-instance-registration-and-tagging#task#2.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): DeploymentGroupInfo declares ec2TagFilters/autoScalingGroups/triggerConfigurations (lists), deploymentStyle and outdatedInstancesStrategy (enum, documented to behave as UPDATE when unspecified); GenericRevisionInfo declares deploymentGroups (DeploymentGroupsList); InstanceInfo declares tags (TagList). The default VALUES above are recorded behavior only (the model marks no @default on these members); the requirement is that the member is present with the server-side default rather than omitted.

Related open issue that does NOT cover these steps: the behavioral "CodeDeploy deployment lifecycle, response normalization and error classification diverge from AWS (6 causes)" raises the same class for onPremisesInstanceTagFilters and autoRollbackConfiguration only, and carries no wire replay key for these fixtures.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus) — all three clusters reproduce.
2. Cause 1: CreateApplication(app); CreateDeploymentGroup(applicationName=app, deploymentGroupName=dg, serviceRoleArn=<valid role>, onPremisesInstanceTagFilters=[one filter]) with ec2TagFilters/autoScalingGroups/triggerConfigurations/deploymentStyle/outdatedInstancesStrategy all omitted; GetDeploymentGroup(app, dg) -> expect deploymentGroupInfo to contain ec2TagFilters [], autoScalingGroups [], triggerConfigurations [], bakeTime 0, deploymentStyle {'deploymentOption':'WITHOUT_TRAFFIC_CONTROL','deploymentType':'IN_PLACE'}, managedHookConfig {'executionRoleArn':'NONE'}, outdatedInstancesStrategy 'UPDATE'.
3. Cause 2: RegisterApplicationRevision(applicationName=app, revision=<S3 revision>); GetApplicationRevision(app, same revision) -> expect revisionInfo['deploymentGroups'] == [].
4. Cause 3: RegisterOnPremisesInstance(instanceName=H, iamUserArn=...) with no tags; GetOnPremisesInstance(H) -> expect instanceInfo['tags'] == [].
