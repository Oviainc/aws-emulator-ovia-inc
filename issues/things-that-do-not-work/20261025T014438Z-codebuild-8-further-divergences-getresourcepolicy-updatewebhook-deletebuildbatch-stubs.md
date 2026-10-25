Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.codebuild-junit-report-ingestion-reports-each-test-case-with-its-exact-status codebuild.describe-test-cases-absent-report-not-found codebuild.e2e-webhook-triggered-build-retry-flow codebuild.list-reports-for-report-group-sort-order-consistent codebuild.report-group-trend-reflects-created-reports codebuild.get-resource-policy-absent-resource-not-found codebuild.resource-policy-sharing-not-listed-for-owner codebuild.webhook-filter-groups-unicode-round-trip codebuild.delete-build-batch-in-progress-rejected codebuild.webhook-secret-and-filter-groups-round-trip codebuild.delete-webhook-oauth-provider-error-blocked codebuild.delete-source-credentials-invalid-arn codebuild.deploy-codebuild-report-group-cfn-policy-applies
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.codebuild-junit-report-ingestion-reports-each-test-case-with-its-exact-status|run#12|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.describe-test-cases-absent-report-not-found|_create_real_report#2|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.e2e-webhook-triggered-build-retry-flow|run#21|AssertionError|assert build1_final["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.list-reports-for-report-group-sort-order-consistent|run#18|AssertionError|assert status_1 == "SUCCEEDED", f"build 1 did not succeed: {status_1}"
Issue-Key: codebuild.report-group-trend-reflects-created-reports|run#15|AssertionError|assert final_build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild.get-resource-policy-absent-resource-not-found|run#9|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: codebuild.resource-policy-sharing-not-listed-for-owner|run#9|AssertionError|assert baseline_code == "ResourceNotFoundException", (
Issue-Key: codebuild.webhook-filter-groups-unicode-round-trip|run#16|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: codebuild.delete-build-batch-in-progress-rejected|run#13|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code}"
Issue-Key: codebuild.webhook-secret-and-filter-groups-round-trip|run#14|AssertionError|assert "secret" not in read_after_create, (
Issue-Key: codebuild.delete-webhook-oauth-provider-error-blocked|_expect_error#9|AssertionError|raise AssertionError(
Issue-Key: codebuild.delete-source-credentials-invalid-arn|run#11|AssertionError|assert code == "InvalidInputException", code
Issue-Key: codebuild.deploy-codebuild-report-group-cfn-policy-applies|run#16|AssertionError|assert report_group_arn in [rg["arn"] for rg in described["reportGroups"]]
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CodeBuild: 8 further divergences — GetResourcePolicy/UpdateWebhook/DeleteBuildBatch stubs, BatchGetProjects leaks the webhook secret, CreateWebhook/DeleteSourceCredentials preconditions, CFN ReportGroup never materialized, and report flows blocked by FAULTing builds

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
EIGHT distinct net-new root causes found in the `codebuild` behavioral suite, all in the CodeBuild service crate services/service-com-amazonaws-codebuild-codebuild-20161006 (plus its CloudFormation resource provider for cause 8). All eight must be closed before this issue is resolved. Checks the replay report marked ALREADY FILED or OWNED are excluded; every cause below names the related open issue where one exists, and none of those issues carries the replay keys listed here.

Cause 1 of 8: builds never execute, so no report/test-case/trend/webhook-retry flow can be observed.
AWS: Smithy StartBuild returns a Build; StatusType enumerates SUCCEEDED, FAILED, FAULT, TIMED_OUT, IN_PROGRESS, STOPPED and BuildPhaseType enumerates SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED. A build whose buildspec commands all exit 0 reaches buildStatus SUCCEEDED; a build whose buildspec declares a JUnit `reports` section mints a real report whose ARN is readable through BatchGetReports / DescribeTestCases / ListReportsForReportGroup / GetReportGroupTrend; a webhook-triggered build and its RetryBuild reach a terminal outcome the same way.
Emulator: every build walks SUBMITTED(SUCCEEDED) -> QUEUED(SUCCEEDED) -> PROVISIONING(FAULT) -> COMPLETED, buildStatus FAULT, with the PROVISIONING phase carrying the context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. No report, test case, trend data point or artifact is ever produced.
Why: a client cannot tell a passing build from a failing one, and every downstream reporting surface (reports, test cases, report-group trends) is permanently empty while the control plane claims the build ran and completed.
Fix: the build state machine behind the StartBuild handler and the report-producing path it feeds (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs plus the build/phase/report state read back by batch_get_builds.rs, batch_get_reports.rs, describe_test_cases.rs, list_reports_for_report_group.rs). A deterministic simulated phase walk driven by the project's buildspec is what the suite grades, not a real container.
Traces: codebuild.codebuild-junit-report-ingestion-reports-each-test-case-with-its-exact-status, codebuild.describe-test-cases-absent-report-not-found, codebuild.e2e-webhook-triggered-build-retry-flow, codebuild.list-reports-for-report-group-sort-order-consistent, codebuild.report-group-trend-reflects-created-reports. Open issue 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md describes the same FAULT-at-PROVISIONING defect but carries no replay key for these five checks, so they must be replayed too before that fix is confirmed.

