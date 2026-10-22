Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-branch-lifecycle
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-branch-lifecycle amplify-create-app-default-state amplify-job-start-and-manage amplify-manual-deployment-flow amplify-environment-variables-replace-semantics
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.cacheConfig.type
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.customHeaders
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.customRules
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.description
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.enableAutoBranchCreation
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.enableBranchAutoDeletion
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.environmentVariables
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.jobConfig
Issue-Key: amplify|DeleteApp|2xx->2xx|$.app.repository
Issue-Key: amplify|GetApp|2xx->2xx|$.app.cacheConfig.type
Issue-Key: amplify|GetApp|2xx->2xx|$.app.customHeaders
Issue-Key: amplify|GetApp|2xx->2xx|$.app.customRules
Issue-Key: amplify|GetApp|2xx->2xx|$.app.description
Issue-Key: amplify|GetApp|2xx->2xx|$.app.enableAutoBranchCreation
Issue-Key: amplify|GetApp|2xx->2xx|$.app.enableBranchAutoDeletion
Issue-Key: amplify|GetApp|2xx->2xx|$.app.environmentVariables
Issue-Key: amplify|GetApp|2xx->2xx|$.app.jobConfig
Issue-Key: amplify|GetApp|2xx->2xx|$.app.repository
Issue-Key: amplify|GetApp|2xx->2xx|$.app.tags
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.cacheConfig.type
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.customHeaders
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.customRules
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.description
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.enableAutoBranchCreation
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.enableBranchAutoDeletion
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.jobConfig
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.repository
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.environmentVariables
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.activeJobId
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.backend
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.customDomains
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.description
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.enablePerformanceMode
Issue-Key: amplify|CreateBranch|2xx->2xx|$.branch.environmentVariables
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.activeJobId
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.backend
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.customDomains
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.description
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.enablePerformanceMode
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.environmentVariables
Issue-Key: amplify|GetBranch|2xx->2xx|$.branch.tags
Issue-Key: amplify|ListBranches|2xx->2xx|$.branches[*]
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Amplify App and Branch responses misproject modeled members

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApp

## Description
Every Amplify operation that returns an `App` or a `Branch` projects the structure wrongly in the SAME two ways: (a) members AWS returns as JSON `null` are emitted as empty placeholders (`""`, `[]`, `{}`), and (b) members AWS always returns are omitted entirely (`backend`, `enablePerformanceMode`, `tags`, `jobConfig`, `customRules`, `customHeaders`, `enableAutoBranchCreation`, `enableBranchAutoDeletion`), plus the `cacheConfig.type` default is wrong. This issue bundles 7 causes, one per diverging operation. Describe-shape only: no status or error code differs on any of them.

