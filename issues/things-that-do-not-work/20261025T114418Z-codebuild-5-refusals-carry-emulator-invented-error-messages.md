Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.cbnv-update-removing-the-buildspec-rejected-and-keeps-the-version codebuild.create-fleet-invalid-capacity-rejected codebuild.create-webhook-rejected-for-no-source-project codebuild.list-builds-for-project-invalid-sort-order-rejected codebuild.start-command-execution-invalid-sandbox-rejected
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.cbnv-update-removing-the-buildspec-rejected-and-keeps-the-version|run#8|AssertionError|assert (code, message) == ("InvalidInputException", expected), (
Issue-Key: codebuild.create-fleet-invalid-capacity-rejected|run#7|AssertionError|assert (code, message) == (
Issue-Key: codebuild.create-webhook-rejected-for-no-source-project|run#8|AssertionError|assert (code, message) == (
Issue-Key: codebuild.list-builds-for-project-invalid-sort-order-rejected|run#10|AssertionError|assert message == "Invalid sort order specified", message
Issue-Key: codebuild.start-command-execution-invalid-sandbox-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 7ec46f8e53e7b4a82f0a526dc76e942bfb70fd11

# CodeBuild: 5 refusals carry emulator-invented error messages instead of CodeBuild's own wording (UpdateProject NO_SOURCE buildspec, CreateFleet baseCapacity, CreateWebhook source type, ListBuildsForProject sortOrder, StartCommandExecution absent sandbox)

