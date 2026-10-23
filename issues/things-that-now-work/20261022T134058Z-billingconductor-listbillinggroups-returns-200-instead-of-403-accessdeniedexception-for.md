Issue-Source: trace-sweep-divergence
Issue-Trace: billingconductor-payer-account-access-denied
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: billingconductor-payer-account-access-denied
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor|ListBillingGroups|4xx->2xx|$.Message
Issue-Key: billingconductor|ListBillingGroups|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: billingconductor|ListBillingGroups|4xx->2xx|status
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# ListBillingGroups returns 200 instead of 403 AccessDeniedException for non-payer account

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBillingGroups

## Description
This issue carries 1 cause.

Cause 1 of 1: ListBillingGroups called by an account that is not the organization's payer (management) account.

AWS: Smithy — ListBillingGroups (POST /list-billing-groups) models AccessDeniedException among its errors, @error("client") with httpError 403 and a single @required member {Message: String}, documented as "You do not have sufficient access to perform this action." AWS Billing Conductor is a payer-account-only API: a caller that is not the payer account is rejected with HTTP 403 AccessDeniedException rather than being served an (empty) result set. On the wire the REST-JSON error carries the shape name in the `x-amzn-errortype` response header, i.e. `x-amzn-errortype: AccessDeniedException`, plus the required `Message` in the body. The recorded message ("Account 000000000000 is not authorized to call ListBillingGroups. Only payer account is authorized") is built from the request — the calling account id and the operation name — so that construction is the requirement; the literal account id in the recording is environment-specific and is an example only.

Emulator: answers HTTP 200 with a normal ListBillingGroups result. No `x-amzn-errortype` header and no `Message` member are emitted — the payer-account authorization check is simply absent, so the operation succeeds for every caller.

Why: a client relying on AWS's behavior cannot distinguish "not authorized" from "authorized, zero billing groups". Code that tests the payer-account precondition by catching AccessDeniedException sees success and proceeds; error-handling and least-privilege paths are never exercised, and tests that assert the 403 pass against AWS but fail here (or vice versa). Because the status itself is wrong, no retry/auth-refresh logic in the SDK is triggered either.

Fix: the ListBillingGroups handler's authorization gate (the caller-identity check that precedes the state read) must reject a non-payer caller by raising the modelled AccessDeniedException, and the error serializer must render it as 403 with `x-amzn-errortype: AccessDeniedException` and a `Message` naming the calling account and the operation. This gate is shared by the whole billingconductor surface, so prefer a service-wide check over a per-operation one.

Traces: billingconductor-payer-account-access-denied#task#1.

## Evidence
Replay verdict `mismatch`.
  fixture_id: billingconductor-payer-account-access-denied
  step_key: task#1
  divergence_key: service=billingconductor|ListBillingGroups|status=403->200|keys=$.Message,$headers.x-amzn-errortype,status
  expected_status: 403, actual_status: 200
  detail:
    status: expected 403, got 200
    $.Message: missing (expected 'Account 000000000000 is not authorized to call ListBillingGroups. Only payer account is authorized', got None)
    $headers.x-amzn-errortype: missing (expected 'AccessDeniedException', got None)

Expected: HTTP 403, response header `x-amzn-errortype: AccessDeniedException`, body carrying the required `Message` identifying the unauthorized calling account and the operation. Actual: HTTP 200 with a success payload, no error type header, no Message. The status and the error code are the requirements; the quoted message text is an example of the request-derived rule.

Model citations: ListBillingGroups errors include AccessDeniedException (errorKind client, httpError 403); AccessDeniedException{Message: String @required}, doc "You do not have sufficient access to perform this action."; ListBillingGroups http {method POST, uri /list-billing-groups, code 200}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), trace set simparity-wire-aws-v1.
2. Replay fixture billingconductor-payer-account-access-denied, step task#1: POST /list-billing-groups signed as an account that is not the organization payer account.
3. Observe HTTP 200 with a ListBillingGroups success body, no `x-amzn-errortype` header and no `Message` member.
4. After the fix, re-replay that step: it must return HTTP 403 with `x-amzn-errortype: AccessDeniedException` and a `Message` naming the calling account id and `ListBillingGroups`.
