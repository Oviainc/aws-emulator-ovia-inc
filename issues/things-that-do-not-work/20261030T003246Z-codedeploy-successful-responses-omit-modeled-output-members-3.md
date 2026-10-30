Issue-Source: trace-sweep-divergence
Issue-Trace: codedeploy-application-revision-registration-and-lookup
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codedeploy-application-revision-registration-and-lookup codedeploy-deployment-group-lookup-and-validation-errors codedeploy-deployment-group-onprem-tag-filter-lifecycle
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|BatchGetApplicationRevisions|2xx->2xx|$.errorMessage
Issue-Key: codedeploy|DeleteDeploymentGroup|2xx->2xx|$
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.bakeTime
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.managedHookConfig
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# CodeDeploy successful responses omit modeled output members (3 causes)

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeploymentGroup

## Description
Three CodeDeploy read/delete operations answer HTTP 200 with the right status but a response document that is missing members AWS always serializes. In every case the emulator emits a narrower body than the service, so an SDK caller gets None/absent where AWS gives a value (including empty-but-present values). Three independent serialization sites, three causes.

Cause 1 of 3: BatchGetApplicationRevisions omits the top-level errorMessage member.
AWS: Smithy BatchGetApplicationRevisionsOutput models three members — applicationName, errorMessage (target ErrorMessage, a plain string) and revisions (RevisionInfoList). Recorded behavior: a successful BatchGetApplicationRevisions for an existing application with resolvable revisions answers HTTP 200 and serializes errorMessage as an empty string (the member is present and empty when no per-batch error occurred); its value is not a per-run value — it is '' whenever the batch succeeded.
Emulator: HTTP 200 with errorMessage entirely absent from the JSON body ($.errorMessage missing; expected '', got None).
Why: a client that reads response['errorMessage'] (or any generated model that treats the member as always-present) raises a KeyError / NoSuchElement instead of seeing the empty no-error string, so batch-revision lookups fail in code that works against AWS.
Fix: the BatchGetApplicationRevisions handler / output serializer in the codedeploy service crate — populate errorMessage ('' on success, the batch error text otherwise).
Traces: codedeploy-application-revision-registration-and-lookup#task#6.

Cause 2 of 3: DeleteDeploymentGroup returns an empty body instead of the modeled hooksNotCleanedUp list.
AWS: Smithy DeleteDeploymentGroup declares DeleteDeploymentGroupOutput with member hooksNotCleanedUp (target AutoScalingGroupList). Recorded behavior: deleting a deployment group answers HTTP 200 with {"hooksNotCleanedUp": []} — the list member is always serialized, empty when no Auto Scaling lifecycle hooks were left behind.
Emulator: HTTP 200 with a completely empty body {} ($: empty_body; expected {'hooksNotCleanedUp': []}, got {}).
Why: tooling that iterates hooksNotCleanedUp after deleting a group (to warn operators about Auto Scaling hooks it must clean up itself) iterates None and crashes, rather than seeing the empty list that means 'nothing left behind'.
Fix: the DeleteDeploymentGroup handler / output serializer — always emit hooksNotCleanedUp, defaulting to an empty list.
Traces: codedeploy-deployment-group-lookup-and-validation-errors#task#5.

Cause 3 of 3: GetDeploymentGroup omits deploymentGroupInfo.bakeTime and deploymentGroupInfo.managedHookConfig.
AWS: recorded behavior only — the on-disk Smithy model's DeploymentGroupInfo structure (com.amazonaws.codedeploy#DeploymentGroupInfo) carries no bakeTime and no managedHookConfig member, so these come from a newer service model revision than the checked-out one. The recording shows GetDeploymentGroup answering HTTP 200 for a freshly created Server/on-premises deployment group with deploymentGroupInfo.bakeTime present (0 for a group that configured no bake time) and deploymentGroupInfo.managedHookConfig present as {"executionRoleArn": "NONE"} (the sentinel AWS reports when the group has no managed-hook execution role). Both appear for every deployment group AWS returns, not only for groups that set them, and the same two members are missing on the group-lookup step that follows creation (task#7 of the same fixture).
Emulator: HTTP 200 and an otherwise matching deploymentGroupInfo, but both members absent ($.deploymentGroupInfo.bakeTime missing, expected 0; $.deploymentGroupInfo.managedHookConfig missing, expected {'executionRoleArn': 'NONE'}).
Why: a client built from a current SDK model reads these two members off every deployment group it describes; against the emulator they are unset, so bake-time and managed-hook configuration look unconfigured/unknown rather than explicitly 'none', and round-tripping a described group back through CreateDeploymentGroup/UpdateDeploymentGroup drops the fields.
Fix: the deployment-group state representation plus the GetDeploymentGroup output serializer — carry bakeTime and managedHookConfig on the stored group and always serialize them (0 / {executionRoleArn: NONE} defaults).
Traces: codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#5, codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#7.

No open issue covers these three serialization sites. The open wire issue 'CodeDeploy accepts invalid input and misclassifies rejections' concerns missing input gates and wrong error codes, and 'CodeDeploy rejection paths return the right error codes with the wrong error messages' concerns error text — neither touches success-path response shape.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against this checkout (14 fixtures, 127 steps, 112 passed, score 88.2%, emulator_up=true, bootstrap_signal=false). Expected = recorded AWS response, Actual = emulator response.

1. codedeploy-application-revision-registration-and-lookup#task#6 — BatchGetApplicationRevisions: expected HTTP 200, actual HTTP 200; $.errorMessage: missing (expected '', got None).
2. codedeploy-deployment-group-lookup-and-validation-errors#task#5 — DeleteDeploymentGroup: expected HTTP 200, actual HTTP 200; $: empty_body (expected {'hooksNotCleanedUp': []}, got {}).
3. codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#5 (and #task#7, same cluster) — GetDeploymentGroup: expected HTTP 200, actual HTTP 200; $.deploymentGroupInfo.bakeTime: missing (expected 0, got None); $.deploymentGroupInfo.managedHookConfig: missing (expected {'executionRoleArn': 'NONE'}, got None).

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): BatchGetApplicationRevisionsOutput = {applicationName, errorMessage: ErrorMessage, revisions: RevisionInfoList}; DeleteDeploymentGroupOutput = {hooksNotCleanedUp: AutoScalingGroupList}; GetDeploymentGroupOutput = {deploymentGroupInfo: DeploymentGroupInfo}, whose member list in this model revision does NOT include bakeTime or managedHookConfig (cause 3 is therefore grounded in the recording, not the on-disk model).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1; the three clusters reproduce.
2. Cause 1: CreateApplication(applicationName=A), RegisterApplicationRevision(A, a revision), then BatchGetApplicationRevisions(applicationName=A, revisions=[that revision]) and inspect the top-level errorMessage member (fixture codedeploy-application-revision-registration-and-lookup, task#6).
3. Cause 2: CreateApplication + CreateDeploymentGroup, then DeleteDeploymentGroup(applicationName, deploymentGroupName) and inspect the response body for hooksNotCleanedUp (fixture codedeploy-deployment-group-lookup-and-validation-errors, task#5).
4. Cause 3: CreateApplication(Server) + CreateDeploymentGroup with onPremisesInstanceTagFilters, then GetDeploymentGroup on it and inspect deploymentGroupInfo.bakeTime and deploymentGroupInfo.managedHookConfig (fixture codedeploy-deployment-group-onprem-tag-filter-lifecycle, task#5 and task#7).
