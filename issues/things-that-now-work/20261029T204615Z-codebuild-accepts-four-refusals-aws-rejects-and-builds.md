Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.batch-get-fleets-invalid-arn-rejected codebuild.delete-resource-policy-absent-policy-rejected codebuild.list-command-executions-for-sandbox-absent-not-found codebuild.e2e-build-batch-lifecycle codebuild.start-build-batch-auto-generates-sub-builds codebuild.e2e-sandbox-command-execution-flow
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.batch-get-fleets-invalid-arn-rejected|_error#3|AssertionError|raise AssertionError(
Issue-Key: codebuild.delete-resource-policy-absent-policy-rejected|_client_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: codebuild.list-command-executions-for-sandbox-absent-not-found|_client_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: codebuild.e2e-build-batch-lifecycle|run#19|AssertionError|assert batch_arn in after_delete["buildBatchesNotFound"], (
Issue-Key: codebuild.start-build-batch-auto-generates-sub-builds|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: codebuild|StartCommandExecution|InvalidInputException
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodeBuild accepts four refusals AWS rejects, and builds/batches/sandboxes never execute (8 causes)

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
EIGHT net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006): four operations ACCEPT input AWS refuses (causes 1-4), one batch read reports not-found identifiers in the wrong form (cause 5), and three execution paths never progress (causes 6-8). ALL EIGHT must be closed before this issue is resolved.

> **Resolution note (filed when causes 1-7 were fixed).** Causes 1, 2, 3, 5, 6 and 7 are fixed, and so is cause 4 — both of its legs (an S3 artifacts override that names no bucket, and one naming a bucket that does not exist) are now refused, and no build is left behind. Cause 8, builds never executing, is NOT fixed and was SPLIT OUT so it is not recorded as working: its own report, carrying the three replay units that still fail on it (`codebuild.report-group-trend-reflects-report-data`, `codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only`, and `codebuild.start-build-artifacts-override-rejected-without-a-location-and-no-artifacts-uploads-nothing`, whose refusal legs pass and whose last assertion needs a build that succeeds), is `issues/things-that-do-not-work/20261107T233000Z-codebuild-builds-never-execute-so-no-report-override-or-artifact-free-build-is-observable.md`. The keys and trace-ids of those three units were moved to that report; the six left here are the ones that pass.

Angle-bracketed parts of every message below are interpolated from the request and from service state (fleet name, resource ARN, sandbox id, bucket name); the quoted text outside them is the fixed part.

Related open issues that do NOT cover these checks: 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md (wire corpus; its cause 7 is the same BatchGetBuildBatches not-found-form defect as cause 5 here but carries only wire replay keys), 20261025T062553Z-codebuild-6-further-divergences-startbuildbatch-takes-no-input.md (batch/sandbox terminal outcomes; owns other checks in this suite, none of the ones listed here), 20261026T234503Z-codebuild-3-net-new-divergences-sandbox-command-stdout.md, and 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md (builds FAULTing; it owns other checks in this suite and carries no replay key for the two checks in cause 8). None of them owns a check listed here.

Cause 1 of 8: BatchGetFleets silently absorbs a malformed fleet identifier.
AWS: Smithy BatchGetFleets(names @required FleetNames @length 1-100) -> {fleets, fleetsNotFound}; the single modeled error is InvalidInputException (client, httpError 400). Fleet names/ARNs must match the CreateFleet name pattern [A-Za-z0-9][A-Za-z0-9\-_]{1,127}, so a name containing spaces and '!' is rejected outright before any lookup. Recorded behavior: HTTP 400, __type InvalidInputException, message 'Invalid characters in fleet name. Only alphanumeric characters, dash, and underscore are supported' — wholly fixed text, no interpolation.
Emulator: HTTP 200 with {'fleets': [], 'fleetsNotFound': []} — it neither validates the name nor even parks it in fleetsNotFound.
Why: a client that typos a fleet name gets an empty, successful answer instead of an actionable 400; a lookup that returned nothing at all is indistinguishable from 'no such fleet', so negative-path tests pass against the emulator and fail against AWS.
Fix: name-pattern validation in the BatchGetFleets handler, ahead of the fleet-registry lookup.
Traces: codebuild.batch-get-fleets-invalid-arn-rejected.

