Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.webhook-filter-groups-unicode-round-trip
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.webhook-filter-groups-unicode-round-trip|_client_error#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 9d990885182cf7376ea90557ef044b425999cd93

# CodeBuild UpdateWebhook accepts a webhook mutation with no imported source credential instead of refusing it with ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `9d990885182cf7376ea90557ef044b425999cd93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWebhook

## Description
ONE net-new root cause in the CodeBuild service crate (services/service-com-amazonaws-codebuild-codebuild-20161006). The UpdateWebhook scaffold stub has since been implemented (fixed issue 20261025T014438Z-codebuild-updatewebhook-stub.md), and the emulator no longer answers NotImplemented — but the implementation is over-permissive: it mutates and returns the webhook where AWS refuses the call. Open issue 20261025T014438Z-codebuild-8-further-divergences-getresourcepolicy-updatewebhook-deletebuildbatch-stubs.md describes the same credential precondition as its Cause 3, but its symptom and its replay key for this check are the now-stale NotImplemented response; it does not carry the replay key for the accept-where-AWS-refuses behaviour reported here, so this check must be replayed too.

Cause 1 of 1: UpdateWebhook does not enforce the source-credential precondition and returns 200 with a mutated webhook.
AWS: Smithy UpdateWebhook(projectName @required, ProjectName @length 2..150 @pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$; branchFilter, rotateSecret default false, filterGroups, buildType, pullRequestBuildPolicy) -> { webhook: Webhook }; modeled errors InvalidInputException, OAuthProviderException, ResourceNotFoundException (all errorKind client, httpError 400). Recorded behavior: when the account holds no imported source credential for the project's source provider (ImportSourceCredentials was never called), CodeBuild refuses UpdateWebhook with HTTP 400, error code ResourceNotFoundException and message "Access token not found in CodeBuild project for server type github" — even when the webhook was created with manualCreation=true and only filterGroups (a CodeBuild-side-only field) is being changed. In that message the fixed text is "Access token not found in CodeBuild project for server type "; the trailing token is a value derived from the request/state, namely the project's source provider type rendered lowercase (github for a GITHUB source, and the corresponding token for the other source types). The refusal has no side effect: the stored webhook keeps its previous filterGroups, so a following BatchGetProjects still shows the pre-update filter groups.
Emulator: the call succeeds. HTTP 200 with { webhook: { payloadUrl, filterGroups: <the newly supplied groups>, manualCreation: true, lastModifiedSecret, status 'ACTIVE' } } — the new filter groups are accepted and persisted, no modeled error is raised, and the source-credential registry is never consulted.
Why: the credential precondition for webhook mutation is invisible to a client. Callers are told a webhook's filter configuration was updated when the project has no credential that can authenticate to the source provider, so the webhook cannot actually be reconciled upstream; code that branches on ResourceNotFoundException (to prompt for ImportSourceCredentials, or to fall back to the supported delete+recreate path for a manually-created webhook) never takes that branch, and the OAuthProviderException auth-boundary class remains unreachable. The emulator also silently mutates state AWS leaves untouched, so a later read diverges.
Fix: the credential/existence precondition in the UpdateWebhook handler (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/update_webhook.rs) — consult the imported source-credentials state for the project's source server type before mutating the stored webhook, and return the modeled 400 ResourceNotFoundException with CodeBuild's own wording when none is present, leaving the webhook record unchanged. This is the same seam as the CreateWebhook/DeleteWebhook precondition (Cause 6 of the open issue named above).
Traces: codebuild.webhook-filter-groups-unicode-round-trip.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout: 33/110 passed, 60 mismatched, 17 errored.

[mismatch] codebuild.webhook-filter-groups-unicode-round-trip — check_codebuild_webhook_filter_groups_unicode_round_trip.py line 105 calls cb.update_webhook(projectName=<project>, filterGroups=<new unicode filter groups>) inside a helper that requires a ClientError; the helper raised instead:
  AssertionError: expected the call to be rejected, but it succeeded: {'webhook': {'payloadUrl': 'https://codebuild.<region>.amazonaws.com/webhooks?t=<token>&v=1', 'filterGroups': [[{'type': 'EVENT', 'pattern': 'PUSH,PULL_REQUEST_CREATED', 'excludeMatchedPattern': False}, {'type': 'HEAD_REF', 'pattern': '^refs/heads/\u6a5f\u80fd-\ud83d\ude80-r\u00e9sum\u00e9-.*$', 'excludeMatchedPattern': True}]], 'manualCreation': True, 'lastModifiedSecret': <ts>, 'status': 'ACTIVE'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Expected per the check's assertions: error code ResourceNotFoundException and message 'Access token not found in CodeBuild project for server type github' (HTTP 400), followed by BatchGetProjects still reporting the ORIGINAL v1 unicode filter groups. Observed: HTTP 200 and the v2 filter groups persisted. The host, region, webhook token, timestamp and the specific unicode patterns above are per-run values, not requirements; the requirements are the 400 status, the ResourceNotFoundException code, the message wording with the project's source server type substituted, and the absence of any mutation.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — UpdateWebhook input members and its three modeled errors InvalidInputException / OAuthProviderException / ResourceNotFoundException (client, httpError 400).

Related already-filed issues: fixed 20261025T014438Z-codebuild-updatewebhook-stub.md (implemented the stub; the implementation omits this precondition) and open 20261025T014438Z-codebuild-8-further-divergences-getresourcepolicy-updatewebhook-deletebuildbatch-stubs.md Cause 3 (same precondition, stale NotImplemented symptom/replay key).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Confirm the account holds no GITHUB source credential: ListSourceCredentials returns no entry with serverType GITHUB (never call ImportSourceCredentials).
3. CreateRole for CodeBuild, then CreateProject with source {type: 'GITHUB', location: <any public repo URL>}, artifacts {type: 'NO_ARTIFACTS'}, a minimal LINUX_CONTAINER environment, and logging disabled.
4. CreateWebhook(projectName=<project>, manualCreation=True, filterGroups=[[{type: 'EVENT', pattern: 'PUSH'}, {type: 'HEAD_REF', pattern: '^refs/heads/\u00fcn\u00efc\u00f6d\u00e9-...-.*$'}]]).
5. Call UpdateWebhook(projectName=<project>, filterGroups=<a different filter-group set>).
   Expected: HTTP 400, code ResourceNotFoundException, message 'Access token not found in CodeBuild project for server type github'.
   Observed: HTTP 200 with the webhook carrying the new filterGroups.
6. BatchGetProjects(names=[<project>]) and inspect projects[0]['webhook']['filterGroups'] — AWS still shows the step-4 groups; the emulator shows the step-5 groups, confirming the refused call was applied.
7. Or simply run run_behavioral_suite and read the codebuild section for check webhook-filter-groups-unicode-round-trip.
