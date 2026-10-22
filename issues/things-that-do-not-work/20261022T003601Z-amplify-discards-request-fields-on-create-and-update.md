Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-environment-variables-replace-semantics
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-environment-variables-replace-semantics amplify-branch-lifecycle amplify-job-start-and-manage amplify-manual-deployment-flow
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|GetApp|2xx->2xx|$.app.environmentVariables.C
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.stage
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.ttl
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.enableAutoBuild
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.framework
Issue-Related: 20261022T003601Z-amplify-app-and-branch-responses-misproject-modeled-members.md
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Amplify discards request fields on create and update

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApp

## Description
Amplify create/update inputs are not written to emulator state: subsequent reads return seeded defaults instead of the values the request supplied. This issue bundles 3 causes, each a distinct operation losing distinct input members. (Each cited step also carries the App/Branch null-vs-empty projection diffs reported in the companion issue 'Amplify App and Branch responses misproject modeled members'; the diffs listed here are the state-loss ones only.)

Cause 1 of 3: UpdateApp does not apply the supplied environmentVariables map, so a key the request sets is missing on read-back (fixture amplify-environment-variables-replace-semantics, step task#3).
AWS: Smithy UpdateApp (POST /apps/{appId}) accepts `environmentVariables` (EnvironmentVariables map) and returns UpdateAppResult.app; App.environmentVariables is @required. The fixture updates the app's environment variables and then reads the app back; the recording shows AWS answering 200 with the replaced map containing the key `C` (recorded value "3"), i.e. UpdateApp replaces the whole map with what the request supplied.
Emulator: 200, but `$.app.environmentVariables.C` is absent - the updated map was not persisted, so GetApp serves the pre-update map.
Why: environment variables drive the build; a client that sets them and verifies via GetApp sees its write silently lost, and any read-modify-write over the map operates on stale content. Replace-vs-merge semantics cannot be exercised at all while the write is dropped.
Fix: the UpdateApp handler's state mutation - replace the stored environmentVariables map from the request before serializing the response.
Traces: amplify-environment-variables-replace-semantics#task#3.

Cause 2 of 3: UpdateBranch does not apply `stage` and `ttl`, so GetBranch serves the pre-update values (fixture amplify-branch-lifecycle, step task#5).
AWS: Smithy UpdateBranch (POST /apps/{appId}/branches/{branchName}) accepts `stage` (Stage enum) and `ttl` (TTL, pattern ^\d*$); Branch.stage and Branch.ttl are @required in the response. The fixture updates the branch and re-reads it; the recording shows AWS answering 200 with the updated values (recorded example: stage BETA, ttl "10").
Emulator: 200 with the creation-time values instead (recorded example: stage DEVELOPMENT, ttl "5") - the update was accepted but not stored.
Why: a client promoting a branch's stage or changing its cache TTL gets a success response and then reads back the old configuration, so idempotent converge-to-desired-state logic loops or wrongly concludes the change was rejected.
Fix: the UpdateBranch handler's state mutation - persist every supplied branch member (at minimum stage and ttl) to the stored branch.
Traces: amplify-branch-lifecycle#task#5.

Cause 3 of 3: CreateBranch ignores `enableAutoBuild` and `framework` from the request (fixture amplify-job-start-and-manage, step setup#2).
AWS: Smithy CreateBranch (POST /apps/{appId}/branches) accepts `enableAutoBuild` (boolean) and `framework` (Framework, length 0-255); Branch.enableAutoBuild and Branch.framework are @required in the response. The recording shows AWS answering 200 echoing the request: enableAutoBuild true, and framework as supplied (null here where the request omitted it).
Emulator: 200 with `enableAutoBuild: false` - its seeded default rather than the requested true - and `framework: ""` instead of the recorded null.
Why: the branch is created with auto-build off even though the caller asked for it on, so the branch's behavior differs from what the client provisioned, and the create response misreports it; nothing in the response signals the loss.
Fix: the CreateBranch handler's state construction - initialize the branch from the request members instead of fixed defaults.
Traces: amplify-job-start-and-manage#setup#2 (cluster also affects amplify-manual-deployment-flow#setup#2).

## Evidence
Replay of simparity-wire-aws-v1 (7 fixtures, 47 steps, 9 passed, score 19.1%); state_reset_failures 0, so no fixture ran against a previous fixture's state.

Cause 1 (amplify-environment-variables-replace-semantics#task#3, GetApp after UpdateApp, expected 200 / actual 200):
  $.app.environmentVariables.C: missing (expected '3', got None)
  (same step also shows the projection diffs: cacheConfig.type AMPLIFY_MANAGED_NO_COOKIES->AMPLIFY_MANAGED; customHeaders/customRules/enableAutoBranchCreation/enableBranchAutoDeletion/jobConfig/tags missing; description/repository None->'')

Cause 2 (amplify-branch-lifecycle#task#5, GetBranch after UpdateBranch, expected 200 / actual 200):
  $.branch.stage: value_mismatch (expected 'BETA', got 'DEVELOPMENT')
  $.branch.ttl: value_mismatch (expected '10', got '5')
  (plus the Branch projection diffs: activeJobId/description None->'', customDomains None->[], environmentVariables None->{}, backend/enablePerformanceMode/tags missing)

Cause 3 (amplify-job-start-and-manage#setup#2, CreateBranch, expected 200 / actual 200):
  $.branch.enableAutoBuild: value_mismatch (expected True, got False)
  $.branch.framework: type_mismatch (expected None, got '')
  cluster also affects amplify-manual-deployment-flow#setup#2

Model references: Smithy UpdateApp.environmentVariables (EnvironmentVariables map input) / App.environmentVariables @required; Smithy UpdateBranch.stage + UpdateBranch.ttl (pattern ^\\d*$) / Branch.stage + Branch.ttl @required; Smithy CreateBranch.enableAutoBuild + CreateBranch.framework / Branch.enableAutoBuild + Branch.framework @required.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture amplify-environment-variables-replace-semantics through step task#3 - CreateApp, UpdateApp with environmentVariables, then GetApp; the key set by the update must be present in $.app.environmentVariables.
3. Cause 2: replay fixture amplify-branch-lifecycle through step task#5 - CreateBranch, UpdateBranch changing stage and ttl, then GetBranch; both members must reflect the update.
4. Cause 3: replay fixture amplify-job-start-and-manage step setup#2 - CreateBranch with enableAutoBuild=true; the response branch must echo enableAutoBuild true and framework exactly as supplied.
All three steps must pass before this issue is closed.
