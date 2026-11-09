Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-access-request-service-not-enabled-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-access-request-service-not-enabled-errors ssm-jit-access-request-invalid-target-error ssm-jit-access-request-service-not-enabled-error ssm-jit-access-request-workflow ssm-access-token-lookup-not-found ssm-jit-access-token-nonexistent-request-error
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|StartAccessRequest|4xx->4xx|$.message
Issue-Key: ssm|GetAccessToken|4xx->4xx|$.message
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# ssm just-in-time node access: the 400 refusal body carries no message member on StartAccessRequest and GetAccessToken

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAccessRequest, GetAccessToken

## Description
Two net-new root causes in the SSM just-in-time (JIT) node access surface, both in the `ssm` service. On both operations the emulator reaches the right refusal class and the right HTTP status, but serializes the error body WITHOUT the member that carries the human-readable text, so the asserted `$.message` is absent from the response. A fixer must close BOTH causes before submitting; each has its own fixture#step binding, listed under its Traces line and carried in this issue's replay pointer.

Cause 1 of 2: StartAccessRequest's account-gate refusal omits the `message` member.
AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models StartAccessRequest with required `Reason` (String1to256, @length 1..256) and required `Targets` (Targets, @length 0..5), and the client errors AccessDeniedException (httpError 400, single REQUIRED member `Message`), ValidationException (400, members Message/ReasonCode), ResourceNotFoundException (400), ServiceQuotaExceededException (400), ThrottlingException (400). The model does not state the message text, so for the wording: recorded behavior only — in an account where the Just-In-Time Node Access feature has not been enabled, every StartAccessRequest is refused with HTTP 400 and the body member `message` set to the sentence `Just-In-Time Node Access service is not enabled in this account.` That sentence is entirely FIXED text: it has no variable part, it does not name the caller, the targets or the account, and AWS emits it ahead of any target-shape or target-existence check (the recorded corpus reaches it with a well-formed target, with an invalid target, and on the happy-path workflow fixture alike). The error-code discriminator the emulator already serves is correct; the body member on this JSON surface is lower-cased `message`, the same envelope member the recorded `__type` sits beside.
Emulator: answers HTTP 400 with a matching error discriminator but NO `message` member at all — the replay reports `$.message: missing (expected 'Just-In-Time Node Access service is not enabled in this account.', got None)`. The status and class are right; the text is simply not projected. Hypothesis: the refusal is raised with the text in a differently-cased/renamed member (e.g. `Message`) and the JIT error serializer never maps it onto the `message` key this protocol's error envelope uses, or the gate raises a bodyless error. The replay cannot see which, only that no `message` key is present.
Why: a client has no way to learn WHY the call was refused. The AWS CLI and every SDK surface the message verbatim, so an operator sees an empty-message 400 instead of the one sentence that tells them the feature must be switched on for the account; code that branches on the message (distinguishing the account-level feature gate from a shape rejection or a missing target) falls through to its unknown-error path, and retry logic can treat an unenabled-feature refusal as transient.
Fix: the StartAccessRequest handler's refusal path and the SSM JIT error serializer — the AccessDeniedException/feature-gate error must be rendered with its text in the body's `message` member so the envelope carries both the discriminator and the sentence. This is the serializer/validation layer, not a change to the gate decision itself, which is already correct.
Traces: ssm-access-request-service-not-enabled-errors#task#1 (representative), and the same cause observed on ssm-jit-access-request-invalid-target-error#task#1, ssm-jit-access-request-service-not-enabled-error#task#1 and ssm-jit-access-request-workflow#task#1 — the whole cluster this binding expands to.

Cause 2 of 2: GetAccessToken's account-gate refusal omits the `message` member.
AWS: Smithy: GetAccessToken.AccessRequestId -> AccessRequestId, required, @pattern `^(oi)-[0-9a-f]{12}$`; output Credentials + AccessRequestStatus; modeled client errors AccessDeniedException (400, required member Message), ValidationException (400), ResourceNotFoundException (400), ThrottlingException (400). Recorded behavior only for the wording: for a well-formed access-request id in an account where Just-In-Time Node Access is not enabled, AWS answers HTTP 400 with `message` = `Just-In-Time Node Access service is not enabled in this account.` — the identical FIXED sentence as Cause 1, with no variable part, and in particular NOT a per-request not-found message naming the id. AWS reaches this account-level gate before it looks the request id up, which is why a never-issued id yields the gate sentence rather than a ResourceNotFoundException.
Emulator: HTTP 400 with the right class but `$.message: missing (expected 'Just-In-Time Node Access service is not enabled in this account.', got None)` — again no text member in the body.
Why: the token-retrieval half of the JIT flow is the part a client polls after requesting access; with no message a caller cannot distinguish "the feature is off for this account" from "this request id is unknown" or "the id is malformed", which are three different remediations in AWS. Any assertion or log-normalization keyed on AWS's sentence sees an empty string.
Fix: the GetAccessToken handler's refusal path plus the same shared SSM JIT error serializer named in Cause 1 — the two operations share the envelope, so the `message` mapping must be fixed once and cover both.
Traces: ssm-access-request-service-not-enabled-errors#task#2 (representative), also observed on ssm-access-token-lookup-not-found#task#1 and ssm-jit-access-token-nonexistent-request-error#task#1.

