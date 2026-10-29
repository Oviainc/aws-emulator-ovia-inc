Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.delete-webhook-absent-project-not-found codebuild.update-project-absent-not-found codebuild.list-build-batches-for-project-absent-not-found codebuild.get-report-group-trend-absent-report-group-not-found codebuild.list-builds-malformed-next-token-rejected codebuild.create-webhook-duplicate-name-rejected codebuild.create-webhook-duplicate-name-conflict-rejected
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.delete-webhook-absent-project-not-found|run#5|AssertionError|assert message == "Project does not exist", message
Issue-Key: codebuild.update-project-absent-not-found|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codebuild.list-build-batches-for-project-absent-not-found|run#5|AssertionError|assert message == f"The provided project arn:aws:codebuild:{ctx.region}:{ctx.account}:project/{missing_name} does not exist.", message
Issue-Key: codebuild.get-report-group-trend-absent-report-group-not-found|run#6|AssertionError|assert message == f"The provided report group {missing_arn} does not exist", message
Issue-Key: codebuild.list-builds-malformed-next-token-rejected|run#6|AssertionError|assert (code, message) == ("InvalidInputException", expected), (
Issue-Key: codebuild.create-webhook-duplicate-name-rejected|run#11|AssertionError|assert (code, message) == (
Issue-Key: codebuild.create-webhook-duplicate-name-conflict-rejected|run#11|AssertionError|assert (code, message) == (
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodeBuild error-message wording diverges from AWS on 6 refusal paths (6 causes)

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWebhook

## Description
SIX net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006). In each one the emulator returns the right HTTP status and (except where noted) the right error code, but the WRONG message text, so a client that branches on or logs AWS's wording misreads the failure. An error message is part of the contract, so each operation's wording is its own defect. ALL SIX must be closed before this issue is resolved.

In every cause the fixed text is what is quoted outside angle brackets; angle-bracketed parts are interpolated from the request and from service state (project name/ARN, report-group ARN) and must never be hard-coded from one recording.

Related open issues that do NOT cover these checks: 20261029T175929Z-codebuild-absent-resource-and-invalid-input-error-messages.md describes the same wording family from the WIRE corpus (its causes 2, 4, 5 and 8 touch ListBuildBatchesForProject, UpdateProject, GetReportGroupTrend and ListFleets) but carries only wire replay keys and no key for any behavioral check below; 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md covers absent-resource calls answered 200. Neither owns the checks listed here, so these must be replayed.

Cause 1 of 6: DeleteWebhook on a project name that never existed uses an ARN-interpolating not-found template where AWS emits a constant string.
AWS: Smithy DeleteWebhook(projectName @required ProjectName, @length min 2 max 150, @pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$) -> {}; modeled errors InvalidInputException, OAuthProviderException, ResourceNotFoundException (all client, httpError 400). Recorded behavior: HTTP 400, __type ResourceNotFoundException, message exactly 'Project does not exist' — wholly fixed text, no interpolated name or ARN and no trailing period.
Emulator: HTTP 400 with the correct __type but message 'Project cannot be found: <project ARN>'.
Why: a caller matching the documented phrasing sees an unknown failure string on the most common CodeBuild error, and the emulator leaks a per-project ARN into a message AWS keeps constant, so message comparisons across projects differ where AWS's do not.
Fix: the not-found message template used by the DeleteWebhook handler's project lookup.
Traces: codebuild.delete-webhook-absent-project-not-found.

Cause 2 of 6: UpdateProject on an absent project uses the same wrong template.
AWS: Smithy UpdateProject(name @required NonEmptyString @length min 1) -> {project}; modeled errors InvalidInputException, ResourceNotFoundException (client, httpError 400). Recorded: HTTP 400, __type ResourceNotFoundException, message exactly 'Project does not exist' (fixed text, no interpolation).
Emulator: HTTP 400, correct __type, message 'Project cannot be found: <project ARN>'.
Why: same as cause 1 on the update path; the check proves the error is about absence only (the identical call succeeds before the delete and again after recreate), so the message is the sole divergence.
Fix: the not-found message in the UpdateProject handler — the constant string, not the shared ARN-interpolating template.
Traces: codebuild.update-project-absent-not-found.

Cause 3 of 6: ListBuildBatchesForProject on an absent project uses the wrong not-found template (and a DIFFERENT one from causes 1-2, so a single global string is not the fix).
AWS: Smithy ListBuildBatchesForProject(projectName NonEmptyString, filter, maxResults @range 1-100, sortOrder, nextToken) -> {ids,nextToken}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded: HTTP 400, __type ResourceNotFoundException, message 'The provided project <project ARN> does not exist.' — fixed text 'The provided project ' / ' does not exist.' around the ARN of the requested project, WITH a trailing period.
Emulator: 'Project cannot be found: <project ARN>'.
Why: the per-operation wording differs across CodeBuild; a client cannot distinguish the project-absent case by message at all, and alerting keyed on 'does not exist' never fires.
Fix: the not-found message template in the ListBuildBatchesForProject handler.
Traces: codebuild.list-build-batches-for-project-absent-not-found.

Cause 4 of 6: GetReportGroupTrend on an absent report group drops the ARN from the message.
AWS: Smithy GetReportGroupTrend(reportGroupArn @required NonEmptyString, trendField @required ReportGroupTrendFieldType, numOfReports @range 1-100) -> {stats,rawData}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded: HTTP 400, __type ResourceNotFoundException, message 'The provided report group <report-group ARN> does not exist' — fixed text 'The provided report group ' / ' does not exist' around the ARN from the request, no trailing period.
Emulator: constant message 'Report group does not exist', with the ARN removed.
Why: the caller is never told WHICH report group was missing, which is the whole diagnostic value when several ARNs are being trended in one loop.
Fix: the not-found message template in the GetReportGroupTrend handler.
Traces: codebuild.get-report-group-trend-absent-report-group-not-found.