Cause 2 of 8: GetResourcePolicy answers NotImplemented instead of its modeled errors.
AWS: Smithy GetResourcePolicy(resourceArn @required, NonEmptyString @length min 1) -> policy; modeled errors InvalidInputException and ResourceNotFoundException (both client, httpError 400). A well-formed report-group ARN with no policy attached is answered with 400 ResourceNotFoundException; after PutResourcePolicy the same ARN returns the stored policy document.
Emulator: error code 'NotImplemented', message 'operation GetResourcePolicy is scaffolded but not implemented in this emulator'.
Why: a caller cannot distinguish 'this resource has no policy' from 'the service is unavailable', so policy-presence branching and the owner-side sharing read never work.
Fix: implement services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/get_resource_policy.rs against the resource-policy state.
Traces: codebuild.get-resource-policy-absent-resource-not-found, codebuild.resource-policy-sharing-not-listed-for-owner.

Cause 3 of 8: UpdateWebhook answers NotImplemented instead of its modeled errors.
AWS: Smithy UpdateWebhook(projectName @required) -> Webhook; modeled errors InvalidInputException, OAuthProviderException, ResourceNotFoundException (all client, httpError 400). With no source credential imported for the project's source provider, UpdateWebhook is refused with 400 ResourceNotFoundException and the stored webhook is left untouched.
Emulator: error code 'NotImplemented', message 'operation UpdateWebhook is scaffolded but not implemented in this emulator'.
Why: the credential precondition for webhook mutation is unobservable, and no webhook config can be changed after creation.
Fix: implement services/.../src/ops/update_webhook.rs, at least to its existence/credential preconditions and the filter-group round trip. The suite's auto-filed scaffold-stub issue for UpdateWebhook covers only the checks it blocked outright; this check asserts the operation's error classification and carries its own replay key.
Traces: codebuild.webhook-filter-groups-unicode-round-trip.

Cause 4 of 8: DeleteBuildBatch answers NotImplemented instead of its modeled error.
AWS: Smithy DeleteBuildBatch(id @required, NonEmptyString) -> { statusCode, buildsDeleted, buildsNotDeleted }; the single modeled error is InvalidInputException (client, httpError 400), which is what deleting a batch that is still IN_PROGRESS is refused with; the batch stays present afterwards.
Emulator: error code 'NotImplemented', message 'operation DeleteBuildBatch is scaffolded but not implemented in this emulator'.
Why: batch teardown logic cannot distinguish 'not deletable yet' from 'service unavailable', and no batch can ever be deleted.
Fix: implement services/.../src/ops/delete_build_batch.rs, including the in-progress refusal. The auto-filed scaffold-stub issue for DeleteBuildBatch covers the one check it blocked outright, not this classification check.
Traces: codebuild.delete-build-batch-in-progress-rejected.

Cause 5 of 8: BatchGetProjects echoes the webhook `secret` back on every read.
AWS: recorded behavior — the webhook secret is returned exactly once, by the webhook-creating call (the model documents Webhook$secret as 'The secret token of the associated repository' and, under manualCreation, as a value CreateWebhook returns so the hook can be wired up by hand). A later read of the project (BatchGetProjects -> Project$webhook) does NOT carry `secret`, and UpdateWebhook with rotateSecret=false does not return it either.
Emulator: the webhook member of a BatchGetProjects project carries 'secret' alongside payloadUrl/filterGroups/manualCreation/lastModifiedSecret/status, both before and after an update.
Why: the webhook's shared signing secret is handed to any principal holding read-only codebuild:BatchGetProjects, which is a credential-scope escalation relative to AWS; clients that diff webhook configuration also see a member AWS never sends.
Fix: the project/webhook projection on the read path (services/.../src/ops/batch_get_projects.rs and the shared webhook projection in state) must strip `secret`, keeping it only on the CreateWebhook response and on an UpdateWebhook that rotates it.
Traces: codebuild.webhook-secret-and-filter-groups-round-trip.

