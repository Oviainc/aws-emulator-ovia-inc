Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-code-signing-warn-then-enforce
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-code-signing-warn-then-enforce lam-err-policy-length-exceeded lam-err-public-policy-access-flag lam-err-revision-id-function-mutations lam-err-revision-id-permissions lam-err-update-function-configuration-service-limits lam-invoke-alias-executed-version lam-invoke-dry-run lam-invoke-event-payload-too-large lam-invoke-kms-disabled lam-invoke-log-type-tail lam-invoke-unhandled-function-error lam-page-create-alias-numeric-name lam-page-event-invoke-config-range-boundaries lam-page-list-aliases lam-page-list-function-event-invoke-configs lam-page-list-function-url-configs lam-page-list-versions-by-function lambda-alias-lifecycle lambda-alias-traffic-shifting lambda-async-invoke-destinations lambda-code-signing-config-enforcement lambda-concurrency-and-provisioned-concurrency lambda-concurrency-and-scaling-config lambda-delete-alias-nonexistent-is-a-no-op lambda-esm-scaling-config-roundtrip lambda-event-invoke-config-lifecycle lambda-event-invoke-config-recursion-and-runtime-management lambda-extended-api-error-surfaces lambda-function-codesigning-attach-detach-lifecycle lambda-function-codesigning-attach-error-paths lambda-function-policy-permissions lambda-function-resource-policy-permissions lambda-function-scaling-config-error-paths lambda-function-url-config-lifecycle lambda-get-concurrency-unset-empty lambda-invoke-response-stream-deleted-version-error lambda-invoke-response-stream-missing-version lambda-resource-policy-delete-then-recreate lambda-resource-policy-invalid-policy-json lambda-resource-policy-on-alias lambda-resource-policy-precondition-conflict lambda-resource-policy-put-get-delete-lifecycle lambda-resource-policy-revision-mismatch lambda-resource-policy-statement-and-document lambda-scaling-config-rejected-on-standard-function lambda-version-alias-routing lambda-zip-function-invoke-publish-lifecycle lam-err-create-function-service-limits lambda-function-crud-with-tag-round-trip lambda-function-lifecycle-crud lambda-account-and-global-reads lambda-create-function-invoke-response-stream-lifecycle lambda-esm-error-paths lambda-event-source-mapping-lifecycle lambda-function-core-lifecycle lambda-function-invocation-paths lambda-function-scaling-config-non-capacity-provider-function lambda-function-tagging-lifecycle lambda-invoke-modes-comparison lambda-permissions-and-resource-policy lambda-publish-version-then-invoke-pinned-version lambda-put-function-scaling-config-without-capacity-provider-error lambda-recursion-and-runtime-management-config lambda-resource-policy-conditional-update-and-stale-revision lambda-resource-policy-error-paths lambda-resource-policy-errors lambda-resource-policy-lifecycle lambda-scaling-config-unsupported-function lambda-codesigning-config-crud-and-list lam-err-capacity-provider-duplicate-create
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateFunction|2xx->5xx|status
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.Architectures
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.CodeSha256
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.CodeSize
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.Description
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.EphemeralStorage
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.FunctionArn
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.FunctionName
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.Handler
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.LastModified
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.LastUpdateStatus
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.LoggingConfig
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.MemorySize
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.PackageType
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.RevisionId
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.Role
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.Runtime
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.RuntimeVersionConfig
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.SnapStart
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.State
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.Timeout
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.TracingConfig
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$.Version
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|$headers.content-type
Issue-Key: lambda|GetFunctionConfiguration|2xx->5xx|status
Issue-Key: lambda|DeleteFunction|2xx->5xx|status
Issue-Key: lambda|Invoke|2xx->5xx|$.ok
Issue-Key: lambda|Invoke|2xx->5xx|$headers.content-type
Issue-Key: lambda|Invoke|2xx->5xx|$headers.x-amz-executed-version
Issue-Key: lambda|Invoke|2xx->5xx|status
Issue-Key: lambda|PublishVersion|2xx->5xx|status
Issue-Key: lambda|AddPermission|2xx->5xx|$.Statement
Issue-Key: lambda|AddPermission|2xx->5xx|$headers.content-type
Issue-Key: lambda|AddPermission|2xx->5xx|status
Issue-Key: lambda|GetPolicy|2xx->5xx|$.Policy
Issue-Key: lambda|GetPolicy|2xx->5xx|$.PublicAccessAllowed
Issue-Key: lambda|GetPolicy|2xx->5xx|$.RevisionId
Issue-Key: lambda|GetPolicy|2xx->5xx|$headers.content-type
Issue-Key: lambda|GetPolicy|2xx->5xx|status
Issue-Key: lambda|RemovePermission|2xx->5xx|$headers.content-type
Issue-Key: lambda|RemovePermission|2xx->5xx|status
Issue-Key: lambda|CreateCodeSigningConfig|2xx->5xx|$.CodeSigningConfig
Issue-Key: lambda|CreateCodeSigningConfig|2xx->5xx|$headers.content-type
Issue-Key: lambda|CreateCodeSigningConfig|2xx->5xx|status
Issue-Key: lambda|GetCapacityProvider|2xx->5xx|$
Issue-Key: lambda|GetCapacityProvider|2xx->5xx|$.CapacityProvider
Issue-Key: lambda|GetCapacityProvider|2xx->5xx|$headers.content-type
Issue-Key: lambda|GetCapacityProvider|2xx->5xx|status
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# Wire replay: every Lambda request answered 500 AmbiguousService instead of the modeled response

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
WIRE replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, state_reset_failures 0, bootstrap_signal false) produced 175 lambda divergence clusters / 661 mismatched steps. EVERY one of them is the same refusal: the request never reaches a Lambda handler, because the router cannot decide which emulated service owns a request signed for 'lambda'. Four bootstrapped services declare the same wire identity -- Smithy awsService.endpointPrefix='lambda' and sigv4Name='lambda' for Lambda (AWSGirApiService, 2015-03-31), Lambda Core (LambdaCoreApiService, 2026-04-30), Lambda Web (LambdaWeb, 2025-03-07) and Lambda Microvms (LambdaMicrovms, 2025-09-09) -- and the router's service-resolution step keys only on that signing name, so it answers HTTP 500 with __type 'AmbiguousService' and the message "4 emulated services all claim requests for 'lambda': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one" (message text quoted as an EXAMPLE; the requirement is the status and that no such error code exists on AWS). The same envelope also explains the secondary content-type assertion seen on these steps ('application/x-amz-json-1.1' where the restJson1 responses expect 'application/json') -- that is the refusal envelope's own framing, not an independent serializer defect, and it should disappear with the routing fix.

