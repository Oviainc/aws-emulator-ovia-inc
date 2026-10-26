Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.lambda-sesv2-put-suppressed-destination-positive
Issue-Key: lambda__sesv2|GetAccount|NotImplemented
Issue-Commit: 976c0fb9e67fe98b916554c76ebfa4c22f208dcd

# lambda__sesv2: the Lambda-driven PutSuppressedDestination sandbox-refusal scenario cannot start — SESv2 GetAccount is an unimplemented scaffold stub answering 501 NotImplemented

> _Note: this issue was identified against the code base at commit `976c0fb9e67fe98b916554c76ebfa4c22f208dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccount

## Description
Attribution note: this check's id token is the composite `lambda__sesv2`. Its constituent services (lambda and sesv2, plus the iam surface the check drives) are all stood up in this checkout — `service-com-amazonaws-lambda-awsgirapiservice` and `service-com-amazonaws-sesv2-simpleemailservice-v2` are both in the bootstrapped set and neither is pending — so the per-service bootstrap gate is FILE.

One root cause, 1 failing check. This is the ONLY failure in this replay that the report did not mark ALREADY FILED or OWNED; everything else in the sesv2/lambda__sesv2/s3__ses__sesv2/sesv2__sns/sesv2__sns__sqs/firehose__s3__sesv2 groups is skipped as already carried.

Related, but NOT covering this failure: `20261025T214624Z-sesv2-getaccount-stub.md` (open) describes the same stub but carries only `sesv2.*` replay keys (Issue-Key `sesv2|GetAccount|NotImplemented`, 14 `sesv2.` units), so the cross-service unit below is not in its scope; and `20261023T183615Z-lambda-sesv2-lambda-driven-sesv2-scenarios-all-fail.md` (open) lists this check id, but only under `lambda__sesv2|GetAccount|AmbiguousService` — the pre-dispatch router refusal it describes no longer reproduces (the request now reaches the SESv2 handler), so its key does not match this failure. Fixing the handler closes both this issue's unit and the stub issue's.

=== Cause 1 of 1: SESv2 GetAccount answers 501 NotImplemented, so the account/sandbox plane is unreadable and the Lambda-driven suppression scenario aborts at its first call ===
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) — `GetAccount`: GET /v2/email/account, success code 200, empty input; output `GetAccountResponse` carries `DedicatedIpAutoWarmupEnabled` (Enabled, default false, required), `ProductionAccessEnabled` (Enabled, default false, required), `SendingEnabled` (Enabled, default false, required), plus `SendQuota`, `SuppressionAttributes`, `EnforcementStatus`, `Details`, `VdmAttributes`, `PricingAttributes`. Its ONLY modeled errors are `BadRequestException` (client, httpError 400, "The input you provided is invalid.") and `TooManyRequestsException` (client, httpError 429, "Too many requests have been made to the operation."). `NotImplemented` is not in that error set and 501 is not a status this operation can answer; AWS answers 200 with the account's current attributes for any authorized caller, no input required. The documented meaning of `ProductionAccessEnabled=false` is that the account is in the SES sandbox, which is exactly the account state this check reads to decide its own applicability (and the state that makes `PutSuppressedDestination` — PUT /v2/email/suppression/addresses, modeled errors BadRequestException 400 / NotFoundException 404 / TooManyRequestsException 429 — refusable with `BadRequestException` naming the sandbox rather than an IAM `AccessDeniedException`).
Emulator: the call is refused with HTTP 501, error code `NotImplemented`, message "operation GetAccount is scaffolded but not implemented in this emulator". Fixed text of that message: "operation " … " is scaffolded but not implemented in this emulator"; the variable part is the operation name taken from the request (`GetAccount`). No account state is read and none is persisted. The handler is an unmodified scaffold: `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_account.rs` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4) and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("GetAccount"))` (line 181).
Why: a client cannot discover its own account's sending status, quota, suppression preferences or sandbox/production state at all — every caller gets a 501 with a code outside the modeled error set, which botocore surfaces as a service fault rather than a readable response or a modeled 400/429. Any workflow that branches on `ProductionAccessEnabled` (sandbox-only sends, suppression-list writes, production-access gating) is unreachable, and the more specific downstream contract this check exists to assert — that a role holding `ses:PutSuppressedDestination` clears IAM yet the write is still refused by SES's account-wide sandbox restriction with `BadRequestException`, not `AccessDeniedException` — can never be observed, because the positive control that proves the role/Lambda-to-SESv2 wiring is this very `GetAccount` call.
Fix: implement the operation's handler, `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_account.rs` — read the service's account-level state through `crate::state::service_state` and return the modeled `GetAccountResponse` with 200, keeping the modeled error channel (BadRequestException/TooManyRequestsException) as the only refusals; then delete that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line. The account state it reports must be the same state the account-plane writers mutate, so that a sandbox account reports `ProductionAccessEnabled=false` and `PutSuppressedDestination` can be refused on that basis by its own handler.
Traces: lambda__sesv2.lambda-sesv2-put-suppressed-destination-positive.

## Evidence
Suite simparity-behavioral-ga-prod-v1: 3/113 passed; lambda__sesv2 0/16. Of the 54 reported failure clusters, 53 are marked ALREADY FILED or OWNED and skipped; this is the one survivor.

[error] lambda__sesv2.lambda-sesv2-put-suppressed-destination-positive
Expected: `sesv2.get_account()` returns HTTP 200 with the modeled GetAccountResponse (at minimum the required `SendingEnabled`, `ProductionAccessEnabled`, `DedicatedIpAutoWarmupEnabled` members), so the check can read `ProductionAccessEnabled` and then assert that a Lambda role granting both `ses:GetAccount` and `ses:PutSuppressedDestination` clears IAM while the suppression write is still refused by SES with `BadRequestException` naming the sandbox.
Actual:
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAccount operation: operation GetAccount is scaffolded but not implemented in this emulator

The check aborts on its first SESv2 call (`acct = sesv2.get_account()`), before any role, function or suppression write is created, so nothing downstream of GetAccount is graded.

Source of the stub: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_account.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 181 (`Err(crate::scaffolded::Scaffolded::not_implemented("GetAccount"))`).

## Steps to Reproduce
1. Boot the emulator from this checkout (sesv2 and lambda are both bootstrapped).
2. Call SESv2 GetAccount with no input (GET /v2/email/account) as any authorized caller: observe HTTP 501 with error code `NotImplemented` and message "operation GetAccount is scaffolded but not implemented in this emulator", instead of HTTP 200 with GetAccountResponse (SendingEnabled / ProductionAccessEnabled / DedicatedIpAutoWarmupEnabled, SendQuota, SuppressionAttributes, ...). No state is read or written.
3. Replay lambda__sesv2.lambda-sesv2-put-suppressed-destination-positive and observe it abort at its first statement (`sesv2.get_account()`) on that refusal, never reaching the IAM role, the Lambda function, or the PutSuppressedDestination sandbox-refusal assertion.
4. After implementing the handler, re-replay that check id to confirm.