Cause 1 of 7 - DeleteApp returns a misprojected App (fixture amplify-branch-lifecycle, step teardown#9).
AWS: Smithy DeleteApp -> DeleteAppResult.app @required, target com.amazonaws.amplify#App. App.description / App.repository / App.environmentVariables are @required members, and the recording shows AWS answering 200 with `description: null`, `repository: null`, `environmentVariables: null`; `customHeaders: ""`, `customRules: []`, `enableAutoBranchCreation: false`, `enableBranchAutoDeletion: false`, `jobConfig: {"buildComputeType": "STANDARD_8GB"}` (Smithy JobConfig.buildComputeType: "If you don't specify a value, Amplify uses the STANDARD_8GB default") and `cacheConfig.type: "AMPLIFY_MANAGED_NO_COOKIES"` (Smithy CacheConfig.type: "The AMPLIFY_MANAGED_NO_COOKIES cache configuration type ... This is the default setting"; note App.cacheConfig's own prose says AMPLIFY_MANAGED, and the recording settles it as AMPLIFY_MANAGED_NO_COOKIES).
Emulator: 200, but `description` and `repository` are `""` instead of null, `environmentVariables` is `{}` instead of null, `customHeaders` / `customRules` / `enableAutoBranchCreation` / `enableBranchAutoDeletion` / `jobConfig` are absent from the body, and `cacheConfig.type` is `AMPLIFY_MANAGED`.
Why: a client that branches on `cacheConfig.type` sees cookie-inclusive caching on an app AWS reports as cookie-excluding; a client that reads `jobConfig.buildComputeType` or the two auto-branch booleans gets a null/undefined where AWS always supplies a value, and generated SDK deserializers that treat an absent @required member as a protocol error fail outright. Empty-string-for-null also flips emptiness checks (`"" != null`).
Fix: the DeleteApp handler's App response serializer, and the App default-state construction that seeds cacheConfig/jobConfig.
Traces: amplify-branch-lifecycle#teardown#9, and the same cluster at amplify-create-app-default-state#task#3, amplify-job-start-and-manage#teardown#10, amplify-manual-deployment-flow#teardown#6.

Cause 2 of 7 - GetApp returns a misprojected App (fixture amplify-create-app-default-state, step task#2).
AWS: Smithy GetApp -> GetAppResult.app @required. Recording: 200 with the same App projection as Cause 1 plus `tags: {}`.
Emulator: 200 with `cacheConfig.type: AMPLIFY_MANAGED` (expected AMPLIFY_MANAGED_NO_COOKIES), `customHeaders` / `customRules` / `enableAutoBranchCreation` / `enableBranchAutoDeletion` / `jobConfig` / `tags` absent, `description` and `repository` `""` instead of null, `environmentVariables` `{}` instead of null.
Why: GetApp is the canonical read of app configuration, so every one of these fields is read back wrong by clients polling app state after create.
Fix: the GetApp handler's App response serializer (same projection as Cause 1).
Traces: amplify-create-app-default-state#task#2.

Cause 3 of 7 - CreateApp returns a misprojected App (fixture amplify-environment-variables-replace-semantics, step task#1).
AWS: Smithy CreateApp -> CreateAppResult.app @required. Recording: 200 with `cacheConfig.type: AMPLIFY_MANAGED_NO_COOKIES`, `customHeaders: ""`, `customRules: []`, `enableAutoBranchCreation: false`, `enableBranchAutoDeletion: false`, `jobConfig: {"buildComputeType": "STANDARD_8GB"}`, `description: null`, `repository: null`.
Emulator: 200 with cacheConfig.type AMPLIFY_MANAGED, the five listed members absent, and `""` for description/repository.
Why: the create response is what a client caches as the app's initial state, so the defect is inherited by everything downstream of the create call.
Fix: the CreateApp handler's App response serializer / default-state construction.
Traces: amplify-environment-variables-replace-semantics#task#1.

Cause 4 of 7 - CreateApp misprojection also reproduces on a second app shape (fixture amplify-job-start-and-manage, step setup#1).
AWS: as Cause 3; recording additionally shows `environmentVariables: null`.
Emulator: 200 with `environmentVariables: {}` plus the Cause 3 differences.
Why: same as Cause 3; recorded separately because this step is its own divergence cluster and must be re-replayed on its own.
Fix: the CreateApp handler's App response serializer.
Traces: amplify-job-start-and-manage#setup#1.

Cause 5 of 7 - CreateBranch returns a misprojected Branch (fixture amplify-branch-lifecycle, step task#2).
AWS: Smithy CreateBranch -> CreateBranchResult.branch @required, target Branch. Branch.description, Branch.environmentVariables, Branch.customDomains, Branch.activeJobId are @required; recording shows AWS answering 200 with `activeJobId: null`, `description: null`, `customDomains: null`, `environmentVariables: null`, and with `backend: {"stackArn": null}` and `enablePerformanceMode: false` PRESENT.
Emulator: 200 with `activeJobId: ""`, `description: ""`, `customDomains: []`, `environmentVariables: {}`, and `backend` / `enablePerformanceMode` absent.
Why: a client that tests `activeJobId` for a running job sees an empty string rather than null, and a client reading `branch.backend.stackArn` dereferences a missing object; `enablePerformanceMode` reads as undefined where AWS states false.
Fix: the CreateBranch handler's Branch response serializer and Branch default-state construction (seed `backend`).
Traces: amplify-branch-lifecycle#task#2.

Cause 6 of 7 - GetBranch returns a misprojected Branch (fixture amplify-branch-lifecycle, step task#3).
AWS: Smithy GetBranch -> GetBranchResult.branch @required. Recording: 200 with the Cause 5 projection plus `tags: {}`.
Emulator: 200 with the Cause 5 placeholders and `tags` absent as well.
Why: GetBranch is the read-back path for branch state; `tags` absent breaks tag-reading clients that expect an always-present (possibly empty) map.
Fix: the GetBranch handler's Branch response serializer.
Traces: amplify-branch-lifecycle#task#3.

Cause 7 of 7 - ListBranches emits a misprojected Branch element (fixture amplify-branch-lifecycle, step task#6).
AWS: Smithy ListBranches -> ListBranchesResult.branches @required, list of Branch. Recording: 200 with a fully populated Branch element carrying the null-valued members (`activeJobId`, `associatedResources`, `customDomains`, `description`, `destinationBranch`, `environmentVariables`, `sourceBranch`, ...) and the always-present ones (`backend: {"stackArn": null}`, `enablePerformanceMode: false`, `tags: {}`, `enableSkewProtection: null`, `buildSpec: null`, `computeRoleArn: null`, `backendEnvironmentArn: null`, `basicAuthCredentials: null`, `pullRequestEnvironmentName: null`, `thumbnailUrl: null`).
Emulator: 200 with a short element that both drops those members and substitutes `""` / `[]` / `{}` for the null ones, so the replay reports the element as simultaneously `missing` and `extra`.
Why: list and get disagree on the same resource, so a client that lists then compares to a get sees spurious drift, and element-level deserialization of @required members fails.
Fix: the ListBranches handler must serialize list elements through the same Branch projection as GetBranch.
Traces: amplify-branch-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1 (7 fixtures, 47 steps, 9 passed, score 19.1%).

Cause 1 (amplify-branch-lifecycle#teardown#9, DeleteApp, expected 200 / actual 200):
  $.app.cacheConfig.type: value_mismatch (expected 'AMPLIFY_MANAGED_NO_COOKIES', got 'AMPLIFY_MANAGED')
  $.app.customHeaders: missing (expected '', got None)
  $.app.customRules: missing (expected [], got None)
  $.app.description: type_mismatch (expected None, got '')
  $.app.enableAutoBranchCreation: missing (expected False, got None)
  $.app.enableBranchAutoDeletion: missing (expected False, got None)
  $.app.environmentVariables: type_mismatch (expected None, got {})
  $.app.jobConfig: missing (expected {'buildComputeType': 'STANDARD_8GB'}, got None)
  $.app.repository: type_mismatch (expected None, got '')
  cluster also affects amplify-create-app-default-state#task#3, amplify-job-start-and-manage#teardown#10, amplify-manual-deployment-flow#teardown#6

Cause 2 (amplify-create-app-default-state#task#2, GetApp, 200/200): same nine diffs plus $.app.tags: missing (expected {}, got None).

Cause 3 (amplify-environment-variables-replace-semantics#task#1, CreateApp, 200/200): cacheConfig.type AMPLIFY_MANAGED_NO_COOKIES->AMPLIFY_MANAGED; customHeaders, customRules, enableAutoBranchCreation, enableBranchAutoDeletion, jobConfig missing; description, repository None->''.

Cause 4 (amplify-job-start-and-manage#setup#1, CreateApp, 200/200): as Cause 3 plus $.app.environmentVariables: type_mismatch (expected None, got {}).

Cause 5 (amplify-branch-lifecycle#task#2, CreateBranch, 200/200):
  $.branch.activeJobId: type_mismatch (expected None, got '')
  $.branch.backend: missing (expected {'stackArn': None}, got None)
  $.branch.customDomains: type_mismatch (expected None, got [])
  $.branch.description: type_mismatch (expected None, got '')
  $.branch.enablePerformanceMode: missing (expected False, got None)
  $.branch.environmentVariables: type_mismatch (expected None, got {})

Cause 6 (amplify-branch-lifecycle#task#3, GetBranch, 200/200): the six diffs of Cause 5 plus $.branch.tags: missing (expected {}, got None).

Cause 7 (amplify-branch-lifecycle#task#6, ListBranches, 200/200): $.branches[0] reported both 'missing' and 'extra' - expected element carries activeJobId/associatedResources/backend{stackArn:null}/backendEnvironmentArn/basicAuthCredentials/buildSpec/computeRoleArn/customDomains/description/destinationBranch/enablePerformanceMode:false/enableSkewProtection/environmentVariables/pullRequestEnvironmentName/sourceBranch/tags:{}/thumbnailUrl; emulator element carries only 18 keys and uses '' / [] / {} for activeJobId, description, customDomains, environmentVariables.

Model references: Smithy App (description/repository/environmentVariables @required), Smithy Branch (description/environmentVariables/customDomains/activeJobId/framework @required), Smithy CacheConfig.type (AMPLIFY_MANAGED_NO_COOKIES is the default setting), Smithy JobConfig.buildComputeType (STANDARD_8GB default).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture amplify-branch-lifecycle step teardown#9 (DeleteApp) and diff $.app against the recorded body.
3. Cause 2: fixture amplify-create-app-default-state step task#2 (GetApp).
4. Cause 3: fixture amplify-environment-variables-replace-semantics step task#1 (CreateApp).
5. Cause 4: fixture amplify-job-start-and-manage step setup#1 (CreateApp).
6. Cause 5: fixture amplify-branch-lifecycle step task#2 (CreateBranch) and diff $.branch.
7. Cause 6: fixture amplify-branch-lifecycle step task#3 (GetBranch).
8. Cause 7: fixture amplify-branch-lifecycle step task#6 (ListBranches) and diff $.branches[0].
All seven steps must project null vs empty-placeholder and the always-present members exactly as recorded before this issue is closed.
