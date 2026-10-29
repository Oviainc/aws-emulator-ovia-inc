Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-build-and-batch-registry-reads
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-build-and-batch-registry-reads codebuild-project-registry-and-absent-project-errors
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|RetryBuild|4xx->4xx|$.message
Issue-Key: codebuild|UpdateWebhook|4xx->4xx|$.message
Issue-Key: codebuild|DeleteWebhook|4xx->4xx|$.message
Issue-Key: codebuild|DeleteBuildBatch|4xx->2xx|$.__type
Issue-Key: codebuild|DeleteBuildBatch|4xx->2xx|$.message
Issue-Key: codebuild|DeleteBuildBatch|4xx->2xx|status
Issue-Commit: 6f542d1dda0a64c4c626f1148e416fc4d064c349

# CodeBuild webhook, retry and batch-delete preconditions validated in wrong order or not at all (4 causes)

> _Note: this issue was identified against the code base at commit `6f542d1dda0a64c4c626f1148e416fc4d064c349`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RetryBuild

## Description
FOUR net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006), all of the same shape: the handler checks the wrong precondition first — or no precondition at all — so the client is told about the wrong missing resource, or about nothing. ALL FOUR must be closed before this issue is resolved. Open issue 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md covers other absent-resource operations and carries only their replay keys; none of the steps below is covered by it.

Cause 1 of 4: RetryBuild on a build whose project does not exist reports the BUILD as missing instead of the PROJECT.
AWS: Smithy RetryBuild(id NonEmptyString, idempotencyToken) -> {build}; modeled errors AccountLimitExceededException, InvalidInputException, ResourceNotFoundException (all client, httpError 400). A build id is '<project-name>:<build uuid>', so AWS resolves the project component FIRST and, when that project is absent, answers 400 ResourceNotFoundException with message 'Project cannot be found: <project ARN>' — fixed text 'Project cannot be found: ' plus the ARN derived from the id's project component.
Emulator: 400 with the same __type but message 'Build cannot be found: <build ARN>' — it resolves the build record and never reports the project.
Why: the caller is pointed at a build id it believes it holds instead of at the project it must re-create; recovery logic that re-creates a project on 'Project cannot be found' never triggers.
Fix: precondition ordering in the RetryBuild handler — resolve and validate the project named by the build id before looking up the build record, and name the project ARN in the message.
Traces: codebuild-build-and-batch-registry-reads#task#11.

Cause 2 of 4: UpdateWebhook on an absent project reports a missing webhook instead of a missing project.
AWS: Smithy UpdateWebhook(projectName @required ProjectName @length 2-150 @pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$, branchFilter, rotateSecret, filterGroups, buildType, pullRequestBuildPolicy) -> {webhook}; modeled errors InvalidInputException, OAuthProviderException, ResourceNotFoundException (client, 400). With the project itself absent, the project lookup fails first: 400 ResourceNotFoundException, message exactly 'Project does not exist' (wholly fixed text, no interpolation).
Emulator: 400 with message 'Webhook does not exist for this project' — it consults the webhook table for a project that does not exist.
Why: a caller is told to create a webhook on a project that is not there, so the real fault (the project was deleted or never created) stays hidden; the two conditions are separately actionable in AWS and indistinguishable here.
Fix: precondition ordering in the UpdateWebhook handler — project existence before webhook existence.
Traces: codebuild-project-registry-and-absent-project-errors#task#11.

Cause 3 of 4: DeleteWebhook on an absent project has the same inverted ordering.
AWS: Smithy DeleteWebhook(projectName @required ProjectName) -> {}; modeled errors InvalidInputException, OAuthProviderException, ResourceNotFoundException (client, 400). Absent project: 400 ResourceNotFoundException, message 'Project does not exist'.
Emulator: 400 with 'Webhook does not exist for this project'.
Why: idempotent teardown code that tolerates 'webhook already gone' but must surface 'project already gone' swallows the wrong case.
Fix: precondition ordering in the DeleteWebhook handler, same seam as cause 2.
Traces: codebuild-project-registry-and-absent-project-errors#task#12.

Cause 4 of 4: DeleteBuildBatch accepts a build-batch id it cannot resolve, answering 200 instead of the modeled error.
AWS: Smithy DeleteBuildBatch(id @required NonEmptyString @length min 1) -> {statusCode, buildsDeleted, buildsNotDeleted}; the single modeled error is InvalidInputException (client, httpError 400). An id that does not denote a batch of this account is rejected with 400 InvalidInputException and message 'Invalid BuildBatch ID provided' (wholly fixed text), and nothing is deleted.
Emulator: HTTP 200 with no __type and no message at all — the id is not validated, so a delete of a batch that never existed is reported as success.
Why: this is the worst failure mode of the four: a client that deletes an id it mistyped (or that belongs to another account) is told the delete succeeded, so cleanup scripts and tests report green while the real batch survives; the modeled InvalidInputException path is unreachable.
Fix: id validation/resolution in the DeleteBuildBatch handler ahead of the deletion, emitting the modeled InvalidInputException. Open issue 20261025T014438Z-codebuild-8-further-divergences-getresourcepolicy-updatewebhook-deletebuildbatch-stubs.md describes DeleteBuildBatch answering NotImplemented for the in-progress refusal; the operation now answers 200 on this path and that issue carries no wire replay key for this step.
Traces: codebuild-build-and-batch-registry-reads#task#14.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (106 steps, 71 passed, 35 mismatched, score 67%).

Cause 1 — codebuild-build-and-batch-registry-reads#task#11 RetryBuild, expected 400 / actual 400: $.message expected 'Project cannot be found: arn:aws:codebuild:<region>:<account>:project/<project>', got 'Build cannot be found: arn:aws:codebuild:<region>:<account>:build/<project>:<uuid>'.
Cause 2 — codebuild-project-registry-and-absent-project-errors#task#11 UpdateWebhook, expected 400 / actual 400: $.message expected 'Project does not exist', got 'Webhook does not exist for this project'.
Cause 3 — codebuild-project-registry-and-absent-project-errors#task#12 DeleteWebhook, expected 400 / actual 400: $.message expected 'Project does not exist', got 'Webhook does not exist for this project'.
Cause 4 — codebuild-build-and-batch-registry-reads#task#14 DeleteBuildBatch, expected 400, ACTUAL 200: $.__type missing (expected 'InvalidInputException', got None); $.message missing (expected 'Invalid BuildBatch ID provided', got None).

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — RetryBuild (errors AccountLimitExceededException/InvalidInputException/ResourceNotFoundException), UpdateWebhook and DeleteWebhook (errors InvalidInputException/OAuthProviderException/ResourceNotFoundException, projectName @required ProjectName), DeleteBuildBatch (id @required, sole error InvalidInputException, output statusCode/buildsDeleted/buildsNotDeleted); all errors client with httpError 400.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus) and read the codebuild divergences, or reproduce each cause directly:
3. Cause 1: RetryBuild(id='<project that does not exist>:<uuid>') — expect 400 ResourceNotFoundException with 'Project cannot be found: <project ARN>'; observe 'Build cannot be found: <build ARN>'.
4. Cause 2: UpdateWebhook(projectName=<project never created>, branchFilter='main') — expect 400 with 'Project does not exist'; observe 'Webhook does not exist for this project'.
5. Cause 3: DeleteWebhook(projectName=<project never created>) — same expected message, same observed divergence.
6. Cause 4: DeleteBuildBatch(id='<project>:<uuid that denotes no batch>') — expect 400 InvalidInputException 'Invalid BuildBatch ID provided'; observe HTTP 200 with no error body.