Cause 6 of 8: CreateWebhook succeeds for an account with no imported source credential.
AWS: Smithy CreateWebhook errors include ResourceNotFoundException (client, httpError 400), which is how a webhook call is refused when the account holds no source credential for the project's provider — the 'no credential' boundary, distinct from OAuthProviderException, which is what a stored-but-rejected credential produces. Nothing is created.
Emulator: HTTP 200 with a minted webhook ({payloadUrl, secret, lastModifiedSecret, status 'ACTIVE'}) even though no ImportSourceCredentials call was ever made.
Why: the credential precondition is invisible, so a webhook looks wired when nothing can authenticate to the provider, and the distinct OAuthProviderException auth-boundary class is unreachable.
Fix: the credential precondition in services/.../src/ops/create_webhook.rs (and the matching path in delete_webhook.rs) — consult the imported source-credentials state before minting or deleting a webhook.
Traces: codebuild.delete-webhook-oauth-provider-error-blocked.

Cause 7 of 8: DeleteSourceCredentials classifies a malformed token ARN as not-found.
AWS: Smithy DeleteSourceCredentials(arn @required, NonEmptyString @length min 1); modeled errors InvalidInputException and ResourceNotFoundException (both client, httpError 400). The ARN is parsed first, so one whose server-type component is not a valid source provider is rejected with 400 InvalidInputException before any registry lookup.
Emulator: 400 ResourceNotFoundException.
Why: a caller that sent a structurally invalid ARN is told the credential is merely absent, so it retries or re-imports instead of fixing the request; code that separates 'bad request' from 'not found' misclassifies the failure.
Fix: ARN shape validation (including the server-type segment) ahead of the state lookup in services/.../src/ops/delete_source_credentials.rs. Open wire issue 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md names the same defect as its Cause 5, but carries only wire fixture keys; this adds the behavioral replay key.
Traces: codebuild.delete-source-credentials-invalid-arn.

Cause 8 of 8: an AWS::CodeBuild::ReportGroup provisioned by CloudFormation is never materialized in CodeBuild.
AWS: once a stack containing AWS::CodeBuild::ReportGroup reaches CREATE_COMPLETE, the report group exists in CodeBuild: BatchGetReportGroups(reportGroupArns=['arn:aws:codebuild:<region>:<account>:report-group/<name>']) returns it in `reportGroups` with `reportGroupsNotFound` empty, and PutResourcePolicy/GetResourcePolicy against that ARN work.
Emulator: the stack reports CREATE_COMPLETE, but BatchGetReportGroups does not return the stack's report-group ARN (the membership assertion on reportGroups fails), so nothing the stack declared is addressable.
Why: a caller is told provisioning succeeded and then cannot address what it provisioned — any CFN-managed report group, and the resource policies and reports hung off it, is unusable.
Fix: the CloudFormation resource provider for AWS::CodeBuild::ReportGroup must create the record in the CodeBuild service state (the same seam Cause 9 of 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md identifies for AWS::CodeBuild::Project/Fleet; that cause does not cover the ReportGroup type or this check).
Traces: codebuild.deploy-codebuild-report-group-cfn-policy-applies.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout: 25/110 passed, 65 mismatched, 20 errored.

