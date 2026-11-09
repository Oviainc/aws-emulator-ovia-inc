Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-project-registry-and-absent-project-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-project-registry-and-absent-project-errors codebuild-source-credentials-import-and-remove codebuild-build-and-batch-registry-reads codebuild-report-group-config-and-trend codebuild-report-group-create-get-delete
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|CreateProject|4xx->2xx|$.__type
Issue-Key: codebuild|CreateProject|4xx->2xx|$.message
Issue-Key: codebuild|CreateProject|4xx->2xx|status
Issue-Key: codebuild|UpdateProject|4xx->2xx|$.__type
Issue-Key: codebuild|UpdateProject|4xx->2xx|$.message
Issue-Key: codebuild|UpdateProject|4xx->2xx|status
Issue-Key: codebuild|UpdateProjectVisibility|4xx->2xx|$.__type
Issue-Key: codebuild|UpdateProjectVisibility|4xx->2xx|$.message
Issue-Key: codebuild|UpdateProjectVisibility|4xx->2xx|status
Issue-Key: codebuild|CreateWebhook|4xx->4xx|$.__type
Issue-Key: codebuild|CreateWebhook|4xx->4xx|$.message
Issue-Key: codebuild|DeleteSourceCredentials|4xx->4xx|$.__type
Issue-Key: codebuild|DeleteSourceCredentials|4xx->4xx|$.message
Issue-Key: codebuild|BatchDeleteBuilds|2xx->2xx|$.buildsDeleted
Issue-Key: codebuild|BatchDeleteBuilds|2xx->2xx|$.buildsNotDeleted[*]
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatchesNotFound[*]
Issue-Key: codebuild|BatchGetReportGroups|2xx->2xx|$.reportGroupsNotFound
Issue-Key: codebuild|BatchGetReportGroups|2xx->2xx|$.reportGroups[*]
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Absent CodeBuild resources accepted instead of modeled error

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProject

## Description
The CodeBuild emulator does not gate mutating and batch operations on the existence (or well-formedness) of the resource they name. Requests that AWS rejects with a modeled client error either succeed with 200, or fail with the wrong error code, or are reported in the wrong half of a batch partition. EIGHT distinct causes are bundled here; all eight must be closed before this issue is resolved.

