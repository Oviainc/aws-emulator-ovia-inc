Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-readiness.cross-account-authorization-lifecycle
Issue-Shape-Id: com.amazonaws.route53recoveryreadiness#Route53RecoveryReadiness
Issue-Key: route53-recovery-readiness.cross-account-authorization-lifecycle|run#13|AssertionError|raise AssertionError(
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# route53-recovery-readiness CreateCrossAccountAuthorization accepts a malformed authorization value (trailing whitespace) instead of rejecting it with 400 Validation

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCrossAccountAuthorization

## Description
Cause 1 of 1: CreateCrossAccountAuthorization performs no validation of the CrossAccountAuthorization value, so a value that is not a well-formed account root ARN (here: one differing from a valid value only by a trailing space) is accepted and persisted instead of being refused.

AWS: The Smithy model for com.amazonaws.route53recoveryreadiness#CreateCrossAccountAuthorization declares the single required member CrossAccountAuthorization (jsonName `crossAccountAuthorization`, target shape `CrossAccountAuthorization`, a plain string with NO @length/@pattern constraint) and models ValidationException (httpError 400) among its errors. The model therefore states only that a validation rejection path exists for this operation; it does not state the literal-bytes rule. The rule itself is recorded behavior only: real ARC validates the literal bytes of the submitted value, whitespace included, and refuses a value that is not a well-formed `arn:aws:iam::<12 digits>:root` — the recorded rejection is HTTP 400 with error code `Validation` (as recorded; note this differs from the modeled shape name ValidationException) and the recorded message, as an example, `Invalid cross account authorization`. Nothing is persisted by the refused call, so the value never appears in ListCrossAccountAuthorizations.

Emulator: The call returns success for `arn:aws:iam::<12 digits>:root ` (valid ARN plus one trailing space). No error is raised at all, so the check's negative beat fails with `AssertionError: expected trailing-whitespace value 'arn:aws:iam::...:root ' to be rejected, but create succeeded`. The emulator accepts the value and, by the same path, admits it into the cross-account authorization list.

Why: A client that relies on AWS rejecting malformed authorization values gets no feedback that it submitted an unusable grant. The authorization is silently stored, so ListCrossAccountAuthorizations returns an entry that could never be honored, and typos/whitespace from config or CLI pipelines become persistent phantom grants rather than an immediate 400. Code that distinguishes "grant created" from "grant rejected" by catching the client error takes the success branch for input AWS refuses, and no later call surfaces the problem.

Fix: Input validation for the CreateCrossAccountAuthorization handler (services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/create_cross_account_authorization.rs): validate the CrossAccountAuthorization value's literal bytes before touching state — no stripping or other normalization — and on failure return the operation's modeled 400 validation error with error code `Validation`, persisting nothing. The positive beat of the same check (a well-formed value round-tripping byte-for-byte through list, then disappearing after delete) passes today and must keep passing.

Related-but-not-covering: the open issue `20261022T185117Z-route53-recovery-readiness-createcrossaccountauthorization-stub.md` asks for CreateCrossAccountAuthorization to be implemented at all (it recorded a 501 scaffold refusal). The operation now answers, and this issue is about the validation gap in that answer, not about the 501.

Traces: route53-recovery-readiness.cross-account-authorization-lifecycle (the `padded_auth` negative beat; the preceding create/list/delete beats of the same check pass).

## Evidence
Check: route53-recovery-readiness.cross-account-authorization-lifecycle — verdict [mismatch].

Expected (recorded AWS): create_cross_account_authorization(CrossAccountAuthorization='arn:aws:iam::<12 digits>:root ') raises ClientError with HTTP 400, Error.Code == 'Validation' and Error.Message == 'Invalid cross account authorization' (message shown as the recorded example); the value is absent from every page of list_cross_account_authorizations afterwards.

Actual (emulator): the call returned successfully, no ClientError raised. Replay output:
  File "check_route53_recovery_readiness_cross_account_authorization_lifecycle.py", line 70, in run
    raise AssertionError(
        f"expected trailing-whitespace value {padded_auth!r} to be rejected, but create succeeded"
    )
  AssertionError: expected trailing-whitespace value 'arn:aws:iam::631613624466:root ' to be rejected, but create succeeded

Model evidence: smithy_query(operation=CreateCrossAccountAuthorization) — input member CrossAccountAuthorization (required, jsonName crossAccountAuthorization); errors include ValidationException httpError 400. smithy_query(shape=CrossAccountAuthorization) — type string, no length/pattern constraint, so the literal-bytes rule is recorded behavior only.

## Steps to Reproduce
1. Boot the route53-recovery-readiness emulator.
2. Call CreateCrossAccountAuthorization with CrossAccountAuthorization='arn:aws:iam::123456789012:root' (well-formed) and confirm it succeeds and appears verbatim in ListCrossAccountAuthorizations — positive control.
3. Call CreateCrossAccountAuthorization with CrossAccountAuthorization='arn:aws:iam::210987654321:root ' (same form plus a single trailing space).
4. Observe the emulator returns 200/success. Expected: HTTP 400 with error code 'Validation' and nothing persisted.
5. Call ListCrossAccountAuthorizations and observe the trailing-space value present; it should never appear.
6. Equivalently: replay check route53-recovery-readiness.cross-account-authorization-lifecycle.
