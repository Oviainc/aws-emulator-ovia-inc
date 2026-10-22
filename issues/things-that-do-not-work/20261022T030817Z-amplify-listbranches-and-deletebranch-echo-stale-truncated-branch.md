Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-branch-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-branch-lifecycle amplify-job-start-and-manage amplify-manual-deployment-flow
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|ListBranches|2xx->2xx|$.branches[*]
Issue-Key: amplify|DeleteBranch|2xx->2xx|$.branch.stage
Issue-Key: amplify|DeleteBranch|2xx->2xx|$.branch.ttl
Issue-Key: amplify|DeleteBranch|2xx->2xx|$.branch.enableAutoBuild
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Amplify ListBranches and DeleteBranch echo stale truncated Branch records

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBranches

## Description
The two Amplify branch operations that are NOT the create/get read path - ListBranches and DeleteBranch - serve a Branch record that is both stale (it does not reflect members the caller supplied to CreateBranch/UpdateBranch) and truncated/misprojected (modeled Branch members AWS always returns are absent, and null-valued ones are replaced by empty placeholders). Both operations re-serialize whatever the shared branch store holds, so both inherit the same defect from a single Branch projection + state path. This issue bundles 3 causes, each its own divergence cluster with its own fixture step; all three must be closed. Related open issues cover the create/update and get/create projections but their replay pointers do NOT name these steps, so these clusters are unowned: 20261022T003601Z-amplify-discards-request-fields-on-create-and-update.md (CreateBranch/UpdateBranch state loss) and 20261022T003601Z-amplify-app-and-branch-responses-misproject-modeled-members.md (CreateBranch/GetBranch/DeleteApp projection).