Cause 2 of 8: DeleteResourcePolicy on a resource ARN that does not exist succeeds.
AWS: Smithy DeleteResourcePolicy(resourceArn @required NonEmptyString @length min 1) -> {}; the model lists only InvalidInputException (client, 400). Recorded behavior: deleting the policy of a well-formed but NON-EXISTENT resource ARN is refused with HTTP 400, __type ResourceNotFoundException, message 'Resource ARN does not exist' (fixed text, ARN not interpolated). The same check confirms AWS treats DeleteResourcePolicy on a REAL resource with no policy as an idempotent 200 no-op, so the boundary is resource existence, not policy presence.
Emulator: HTTP 200 with an empty body for the non-existent ARN.
Why: a caller deleting a policy from a mistyped or already-deleted ARN is told it worked, so IaC teardown and policy-reconciliation loops never notice the resource is gone. The check's baseline legs (GetResourcePolicy answering ResourceNotFoundException 'Resource ARN resource policy does not exist') already pass, so only the Delete path is permissive.
Fix: resolve resourceArn against state in the DeleteResourcePolicy handler and raise ResourceNotFoundException when the resource is absent, while keeping the no-policy case a 200 no-op.
Traces: codebuild.delete-resource-policy-absent-policy-rejected.

Cause 3 of 8: ListCommandExecutionsForSandbox accepts a malformed sandboxId.
AWS: Smithy ListCommandExecutionsForSandbox(sandboxId @required NonEmptyString, maxResults @range 1-100, sortOrder, nextToken @sensitive) -> {commandExecutions,nextToken}; modeled errors InvalidInputException and ResourceNotFoundException (client, 400). Recorded behavior: a BARE, unstructured sandboxId (not '<project>:<uuid>') is rejected before any listing with HTTP 400, __type InvalidInputException, message 'Invalid sandbox ID provided' (fixed text). A well-SHAPED but absent id is the opposite case: it succeeds with an empty commandExecutions list.
Emulator: HTTP 200 with {'commandExecutions': []} for the malformed id, collapsing the two cases.
Why: the operation's two absent-input classes (malformed vs. shaped-but-unknown) must stay distinguishable; a client that sent a garbage id is told the sandbox simply has no executions and keeps polling forever instead of fixing the id.
Fix: sandboxId shape validation in the ListCommandExecutionsForSandbox handler, ahead of the listing query (the shaped-but-absent case must stay a 200 empty list).
Traces: codebuild.list-command-executions-for-sandbox-absent-not-found.

Cause 4 of 8: StartBuild accepts an S3 artifactsOverride with no location and starts a build.
AWS: Smithy StartBuild(projectName @required, artifactsOverride ProjectArtifacts, ...) -> {build}; modeled errors AccountLimitExceededException, InvalidInputException, ResourceNotFoundException (client, 400). ProjectArtifacts of type S3 requires a location, so the override is validated before any build is created: HTTP 400, __type InvalidInputException, message 'Invalid artifacts: location is required' (fixed text). The same check also requires that an override naming a non-existent bucket is refused with 'Bucket <bucket name> does not exist' — fixed text 'Bucket ' / ' does not exist' around the bucket name from the request; that leg was never reached because the first call was accepted.
Emulator: HTTP 200 returning a new build (currentPhase QUEUED, buildStatus IN_PROGRESS) with 'artifacts': {} — the override is neither validated nor honored, and a build resource is created where AWS creates none.
Why: the caller believes a build is running and later cannot explain why nothing was uploaded; worse, the refused call leaves real state (a build id, a build number) behind, so 'effect absent' guarantees and build-count assertions break.
Fix: artifactsOverride validation in the StartBuild handler (type/location consistency, then bucket existence) before the build is inserted into state.
Traces: codebuild.start-build-artifacts-override-rejected-without-a-location-and-no-artifacts-uploads-nothing.

Cause 5 of 8: BatchGetBuildBatches reports not-found batches as bare ids instead of the ARN form.
AWS: Smithy BatchGetBuildBatches(ids @required BuildBatchIds @length 0-100) -> {buildBatches, buildBatchesNotFound: BuildBatchIds}; single modeled error InvalidInputException. Recorded behavior: after the batch is deleted, the requested identifier comes back in buildBatchesNotFound in the same ARN form the service uses elsewhere, 'arn:aws:codebuild:<region>:<account>:build-batch/<project>:<uuid>'.
Emulator: HTTP 200 with buildBatchesNotFound carrying the bare '<project>:<uuid>' with no 'arn:aws:codebuild:...:build-batch/' prefix.
Why: clients find missing batches by diffing the ARNs they requested against buildBatchesNotFound; with a bare id the diff matches nothing, so every deleted batch still looks found and delete/verify loops never converge.
Fix: the BatchGetBuildBatches handler/serializer — normalise not-found identifiers to build-batch ARNs, consistently with the found entries.
Traces: codebuild.e2e-build-batch-lifecycle.

