Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-build-and-batch-registry-reads
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-build-and-batch-registry-reads codebuild-list-build-batches-for-project-absent-not-found codebuild-project-registry-and-absent-project-errors codebuild-report-group-trend-absent-vs-present codebuild-resource-policy-error-paths codebuild-sandbox-and-command-execution-reads codebuild-list-fleets-invalid-input
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|ListBuildsForProject|4xx->4xx|$.message
Issue-Key: codebuild|ListBuildBatchesForProject|4xx->4xx|$.message
Issue-Key: codebuild|StopBuildBatch|4xx->4xx|$.message
Issue-Key: codebuild|UpdateProject|4xx->4xx|$.message
Issue-Key: codebuild|GetReportGroupTrend|4xx->4xx|$.message
Issue-Key: codebuild|PutResourcePolicy|4xx->4xx|$.message
Issue-Key: codebuild|StartSandboxConnection|4xx->4xx|$.__type
Issue-Key: codebuild|StartSandboxConnection|4xx->4xx|$.message
Issue-Key: codebuild|ListFleets|4xx->4xx|$.message
Issue-Commit: 6f542d1dda0a64c4c626f1148e416fc4d064c349

# CodeBuild absent-resource and invalid-input error messages diverge from AWS wording (8 causes)

> _Note: this issue was identified against the code base at commit `6f542d1dda0a64c4c626f1148e416fc4d064c349`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBuildsForProject

## Description
EIGHT net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006): the operations return the right HTTP status but the WRONG error message text (and, in one case, the wrong error code), so a client that keys on the service's error wording or error class misreads the failure. An error message is part of the wire contract, so each of these is a separate defect in the operation that emits it. ALL EIGHT must be closed before this issue is resolved. Open issue 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md covers absent-resource calls that are ACCEPTED with 200 instead of erroring and carries the replay keys for those steps only; it carries no key for any step below, so these must be replayed too.

In every cause below, the fixed text of the message is the part quoted outside angle brackets; the angle-bracketed parts are interpolated from the request and from service state (project name/ARN, batch id, report-group ARN) and must never be hard-coded from one recording.

Cause 1 of 8: ListBuildsForProject on an absent project uses the wrong not-found message template.
AWS: Smithy ListBuildsForProject(projectName @required NonEmptyString @length min 1) -> {ids,nextToken}; modeled errors InvalidInputException and ResourceNotFoundException (client, httpError 400). Recorded response: HTTP 400, __type ResourceNotFoundException, message 'The provided project <project ARN> does not exist.' — fixed text 'The provided project ' / ' does not exist.' around the ARN of the requested project, with a trailing period.
Emulator: HTTP 400 with the same __type but message 'Project cannot be found: <project ARN>'.
Why: callers and SDK/CLI users that match on the documented phrasing (and recorded-trace comparisons) see an unknown failure string for the single most common CodeBuild error, so error-path branching and log-based alerting on 'does not exist' never fires.
Fix: the not-found message template used by the ListBuildsForProject handler's project lookup.
Traces: codebuild-build-and-batch-registry-reads#task#6.

Cause 2 of 8: ListBuildBatchesForProject on an absent project uses the same wrong template.
AWS: Smithy ListBuildBatchesForProject(projectName) -> {ids,nextToken}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded: HTTP 400, message 'The provided project <project ARN> does not exist.'
Emulator: 'Project cannot be found: <project ARN>'.
Why: same as cause 1, on the batch listing path; a client cannot distinguish the project-absent case by message at all.
Fix: the not-found message template in the ListBuildBatchesForProject handler (same shared helper as cause 1 if one exists — the per-operation wording differs across CodeBuild, see cause 4, so a single global string is not the fix).
Traces: codebuild-build-and-batch-registry-reads#task#7 (same cluster also covers codebuild-list-build-batches-for-project-absent-not-found#task#3).