Cause 1 of 3 - ListBranches returns a Branch list element that is stale AND structurally short (fixture amplify-branch-lifecycle, step task#6).
AWS: Smithy ListBranches (GET /apps/{appId}/branches, 200) -> ListBranchesResult.branches @required, a list of com.amazonaws.amplify#Branch with the SAME shape GetBranch returns. Branch marks description, stage, displayName, enableNotification, createTime, updateTime, environmentVariables, enableAutoBuild, customDomains, framework, activeJobId, totalNumberOfJobs, enableBasicAuth, ttl and enablePullRequestPreview @required, and models the optional members associatedResources, backendEnvironmentArn, backend (Backend{stackArn}), basicAuthCredentials, buildSpec, computeRoleArn, destinationBranch, enableSkewProtection, pullRequestEnvironmentName, sourceBranch and thumbnailUrl. The recording shows AWS answering 200 with a FULL element: every optional member present (null where unset), backend present as {"stackArn": null}, and stage/ttl carrying the values the preceding UpdateBranch set (recorded example stage BETA, ttl "10").
Emulator: 200, but the element is reported simultaneously missing and extra because it carries only a subset of the modeled members - associatedResources, backendEnvironmentArn, basicAuthCredentials, buildSpec, computeRoleArn, destinationBranch, enableSkewProtection, pullRequestEnvironmentName, sourceBranch and thumbnailUrl are absent entirely; backend is serialized as {} instead of {"stackArn": null}; and stage/ttl are the creation-time values (recorded example DEVELOPMENT / "5") rather than the updated ones.
Why: list and get disagree about the same branch, so a client that lists branches and compares them against a GetBranch (or against the state it just wrote) sees spurious drift; SDK deserializers that require the @required members on a list element fail outright; and a caller dereferencing branch.backend.stackArn on a list element gets undefined where AWS gives an explicit null.
Fix: the ListBranches handler's response serializer - project each list element through the same Branch projection as GetBranch (emitting every modeled member, null for unset, backend seeded with stackArn) and read from the post-update stored branch rather than a creation-time snapshot.
Traces: amplify-branch-lifecycle#task#6.

Cause 2 of 3 - DeleteBranch returns the pre-update branch for stage and ttl (fixture amplify-branch-lifecycle, step task#7).
AWS: Smithy DeleteBranch (DELETE /apps/{appId}/branches/{branchName}, 200) -> DeleteBranchResult.branch @required, target Branch; Branch.stage (Stage enum) and Branch.ttl (TTL, @length 0-32, pattern ^\\d*$) are both @required. The recording shows AWS answering 200 and echoing the branch as it stood at deletion - i.e. including the stage/ttl the preceding UpdateBranch set (recorded example stage BETA, ttl "10").
Emulator: 200, but $.branch.stage is DEVELOPMENT (expected BETA) and $.branch.ttl is "5" (expected "10") - the delete response is built from the creation-time branch.
Why: DeleteBranch's echoed body is the last observation a client gets of a resource it can no longer read; a client that records it for audit, or that confirms a promotion took effect by deleting and inspecting the returned record, is told the branch was never promoted.
Fix: the DeleteBranch handler's state lookup/serializer - echo the stored branch at its current revision instead of a stale snapshot (this surfaces the same stored-state gap the related create/update issue reports, but on the delete path, which that issue's replay pointer does not exercise).
Traces: amplify-branch-lifecycle#task#7.

Cause 3 of 3 - DeleteBranch reports enableAutoBuild false for a branch created with enableAutoBuild true (fixture amplify-job-start-and-manage, step teardown#9).
AWS: Smithy DeleteBranch -> DeleteBranchResult.branch @required; Branch.enableAutoBuild is a @required boolean. The recording shows AWS answering 200 with enableAutoBuild true, matching what CreateBranch was given earlier in the same fixture.
Emulator: 200 with $.branch.enableAutoBuild false - the seeded default, not the provisioned value.
Why: the delete response misreports the branch's build configuration, so a client reconciling or re-creating the branch from that record recreates it with auto-build off.
Fix: the DeleteBranch handler's state lookup/serializer, over a branch record that must have been initialized from the CreateBranch request members.
Traces: amplify-job-start-and-manage#teardown#9 (cluster also affects amplify-manual-deployment-flow#teardown#5).

## Evidence
Replay of simparity-wire-aws-v1 against this checkout: emulator_up true, 7 fixtures / 47 steps, 21 passed, 24 mismatched, 2 blocked, score 44.7%; state_reset_failures 0, so no fixture ran against a previous fixture's state and these diffs are not reset artifacts. bootstrap_signal false (the service answers). No status or error code differs on any cause: all three are 200 -> 200 body divergences.

Cause 1 - amplify-branch-lifecycle#task#6, ListBranches, expected 200 / actual 200:
  $.branches[0]: missing AND extra (same element reported both ways)
  expected element keys include: activeJobId, associatedResources, backend {"stackArn": null}, backendEnvironmentArn, basicAuthCredentials, branchArn, branchName, buildSpec, computeRoleArn, createTime, customDomains, description, destinationBranch, displayName, enableAutoBuild, enableBasicAuth, enableNotification, enablePerformanceMode, enablePullRequestPreview, enableSkewProtection, environmentVariables, framework, pullRequestEnvironmentName, sourceBranch, stage=BETA, tags, thumbnailUrl, totalNumberOfJobs, ttl="10", updateTime
  actual element omits associatedResources, backendEnvironmentArn, basicAuthCredentials, buildSpec, computeRoleArn, destinationBranch, enableSkewProtection, pullRequestEnvironmentName, sourceBranch, thumbnailUrl; emits backend as {}; and carries stage=DEVELOPMENT, ttl="5"

Cause 2 - amplify-branch-lifecycle#task#7, DeleteBranch, expected 200 / actual 200:
  $.branch.stage: value_mismatch (expected 'BETA', got 'DEVELOPMENT')
  $.branch.ttl: value_mismatch (expected '10', got '5')

Cause 3 - amplify-job-start-and-manage#teardown#9, DeleteBranch, expected 200 / actual 200:
  $.branch.enableAutoBuild: value_mismatch (expected True, got False)
  cluster also affects amplify-manual-deployment-flow#teardown#5

Model references used: Smithy ListBranches GET /apps/{appId}/branches -> ListBranchesResult.branches @required (list of Branch, @length 0-255); Smithy DeleteBranch DELETE /apps/{appId}/branches/{branchName} -> DeleteBranchResult.branch @required; Smithy Branch @required members include stage, ttl (@length 0-32, pattern ^\\d*$), enableAutoBuild, description, environmentVariables, customDomains, activeJobId, framework, totalNumberOfJobs; Smithy Branch optional members associatedResources, backendEnvironmentArn, backend, basicAuthCredentials, buildSpec, computeRoleArn, destinationBranch, enableSkewProtection, enablePerformanceMode, pullRequestEnvironmentName, sourceBranch, thumbnailUrl; Smithy Backend.stackArn. Recorded timestamps, app ids, branch ARNs and generated names differ per environment and are not requirements here.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (no arguments; all three causes are in it).
2. Cause 1: replay fixture amplify-branch-lifecycle through step task#6 - CreateApp, CreateBranch, GetBranch, UpdateBranch (stage/ttl), then ListBranches. Diff $.branches[0] against the recorded element: every modeled Branch member must be present (null where unset), backend must serialize as {"stackArn": null}, and stage/ttl must reflect the UpdateBranch call.
3. Cause 2: continue the same fixture to step task#7 - DeleteBranch. $.branch.stage and $.branch.ttl must carry the post-update values.
4. Cause 3: replay fixture amplify-job-start-and-manage to step teardown#9 - CreateBranch with enableAutoBuild=true, then DeleteBranch. $.branch.enableAutoBuild must be true (re-check amplify-manual-deployment-flow#teardown#5, the other member of that cluster).
All three steps (amplify-branch-lifecycle#task#6, amplify-branch-lifecycle#task#7, amplify-job-start-and-manage#teardown#9) must pass before this issue is closed.