Cause 6 of 8: StartBuildBatch creates no sub-builds, so the batch is never orchestrated.
AWS: Smithy StartBuildBatch(projectName @required, buildBatchConfigOverride, ...) -> {buildBatch}; modeled errors InvalidInputException, ResourceNotFoundException. Recorded behavior: for a project whose buildspec declares a batch build-list, the service autonomously creates one sub-build per declared identifier with no further client action — BatchGetBuildBatches(ids=[<batch id>]) grows a buildGroups entry per identifier, each with a currentBuildSummary.arn naming a real, independently retrievable Build that belongs to the same project, and the batch id appears in ListBuildBatches.
Emulator: the batch is created but buildGroups never reaches two entries with currentBuildSummary.arn populated; the check polls BatchGetBuildBatches and never observes a sub-build (the check exhausts its 60s budget).
Why: batch builds are the whole point of StartBuildBatch; a client gets a batch object that never fans out, so no sub-build is ever retrievable and the batch never reaches a terminal outcome.
Fix: the StartBuildBatch handler plus the build-batch orchestration in the service's batch state — expand the batchspec's build-list into sub-build records and publish them in buildGroups[].currentBuildSummary.
Traces: codebuild.start-build-batch-auto-generates-sub-builds.

Cause 7 of 8: a started sandbox stays QUEUED forever, so StartCommandExecution is refused.
AWS: Smithy StartSandbox(projectName, idempotencyToken) -> {sandbox}; StartCommandExecution(sandboxId @required, command @required @sensitive, type) -> {commandExecution}; modeled errors InvalidInputException, ResourceNotFoundException (client, 400). Recorded behavior: a started sandbox leaves QUEUED and reaches a running status on its own, and StartCommandExecution against it is ACCEPTED (the execution then shows up in ListCommandExecutionsForSandbox, and only after StopSandbox is further submission refused).
Emulator: the sandbox remains in status QUEUED, and StartCommandExecution is refused with HTTP 400 InvalidInputException 'Invalid input: sandbox <sandbox id> is not running (status QUEUED) and cannot run a command execution' — a refusal AWS does not issue for a freshly started sandbox.
Why: the sandbox data plane is unusable: every StartCommandExecution on a healthy sandbox fails, so the StartSandbox -> StartCommandExecution -> List -> StopSandbox flow cannot be exercised at all, and the refusal AWS reserves for a stopped sandbox is emitted for a live one.
Fix: the sandbox lifecycle in the StartSandbox handler / sandbox state (advance QUEUED to the running status) rather than the StartCommandExecution precondition, which is correct once the status is right.
Traces: codebuild.e2e-sandbox-command-execution-flow.

