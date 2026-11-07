Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.build-executes-buildspec-and-exports-computed-variable codebuild.build-failing-command-reaches-failed-terminal codebuild.build-lifecycle-transitions-to-terminal-state codebuild.cbnv-unknown-runtime-name-build-rejected-as-yaml-file-error codebuild.codebuild-concurrent-build-limit-rejects-the-second-start-and-no-second-build-appears codebuild.codebuild-environment-variable-precedence-resolves-inside-the-build codebuild.codebuild-invalid-buildspec-yaml-rejected-before-the-build-phase-runs codebuild.failing-pre-build-phase-skips-later-phases-and-reports-the-exit-code codebuild.on-failure-continue-runs-the-next-phase-while-the-failed-phase-stays-failed codebuild.phase-durations-equal-their-own-windows-and-the-slow-phase-dominates codebuild.retry-build-account-limit-exceeded-blocked codebuild.stop-build-during-pre-build-stops-exactly-that-phase-and-leaves-the-rest-unrun codebuild.stop-build-on-completed-build-is-an-idempotent-no-op codebuild.stop-build-race-idempotent-single-transition codebuild.timeout-in-minutes-kills-the-running-build-and-marks-that-phase-timed-out codebuild.batch-delete-builds-partial-failure-isolated codebuild.batch-delete-builds-empty-ids-rejected codebuild.batch-get-builds-id-limit-and-not-found-partition codebuild.cbnv-batch-get-projects-over-one-hundred-names-rejected codebuild.create-project-timeout-above-maximum-rejected codebuild.update-project-visibility-invalid-enum-rejected codebuild.update-project-visibility-invalid-value-rejected codebuild.list-builds-for-project-absent-project-not-found codebuild.list-builds-for-project-nonexistent-project-rejected codebuild.stop-build-nonexistent-id-rejected codebuild.batch-get-builds-invalid-id-format-rejected codebuild.create-project-invalid-role-rejected codebuild.start-build-unassumable-role-override-rejected codebuild.create-project-s3-source-without-location-rejected codebuild.cbnv-batch-get-projects-by-arn-matches-by-name codebuild.cbnv-cdk-json-buildspec-round-trips-byte-for-byte codebuild.cbnv-three-projects-each-report-their-own-nodejs-version codebuild.cbnv-project-environment-defaults-applied codebuild.cbnv-undeclared-project-blocks-stay-absent codebuild.list-builds-for-project-invalid-sort-order-rejected codebuild.deploy-codebuild-project-cfn-starts-build codebuild.deploy-project-start-build-succeeds codebuild.e2e-cbnv-check-codebuild-project-nodejs-version-lifecycle codebuild.deploy-fleet-list-reflects-fleet codebuild.delete-webhook-absent-project-not-found codebuild.get-report-group-trend-absent-report-group-not-found codebuild.invalidate-project-cache-absent-not-found codebuild.list-build-batches-for-project-absent-not-found codebuild.list-command-executions-for-sandbox-absent-not-found codebuild.list-sandboxes-for-project-absent-not-found codebuild.start-build-batch-malformed-input-rejected codebuild.start-build-batch-without-batch-config-rejected codebuild.start-command-execution-invalid-sandbox-rejected codebuild.start-sandbox-account-suspended-blocked codebuild.start-sandbox-connection-absent-sandbox-not-found codebuild.start-sandbox-suspended-account-blocked codebuild.sandbox-timeout-minutes-preserved codebuild.batch-get-fleets-invalid-arn-rejected codebuild.put-resource-policy-malformed-policy-rejected codebuild.put-resource-policy-unsupported-resource-type-rejected codebuild.resource-policy-malformed-json-rejected codebuild.deploy-codebuild-report-group-put-resource-policy-malformed-rejected codebuild.codebuild-sandbox-command-execution-returns-the-commands-real-stdout
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.build-executes-buildspec-and-exports-computed-variable|run#11|AssertionError|assert final["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.build-failing-command-reaches-failed-terminal|run#9|AssertionError|assert bad_final["buildStatus"] == "FAILED", (
Issue-Key: codebuild.build-lifecycle-transitions-to-terminal-state|run#9|AssertionError|assert build_a["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.cbnv-unknown-runtime-name-build-rejected-as-yaml-file-error|run#10|AssertionError|assert builds[good]["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.codebuild-concurrent-build-limit-rejects-the-second-start-and-no-second-build-appears|run#9|AssertionError|assert not live.get("buildComplete"), (
Issue-Key: codebuild.codebuild-environment-variable-precedence-resolves-inside-the-build|run#10|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.codebuild-invalid-buildspec-yaml-rejected-before-the-build-phase-runs|run#9|AssertionError|assert bad["buildStatus"] == "FAILED", (
Issue-Key: codebuild.failing-pre-build-phase-skips-later-phases-and-reports-the-exit-code|run#20|AssertionError|assert good["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.on-failure-continue-runs-the-next-phase-while-the-failed-phase-stays-failed|run#19|AssertionError|assert pre is not None, (
Issue-Key: codebuild.phase-durations-equal-their-own-windows-and-the-slow-phase-dominates|run#9|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.retry-build-account-limit-exceeded-blocked|run#11|AssertionError|assert build_a["buildStatus"] == "SUCCEEDED", f"build A did not succeed: {build_a}"
Issue-Key: codebuild.stop-build-during-pre-build-stops-exactly-that-phase-and-leaves-the-rest-unrun|_poll_until_phase#13|AssertionError|assert build.get("buildComplete") is not True, (
Issue-Key: codebuild.stop-build-on-completed-build-is-an-idempotent-no-op|run#6|AssertionError|assert final["buildStatus"] == "SUCCEEDED", f"expected SUCCEEDED, got {final['buildStatus']!r}"
Issue-Key: codebuild.stop-build-race-idempotent-single-transition|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: codebuild.timeout-in-minutes-kills-the-running-build-and-marks-that-phase-timed-out|run#15|AssertionError|assert ok["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.batch-delete-builds-partial-failure-isolated|run#11|AssertionError|assert len(still_running) == 1 and not still_running[0]["buildComplete"], (
Issue-Key: codebuild.batch-delete-builds-empty-ids-rejected|run#11|AssertionError|assert (code, message) == (
Issue-Key: codebuild.batch-get-builds-id-limit-and-not-found-partition|run#11|AssertionError|assert (code, message) == (
Issue-Key: codebuild.cbnv-batch-get-projects-over-one-hundred-names-rejected|run#9|AssertionError|assert (code, message) == ("InvalidInputException", expected), (
Issue-Key: codebuild.create-project-timeout-above-maximum-rejected|run#6|AssertionError|assert (code, message) == (
Issue-Key: codebuild.update-project-visibility-invalid-enum-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codebuild.update-project-visibility-invalid-value-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codebuild.list-builds-for-project-absent-project-not-found|run#5|AssertionError|assert message == f"The provided project arn:aws:codebuild:{ctx.region}:{ctx.account}:project/{missing_name} does not exist.", message
Issue-Key: codebuild.list-builds-for-project-nonexistent-project-rejected|run#5|AssertionError|assert message == f"The provided project arn:aws:codebuild:{ctx.region}:{ctx.account}:project/{bogus_name} does not exist.", message
Issue-Key: codebuild.stop-build-nonexistent-id-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: codebuild.batch-get-builds-invalid-id-format-rejected|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codebuild.create-project-invalid-role-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codebuild.start-build-unassumable-role-override-rejected|_client_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: codebuild.create-project-s3-source-without-location-rejected|_error#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codebuild.cbnv-batch-get-projects-by-arn-matches-by-name|run#8|AssertionError|assert by_arn["projectsNotFound"] == [], (
Issue-Key: codebuild.cbnv-cdk-json-buildspec-round-trips-byte-for-byte|run#8|KeyError|assert got["projectsNotFound"] == [], f"projectsNotFound={got['projectsNotFound']!r}"
Issue-Key: codebuild.cbnv-three-projects-each-report-their-own-nodejs-version|run#6|KeyError|assert got["projectsNotFound"] == [], f"projectsNotFound={got['projectsNotFound']!r}"
Issue-Key: codebuild.cbnv-project-environment-defaults-applied|run#8|AssertionError|assert env.get("privilegedMode") is False, f"privilegedMode={env.get('privilegedMode')!r}"
Issue-Key: codebuild.cbnv-undeclared-project-blocks-stay-absent|run#12|AssertionError|assert proj.get("tags") == [], f"an untagged project reports tags {proj.get('tags')!r}"
Issue-Key: codebuild.list-builds-for-project-invalid-sort-order-rejected|run#6|KeyError|before = cb.list_builds_for_project(projectName=project_name)["ids"]
Issue-Key: codebuild|StartBuild|ResourceNotFoundException
Issue-Key: codebuild.deploy-project-start-build-succeeds|run#8|AssertionError|assert build_id is not None, f"start_build never succeeded: {last_err}"
Issue-Key: codebuild.e2e-cbnv-check-codebuild-project-nodejs-version-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: codebuild.deploy-fleet-list-reflects-fleet|_deploy_cfn#2|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}
Issue-Key: codebuild.delete-webhook-absent-project-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: codebuild.get-report-group-trend-absent-report-group-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: codebuild.invalidate-project-cache-absent-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: codebuild.list-build-batches-for-project-absent-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: codebuild.list-command-executions-for-sandbox-absent-not-found|run#4|AssertionError|assert malformed_code == "InvalidInputException", f"expected InvalidInputException, got {malformed_code}"
Issue-Key: codebuild.list-sandboxes-for-project-absent-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: codebuild.start-build-batch-malformed-input-rejected|run#8|AssertionError|assert code == "InvalidInputException", f"unexpected code: {code}"
Issue-Key: codebuild.start-build-batch-without-batch-config-rejected|run#11|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code!r}"
Issue-Key: codebuild.start-command-execution-invalid-sandbox-rejected|run#5|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: codebuild.start-sandbox-account-suspended-blocked|run#5|AssertionError|assert code == "ResourceNotFoundException", f"unexpected code: {code}"
Issue-Key: codebuild.start-sandbox-connection-absent-sandbox-not-found|run#4|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code}"
Issue-Key: codebuild.start-sandbox-suspended-account-blocked|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: codebuild.sandbox-timeout-minutes-preserved|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: codebuild.batch-get-fleets-invalid-arn-rejected|run#4|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code!r}"
Issue-Key: codebuild.put-resource-policy-malformed-policy-rejected|run#9|AssertionError|assert code == "InvalidInputException", f"unexpected code: {code}"
Issue-Key: codebuild.put-resource-policy-unsupported-resource-type-rejected|run#11|AssertionError|assert code == "InvalidInputException"
Issue-Key: codebuild.resource-policy-malformed-json-rejected|run#11|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code}"
Issue-Key: codebuild.deploy-codebuild-report-group-put-resource-policy-malformed-rejected|run#17|AssertionError|assert code == "InvalidInputException", code
Issue-Key: codebuild.codebuild-sandbox-command-execution-returns-the-commands-real-stdout|_start_sandbox_with_retry#8|Timeout|time.sleep(5)
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeBuild: builds never execute (every build FAULTs at PROVISIONING) plus 9 further control-plane divergences

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Ten distinct root causes found in the `codebuild` behavioral suite (58 failing checks after skipping the 15 clusters the report marks ALREADY FILED). All ten live in the CodeBuild service crate services/service-com-amazonaws-codebuild-codebuild-20161006.