Cause 3 of 8: StopBuildBatch on an absent batch mis-cases the resource noun.
AWS: Smithy StopBuildBatch(id @required) -> {buildBatch}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded: HTTP 400, message 'BuildBatch <batch id> does not exist' — fixed text 'BuildBatch ' / ' does not exist', with the batch id '<project-name>:<uuid>' taken from the request, and NO trailing period.
Emulator: 'Build batch <batch id> does not exist' (noun spelled as two words).
Why: exact-match assertions and message-keyed retry logic fail on an otherwise correct error.
Fix: the not-found message emitted by the StopBuildBatch handler.
Traces: codebuild-build-and-batch-registry-reads#task#12.

Cause 4 of 8: UpdateProject on an absent project emits an ARN-interpolated message where AWS emits a fixed one.
AWS: Smithy UpdateProject(name @required) -> {project}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded: HTTP 400, __type ResourceNotFoundException, message exactly 'Project does not exist' — wholly fixed text, with NO interpolated name or ARN and no trailing period.
Emulator: 'Project cannot be found: <project ARN>'.
Why: the emulator both changes the wording and leaks a resource ARN into a message AWS keeps constant, so clients that compare messages across calls see a value that varies per project where AWS's does not.
Fix: the not-found message in the UpdateProject handler — it must be the constant string, not the shared ARN-interpolating template.
Traces: codebuild-project-registry-and-absent-project-errors#task#14.

Cause 5 of 8: GetReportGroupTrend on an absent report group uses the wrong not-found message.
AWS: Smithy GetReportGroupTrend(reportGroupArn @required, trendField @required) -> {stats,rawData}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded: HTTP 400, message 'The provided report group <report-group ARN> does not exist' — fixed text 'The provided report group ' / ' does not exist' around the ARN from the request, no trailing period.
Emulator: 'Report group does not exist' (constant, dropping the ARN entirely).
Why: the caller is not told WHICH report group was missing, which is the whole diagnostic value of the message when several ARNs are being trended.
Fix: the not-found message template in the GetReportGroupTrend handler.
Traces: codebuild-report-group-trend-absent-vs-present#task#2.

Cause 6 of 8: PutResourcePolicy against a non-existent resource ARN reports the wrong resource class.
AWS: Smithy PutResourcePolicy(policy @required, resourceArn @required) -> {resourceArn}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded: HTTP 400, message exactly 'Resource ARN does not exist' — fixed text, no interpolation. The operation is ARN-generic (report groups and projects), so the message names the ARN, not a resource type.
Emulator: 'Report group does not exist'.
Why: a caller attaching a policy to a project ARN is told a report group is missing, which points debugging at the wrong resource type.
Fix: the not-found message in the PutResourcePolicy handler's ARN resolution step.
Traces: codebuild-resource-policy-error-paths#task#3.

Cause 7 of 8: StartSandboxConnection for an unknown sandbox id returns the wrong ERROR CODE as well as the wrong message.
AWS: Smithy StartSandboxConnection(sandboxId @required) -> {ssmSession}; modeled errors InvalidInputException and ResourceNotFoundException (both client, httpError 400). Recorded: HTTP 400, __type InvalidInputException, message 'No sandbox found with specified ID' — AWS classifies an unresolvable sandbox id as invalid input on this operation, not as a missing resource.
Emulator: HTTP 400, __type ResourceNotFoundException, message 'Sandbox does not exist'.
Why: the error CODE is what SDK error handling switches on; a client that treats ResourceNotFoundException as retry-after-create and InvalidInputException as fatal takes the opposite branch to the one AWS forces. This is why the bundle is HIGH.
Fix: the error classification and message in the StartSandboxConnection handler's sandbox lookup.
Traces: codebuild-sandbox-and-command-execution-reads#task#7.