THIS ISSUE CARRIES 10 BUNDLED CAUSES (Cause 1 of 10 .. Cause 10 of 10), one replay unit per affected Lambda operation family; each names its own fixture and step and ALL TEN must replay clean before this is closed. They share the single root cause above, so one edit in the resolution seam is expected to close all ten -- and also the remaining 165 clusters of this corpus, which are the identical refusal on further Lambda operations (UpdateFunctionConfiguration, DeleteCapacityProvider, DeleteFunctionCodeSigningConfig, ListAliases, ListVersionsByFunction, PutFunctionConcurrency, event-source-mapping and function-url operations, ...). Confirm with a FULL corpus re-replay, not only the ten bindings below.

Relation to already-open issues: two open issues describe this same router refusal from the BEHAVIORAL corpus -- "Lambda is unreachable: four emulated services all claim the 'lambda' signing name, so every Lambda request is refused with AmbiguousService" and "Lambda is unreachable (router AmbiguousService across four 'lambda'-signing services), and a CloudFormation-deployed Lambda stack exposes no Outputs". Neither carries any wire fixture/step replay key (their Issue-Trace-Ids are lambda__resource-groups-tagging-api check ids), so none of the 175 wire clusters is owned by them; the replay report reports owned_by empty for all of them. This issue exists to carry the wire replay keys.

=== Cause 1 of 10: CreateFunction -- fixture lam-err-code-signing-warn-then-enforce, step setup#1 (48 steps in cluster) ===
AWS: Smithy: CreateFunction, POST /2015-03-31/functions, http code 201, output shape FunctionConfiguration (FunctionName, FunctionArn, Role, Runtime, Handler, CodeSha256, CodeSize, Version '$LATEST', State, StateReason/StateReasonCode, RevisionId, LastModified, MemorySize, Timeout, TracingConfig, SnapStart, EphemeralStorage, LoggingConfig, RuntimeVersionConfig, Architectures); modeled errors InvalidParameterValueException 400, ResourceConflictException 409, ResourceNotFoundException 404, CodeStorageExceededException 400, CodeSigningConfigNotFoundException 404, TooManyRequestsException 429, ServiceException 500. 'AmbiguousService' is not a modeled error of this (or any) Lambda operation.
Emulator: HTTP 500 with __type AmbiguousService and no response body members; the recorded 201 FunctionConfiguration is entirely absent.
Why: no client can create a function, so every Lambda workflow dies on its first call; the failure arrives as a retryable server error rather than a modeled client error, so SDKs burn their whole retry budget before surfacing a code that no error handler can branch on.
Fix: the router's service-resolution step -- the `can_handle` seam of the lambda-family services, which must disambiguate a shared signing name instead of all four claiming it.
Traces: lam-err-code-signing-warn-then-enforce#setup#1 (plus 47 further steps in the same cluster, e.g. lam-invoke-dry-run#setup#3, lambda-alias-lifecycle#setup#2).