Cause 1 of 10: StartBuild never runs the build — every build terminates FAULT at PROVISIONING.
AWS: Smithy StartBuild returns a Build; BuildPhaseType enumerates SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED and StatusType enumerates SUCCEEDED, FAILED, FAULT, TIMED_OUT, IN_PROGRESS, STOPPED. A real build walks those phases, honours the buildspec (exit 0 -> buildStatus SUCCEEDED, non-zero -> FAILED, on-failure: ABORT|CONTINUE, finally blocks), reports per-phase startTime/endTime/durationInSeconds, exportedEnvironmentVariables, logs and artifacts, and respects timeoutInMinutes (TIMED_OUT) and StopBuild (STOPPED, with a BUILD phase actually observable while in progress).
Emulator: every build goes SUBMITTED(SUCCEEDED) -> QUEUED(SUCCEEDED) -> PROVISIONING(FAULT) -> COMPLETED, buildStatus FAULT, all durations 0, and the PROVISIONING phase carries the context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. No DOWNLOAD_SOURCE/INSTALL/PRE_BUILD/BUILD/POST_BUILD/UPLOAD_ARTIFACTS/FINALIZING phase is ever reported, SUCCEEDED/FAILED/TIMED_OUT/STOPPED are unreachable, and no exported environment variables appear. Checks that poll for currentPhase == 'BUILD' never observe it and exhaust their budget (stop-build-race-idempotent-single-transition timed out at 60s for exactly that reason).
Why: a client cannot distinguish a passing build from a failing one; CI logic keyed on buildStatus SUCCEEDED/FAILED, phase progression, exit codes, timeouts, concurrency slots or StopBuild semantics all break, and FAULT is the code AWS reserves for an internal/infrastructure failure, so callers see every build as a service fault.
Fix: the build state machine behind the StartBuild handler and the build projection (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs plus the build/phase state it writes, read back by batch_get_builds.rs). A deterministic simulated phase walk driven by the project's buildspec is what the suite grades, not a real container.
Traces: codebuild.build-executes-buildspec-and-exports-computed-variable, codebuild.build-failing-command-reaches-failed-terminal, codebuild.build-lifecycle-transitions-to-terminal-state, codebuild.cbnv-unknown-runtime-name-build-rejected-as-yaml-file-error, codebuild.codebuild-concurrent-build-limit-rejects-the-second-start-and-no-second-build-appears, codebuild.codebuild-environment-variable-precedence-resolves-inside-the-build, codebuild.codebuild-invalid-buildspec-yaml-rejected-before-the-build-phase-runs, codebuild.failing-pre-build-phase-skips-later-phases-and-reports-the-exit-code, codebuild.on-failure-continue-runs-the-next-phase-while-the-failed-phase-stays-failed, codebuild.phase-durations-equal-their-own-windows-and-the-slow-phase-dominates, codebuild.retry-build-account-limit-exceeded-blocked, codebuild.stop-build-during-pre-build-stops-exactly-that-phase-and-leaves-the-rest-unrun, codebuild.stop-build-on-completed-build-is-an-idempotent-no-op, codebuild.stop-build-race-idempotent-single-transition, codebuild.timeout-in-minutes-kills-the-running-build-and-marks-that-phase-timed-out, codebuild.batch-delete-builds-partial-failure-isolated.