> _Note: this issue was identified against the code base at commit `7ec46f8e53e7b4a82f0a526dc76e942bfb70fd11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFleet

## Description
FIVE distinct net-new root causes in the `codebuild` behavioral suite, all in the CodeBuild service crate services/service-com-amazonaws-codebuild-codebuild-20161006. All five must be closed before this issue is resolved. In every one of them the emulator reaches the RIGHT refusal decision and the RIGHT modeled error code and HTTP status, but serializes a message string it invented itself instead of the message CodeBuild emits for that error class. An error message is part of the contract: clients, CLIs and SDK retry/branching wrappers surface and match on it. Each cause lives in a different validation site, so each needs its own repair. Checks the replay report marked ALREADY FILED or OWNED are excluded here; the open codebuild issues listed in the suite report (20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md, 20261025T014438Z-codebuild-8-further-divergences-....md, 20261025T062553Z-codebuild-6-further-divergences-....md) carry none of the five replay keys below, and none of them describes a message-wording defect.

Cause 1 of 5: UpdateProject's NO_SOURCE buildspec-required refusal uses the wrong message.
AWS: Smithy UpdateProject(name @required NonEmptyString; source ProjectSource) -> { project }, modeled errors InvalidInputException and ResourceNotFoundException (both client, httpError 400). Emptying the buildspec of a project whose source type is NO_SOURCE is refused with 400 InvalidInputException, message `Invalid input: a buildspec is required for source type NO_SOURCE`. Fixed text: the whole string, including the `Invalid input: ` prefix CodeBuild puts on this class of InvalidInputException; the only variable part is the source type token (`NO_SOURCE`), taken from the project's stored source.type. The cross-field rule itself (NO_SOURCE has no repository to hold a buildspec file, so source.buildspec must be non-empty) is not modeled; the message text is recorded behavior only.
Emulator: 400 InvalidInputException with message `A buildspec must be provided when the project source type is NO_SOURCE, which has no source code to hold a buildspec file` — a prose sentence of the emulator's own composition, with no `Invalid input: ` prefix.
Why: a client that branches on, logs or asserts the refusal reason gets text AWS never emits; tooling that string-matches `Invalid input:` to classify request-shape errors mis-sorts this refusal, and the project's stored buildspec version is reported as kept for a reason the operator cannot match to AWS documentation.
Fix: the message constructed by the NO_SOURCE/buildspec cross-field validation behind the UpdateProject handler (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/update_project.rs and the shared project-source validation it calls), which must also be used by create_project.rs for the same rule.
Traces: codebuild.cbnv-update-removing-the-buildspec-rejected-and-keeps-the-version.

Cause 2 of 5: CreateFleet's baseCapacity floor refusal uses the wrong message.
AWS: Smithy CreateFleet(name @required FleetName, baseCapacity @required FleetCapacity integer, environmentType @required, computeType @required) -> { fleet }, modeled errors AccountLimitExceededException, InvalidInputException, ResourceAlreadyExistsException (all client, httpError 400). FleetCapacity carries no @range in the model, so the floor is enforced by the service, not by constraint validation: baseCapacity below 1 is refused with 400 InvalidInputException, message `Capacity must be greater or equal to 1`. That message is entirely fixed text — it names neither the member, the submitted value nor the fleet, so nothing in it comes from the request. An otherwise identical request with baseCapacity=1 is accepted, and the refused request creates no fleet (confirmed by an independent ListFleets read, which the emulator already gets right).
Emulator: 400 InvalidInputException with message `Invalid input: baseCapacity must be at least 1, got 0` — it adds an `Invalid input: ` prefix AWS does not use here, names the member, and interpolates the submitted value.
Why: the message is unmatchable against AWS, and because it echoes the submitted value it also changes shape per request, so a client cannot key on it at all.
Fix: the baseCapacity floor check in the CreateFleet handler's validation (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/create_fleet.rs) must emit CodeBuild's fixed sentence rather than a composed one.
Traces: codebuild.create-fleet-invalid-capacity-rejected.

Cause 3 of 5: CreateWebhook's unsupported-source-type refusal uses the wrong message.
AWS: Smithy CreateWebhook(projectName @required ProjectName) -> { webhook }, modeled errors InvalidInputException, OAuthProviderException, ResourceAlreadyExistsException, ResourceNotFoundException (all client, httpError 400). CreateWebhookInput models only projectName, so the source-type precondition is service-side: a project whose source type cannot receive a webhook (NO_SOURCE) is refused with 400 InvalidInputException, message `Invalid build type provided for NO_SOURCE type webhook` — fixed text whose only variable part is the source type token taken from the project's stored source.type. Message text is recorded behavior only. Nothing is created and the project keeps no `webhook` member (which the emulator already gets right).
Emulator: 400 InvalidInputException with message `A webhook cannot be created for project <project ARN>: its source type NO_SOURCE does not support webhooks. Supported source types are BITBUCKET, GITHUB, GITHUB_ENTERPRISE, GITLAB, GITLAB_SELF_MANAGED` — invented prose that additionally interpolates the project ARN (region/account/project name all per-environment), so the string differs on every run and account.
Why: the refusal reason is unmatchable, and embedding the full ARN makes the message environment-specific where AWS's is constant. Note this is NOT the credential precondition already filed as Cause 6 of 20261025T014438Z-codebuild-8-further-divergences-getresourcepolicy-updatewebhook-deletebuildbatch-stubs.md: here the refusal correctly happens, only its message diverges.
Fix: the source-type precondition's message in services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/create_webhook.rs.
Traces: codebuild.create-webhook-rejected-for-no-source-project.

Cause 4 of 5: ListBuildsForProject answers an invalid sortOrder with generic constraint-validation plumbing text instead of CodeBuild's own message.
AWS: Smithy ListBuildsForProject(projectName @required NonEmptyString, sortOrder SortOrderType enum, nextToken) -> { ids, nextToken }, modeled errors InvalidInputException and ResourceNotFoundException (both client, httpError 400). An unmodeled sortOrder value is refused with 400 InvalidInputException, message `Invalid sort order specified` — wholly fixed text: CodeBuild does not quote the rejected value, the member path or the permitted enum set. Message text is recorded behavior only; the enum SortOrderType [ASCENDING, DESCENDING] is modeled.
Emulator: the error CODE is correct (InvalidInputException) but the message is the generic framework string `1 validation error detected: Value 'NOT_A_VALID_ORDER' at 'sortOrder' failed to satisfy constraint: Member must satisfy enum value set: [ASCENDING, DESCENDING]`. Fixed part of that emulator text is the `N validation error detected: ... failed to satisfy constraint: Member must satisfy enum value set: [...]` template; the quoted value is the caller's, which means the emulator is routing this refusal through a shared Smithy-constraint validator rather than through the operation's own validation.
Why: the message leaks the emulator's internal validation layer, is per-request variable where AWS's is constant, and tells a client the error came from generic request validation rather than from CodeBuild's sortOrder handling. The rejected call correctly leaves no phantom builds and the ASCENDING control still succeeds, so only the message diverges.
Fix: the sortOrder handling in the ListBuildsForProject handler (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/list_builds_for_project.rs) must raise CodeBuild's own InvalidInputException message instead of letting the shared enum-constraint validator's message reach the wire; the same applies wherever SortOrderType is validated on sibling list operations.
Traces: codebuild.list-builds-for-project-invalid-sort-order-rejected.

Cause 5 of 5: StartCommandExecution's absent-sandbox refusal uses the wrong message.
AWS: Smithy StartCommandExecution(sandboxId @required NonEmptyString @length min 1, command @required SensitiveNonEmptyString, type CommandType) -> { commandExecution }, modeled errors InvalidInputException and ResourceNotFoundException (both client, httpError 400). A well-formed but nonexistent sandboxId (the documented `<project-name>:<uuid>` shape) is refused with 400 ResourceNotFoundException, message `Sandbox does not exist` — wholly fixed text that names no identifier. Message text is recorded behavior only.
Emulator: 400 ResourceNotFoundException with message `Sandbox cannot be found: <sandbox ARN>`; the fixed part is `Sandbox cannot be found: ` and the appended value is a sandbox ARN assembled from region, account, project name and uuid, i.e. per-run and per-environment.
Why: the refusal reason is unmatchable against AWS and, because the appended ARN varies per request, cannot be keyed on at all; it also discloses an ARN for a resource that does not exist.
Fix: the not-found path in services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_command_execution.rs (and the shared sandbox-lookup helper it uses, so the sibling sandbox operations report the same sentence).
Traces: codebuild.start-command-execution-invalid-sandbox-rejected.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout: 27/110 passed (24.5%), 65 mismatched, 18 errored, 0 skipped. All five causes below are codebuild checks the report marked neither ALREADY FILED nor OWNED.

Cause 1 — codebuild.cbnv-update-removing-the-buildspec-rejected-and-keeps-the-version, assertion `(code, message) == ("InvalidInputException", expected)`:
  AssertionError: emptying the buildspec returned 'InvalidInputException': 'A buildspec must be provided when the project source type is NO_SOURCE, which has no source code to hold a buildspec file', expected InvalidInputException 'Invalid input: a buildspec is required for source type NO_SOURCE'
  -> code correct, message diverges.

Cause 2 — codebuild.create-fleet-invalid-capacity-rejected, assertion `(code, message) == ("InvalidInputException", "Capacity must be greater or equal to 1")`:
  AssertionError: ('InvalidInputException', 'Invalid input: baseCapacity must be at least 1, got 0')
  -> code correct, message diverges. The check's positive control (baseCapacity=1) and its ListFleets effect-absent read both passed.

Cause 3 — codebuild.create-webhook-rejected-for-no-source-project, assertion `(code, message) == ("InvalidInputException", "Invalid build type provided for NO_SOURCE type webhook")`:
  AssertionError: ('InvalidInputException', 'A webhook cannot be created for project arn:aws:codebuild:<region>:<account>:project/<project-name>: its source type NO_SOURCE does not support webhooks. Supported source types are BITBUCKET, GITHUB, GITHUB_ENTERPRISE, GITLAB, GITLAB_SELF_MANAGED')
  -> code correct, refusal correct, message diverges (and embeds a per-environment ARN).

Cause 4 — codebuild.list-builds-for-project-invalid-sort-order-rejected. The preceding `assert code == "InvalidInputException"` PASSED; the failure is the next line, `assert message == "Invalid sort order specified"`:
  AssertionError: 1 validation error detected: Value 'NOT_A_VALID_ORDER' at 'sortOrder' failed to satisfy constraint: Member must satisfy enum value set: [ASCENDING, DESCENDING]
  -> code correct, message is the generic constraint-validator text.

Cause 5 — codebuild.start-command-execution-invalid-sandbox-rejected. The preceding `assert code == "ResourceNotFoundException"` PASSED; the failure is the message assertion:
  AssertionError: expected message 'Sandbox does not exist', got 'Sandbox cannot be found: arn:aws:codebuild:<region>:<account>:sandbox/<project-name>:<uuid>'
  -> code correct, message diverges (and embeds a per-run ARN).

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — UpdateProject (name @required, source ProjectSource; errors InvalidInputException, ResourceNotFoundException, client/400), CreateFleet (baseCapacity @required FleetCapacity integer with NO @range; errors AccountLimitExceededException, InvalidInputException, ResourceAlreadyExistsException, client/400), CreateWebhook (projectName @required only; errors InvalidInputException, OAuthProviderException, ResourceAlreadyExistsException, ResourceNotFoundException, client/400), ListBuildsForProject (sortOrder SortOrderType enum [ASCENDING, DESCENDING]; errors InvalidInputException, ResourceNotFoundException, client/400), StartCommandExecution (sandboxId @required NonEmptyString @length min 1, command @required SensitiveNonEmptyString; errors InvalidInputException, ResourceNotFoundException, client/400). None of the five message strings is stated by the model, so each expected message is recorded behavior; the error codes and 400 statuses are modeled and the emulator already matches them.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the behavioral suite (run_behavioral_suite) and look at the five `codebuild` checks named in Issue-Trace-Ids, or reproduce each cause directly against the emulator endpoint:
3. Cause 1: create an IAM role assumable by codebuild.amazonaws.com; CreateProject(name=<p>, source={type: NO_SOURCE, buildspec: <a JSON/YAML buildspec>}, artifacts={type: NO_ARTIFACTS}, environment={type: LINUX_CONTAINER, image: aws/codebuild/amazonlinux2-x86_64-standard:5.0, computeType: BUILD_GENERAL1_SMALL}, serviceRole=<role arn>); then UpdateProject(name=<p>, source={type: NO_SOURCE, buildspec: ""}). Expect 400 InvalidInputException with message exactly `Invalid input: a buildspec is required for source type NO_SOURCE`; observe `A buildspec must be provided when the project source type is NO_SOURCE, which has no source code to hold a buildspec file`.
4. Cause 2: CreateFleet(name=<f1>, baseCapacity=1, environmentType=LINUX_CONTAINER, computeType=BUILD_GENERAL1_SMALL) — accepted. Then CreateFleet(name=<f2>, baseCapacity=0, environmentType=LINUX_CONTAINER, computeType=BUILD_GENERAL1_SMALL). Expect 400 InvalidInputException with message exactly `Capacity must be greater or equal to 1`; observe `Invalid input: baseCapacity must be at least 1, got 0`.
5. Cause 3: CreateProject with source {type: NO_SOURCE, buildspec: <inline>} as in step 3, then CreateWebhook(projectName=<p>). Expect 400 InvalidInputException with message exactly `Invalid build type provided for NO_SOURCE type webhook`; observe the prose message naming the project ARN and listing supported source types.
6. Cause 4: with the NO_SOURCE project from step 3, call ListBuildsForProject(projectName=<p>, sortOrder='NOT_A_VALID_ORDER'). Expect 400 InvalidInputException with message exactly `Invalid sort order specified`; observe `1 validation error detected: Value 'NOT_A_VALID_ORDER' at 'sortOrder' failed to satisfy constraint: Member must satisfy enum value set: [ASCENDING, DESCENDING]`. Confirm ListBuildsForProject(projectName=<p>, sortOrder='ASCENDING') still returns [].
7. Cause 5: StartCommandExecution(sandboxId='<any-project-name>:<a random uuid4>', command='echo hi') with no such sandbox. Expect 400 ResourceNotFoundException with message exactly `Sandbox does not exist`; observe `Sandbox cannot be found: arn:aws:codebuild:<region>:<account>:sandbox/<project-name>:<uuid>`.