=== Cause 2 of 10: GetFunctionConfiguration -- fixture lam-err-create-function-service-limits, step task#5 (7 steps) ===
AWS: Smithy: GetFunctionConfiguration, GET /2015-03-31/functions/{FunctionName}/configuration, http code 200, output FunctionConfiguration (State 'Active', LastUpdateStatus 'Successful', RevisionId, ...); modeled errors ResourceNotFoundException 404, InvalidParameterValueException 400, TooManyRequestsException 429, ServiceException 500.
Emulator: HTTP 500 AmbiguousService; every asserted output member missing.
Why: a client cannot read back or poll a function's configuration, so no create/update readiness wait can ever converge.
Fix: same resolution seam; no handler was reached.
Traces: lam-err-create-function-service-limits#task#5.

=== Cause 3 of 10: DeleteFunction -- fixture lam-err-code-signing-warn-then-enforce, step teardown#9 (67 steps, the largest cluster) ===
AWS: Smithy: DeleteFunction, DELETE /2015-03-31/functions/{FunctionName}, empty output whose StatusCode is bound with httpResponseCode (the recording shows 204 on success and 404 ResourceNotFoundException for an already-deleted function); modeled errors ResourceNotFoundException 404, ResourceConflictException 409, InvalidParameterValueException 400.
Emulator: HTTP 500 AmbiguousService for both the success and the not-found case, so the 204/404 distinction is unobservable.
Why: resources can never be torn down and a deleted-vs-existing check cannot be made; test and CI cleanup paths leak.
Fix: same resolution seam.
Traces: lam-err-code-signing-warn-then-enforce#teardown#9 (and the 404 sibling cluster lam-err-code-signing-warn-then-enforce#teardown#10).

=== Cause 4 of 10: Invoke -- fixture lam-invoke-alias-executed-version, step task#7 (6 steps) ===
AWS: Smithy: Invoke, POST /2015-03-31/functions/{FunctionName}/invocations; output StatusCode bound with httpResponseCode (200 RequestResponse, 202 Event, 204 DryRun per the recordings), Payload bound with httpPayload, and headers X-Amz-Executed-Version, X-Amz-Log-Result, X-Amz-Function-Error; modeled errors include RequestTooLargeException 413, InvalidRequestContentException 400, ResourceNotFoundException 404, KMSDisabledException 502, TooManyRequestsException 429.
Emulator: HTTP 500 AmbiguousService; no payload, and x-amz-executed-version / x-amz-log-result absent.
Why: the central Lambda data-plane call is unusable, and an invocation error cannot be told apart from a transport failure.
Fix: same resolution seam.
Traces: lam-invoke-alias-executed-version#task#7 (siblings: lam-invoke-dry-run#task#6 expecting 204, lam-invoke-event-payload-too-large#task#5 expecting 202, lam-invoke-log-type-tail#task#6, lam-invoke-kms-disabled#task#7).

=== Cause 5 of 10: PublishVersion -- fixture lam-invoke-alias-executed-version, step setup#5 (9 steps) ===
AWS: Smithy: PublishVersion, POST /2015-03-31/functions/{FunctionName}/versions, http code 201, output FunctionConfiguration with a numeric Version; modeled errors PreconditionFailedException 412, ResourceConflictException 409, ResourceNotFoundException 404, CodeStorageExceededException 400.
Emulator: HTTP 500 AmbiguousService instead of 201.
Why: versioning and every alias/qualifier scenario built on it is unreachable.
Fix: same resolution seam.
Traces: lam-invoke-alias-executed-version#setup#5.