Cause 1 — expected buildStatus SUCCEEDED/FAILED, got 'FAULT' with phases [('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)]: 'every buildspec command exited zero, so the build must be SUCCEEDED even though a test case failed; got FAULT' (junit ingestion); 'prerequisite build must succeed to mint a real report, got FAULT' (describe-test-cases); 'expected the original build to succeed, got FAULT' (webhook retry); 'build 1 did not succeed: FAULT' (list-reports ordering); 'build did not succeed: FAULT' (report-group trend). The PROVISIONING phase carries contexts [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}].

Cause 2 — GetResourcePolicy: 'expected ResourceNotFoundException, got NotImplemented' (absent policy) and 'expected ResourceNotFoundException before any policy is attached, got NotImplemented' (owner-side sharing read).

Cause 3 — UpdateWebhook: 'expected UpdateWebhook without a linked source credential to be rejected with ResourceNotFoundException, got NotImplemented'.

Cause 4 — DeleteBuildBatch: 'expected InvalidInputException, got NotImplemented'.

Cause 5 — BatchGetProjects: 'BatchGetProjects leaked the webhook secret: {payloadUrl: 'https://codebuild.<region>.amazonaws.com/webhooks?t=...&v=1', secret: '<40-char token>', filterGroups: [[{type EVENT...},{type HEAD_REF, pattern ^refs/heads/\u529f\u80fd-.*$}]], manualCreation: True, lastModifiedSecret: <ts>, status: 'ACTIVE'}'. The check also records that CreateWebhook itself DID return a secret (correct) — the divergence is the read echoing it.

Cause 6 — CreateWebhook: 'expected ResourceNotFoundException for CreateWebhook with no stored credential, but the call SUCCEEDED and returned {webhook: {payloadUrl: ..., secret: ..., lastModifiedSecret: ..., status: 'ACTIVE'}, ResponseMetadata: {HTTPStatusCode: 200}}'.

Cause 7 — DeleteSourceCredentials with an invalid token ARN: expected code InvalidInputException, got ResourceNotFoundException (assertion detail 'AssertionError: ResourceNotFoundException').

Cause 8 — after the AWS::CodeBuild::ReportGroup stack reached CREATE_COMPLETE, 'assert report_group_arn in [rg["arn"] for rg in described["reportGroups"]]' failed on the BatchGetReportGroups response for arn:aws:codebuild:<region>:<account>:report-group/<name>.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — GetResourcePolicy, PutResourcePolicy, UpdateWebhook, CreateWebhook, DeleteBuildBatch, DeleteSourceCredentials signatures and modeled errors (InvalidInputException / ResourceNotFoundException / OAuthProviderException, all client, httpError 400), and the Webhook structure (secret: NonEmptyString, documented as returned by the webhook-creating call).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the behavioral suite (run_behavioral_suite) and look at the `codebuild` section, or reproduce each cause directly:
3. Cause 1: CreateProject with a NO_SOURCE inline buildspec that echoes and exits 0 (optionally with a `reports` section pointing at a JUnit XML it writes), StartBuild, then BatchGetBuilds until buildComplete — observe buildStatus FAULT at PROVISIONING and no report in BatchGetReports/ListReportsForReportGroup/DescribeTestCases/GetReportGroupTrend.
4. Cause 2: CreateReportGroup, then GetResourcePolicy(resourceArn=<its ARN>) with no policy attached — expect 400 ResourceNotFoundException, observe NotImplemented.
5. Cause 3: CreateProject with a GITHUB source, CreateWebhook(manualCreation=True), then UpdateWebhook(projectName=..., filterGroups=..., rotateSecret=False) with no ImportSourceCredentials — expect 400 ResourceNotFoundException, observe NotImplemented.
6. Cause 4: StartBuildBatch on a batch-enabled project, then DeleteBuildBatch(id=<batch id>) while it is IN_PROGRESS — expect 400 InvalidInputException, observe NotImplemented.
7. Cause 5: CreateWebhook(manualCreation=True) on a GITHUB-source project, then BatchGetProjects(names=[<project>]) and inspect projects[0]['webhook'] — the `secret` member is present.
8. Cause 6: on an account with no imported source credentials, CreateWebhook(projectName=<GITHUB-source project>) — expect 400 ResourceNotFoundException, observe HTTP 200 with a webhook.
9. Cause 7: DeleteSourceCredentials(arn='arn:aws:codebuild:<region>:<account>:token/github_does_not_exist' with an invalid server-type segment) — expect 400 InvalidInputException, observe 400 ResourceNotFoundException.
10. Cause 8: CreateStack with a template whose only resource is AWS::CodeBuild::ReportGroup (Type TEST, ExportConfig NO_EXPORT), wait for CREATE_COMPLETE, then BatchGetReportGroups(reportGroupArns=['arn:aws:codebuild:<region>:<account>:report-group/<name>']) — the ARN is not in reportGroups.