Cause 8 of 8: ListFleets rejects a malformed pagination token with the wrong message.
AWS: Smithy ListFleets(nextToken sensitive, maxResults @range 1-100, sortOrder, sortBy) -> {fleets,nextToken}; the single modeled error is InvalidInputException (client, httpError 400). Recorded: HTTP 400, message 'Invalid continuation token specified.' — wholly fixed text including the trailing period; the token value is never echoed (the member is @sensitive).
Emulator: 'Invalid nextToken'.
Why: paginating clients that distinguish an expired/garbled continuation token from other invalid input by message cannot do so, and the emulator's wording is the wire member name rather than AWS's user-facing phrase.
Fix: the nextToken validation message in the ListFleets handler (the same phrase AWS uses for every CodeBuild list operation's continuation token).
Traces: codebuild-list-fleets-invalid-input#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 106 steps, 71 passed, 35 mismatched, score 67%. Status codes matched in all eight causes below (the divergence is message text, plus __type in cause 7).

Cause 1 — codebuild-build-and-batch-registry-reads#task#6 ListBuildsForProject, expected 400 / actual 400: $.message expected 'The provided project arn:aws:codebuild:<region>:<account>:project/<project> does not exist.', got 'Project cannot be found: arn:aws:codebuild:<region>:<account>:project/<project>'.
Cause 2 — codebuild-build-and-batch-registry-reads#task#7 ListBuildBatchesForProject (cluster also contains codebuild-list-build-batches-for-project-absent-not-found#task#3): same expected/actual pair as cause 1.
Cause 3 — codebuild-build-and-batch-registry-reads#task#12 StopBuildBatch: $.message expected 'BuildBatch <project>:<uuid> does not exist', got 'Build batch <project>:<uuid> does not exist'.
Cause 4 — codebuild-project-registry-and-absent-project-errors#task#14 UpdateProject: $.message expected 'Project does not exist', got 'Project cannot be found: arn:aws:codebuild:<region>:<account>:project/<project>'.
Cause 5 — codebuild-report-group-trend-absent-vs-present#task#2 GetReportGroupTrend: $.message expected 'The provided report group arn:aws:codebuild:<region>:<account>:report-group/<name> does not exist', got 'Report group does not exist'.
Cause 6 — codebuild-resource-policy-error-paths#task#3 PutResourcePolicy: $.message expected 'Resource ARN does not exist', got 'Report group does not exist'.
Cause 7 — codebuild-sandbox-and-command-execution-reads#task#7 StartSandboxConnection: $.__type expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message expected 'No sandbox found with specified ID', got 'Sandbox does not exist'.
Cause 8 — codebuild-list-fleets-invalid-input#task#1 ListFleets: $.message expected 'Invalid continuation token specified.', got 'Invalid nextToken'.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — ListBuildsForProject, ListBuildBatchesForProject, StopBuildBatch, UpdateProject, GetReportGroupTrend, PutResourcePolicy, StartSandboxConnection, ListFleets signatures and modeled errors (InvalidInputException / ResourceNotFoundException, client, httpError 400; ListFleets$nextToken @sensitive).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus) and read the codebuild divergences, or reproduce each cause directly against the emulator endpoint:
3. Cause 1: ListBuildsForProject(projectName=<a name/ARN that was never created>) — compare $.message to 'The provided project <arn> does not exist.'
4. Cause 2: ListBuildBatchesForProject(projectName=<absent project>) — same expected message.
5. Cause 3: StopBuildBatch(id='<absent project>:<uuid>') — expect 'BuildBatch <id> does not exist'.
6. Cause 4: UpdateProject(name=<absent project>) — expect the constant message 'Project does not exist'.
7. Cause 5: GetReportGroupTrend(reportGroupArn=<absent report-group ARN>, trendField=DURATION) — expect 'The provided report group <arn> does not exist'.
8. Cause 6: PutResourcePolicy(resourceArn=<ARN of a resource that does not exist>, policy=<any policy>) — expect 'Resource ARN does not exist'.
9. Cause 7: StartSandboxConnection(sandboxId=<unknown id>) — expect __type InvalidInputException with 'No sandbox found with specified ID'; observe ResourceNotFoundException / 'Sandbox does not exist'.
10. Cause 8: ListFleets(nextToken='<garbage>') — expect 'Invalid continuation token specified.'