=== Cause 6 of 10: AddPermission -- fixture lam-err-policy-length-exceeded, step task#2 (11 steps) ===
AWS: Smithy: AddPermission, POST /2015-03-31/functions/{FunctionName}/policy, http code 201, output member Statement (the JSON statement document); modeled errors PolicyLengthExceededException 400, PublicPolicyException 400, PreconditionFailedException 412, ResourceConflictException 409, ResourceNotFoundException 404.
Emulator: HTTP 500 AmbiguousService; Statement missing.
Why: resource policies cannot be written, and the policy-length / public-policy guard rails these fixtures probe can never be observed -- a client cannot distinguish 'rejected for policy size' from 'server broken'.
Fix: same resolution seam.
Traces: lam-err-policy-length-exceeded#task#2.

=== Cause 7 of 10: GetPolicy -- fixture lam-err-policy-length-exceeded, step task#7 (11 steps) ===
AWS: Smithy: GetPolicy, GET /2015-03-31/functions/{FunctionName}/policy, http code 200, output Policy + RevisionId; modeled errors ResourceNotFoundException 404, InvalidParameterValueException 400.
Emulator: HTTP 500 AmbiguousService; Policy, RevisionId and the recorded PublicAccessAllowed key all missing.
Why: a client cannot read back the policy document it just attached, so conditional (revision-gated) policy updates are impossible.
Fix: same resolution seam.
Traces: lam-err-policy-length-exceeded#task#7.

=== Cause 8 of 10: RemovePermission -- fixture lam-err-public-policy-access-flag, step task#8 (5 steps) ===
AWS: Smithy: RemovePermission, DELETE /2015-03-31/functions/{FunctionName}/policy/{StatementId}, http code 204, empty output; modeled errors PreconditionFailedException 412, PublicPolicyException 400, ResourceNotFoundException 404.
Emulator: HTTP 500 AmbiguousService instead of 204.
Why: a granted permission can never be revoked.
Fix: same resolution seam.
Traces: lam-err-public-policy-access-flag#task#8.

=== Cause 9 of 10: CreateCodeSigningConfig -- fixture lam-err-code-signing-warn-then-enforce, step task#2 (5 steps) ===
AWS: Smithy: CreateCodeSigningConfig, POST /2020-04-22/code-signing-configs, http code 201, output member CodeSigningConfig (required: AllowedPublishers, CodeSigningConfigArn, CodeSigningConfigId, CodeSigningPolicies, Description, LastModified); modeled errors InvalidParameterValueException 400, ServiceException 500.
Emulator: HTTP 500 AmbiguousService; CodeSigningConfig missing.
Why: code-signing configuration cannot be created, so the whole warn-then-enforce deployment-policy surface is unusable.
Fix: same resolution seam.
Traces: lam-err-code-signing-warn-then-enforce#task#2 (sibling: lam-err-code-signing-warn-then-enforce#teardown#8 DeleteFunctionCodeSigningConfig expecting 204).

=== Cause 10 of 10: GetCapacityProvider -- fixture lam-err-capacity-provider-duplicate-create, step task#7 (1 step) ===
AWS: recorded behavior only for this newer capacity-provider surface (it is bound under the lambda endpointPrefix alongside the 2015-03-31 control plane). The recording shows HTTP 200 with a CapacityProvider envelope (CapacityProviderArn, State 'Active', CapacityProviderScalingConfig{ScalingMode,MaxVCpuCount}, InstanceRequirements, VpcConfig, PermissionsConfig, TelemetryConfig, LastModified) and HTTP 404 once the provider is deleted.
Emulator: HTTP 500 AmbiguousService, so the fixture's poll for State 'Active' never satisfies and ends '500 AmbiguousService cannot satisfy until $.CapacityProvider.State in [Active] after 1 attempt'; the 404-after-delete assertion fails the same way.
Why: this confirms the refusal is not specific to the 2015-03-31 operations -- it precedes dispatch for every operation under the lambda prefix, including the ones the three newer lambda-family models own. It is also the strongest hint at the fix: those models bind distinct URI prefixes (Lambda Core serves /2026-04-04/network-connectors), so method+URI is enough to disambiguate.
Fix: same resolution seam; disambiguate by matching the request's method+URI against each candidate service's bound operations.
Traces: lam-err-capacity-provider-duplicate-create#task#7 (sibling: lam-err-capacity-provider-duplicate-create#teardown#9 DeleteCapacityProvider expecting 202, #teardown#10 expecting 404).