Cause 2 of 10: InvalidInputException messages are generic generated constraint text instead of CodeBuild's own sentences.
AWS: the code (InvalidInputException, HTTP 400) is right, but CodeBuild authors its own message per rule, built from the request: an empty BatchDeleteBuilds ids list -> 'The input build ID list cannot be empty.'; a >100-entry ids/names list -> 'The maximum length of input resource list is 100 but input list with size 101 is found.' (Smithy BuildIds/ProjectNames @length max 100); CreateProject timeoutInMinutes above BuildTimeOut @range max 2160 -> 'Invalid timeout. Should be between 5 minutes and 36 hours'. For an unmodeled projectVisibility value the recorded message enumerates the set as '[PRIVATE, PUBLIC_READ]' (recorded behavior only — the Smithy ProjectVisibilityType declaration order is the reverse).
Emulator: emits the framework's generated constraint text instead, e.g. "1 validation error detected: Value '0 items' at 'ids' failed to satisfy constraint: Member must have length greater than or equal to 1", "Value '101 items' at 'ids' ... less than or equal to 100", "Value '2161' at 'timeoutInMinutes' ... less than or equal to 2160", and '... enum value set: [PUBLIC_READ, PRIVATE]'.
Why: the status and code match, so this only breaks clients and tests that match on CodeBuild's documented sentence; the recorded strings above are examples of the per-rule sentences, not a per-request literal.
Fix: the message construction in CodeBuild's input-validation layer (the shared validation/error-rendering helper used by batch_delete_builds.rs, batch_get_builds.rs, batch_get_projects.rs, create_project.rs and update_project_visibility.rs).
Traces: codebuild.batch-delete-builds-empty-ids-rejected, codebuild.batch-get-builds-id-limit-and-not-found-partition, codebuild.cbnv-batch-get-projects-over-one-hundred-names-rejected, codebuild.create-project-timeout-above-maximum-rejected, codebuild.update-project-visibility-invalid-enum-rejected, codebuild.update-project-visibility-invalid-value-rejected.