Relation to open issues: the open behavioral issue 20261109T034142Z-ssm-just-in-time-node-access-refusal-messages.md covers two DIFFERENT JIT message defects (StartAccessRequest's AccessDenied naming `resource: *`, and GetAccessToken's pattern ValidationException echoing the submitted value) and in fact asserts that the account-gate sentence is served correctly. It carries no wire replay key for these fixtures and does not describe the `message` member being absent from the gate refusal, so these two causes must be re-replayed here.

## Evidence
Cause 1 — fixture ssm-access-request-service-not-enabled-errors, step task#1, StartAccessRequest [verdict mismatch, status 400 -> 400]:
  expected $.message: 'Just-In-Time Node Access service is not enabled in this account.'
  actual   $.message: <member absent from the response body>
  status and error discriminator match (400 -> 400, no $.__type divergence reported); the whole divergence is the missing text member.
  Cluster members (affected_count 4): ssm-access-request-service-not-enabled-errors#task#1, ssm-jit-access-request-invalid-target-error#task#1, ssm-jit-access-request-service-not-enabled-error#task#1, ssm-jit-access-request-workflow#task#1 — the gate sentence is the same fixed text in all four, including the one whose target is invalid.

Cause 2 — fixture ssm-access-request-service-not-enabled-errors, step task#2, GetAccessToken [verdict mismatch, status 400 -> 400]:
  expected $.message: 'Just-In-Time Node Access service is not enabled in this account.'
  actual   $.message: <member absent from the response body>
  Cluster members (affected_count 3): ssm-access-request-service-not-enabled-errors#task#2, ssm-access-token-lookup-not-found#task#1, ssm-jit-access-token-nonexistent-request-error#task#1 — note the never-issued-id fixture also expects the gate sentence, not a not-found message.

The sentence `Just-In-Time Node Access service is not enabled in this account.` is fixed text in its entirety; there are no request- or state-derived variable parts to fill in, and no per-run value (request id, timestamp, account id) appears in it.

Model citations (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): StartAccessRequest input Reason(required, String1to256 @length 1..256) + Targets(required, @length 0..5) + Tags, output AccessRequestId(@pattern ^(oi)-[0-9a-f]{12}$), errors AccessDeniedException/InternalServerError/ResourceNotFoundException/ServiceQuotaExceededException/ThrottlingException/ValidationException; GetAccessToken input AccessRequestId(required, @pattern ^(oi)-[0-9a-f]{12}$), output Credentials+AccessRequestStatus, errors AccessDeniedException/InternalServerError/ResourceNotFoundException/ThrottlingException/ValidationException; AccessDeniedException = client error, httpError 400 (defaulted), single required member Message.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1 — fixture ssm-access-request-service-not-enabled-errors, step task#1: call ssm.StartAccessRequest(Reason="<any 1..256 chars>", Targets=[{"Key": "InstanceIds", "Values": ["<any instance id>"]}]) against an account where Just-In-Time Node Access has not been enabled. Observe HTTP 400 with the right error class but no `message` member in the JSON body; AWS returns `message` = 'Just-In-Time Node Access service is not enabled in this account.' Repeat with a malformed/invalid target (fixture ssm-jit-access-request-invalid-target-error) — AWS still returns the same fixed gate sentence.
3. Cause 2 — same fixture, step task#2: call ssm.GetAccessToken(AccessRequestId="oi-<12 hex chars>") with a well-formed id that was never issued. Observe HTTP 400 with no `message` member; AWS returns the identical fixed gate sentence rather than a not-found message naming the id.
4. In both cases inspect the raw response body: the error discriminator is present and correct, the lower-cased `message` key is absent.