Cause 1 of 8 — CreateProject with a service role CodeBuild cannot assume (fixture codebuild-project-registry-and-absent-project-errors, step task#6).
AWS: CreateProject validates serviceRole (required, NonEmptyString) by attempting sts:AssumeRole before the project is created, and rejects an unassumable/absent role with HTTP 400 __type InvalidInputException. Smithy: CreateProject.serviceRole @required; errors InvalidInputException (client, httpError 400), AccountLimitExceededException, ResourceAlreadyExistsException. Recorded message example: "CodeBuild is not authorized to perform: sts:AssumeRole on service role. Please verify that: 1) The provided service role exists, ..." (the requirement is the 400 + InvalidInputException, the text is an example).
Emulator: answers HTTP 200 with a created project; no __type, no message.
Why: a client that provisions a project with a bad role sees success and then never gets a build, instead of the immediate, actionable role error AWS gives it. Negative-path IaC and CI tests that assert on InvalidInputException pass against the emulator and fail against AWS.
Fix: CreateProject's handler/validation in the codebuild service crate — add the serviceRole resolvability check ahead of project insertion into state.
Traces: codebuild-project-registry-and-absent-project-errors#task#6.

Cause 2 of 8 — UpdateProject on a project that does not exist (step task#7).
AWS: HTTP 400 __type ResourceNotFoundException. Smithy: UpdateProject errors InvalidInputException, ResourceNotFoundException (client, httpError 400). Recorded message example: "Project does not exist".
Emulator: HTTP 200 with no __type and no message, i.e. the update is treated as a successful no-op/upsert.
Why: an update against a mistyped or already-deleted project name silently "succeeds", so clients lose the only signal that their project is gone and may create ghost state.
Fix: UpdateProject's handler — look the project up in state and raise ResourceNotFoundException when absent, instead of upserting.
Traces: codebuild-project-registry-and-absent-project-errors#task#7.

Cause 3 of 8 — UpdateProjectVisibility on an absent project ARN (step task#9).
AWS: HTTP 400 __type ResourceNotFoundException. Smithy: UpdateProjectVisibility.projectArn @required; errors InvalidInputException, ResourceNotFoundException (client, httpError 400). Recorded message example: "Project does not exist.".
Emulator: HTTP 200, no __type/message.
Why: same as cause 2 — a visibility change on a nonexistent project reports success, so a caller believes a project was made PUBLIC_READ/PRIVATE when no project exists.
Fix: UpdateProjectVisibility's handler — resolve projectArn against state before applying visibility.
Traces: codebuild-project-registry-and-absent-project-errors#task#9.

Cause 4 of 8 — CreateWebhook on an absent project raises the wrong modeled error (step task#10).
AWS: when projectName names no project, the existence check runs first and the call fails with HTTP 400 __type ResourceNotFoundException (recorded message example "Project does not exist"). Smithy: CreateWebhook errors include both InvalidInputException and ResourceNotFoundException (both client, httpError 400).
Emulator: HTTP 400 but __type InvalidInputException, complaining about the source type ("... its source type NO_SOURCE does not support webhooks ...") — it evaluates a source-type precondition against a project it never found.
Why: the status matches but the error code does not, so clients that branch on ResourceNotFoundException (retry/create-project-then-webhook flows) take the wrong branch, and the message misattributes the fault to the project's source type.
Fix: CreateWebhook's validation ordering — perform project lookup, raising ResourceNotFoundException, before any source-type/webhook-support validation.
Traces: codebuild-project-registry-and-absent-project-errors#task#10.

Cause 5 of 8 — DeleteSourceCredentials with a malformed token ARN raises the wrong modeled error (fixture codebuild-source-credentials-import-and-remove, step task#3).
AWS: the arn member is parsed first, and an ARN whose server-type component is not a valid source provider is rejected with HTTP 400 __type InvalidInputException (recorded message example "Invalid token ARN: server type is invalid"). Smithy: DeleteSourceCredentials.arn @required NonEmptyString; errors InvalidInputException, ResourceNotFoundException (both client, httpError 400).
Emulator: HTTP 400 __type ResourceNotFoundException — it skips ARN-shape validation and goes straight to a registry lookup miss.
Why: a client that sent a structurally invalid ARN is told the credential is merely absent, so it retries/creates instead of fixing the malformed input; code that distinguishes "bad request" from "not found" misclassifies the failure.
Fix: DeleteSourceCredentials' validation — validate the token ARN's shape (including the server-type segment) before the state lookup.
Traces: codebuild-source-credentials-import-and-remove#task#3.

Cause 6 of 8 — BatchDeleteBuilds puts an absent build in the wrong partition (fixture codebuild-build-and-batch-registry-reads, step task#5).
AWS: deletion is idempotent — an id that names no build is returned in buildsDeleted (as the full build ARN) and buildsNotDeleted is omitted. Smithy: BatchDeleteBuildsOutput { buildsDeleted: BuildIds, buildsNotDeleted: BuildsNotDeleted }; recorded response puts the absent build's ARN in buildsDeleted.
Emulator: omits buildsDeleted entirely and returns a buildsNotDeleted entry { id, statusCode: "Build <id> does not exist" } keyed by the raw id.
Why: cleanup code that treats a non-empty buildsNotDeleted as a failure aborts teardown on builds AWS would have reported as deleted, and the ARN a caller needs for correlation is replaced by a bare id.
Fix: BatchDeleteBuilds' handler — treat an unknown id as deleted, emit buildsDeleted with ARNs, and reserve buildsNotDeleted for genuinely undeletable builds.
Traces: codebuild-build-and-batch-registry-reads#task#5.

Cause 7 of 8 — BatchGetBuildBatches echoes a bare id in buildBatchesNotFound instead of the ARN (same fixture, step task#3).
AWS: not-found entries are returned in the same ARN form the service uses elsewhere: arn:aws:codebuild:<region>:<account>:build-batch/<project>:<uuid>. Smithy: BatchGetBuildBatchesOutput.buildBatchesNotFound targets BuildBatchIds.
Emulator: returns the request's bare "<project>:<uuid>" with no arn:aws:codebuild:...:build-batch/ prefix; status is 200 in both.
Why: clients that diff the requested ARNs against buildBatchesNotFound (the normal way to find which batches are missing) match nothing, so every requested batch looks found.
Fix: BatchGetBuildBatches' serializer/handler — normalise not-found ids to build-batch ARNs, consistently with the found entries.
Traces: codebuild-build-and-batch-registry-reads#task#3.

Cause 8 of 8 — BatchGetReportGroups still returns a deleted report group (fixture codebuild-report-group-config-and-trend, step teardown#9; also codebuild-report-group-create-get-delete#task#4).
AWS: after DeleteReportGroup succeeds, the ARN is reported in reportGroupsNotFound and reportGroups is empty. Smithy: BatchGetReportGroupsOutput { reportGroups, reportGroupsNotFound }.
Emulator: HTTP 200 with the full deleted report group object (arn/name/type/exportConfig/created/lastModified/status ACTIVE) in reportGroups[0] and reportGroupsNotFound absent — the delete did not remove it from state.
Why: deletion is not observable, so create/delete/verify lifecycles and idempotent teardown loops never converge, and a caller sees an ACTIVE report group it already deleted.
Fix: DeleteReportGroup / the report-group state store — actually remove the record so BatchGetReportGroups partitions it as not-found.
Traces: codebuild-report-group-config-and-trend#teardown#9, codebuild-report-group-create-get-delete#task#4.

Related (does NOT cover these causes): open issue 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md is a behavioral issue about builds FAULTing at PROVISIONING and carries no replay key for any of the fixture steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 105 steps, 34 passed, score 32.4%). Per cause, expected (recorded AWS) vs actual (emulator):

1. CreateProject @ codebuild-project-registry-and-absent-project-errors#task#6 — expected status 400, $.__type 'InvalidInputException', $.message 'CodeBuild is not authorized to perform: sts:AssumeRole on service role. ...'; actual status 200, $.__type missing, $.message missing.
2. UpdateProject @ ...#task#7 — expected status 400, $.__type 'ResourceNotFoundException', $.message 'Project does not exist'; actual status 200, both members missing.
3. UpdateProjectVisibility @ ...#task#9 — expected status 400, $.__type 'ResourceNotFoundException', $.message 'Project does not exist.'; actual status 200, both members missing.
4. CreateWebhook @ ...#task#10 — status 400 both sides; expected $.__type 'ResourceNotFoundException' / $.message 'Project does not exist'; actual $.__type 'InvalidInputException' / message 'A webhook cannot be created for project arn:aws:codebuild:us-west-2:000000000000:project/sim2real-probe-codebuild-codebuild-absent-proj-74c4d814: its source type NO_SOURCE does not support webhooks...'.
5. DeleteSourceCredentials @ codebuild-source-credentials-import-and-remove#task#3 — status 400 both sides; expected $.__type 'InvalidInputException' ('Invalid token ARN: server type is invalid'); actual $.__type 'ResourceNotFoundException' ('Source credentials not found: arn:aws:codebuild:us-west-2:000000000000:token/github_does_not_exist').
6. BatchDeleteBuilds @ codebuild-build-and-batch-registry-reads#task#5 — status 200 both sides; expected $.buildsDeleted ['arn:aws:codebuild:us-west-2:000000000000:build/<project>:11111111-1111-1111-1111-111111111111'] with no buildsNotDeleted; actual $.buildsDeleted missing and $.buildsNotDeleted[0] = {'id': '<project>:1111...', 'statusCode': 'Build <project>:1111... does not exist'}.
7. BatchGetBuildBatches @ codebuild-build-and-batch-registry-reads#task#3 — status 200 both sides; expected $.buildBatchesNotFound[0] 'arn:aws:codebuild:us-west-2:000000000000:build-batch/<project>:33333333-3333-3333-3333-333333333333'; actual '<project>:33333333-3333-3333-3333-333333333333'.
8. BatchGetReportGroups @ codebuild-report-group-config-and-trend#teardown#9 (cluster also covers codebuild-report-group-create-get-delete#task#4) — status 200 both sides; expected $.reportGroupsNotFound ['arn:aws:codebuild:us-west-2:000000000000:report-group/<name>'] with empty reportGroups; actual reportGroupsNotFound missing and reportGroups[0] = the deleted group with status 'ACTIVE'.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json (CreateProject, UpdateProject, UpdateProjectVisibility, CreateWebhook, DeleteSourceCredentials, BatchDeleteBuilds, BatchGetBuildBatches, BatchGetReportGroups signatures + modeled errors, all client/httpError 400).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative cause: fixture codebuild-project-registry-and-absent-project-errors, step task#6 — POST CreateProject with a serviceRole ARN that does not exist/cannot be assumed; emulator answers 200 instead of 400 InvalidInputException.
4. Same fixture, step task#7 — UpdateProject on an absent project name: 200 instead of 400 ResourceNotFoundException.
5. Same fixture, step task#9 — UpdateProjectVisibility on an absent project ARN: 200 instead of 400 ResourceNotFoundException.
6. Same fixture, step task#10 — CreateWebhook with projectName of an absent project: 400 InvalidInputException instead of 400 ResourceNotFoundException.
7. Fixture codebuild-source-credentials-import-and-remove, step task#3 — DeleteSourceCredentials with arn '.../token/github_does_not_exist': 400 ResourceNotFoundException instead of 400 InvalidInputException.
8. Fixture codebuild-build-and-batch-registry-reads, step task#5 — BatchDeleteBuilds with an absent build id: entry lands in buildsNotDeleted instead of buildsDeleted (as an ARN).
9. Same fixture, step task#3 — BatchGetBuildBatches with an absent batch id: buildBatchesNotFound[0] is the bare id, not the build-batch ARN.
10. Fixture codebuild-report-group-config-and-trend, step teardown#9 — after DeleteReportGroup, BatchGetReportGroups still returns the group in reportGroups instead of reportGroupsNotFound.