Cause 3 of 10: ResourceNotFoundException messages use a non-AWS sentence form.
AWS: ResourceNotFoundException (HTTP 400) with a message built from the request — for a missing project 'The provided project arn:aws:codebuild:<region>:<account>:project/<name> does not exist.' and for a missing build 'Build <project>:<uuid> does not exist'.
Emulator: 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/<name>' and 'Build cannot be found: arn:aws:codebuild:us-west-2:000000000000:build/<project>:<uuid>' — a generic 'X cannot be found: <arn>' form, and for builds it substitutes an ARN where AWS names the build id.
Why: the code and status are right, so only message-matching clients break; but the build case also changes the identifier kind (ARN vs build id), which callers do parse.
Fix: the not-found error construction in the ListBuildsForProject and StopBuild handlers (services/.../src/ops/list_builds_for_project.rs, stop_build.rs) or the shared not-found helper they call.
Traces: codebuild.list-builds-for-project-absent-project-not-found, codebuild.list-builds-for-project-nonexistent-project-rejected, codebuild.stop-build-nonexistent-id-rejected.

Cause 4 of 10: BatchGetBuilds accepts a malformed build id instead of rejecting it.
AWS: an id that is not '<project-name>:<uuid>' is rejected with InvalidInputException (HTTP 400), message 'Invalid build ID provided'; a well-formed id for a non-existent build is reported in buildsNotFound instead.
Emulator: the call succeeds (HTTP 200) for id 'this-id-has-no-colon-separator'.
Why: callers that rely on format rejection to catch id-construction bugs instead get a silent empty result, so malformed ids are indistinguishable from absent builds.
Fix: input validation in the BatchGetBuilds handler (services/.../src/ops/batch_get_builds.rs).
Traces: codebuild.batch-get-builds-invalid-id-format-rejected.

