Issue-Source: trace-sweep-divergence
Issue-Trace: securitylake-data-lake-empty-state-reads
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securitylake-data-lake-empty-state-reads securitylake-iam-policy-restricted-ops-error-paths
Issue-Shape-Id: com.amazonaws.securitylake#SecurityLake
Issue-Key: securitylake|ListDataLakeExceptions|4xx->4xx|$.message
Issue-Key: securitylake|ListDataLakeExceptions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|ListDataLakeExceptions|4xx->4xx|status
Issue-Key: securitylake|UpdateDataLake|4xx->4xx|$.message
Issue-Key: securitylake|UpdateDataLake|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|UpdateDataLake|4xx->4xx|status
Issue-Commit: e635fc8db7925c597029a243d97ecda6419e6d1a

# securitylake: spurious 403 AccessDeniedException entitlement gate preempts recorded refusals

> _Note: this issue was identified against the code base at commit `e635fc8db7925c597029a243d97ecda6419e6d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDataLakeExceptions

## Description
The emulated Security Lake front door runs a pre-dispatch "account-entitlement gate" (services/service-com-amazonaws-securitylake-securitylake/src/service.rs, `ENTITLEMENT_GATED_OPERATIONS` + `Service::override_operation`) that answers HTTP 403 `AccessDeniedException` with the fixed text "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account." whenever the stored caller role is not delegated-administrator/standalone. That gate is wrong on both sides: the caller role defaults to `AccountRole::OrganizationMember` and no modelled operation ever sets it, so the gate fires for ORDINARY callers whose calls AWS answers from account state; and because it sits AFTER the principal-admission check that never recognises the recorded restricted principal, it also swallows the 401 "Unauthorized" refusal AWS gives a caller it refuses outright. One root cause, two causes to close, both in the securitylake service crate.

Related but NOT covering: the open issue "securitylake: restricted-caller operations answer 404 ResourceNotFoundException instead of the 401 UnauthorizedException refusal" (20261030T020226Z-securitylake-restricted-caller-operations-answer-404-resourcenotfoundexception-instead.md) carries the replay keys for GetSubscriber#task#1, DeleteDataLake#task#4 and DeleteSubscriber#task#12 of the same restricted-caller fixture — the UN-gated operations, which fall through to a handler and answer 404. Neither of the two causes below (the GATED route that answers 403, and the empty-state read that answers 403) is in it, and the replay reports both clusters as unowned.

=== Cause 1 of 2: ListDataLakeExceptions answers 403 AccessDeniedException where AWS answers the 404 not-enabled precondition ===

AWS: ListDataLakeExceptions is POST /v1/datalake/exceptions, success HTTP 200 with `exceptions` (DataLakeExceptionList) and `nextToken` (len ..2048) (Smithy: ListDataLakeExceptions, @readonly, @paginated inputToken=nextToken/outputToken=nextToken/pageSize=maxResults/items=exceptions; input `regions` RegionList uniqueItems, `maxResults` range 1..100 default 50; modeled errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429). It reports the caller's OWN account state, so it is not refused on the caller's Security Lake role: when the account has Security Lake enabled in no Region at all, the recorded answer is HTTP 404, header x-amzn-errortype `ResourceNotFoundException`, body `message` exactly "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again." (recorded behavior only for the status/message pairing; ResourceNotFoundException 404 is modeled). That message is ENTIRELY FIXED TEXT — it interpolates no Region, account id or request value — and must be reproduced verbatim; the emulator already holds it as `state::NOT_ENABLED_IN_ANY_REGION_MESSAGE`.

Emulator: HTTP 403, x-amzn-errortype `AccessDeniedException`, message "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account." — the entitlement gate's verdict, reached before dispatch, for a caller AWS does not refuse at all.

Why: an empty-state read is exactly how a client discovers that Security Lake is not enabled yet and then enables it. Turned into a 403 AccessDeniedException, the condition reads as an IAM/permissions problem: SDK error classification makes it terminal, the enablement bootstrap path ("enable Security Lake, then retry") is never taken, and operators are sent to fix a policy that is not broken. The documented remedy in the 404 message is lost, and the operation's `exceptions`/`nextToken` pagination surface is never exercised.

Fix: the pre-dispatch entitlement gate's gated-operation table and the operation's own handler. ListDataLakeExceptions must not be refused on the caller's Security Lake role (it reports only the caller's own state, like the deliberately un-gated ListDataLakes), and the operation's handler must raise the not-enabled-in-any-Region precondition from state and otherwise serve the modelled 200 — that handler is still the scaffolded 501 stub, so un-gating alone does not close this cause. File/line citations under Traces.

Traces: securitylake-data-lake-empty-state-reads#task#4. Source: services/service-com-amazonaws-securitylake-securitylake/src/service.rs:121-134 (`ENTITLEMENT_GATED_OPERATIONS`, line 131 lists `("ListDataLakeExceptions", Gate::AccessDenied)`), :337-374 (`override_operation`, gate applied at 366-373), :206-234 (`NotEntitledAnswer`, 403 + NOT_ENTITLED_MESSAGE); services/service-com-amazonaws-securitylake-securitylake/src/state.rs:61 (the required 404 message), :258-262 (`account_role` defaults to OrganizationMember, written by no modelled operation), :331-333 (`enabled_in_any_region`), :466-472 (`not_enabled_in_any_region()`); services/service-com-amazonaws-securitylake-securitylake/src/ops/list_data_lake_exceptions.rs:166-184 (handler still returns `Scaffolded::not_implemented`).

=== Cause 2 of 2: UpdateDataLake answers 403 AccessDeniedException for a caller AWS refuses with the 401 "Unauthorized" refusal ===

AWS: UpdateDataLake is PUT /v1/datalake, @idempotent, success HTTP 200 with `dataLakes` (DataLakeResourceList) (Smithy: UpdateDataLake, input `configurations` DataLakeConfigurationList @required length min 1, `metaStoreManagerRoleArn` RoleArn pattern ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$; modeled errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429). When the signing principal is one Security Lake refuses outright, the request is rejected before the service reads it: recorded behavior only — HTTP 401 with body `message` exactly "Unauthorized" (the recording does not assert x-amzn-errortype on this step; no 401 shape is modeled for this service, so the status and message are the only authority). "Unauthorized" is entirely fixed text: it interpolates nothing from the request (no configurations, no Region, no role ARN) or from state.

Emulator: HTTP 403, x-amzn-errortype `AccessDeniedException`, message "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account." The emulator does have the correct 401 refusal wired (`GatewayRefusal` with UNAUTHORIZED_STATUS/UNAUTHORIZED_MESSAGE), but nothing ever records the fixture's restricted principal in `unauthorized_principals`, so `admission()` answers `Admitted` and control falls through to the entitlement gate, which refuses the same request with the wrong status, the wrong error code and a state-flavoured message.

Why: the two refusals mean different things and clients branch on them. A 403 AccessDeniedException on an idempotent upsert reads as "this principal lacks an IAM action", so remediation targets the policy for `securitylake:UpdateDataLake`, while AWS refused the caller for the whole control plane; retry/role-reassumption logic keyed on the 401 refusal is dead code, and a client cannot tell "you may not call Security Lake" from "your account is not the delegated administrator".

Fix: the pre-dispatch admission stage. The principal-authorization verdict must be established for the recorded restricted caller and must take precedence over the account-entitlement gate, so a caller the authorizer refuses gets HTTP 401 / message "Unauthorized" on every route, gated or not, and the entitlement gate answers only for a caller that IS admitted. File/line citations under Traces.

Traces: securitylake-iam-policy-restricted-ops-error-paths#task#8. Source: services/service-com-amazonaws-securitylake-securitylake/src/service.rs:295-302 (`admission`), :359-361 (the 401 branch that never fires), :366-373 (the entitlement gate that answers instead), :149-160 (UNAUTHORIZED_STATUS/MESSAGE/CODE), :238-241 (ACCESS_DENIED_CODE/STATUS), :132 (`("UpdateDataLake", Gate::AccessDenied)`); services/service-com-amazonaws-securitylake-securitylake/src/state.rs:251-257 (`unauthorized_principals`), :271-283 (`admission`/`restrict_principal`, which no modelled operation calls); services/service-com-amazonaws-securitylake-securitylake/src/ops/update_data_lake.rs:171-189 (handler is the scaffolded 501 stub, never reached for this caller).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, bootstrap_signal false, /__admin/health 200 and the data listener accepting; 21 steps, 4 passed, 17 mismatched, 4 divergence clusters). All 4 clusters are service `securitylake`, verdict `mismatch`, none stubbed. Two are owned by 20261030T020226Z-securitylake-restricted-caller-operations-answer-404-resourcenotfoundexception-instead.md and are not re-filed here; the two below are reported unowned.

Cause 1 — fixture securitylake-data-lake-empty-state-reads, step task#4, operation ListDataLakeExceptions (divergence_key securitylake|ListDataLakeExceptions|status=404->403|keys=$.message,$headers.x-amzn-errortype,status):
  status: expected 404, got 403
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException', got 'AccessDeniedException'
  $.message: expected "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again.", got 'To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account.'
  Required: HTTP 404 / x-amzn-errortype ResourceNotFoundException / that exact message. Both strings are wholly fixed text — no part of either is taken from the request or from state, so there is no per-run value in this comparison.

Cause 2 — fixture securitylake-iam-policy-restricted-ops-error-paths, step task#8, operation UpdateDataLake (divergence_key securitylake|UpdateDataLake|status=401->403|keys=$.message,$headers.x-amzn-errortype,status):
  status: expected 401, got 403
  $.message: expected 'Unauthorized', got 'To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account.'
  $headers.x-amzn-errortype: expected <shaped-not-asserted>, got 'AccessDeniedException'
  Required: HTTP 401 with body {"message":"Unauthorized"} — fixed text, nothing interpolated.

Model check (smithy_query operation="ListDataLakeExceptions,UpdateDataLake" on services/service-com-amazonaws-securitylake-securitylake/smithy-model/com-amazonaws-securitylake-securitylake.json): both operations model AccessDeniedException 403 and ResourceNotFoundException 404, and neither models any 401 shape — so the 404 pairing in Cause 1 is a modeled error reached from account state, and the 401 refusal in Cause 2 is recorded behavior the front door must state itself.

Source confirmation that one stage causes both: src/service.rs `override_operation` evaluates `admission` (which can never be Unauthorized, because no operation writes `unauthorized_principals`) and then `entitlement_gate(operation)` against `account_role()`, which defaults to `AccountRole::OrganizationMember` and is likewise written by no modelled operation. ListDataLakeExceptions and UpdateDataLake are both in `ENTITLEMENT_GATED_OPERATIONS` with `Gate::AccessDenied`, so every request to either — ordinary caller or refused caller — is answered 403 by `NotEntitledAnswer` before dispatch.

## Steps to Reproduce
1. git_clone, then boot the emulator from the checkout.
2. replay_wire_corpus to replay trace set simparity-wire-aws-v1.
3. Cause 1 — fixture `securitylake-data-lake-empty-state-reads`, step task#4: with no data lake created in any Region, POST /v1/datalake/exceptions with a well-formed body (optional `regions`, `maxResults` within 1..100). Expect HTTP 404, x-amzn-errortype ResourceNotFoundException, body {"message":"The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again."}; observe HTTP 403 AccessDeniedException with the delegated-administrator/standalone entitlement message.
4. Cause 2 — fixture `securitylake-iam-policy-restricted-ops-error-paths`, step task#8: as the restricted caller of that fixture, PUT /v1/datalake with {"configurations":[...]}. Expect HTTP 401 and body {"message":"Unauthorized"}; observe HTTP 403 with x-amzn-errortype AccessDeniedException and the entitlement message.
5. Re-replay to confirm the fix: securitylake-data-lake-empty-state-reads#task#4 AND securitylake-iam-policy-restricted-ops-error-paths#task#8 must both pass. Both causes must be closed before submitting; also re-check the other steps of both fixtures do not regress (the un-gated restricted-caller steps task#1/#4/#12 are owned by the related issue above).
