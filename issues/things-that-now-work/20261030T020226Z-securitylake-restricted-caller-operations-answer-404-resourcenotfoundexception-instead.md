Issue-Source: trace-sweep-divergence
Issue-Trace: securitylake-iam-policy-restricted-ops-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securitylake-iam-policy-restricted-ops-error-paths
Issue-Shape-Id: com.amazonaws.securitylake#SecurityLake
Issue-Key: securitylake|GetSubscriber|4xx->4xx|$.message
Issue-Key: securitylake|GetSubscriber|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|GetSubscriber|4xx->4xx|status
Issue-Key: securitylake|DeleteDataLake|4xx->4xx|$.message
Issue-Key: securitylake|DeleteDataLake|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|DeleteDataLake|4xx->4xx|status
Issue-Key: securitylake|DeleteSubscriber|4xx->4xx|$.message
Issue-Key: securitylake|DeleteSubscriber|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|DeleteSubscriber|4xx->4xx|status
Issue-Commit: 3ca7a48f5c7417112003e42cf25e2b2527498f37

# securitylake: restricted-caller operations answer 404 ResourceNotFoundException instead of the 401 UnauthorizedException refusal

> _Note: this issue was identified against the code base at commit `3ca7a48f5c7417112003e42cf25e2b2527498f37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSubscriber

## Description
The emulated Security Lake control plane has no caller-authorization stage: every handler goes straight from state lookup to a state-derived answer, so a caller that Security Lake refuses outright is instead told about the state of the (empty) data lake. Three operations in the recorded restricted-caller fixture diverge the same way — all three answer HTTP 404 ResourceNotFoundException where AWS answers HTTP 401 UnauthorizedException with the message "Unauthorized". One root cause, three causes to close, all in the securitylake service crate.

Related but NOT covering: the open behavioral issue "securitylake: the account-entitlement gate is missing entirely" (20261029T180250Z-securitylake-the-account-entitlement-gate-is-missing-entirely.md) describes the same missing gate for CreateDataLake / CreateDataLakeExceptionSubscription / GetDataLakeExceptionSubscription / GetDataLakeSources and mentions DeleteDataLake's UnauthorizedException pair in prose; it carries no wire replay key for any step below, and the subscriber-surface operations (GetSubscriber, DeleteSubscriber) are not in it at all. The replay reports these three clusters as unowned, so they are filed here.

=== Cause 1 of 3: GetSubscriber answers 404 ResourceNotFoundException for a caller AWS refuses with 401 UnauthorizedException ===

AWS: GetSubscriber is GET /v1/subscribers/{subscriberId}, success 200 with `subscriber` (Smithy: GetSubscriber, input member subscriberId @httpLabel @required target UUID pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$; modeled errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429). When the caller is not permitted to use the Security Lake control plane, AWS refuses the request BEFORE any resource or enablement lookup: recorded behavior only — HTTP 401, header x-amzn-errortype `UnauthorizedException`, body `message` exactly "Unauthorized". UnauthorizedException is NOT in this operation's modeled error set nor in the service's common errors, so the recording is the only authority for it. The message is ENTIRELY FIXED TEXT: "Unauthorized" interpolates nothing from the request or from state — no subscriber id, no Region, no account id — so it must be reproduced verbatim and must not be replaced by a state-derived sentence.

Emulator: HTTP 404 with x-amzn-errortype `ResourceNotFoundException` and message "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again." — i.e. the enablement precondition answered a request authorization should have rejected first.

Why: a client cannot tell "you may not call this API" from "Security Lake is not enabled yet". Code that handles the 401 refusal (re-assuming a permitted role, aborting with a permissions error, surfacing an IAM remediation) never runs; instead the client follows the message's documented remedy and tries to enable Security Lake, a call the same restriction also refuses. SDK error handling also classifies the two differently, so retry/branch logic keyed on UnauthorizedException is dead code.

Fix: the GetSubscriber handler's admission/authorization stage — the caller-permission check must run before the enablement precondition and before any store read, and belongs in a shared gate in the service state rather than per-handler. See Traces for the exact location.

Traces: securitylake-iam-policy-restricted-ops-error-paths#task#1. Source: services/service-com-amazonaws-securitylake-securitylake/src/ops/get_subscriber.rs, async fn handle (lines 150-196) — state lookup at 155, caller_region at 164, the `enabled_in_any_region()` -> ResourceNotFoundException branch at 172-177; there is no authorization branch anywhere ahead of it.

=== Cause 2 of 3: DeleteDataLake answers 404 ResourceNotFoundException for a caller AWS refuses with 401 UnauthorizedException ===

AWS: DeleteDataLake is POST /v1/datalake/delete, success 200 with an empty body (Smithy: DeleteDataLake, input member regions @required, list of RegionList, uniqueItems; @idempotent; modeled errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429). For a restricted caller the recorded response is HTTP 401, x-amzn-errortype `UnauthorizedException`, `message` exactly "Unauthorized" (recorded behavior only; UnauthorizedException is unmodeled here). All fixed text — the Region list in the request does not appear in the message.

Emulator: HTTP 404, x-amzn-errortype `ResourceNotFoundException`, message "Security Lake is not enabled in Region us-west-2" — a state-derived sentence whose variable part is the caller's Region. The divergence is not the Region value but that a per-Region enablement verdict is produced at all for a request that is refused before state is consulted.

Why: DeleteDataLake is a destructive, idempotent teardown. A client that treats 404 as "already gone / nothing to delete" will record the teardown as complete and move on, while AWS never authorized the call and the data lake (in a real account) still exists. Reconciling tooling therefore silently skips work it believes done.

Fix: the DeleteDataLake handler's admission/authorization stage, ahead of the enablement/Region lookup and ahead of any mutation, routed through the same shared gate as Cause 1 so one caller yields one verdict across operations. See Traces for the exact location.

Traces: securitylake-iam-policy-restricted-ops-error-paths#task#4. Source: services/service-com-amazonaws-securitylake-securitylake/src/ops/delete_data_lake.rs, async fn handle.

=== Cause 3 of 3: DeleteSubscriber answers 404 ResourceNotFoundException for a caller AWS refuses with 401 UnauthorizedException ===

AWS: DeleteSubscriber is DELETE /v1/subscribers/{subscriberId}, success 200 with an empty body (Smithy: DeleteSubscriber, input member subscriberId @httpLabel @required target UUID with the same UUID pattern; @idempotent; modeled errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429). For a restricted caller: HTTP 401, x-amzn-errortype `UnauthorizedException`, `message` exactly "Unauthorized" (recorded behavior only; unmodeled error), entirely fixed text with no interpolated subscriber id.

Emulator: HTTP 404, x-amzn-errortype `ResourceNotFoundException`, message "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again."

Why: same class as Cause 2 on the subscriber surface — a deletion a client reads as already-applied when AWS in fact refused it, plus the loss of the authorization/enablement distinction a client branches on.

Fix: the DeleteSubscriber handler's admission/authorization stage, ahead of the enablement precondition and of any store mutation, through the shared gate from Cause 1. See Traces for the exact location.

Traces: securitylake-iam-policy-restricted-ops-error-paths#task#12. Source: services/service-com-amazonaws-securitylake-securitylake/src/ops/delete_subscriber.rs, async fn handle.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200): 3 divergence clusters, all service `securitylake`, all verdict `mismatch`, none stubbed and none owned by an existing issue.

Cause 1 — fixture securitylake-iam-policy-restricted-ops-error-paths, step task#1, operation GetSubscriber:
  status: expected 401, got 404
  $.message: expected 'Unauthorized', got "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again."
  $headers.x-amzn-errortype: got 'ResourceNotFoundException' (AWS: UnauthorizedException)

Cause 2 — same fixture, step task#4, operation DeleteDataLake:
  status: expected 401, got 404
  $.message: expected 'Unauthorized', got 'Security Lake is not enabled in Region us-west-2' (the Region token is a per-environment value from the request, not part of any requirement)
  $headers.x-amzn-errortype: got 'ResourceNotFoundException'

Cause 3 — same fixture, step task#12, operation DeleteSubscriber:
  status: expected 401, got 404
  $.message: expected 'Unauthorized', got "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again."
  $headers.x-amzn-errortype: got 'ResourceNotFoundException'

Required in every case: HTTP 401 / x-amzn-errortype UnauthorizedException / message "Unauthorized" — the status, the error code and the message are all fixed, with no part taken from the request or from state. Model check: `smithy_query` on services/service-com-amazonaws-securitylake-securitylake/smithy-model/com-amazonaws-securitylake-securitylake.json shows no UnauthorizedException shape bound to these operations (AccessDeniedException 403 / ResourceNotFoundException 404 are the nearest modeled errors), so this status/code pair is recorded behavior only and must be emitted explicitly by the handlers.

Source confirmation that the cause is a missing stage rather than a wrong branch: services/service-com-amazonaws-securitylake-securitylake/src/ops/get_subscriber.rs:150-196 performs state lookup -> caller_region -> `enabled_in_any_region()` -> ResourceNotFoundException, with no authorization check at any point.

## Steps to Reproduce
1. git_clone, then boot the emulator from the checkout.
2. Replay fixture `securitylake-iam-policy-restricted-ops-error-paths` (replay_wire_corpus) — the caller in this fixture is one Security Lake refuses with UnauthorizedException.
3. Cause 1 — step task#1: GET /v1/subscribers/{subscriberId} with a well-formed UUID subscriber id. Expect HTTP 401, x-amzn-errortype UnauthorizedException, body {"message":"Unauthorized"}; observe HTTP 404 ResourceNotFoundException with the "Security Lake isn't enabled for your account in any Regions" message.
4. Cause 2 — step task#4: POST /v1/datalake/delete with {"regions":["<region>"]}. Expect the same 401/UnauthorizedException/"Unauthorized"; observe HTTP 404 ResourceNotFoundException "Security Lake is not enabled in Region <region>".
5. Cause 3 — step task#12: DELETE /v1/subscribers/{subscriberId}. Expect the same 401/UnauthorizedException/"Unauthorized"; observe HTTP 404 ResourceNotFoundException with the not-enabled-in-any-Region message.
6. Re-replay to confirm the fix: securitylake-iam-policy-restricted-ops-error-paths#task#1, #task#4 and #task#12 must all pass (all three causes must be closed before submitting).