Cause 5 of 10: the service role is never validated by CreateProject or StartBuild.
AWS: CreateProject with a serviceRole that does not exist, and StartBuild with a serviceRoleOverride that cannot be assumed by codebuild.amazonaws.com, are both rejected with InvalidInputException (HTTP 400) and no project/build is created.
Emulator: both calls return HTTP 200 — CreateProject echoes a project whose serviceRole is 'arn:aws:iam::000000000000:role/<nonexistent>', and StartBuild returns a build in IN_PROGRESS with that role.
Why: a mis-wired IAM role, one of the most common real CodeBuild misconfigurations, is accepted silently, so permission-failure paths can never be exercised against the emulator.
Fix: role existence/assume-role validation in the CreateProject and StartBuild handlers (services/.../src/ops/create_project.rs, start_build.rs), cross-checking the IAM state.
Traces: codebuild.create-project-invalid-role-rejected, codebuild.start-build-unassumable-role-override-rejected.

Cause 6 of 10: CreateProject accepts an S3 source with no location.
AWS: a ProjectSource of type S3 without `location` is rejected with InvalidInputException (HTTP 400) and no project is created.
Emulator: returns HTTP 200 and persists a project whose source is {'type': 'S3'} with no location.
Why: a project that can never resolve its source is created, and the caller only discovers it at build time (where this emulator does not fail either).
Fix: source validation in the CreateProject handler (services/.../src/ops/create_project.rs).
Traces: codebuild.create-project-s3-source-without-location-rejected.