Cause 8 of 8: every build FAULTs in PROVISIONING, so no buildspec ever runs.
AWS: Smithy StartBuild -> {build}; Build.phases carries SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE ... COMPLETED and Build.buildStatus reaches SUCCEEDED for a buildspec whose commands exit 0 (and FAILED when a command or a declared artifact fails). Recorded behavior: the build's PROVISIONING phase SUCCEEDs and the buildspec executes, so reports generated by the build are listed for their report group and StartBuild overrides (imageOverride/computeTypeOverride) take effect in the build that ran.
Emulator: every build ends buildStatus 'FAULT' with phases [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT'), ('COMPLETED', None)] and the PROVISIONING phase context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'.
Why: no build-produced state is ever observable — GetReportGroupTrend has no reports to derive a PASS_RATE from, and a per-build image/compute override can never be shown to apply to that build only. FAULT is also the wrong terminal status for an unrunnable build: a client retries a service fault where AWS would have reported success or FAILED.
Fix: build execution in the StartBuild/build-lifecycle component of the service crate (the build state machine that advances PROVISIONING), not in the per-operation serializers. Open issue 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md describes this same execution gap and owns OTHER checks in this suite, but carries no replay key for the two checks below.
Traces: codebuild.report-group-trend-reflects-report-data, codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 64/182 passed; codebuild 63/136 (73 failed). Per cause, expected (recorded AWS) vs actual (emulator):

Cause 1 — codebuild.batch-get-fleets-invalid-arn-rejected: BatchGetFleets(names=['not a valid fleet name!!']) expected 400 InvalidInputException 'Invalid characters in fleet name. Only alphanumeric characters, dash, and underscore are supported'; actual "expected the call to be rejected, but it succeeded and returned {'fleets': [], 'fleetsNotFound': [], ... 'HTTPStatusCode': 200}".

Cause 2 — codebuild.delete-resource-policy-absent-policy-rejected: DeleteResourcePolicy(resourceArn=<real report-group ARN>+'-does-not-exist-suffix') expected 400 ResourceNotFoundException 'Resource ARN does not exist'; actual "expected the call to be rejected, but it succeeded: {... 'HTTPStatusCode': 200, 'content-length': '2'}". The preceding GetResourcePolicy baseline legs passed.

Cause 3 — codebuild.list-command-executions-for-sandbox-absent-not-found: ListCommandExecutionsForSandbox(sandboxId=<bare, unstructured name>) expected 400 InvalidInputException 'Invalid sandbox ID provided'; actual "succeeded: {'commandExecutions': [], ... 'HTTPStatusCode': 200}".

Cause 4 — codebuild.start-build-artifacts-override-rejected-without-a-location-and-no-artifacts-uploads-nothing: StartBuild(projectName=<project>, artifactsOverride={'type': 'S3'}) expected 400 InvalidInputException 'Invalid artifacts: location is required'; actual "expected the call to be refused, but it succeeded" returning a build with currentPhase 'QUEUED', buildStatus 'IN_PROGRESS', 'artifacts': {}. The follow-on leg StartBuild(artifactsOverride={'type':'S3','location':<missing bucket>}) expecting 'Bucket <bucket> does not exist' was never reached.

Cause 5 — codebuild.e2e-build-batch-lifecycle: after DeleteBuildBatch, BatchGetBuildBatches(ids=[<batch ARN>]) expected the requested ARN in buildBatchesNotFound; actual "expected arn:aws:codebuild:us-west-2:000000000000:build-batch/<project>:<uuid> in buildBatchesNotFound, got ['<project>:<uuid>']".

Cause 6 — codebuild.start-build-batch-auto-generates-sub-builds: StartBuildBatch on a project with a build-list batchspec; polling BatchGetBuildBatches for two buildGroups each with currentBuildSummary.arn never succeeded and the check exhausted its 60s budget ("Check timed out after 60.0s").

Cause 7 — codebuild.e2e-sandbox-command-execution-flow: StartCommandExecution after StartSandbox raised botocore.errorfactory.InvalidInputException: 'An error occurred (InvalidInputException) when calling the StartCommandExecution operation: Invalid input: sandbox <project>:<uuid> is not running (status QUEUED) and cannot run a command execution'.

Cause 8 — codebuild.report-group-trend-reflects-report-data: "build did not succeed" with buildStatus 'FAULT', phases SUBMITTED/SUCCEEDED, QUEUED/SUCCEEDED, PROVISIONING/FAULT, COMPLETED, contexts [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}]. codebuild.start-build-image-and-compute-overrides-apply-to-that-build-only: "the overridden build ended 'FAULT'; phases [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT'), ('COMPLETED', None)]".

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — BatchGetFleets, DeleteResourcePolicy, ListCommandExecutionsForSandbox, StartBuild, BatchGetBuildBatches, StartBuildBatch, StartSandbox, StartCommandExecution signatures and modeled errors (all client, httpError 400).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the behavioral suite and read the codebuild failures, or reproduce each cause directly:
3. Cause 1: BatchGetFleets(names=['not a valid fleet name!!']) — expect 400 InvalidInputException; observe 200 with empty fleets/fleetsNotFound.
4. Cause 2: CreateReportGroup, then DeleteResourcePolicy(resourceArn=<that ARN>+'-does-not-exist-suffix') — expect 400 ResourceNotFoundException 'Resource ARN does not exist'; observe 200.
5. Cause 3: ListCommandExecutionsForSandbox(sandboxId='<a bare name with no :uuid>') — expect 400 InvalidInputException 'Invalid sandbox ID provided'; then the same call with '<project>:<uuid>' must still return 200 with an empty list.
6. Cause 4: CreateProject with S3 artifacts, then StartBuild(projectName=<it>, artifactsOverride={'type':'S3'}) — expect 400 InvalidInputException 'Invalid artifacts: location is required' and NO new build; then artifactsOverride={'type':'S3','location':<nonexistent bucket>} — expect 'Bucket <bucket> does not exist'.
7. Cause 5: StartBuildBatch, DeleteBuildBatch, then BatchGetBuildBatches(ids=[<the batch ARN>]) — expect the ARN echoed in buildBatchesNotFound.
8. Cause 6: CreateProject with a batchspec declaring build-list identifiers build1/build2 and a buildBatchConfig, StartBuildBatch, then poll BatchGetBuildBatches(ids=[<batch id>]) — expect two buildGroups with currentBuildSummary.arn and both ARNs retrievable via BatchGetBuilds.
9. Cause 7: StartSandbox(projectName=<project>) then StartCommandExecution(sandboxId=<sandbox id>, command='echo hi') — expect acceptance; observe InvalidInputException 'sandbox ... is not running (status QUEUED)'.
10. Cause 8: StartBuild on any NO_SOURCE project whose buildspec exits 0 and poll BatchGetBuilds — expect buildStatus SUCCEEDED; observe FAULT with PROVISIONING/FAULT.