Cause 5 of 6: ListBuilds rejects a malformed pagination token with the wire member name instead of AWS's phrase.
AWS: Smithy ListBuilds(sortOrder, nextToken) -> {ids,nextToken}; the single modeled error is InvalidInputException (client, httpError 400). Recorded: HTTP 400, message 'Invalid continuation token specified.' — wholly fixed text including the trailing period; the token value is never echoed. The check's positive control (a token-less ListBuilds in the same account at the same moment) succeeds, so the rejection is specific to the token.
Emulator: HTTP 400, __type InvalidInputException, message 'Invalid nextToken'.
Why: paginating clients that tell an expired/garbled continuation token from other invalid input by message cannot do so, and the emulator surfaces the wire member name rather than AWS's user-facing phrase (AWS uses the same phrase for every CodeBuild list operation's token).
Fix: the nextToken validation message in the ListBuilds handler (shared continuation-token validation).
Traces: codebuild.list-builds-malformed-next-token-rejected.

Cause 6 of 6: CreateWebhook on a project that already has a webhook returns the right code with the wrong message.
AWS: Smithy CreateWebhook(projectName @required ProjectName, branchFilter, filterGroups, buildType, manualCreation, scopeConfiguration, pullRequestBuildPolicy) -> {webhook}; modeled errors InvalidInputException, OAuthProviderException, ResourceAlreadyExistsException, ResourceNotFoundException (all client, 400). Recorded: HTTP 400, __type ResourceAlreadyExistsException, message exactly 'Webhook already exists for this project' — wholly fixed text, no project name or ARN interpolated.
Emulator: HTTP 400, correct __type ResourceAlreadyExistsException, message 'A webhook already exists for CodeBuild project: <project ARN>'.
Why: the duplicate-webhook conflict is the branch idempotent provisioning code keys on; the emulator's wording plus an interpolated ARN defeats exact-match handling and makes the message vary per project where AWS's is constant. Both checks confirm the rest of the behavior is right (the original webhook survives and re-creation after DeleteWebhook works), so the message is the only divergence.
Fix: the already-exists message emitted by the CreateWebhook handler's existing-webhook precondition.
Traces: codebuild.create-webhook-duplicate-name-rejected, codebuild.create-webhook-duplicate-name-conflict-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 64/182 passed; codebuild 63/136. Per cause, expected (recorded AWS) vs actual (emulator):

Cause 1 — codebuild.delete-webhook-absent-project-not-found: code ResourceNotFoundException matched; assertion `message == "Project does not exist"` failed with 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/sim2real-behav-nonexistent-proj-59d8e80b4706' (the ARN is a per-run value filling the emulator's template, not part of the requirement).

Cause 2 — codebuild.update-project-absent-not-found: "expected message 'Project does not exist', got 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/sim2real-behav-missing-proj-9187c130b3aa'". The check's positive controls (UpdateProject on the live project, and again after recreating the same name) both passed.

Cause 3 — codebuild.list-build-batches-for-project-absent-not-found: expected 'The provided project arn:aws:codebuild:<region>:<account>:project/<name> does not exist.', got 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/sim2real-nonexistent-proj-673be087d732'.

Cause 4 — codebuild.get-report-group-trend-absent-report-group-not-found: expected 'The provided report group <report-group ARN> does not exist', got 'Report group does not exist'.

Cause 5 — codebuild.list-builds-malformed-next-token-rejected: "expected InvalidInputException 'Invalid continuation token specified.', got 'InvalidInputException': 'Invalid nextToken'" for ListBuilds(nextToken='not-a-valid-pagination-token'); the token-less control call returned 'ids' normally.

Cause 6 — codebuild.create-webhook-duplicate-name-rejected and codebuild.create-webhook-duplicate-name-conflict-rejected: both assert ('ResourceAlreadyExistsException', 'Webhook already exists for this project') and both got ('ResourceAlreadyExistsException', 'A webhook already exists for CodeBuild project: arn:aws:codebuild:us-west-2:000000000000:project/<project>').

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — DeleteWebhook, UpdateProject, ListBuildBatchesForProject, GetReportGroupTrend, ListBuilds, CreateWebhook signatures and modeled errors (InvalidInputException / ResourceNotFoundException / ResourceAlreadyExistsException, errorKind client, httpError 400).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the behavioral suite and read the codebuild failures, or reproduce each cause directly against the emulator endpoint:
3. Cause 1: DeleteWebhook(projectName=<a name that was never created>) — expect 400 ResourceNotFoundException with the constant message 'Project does not exist'; observe 'Project cannot be found: <project ARN>'.
4. Cause 2: CreateProject then DeleteProject a name, then UpdateProject(name=<that name>) — expect 'Project does not exist'.
5. Cause 3: ListBuildBatchesForProject(projectName=<absent project>) — expect 'The provided project <project ARN> does not exist.' (trailing period).
6. Cause 4: GetReportGroupTrend(reportGroupArn=<absent report-group ARN>, trendField=PASS_RATE) — expect 'The provided report group <arn> does not exist'.
7. Cause 5: ListBuilds() as a control (succeeds), then ListBuilds(nextToken='<garbage>') — expect 400 InvalidInputException 'Invalid continuation token specified.'
8. Cause 6: CreateProject with a GITHUB source, CreateWebhook(projectName=<it>, manualCreation=True), then CreateWebhook again — expect 400 ResourceAlreadyExistsException 'Webhook already exists for this project'.