Cause 7 of 10: BatchGetProjects does not resolve project ARNs and omits projectsNotFound.
AWS: BatchGetProjects accepts either project names or project ARNs and matches an ARN to the named project; the output carries both `projects` and `projectsNotFound` (Smithy BatchGetProjectsOutput), and a lookup by the project's own ARN returns it in `projects`.
Emulator: a project's own ARN is reported as not found, and in other runs the response has no `projectsNotFound` member at all (the check's direct index raises KeyError 'projectsNotFound').
Why: tooling that round-trips the ARN from CreateProject/CFN Outputs cannot read its own project back, and clients that index projectsNotFound to partition results crash.
Fix: identifier resolution and response projection in the BatchGetProjects handler (services/.../src/ops/batch_get_projects.rs).
Traces: codebuild.cbnv-batch-get-projects-by-arn-matches-by-name, codebuild.cbnv-cdk-json-buildspec-round-trips-byte-for-byte, codebuild.cbnv-three-projects-each-report-their-own-nodejs-version.

Cause 8 of 10: project/listing projections drop server-side defaults and empty collections.
AWS: ProjectEnvironment.privilegedMode is documented to default to false and is echoed on reads; a project created without tags reads back with an empty tag list rather than an absent member; ListBuildsForProject always returns its `ids` member (Smithy ListBuildsForProjectOutput.ids) even when the project has no builds.
Emulator: privilegedMode comes back None, an untagged project reports tags None, and ListBuildsForProject responses omit `ids` entirely (KeyError 'ids').
Why: clients reading documented members get None/absent and either crash or treat 'unset' as 'disabled by the service'.
Fix: the project and build-listing serializers (services/.../src/ops/create_project.rs / batch_get_projects.rs projection and list_builds_for_project.rs).
Traces: codebuild.cbnv-project-environment-defaults-applied, codebuild.cbnv-undeclared-project-blocks-stay-absent, codebuild.list-builds-for-project-invalid-sort-order-rejected.

Cause 9 of 10: CloudFormation-declared CodeBuild resources are never materialized in CodeBuild, and stack Outputs carry no OutputValue.
AWS: after a stack containing AWS::CodeBuild::Project / AWS::CodeBuild::Fleet reaches CREATE_COMPLETE, the project exists in CodeBuild (StartBuild and BatchGetProjects find it) and DescribeStacks Outputs entries each carry an OutputValue holding the real physical id/ARN.
Emulator: the stack completes but StartBuild answers ResourceNotFoundException 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/<stack-derived-name>', BatchGetProjects reports the name in projectsNotFound, and Outputs entries have no OutputValue member at all (KeyError 'OutputValue'). Related open issues exist for the Outputs half in other services (20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md and ...-ec2-emr-iam-cloudformation-network-stack-outputs-carry-synthesized-logical-names.md); neither covers these CodeBuild checks.
Why: any CFN-provisioned CI stack is unusable — the caller is told provisioning succeeded and then cannot address what it provisioned.
Fix: the CloudFormation resource provider for AWS::CodeBuild::Project/Fleet (it must call into the CodeBuild service state) and the stack Outputs resolution that fills OutputValue.
Traces: codebuild.deploy-codebuild-project-cfn-starts-build, codebuild.deploy-project-start-build-succeeds, codebuild.e2e-cbnv-check-codebuild-project-nodejs-version-lifecycle, codebuild.deploy-fleet-list-reflects-fleet.