NOTE on a divergence NOT filed here: the cluster for ec2 CreateVpc (lam-err-capacity-provider-duplicate-create#setup#1, 501) is reported with a stub_path (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc.rs), so its issue is filed by the replay tooling and is excluded from this bundle.

## Evidence
Replay summary (simparity-wire-aws-v1): emulator_up true, bootstrap_signal false, state_reset_failures 0, fixtures_replayed 111, steps_total 1100, steps_passed 185, steps_mismatched 661, score_pct 16.8, divergence_count 176. 175 of the 176 clusters carry service='lambda' and actual_status=500; the only non-lambda, non-500 cluster is the ec2 CreateVpc stub (501).

Expected vs actual, per bundled cause (expected status from the recorded response and the Smithy http binding; actual from the replay):
- Cause 1 CreateFunction: expected 201 + FunctionConfiguration body -> got 500, body {"__type":"AmbiguousService", ...}, every asserted member reported 'missing'.
- Cause 2 GetFunctionConfiguration: expected 200 + FunctionConfiguration (State 'Active', LastUpdateStatus 'Successful') -> got 500.
- Cause 3 DeleteFunction: expected 204 (and 404 for the already-deleted sibling step) -> got 500 for both.
- Cause 4 Invoke: expected 200 with x-amz-executed-version (and 202 Event / 204 DryRun siblings) -> got 500, payload and headers absent.
- Cause 5 PublishVersion: expected 201 -> got 500.
- Cause 6 AddPermission: expected 201 + Statement -> got 500, Statement missing.
- Cause 7 GetPolicy: expected 200 + Policy/RevisionId -> got 500, both missing.
- Cause 8 RemovePermission: expected 204 -> got 500.
- Cause 9 CreateCodeSigningConfig: expected 201 + CodeSigningConfig -> got 500, member missing.
- Cause 10 GetCapacityProvider: expected 200 + CapacityProvider{State 'Active'} (404 after delete) -> got 500; the fixture's poll reports "500 AmbiguousService cannot satisfy until $.CapacityProvider.State in ['Active'] after 1 attempt(s)".

Error identity: status 500, __type 'AmbiguousService'. No Lambda operation models an error named AmbiguousService (Lambda's common error is ServiceException; per-op client errors are InvalidParameterValueException 400, ResourceNotFoundException 404, ResourceConflictException 409, PreconditionFailedException 412, TooManyRequestsException 429, ...), so the code itself is non-AWS. Recorded message (EXAMPLE, not a requirement): "4 emulated services all claim requests for 'lambda': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one". Response framing also diverges as part of this envelope: content-type 'application/x-amz-json-1.1' where these restJson1 operations return 'application/json'.

Root-cause evidence in the checkout: core/router/src/lib.rs documents that "MultiServiceRouter picks the service whose can_handle claims the request (seeded from the model's SigV4 signing name)" and is @generated/re-rendered on every service add; the editable seam is each service crate's service.rs, e.g. services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/service.rs can_handle, whose own comment says to edit it for "a signing name shared by several services" but which still returns `key == self.entry.identity().signing_name()`. Smithy identities confirm the collision: endpointPrefix/sigv4Name 'lambda' for AWSGirApiService (88 ops, /2015-03-31/...), LambdaCoreApiService (6 ops, /2026-04-04/network-connectors), LambdaWeb (20 ops) and LambdaMicrovms (24 ops).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (the data listener plus /__admin/health).
2. Replay the simparity-wire-aws-v1 corpus. Every lambda-signed step fails; the representative binding is fixture lam-err-code-signing-warn-then-enforce, step setup#1 (CreateFunction).
3. Minimal manual reproduction: POST http://<emulator>/2015-03-31/functions with a SigV4 Authorization header whose credential scope names service 'lambda' and a body {"FunctionName":"probe-fn","Role":"arn:aws:iam::000000000000:role/r","Runtime":"python3.12","Handler":"index.handler","Code":{"ZipFile":"<base64 zip>"}}. Observe HTTP 500 with __type 'AmbiguousService' instead of HTTP 201 with a FunctionConfiguration body.
4. Repeat with any other lambda-prefixed path -- GET /2015-03-31/functions/probe-fn/configuration, POST /2015-03-31/functions/probe-fn/invocations, DELETE /2015-03-31/functions/probe-fn, POST /2015-03-31/functions/probe-fn/policy, POST /2020-04-22/code-signing-configs, or a capacity-provider path -- and observe the identical 500 AmbiguousService: the refusal precedes operation dispatch, so no handler is involved.
5. After fixing the resolution seam, re-replay the FULL corpus (not just the ten bindings on this issue): all ten bundled causes plus the remaining 165 lambda clusters must be re-checked, since further divergences may be masked behind this refusal today.