Cause 10 of 10: a family of operations answers NotImplemented where AWS answers a modeled error.
AWS: each of these is rejected with a modeled client error (HTTP 400) — ResourceNotFoundException for DeleteWebhook/GetReportGroupTrend/InvalidateProjectCache/ListBuildBatchesForProject/ListSandboxesForProject/StartCommandExecution against an absent resource, and InvalidInputException for BatchGetFleets with a malformed ARN, StartBuildBatch without a buildBatchConfig or with malformed input, ListCommandExecutionsForSandbox with an absent sandbox, and PutResourcePolicy with a malformed policy or an unsupported resource type.
Emulator: answers the error code 'NotImplemented' with message 'operation <Op> is scaffolded but not implemented in this emulator' (the sandbox-stdout check instead polls a sandbox that never becomes RUNNING and exhausts its 60s budget).
Why: callers cannot tell a missing resource or a bad request from an unavailable service, and retry/branch logic keyed on the modeled codes never fires. Note: separate ALREADY-FILED stub issues exist for StartBuildBatch, StartSandbox, PutResourcePolicy and others; those cover the blocked-by-stub checks, not these checks, which assert the error classification of the same operations.
Fix: implement the listed handlers under services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/ (delete_webhook.rs, get_report_group_trend.rs, invalidate_project_cache.rs, list_build_batches_for_project.rs, list_sandboxes_for_project.rs, list_command_executions_for_sandbox.rs, start_command_execution.rs, start_sandbox.rs, start_build_batch.rs, batch_get_fleets.rs, put_resource_policy.rs) at least to the point of their existence/validation preconditions.
Traces: codebuild.delete-webhook-absent-project-not-found, codebuild.get-report-group-trend-absent-report-group-not-found, codebuild.invalidate-project-cache-absent-not-found, codebuild.list-build-batches-for-project-absent-not-found, codebuild.list-command-executions-for-sandbox-absent-not-found, codebuild.list-sandboxes-for-project-absent-not-found, codebuild.start-build-batch-malformed-input-rejected, codebuild.start-build-batch-without-batch-config-rejected, codebuild.start-command-execution-invalid-sandbox-rejected, codebuild.start-sandbox-account-suspended-blocked, codebuild.start-sandbox-connection-absent-sandbox-not-found, codebuild.start-sandbox-suspended-account-blocked, codebuild.sandbox-timeout-minutes-preserved, codebuild.batch-get-fleets-invalid-arn-rejected, codebuild.put-resource-policy-malformed-policy-rejected, codebuild.put-resource-policy-unsupported-resource-type-rejected, codebuild.resource-policy-malformed-json-rejected, codebuild.deploy-codebuild-report-group-put-resource-policy-malformed-rejected, codebuild.codebuild-sandbox-command-execution-returns-the-commands-real-stdout.

## Evidence
Cause 1: expected buildStatus SUCCEEDED/FAILED, got 'FAULT' with phases [('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)] and context 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed' (16 checks).
Cause 2: expected ('InvalidInputException','The input build ID list cannot be empty.'), got ('InvalidInputException',"1 validation error detected: Value '0 items' at 'ids' failed to satisfy constraint: Member must have length greater than or equal to 1"); expected 'The maximum length of input resource list is 100 but input list with size 101 is found.'; expected 'Invalid timeout. Should be between 5 minutes and 36 hours'; expected enum set '[PRIVATE, PUBLIC_READ]', got '[PUBLIC_READ, PRIVATE]'.
Cause 3: expected 'The provided project arn:aws:codebuild:us-west-2:000000000000:project/<name> does not exist.', got 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/<name>'; expected 'Build <project>:<uuid> does not exist', got 'Build cannot be found: arn:aws:codebuild:...:build/<project>:<uuid>'.
Cause 4: expected ClientError ('InvalidInputException','Invalid build ID provided'), the call SUCCEEDED.
Cause 5: expected a ClientError, CreateProject returned HTTP 200 with serviceRole 'arn:aws:iam::000000000000:role/sim2real-behav-missing-role-...'; StartBuild returned HTTP 200 with buildStatus IN_PROGRESS.
Cause 6: expected a ClientError, CreateProject returned HTTP 200 with source {'type': 'S3'} (no location).
Cause 7: "the project's own ARN was reported not found: ['arn:aws:codebuild:us-west-2:000000000000:project/...']"; KeyError 'projectsNotFound'.
Cause 8: privilegedMode=None; 'an untagged project reports tags None'; KeyError 'ids'.
Cause 9: ResourceNotFoundException 'Project cannot be found: ...project/sim2real-cb-proj-cea66fa4ff0e' after the stack completed; "the deployed stack holds projects [], projectsNotFound=['sim2real-cbnv-e2e-...']"; KeyError 'OutputValue'.
Cause 10: 'expected ResourceNotFoundException, got NotImplemented' / 'expected InvalidInputException, got NotImplemented' / ClientError (NotImplemented) 'operation <Op> is scaffolded but not implemented in this emulator'.

## Resolution

Causes 2, 4, 6, 7 and 10's `InvalidateProjectCache` are fixed, cause 5 is implemented
(the refusal happens; only the tail of AWS's enumerated sentence could not be recovered
from the material available), and causes 3 and 8 were already correct — all covered by
`services/service-com-amazonaws-codebuild-codebuild-20161006/tests/all/input_constraint_wording.rs`,
`.../tests/all/project_source_and_identity.rs` and
`core/router/tests/all/codebuild_service_role_trust.rs`.

TWO causes of this report remain OPEN and are tracked here rather than in the fixed body
above:

* Cause 1 (build execution, 16 checks) and the sandbox-stdout check of cause 10 need a
  runtime that can run the caller's buildspec commands. A CodeBuild build runs a POSIX
  shell inside a container; the in-process engines available to this emulator are a
  JavaScript and a Python interpreter, and the only maintained pure-Rust shell
  (brush-core) executes non-builtin commands by spawning host processes and reads and
  writes host paths, so it cannot be contained. StartBuild therefore still ends a build
  in its modelled FAULT at PROVISIONING with the context message
  `state::BUILD_NOT_EXECUTED_MESSAGE`, unchanged.
* Cause 9 (CloudFormation-declared CodeBuild resources, 4 checks) needs a resource
  provider for `AWS::CodeBuild::Project` / `::Fleet` in the CloudFormation crate (the
  shape `create_codebuild_report_group` already has for `AWS::CodeBuild::ReportGroup`)
  plus the stack-Outputs `OutputValue` resolution this report notes is a
  CloudFormation-wide defect with its own open issues. StartBuild still answers its
  modelled `ResourceNotFoundException` for a project CodeBuild does not hold.

## Steps to Reproduce
Boot the emulator from this checkout and replay the codebuild behavioral suite (run_behavioral_suite), or individually: (1) CreateProject with a NO_SOURCE inline buildspec that echoes and exits 0, StartBuild, then BatchGetBuilds until buildComplete — observe buildStatus FAULT at PROVISIONING; (2) BatchDeleteBuilds(ids=[]) with client-side parameter validation disabled and compare the message; (3) ListBuildsForProject(projectName=<never-created>) and StopBuild(id=<project>:<random-uuid>) and compare messages; (4) BatchGetBuilds(ids=['this-id-has-no-colon-separator']); (5) CreateProject with serviceRole pointing at a non-existent IAM role, and StartBuild with serviceRoleOverride on an unassumable role; (6) CreateProject with source {'type':'S3'} and no location; (7) BatchGetProjects(names=[<project ARN>]) and inspect projectsNotFound; (8) read environment.privilegedMode and tags off BatchGetProjects, and call ListBuildsForProject on a build-less project; (9) deploy a CFN stack with an AWS::CodeBuild::Project and then StartBuild on it / read DescribeStacks Outputs; (10) call DeleteWebhook, GetReportGroupTrend, InvalidateProjectCache, ListBuildBatchesForProject, ListSandboxesForProject, ListCommandExecutionsForSandbox, StartCommandExecution, StartBuildBatch, BatchGetFleets and PutResourcePolicy against absent/malformed inputs.
