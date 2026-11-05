Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-create-validation-project-family
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-create-validation-project-family datazone-domain-lifecycle
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|CreateProject|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|CreateProject|4xx->4xx|status
Issue-Key: datazone|DeleteDomain|4xx->4xx|$
Issue-Related: 20261029T172524Z-datazone-error-messages-diverge-from-aws-wording-in.md
Issue-Commit: 502ffb7d9f8a8be7c4aed6e0f1219303be44c32f

# datazone error responses diverge: missing domain answered as AccessDenied and non-restJson1 error body

> _Note: this issue was identified against the code base at commit `502ffb7d9f8a8be7c4aed6e0f1219303be44c32f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProject

## Description
TWO causes, both in how datazone renders a failing response: the operation handler picks the wrong error class for a domain that does not exist, and the error serializer emits a body that is not the restJson1 error body datazone sends. Fix both before submitting.

Cause 1 of 2: CreateProject against a syntactically valid but NONEXISTENT domain is answered as an authorization failure instead of a not-found failure.
AWS: Smithy CreateProject (POST /v2/domains/{domainIdentifier}/projects, success code 201); domainIdentifier -> DomainId, @required, @httpLabel, @pattern "^dzd[-_][a-zA-Z0-9_-]{1,36}$"; modelled errors include ResourceNotFoundException (httpError 404, doc "The specified resource cannot be found.", message @required) and AccessDeniedException (httpError 403, doc "You do not have sufficient access to perform this action.", message @required). For a permitted caller naming a domain identifier that satisfies the pattern but names no existing domain, the recorded response is 404 with x-amzn-errortype ResourceNotFoundException and message `Domain <domainIdentifier> not found` — fixed text "Domain " + " not found", the only variable part being the domain identifier taken verbatim from the request path (the recording's dzd_bogus00000000001 is just this run's value, not part of the template).
Emulator: 403 with x-amzn-errortype AccessDeniedException and message `User is not permitted to perform operation: CreateProject`, i.e. it reports a permission denial for a request whose only problem is that the named domain does not exist.
Why: 403/AccessDeniedException and 404/ResourceNotFoundException are different branches in every SDK and in every client's error handling. A client that creates a project after a domain was deleted (or that mistypes a domain id) is told it lacks permission, so it escalates to an IAM/role investigation instead of creating or re-resolving the domain; retry/clean-up logic keyed on ResourceNotFoundException never fires, and the message no longer identifies WHICH domain was missing.
Hypothesis: the domain-existence check has been folded into the authorization pre-check, so every unresolvable domain now exits as AccessDenied. Note this is the mirror image of open issue 20261029T172524Z-datazone-domain-scoped-operations-answer-400-validationexception-instead.md (pattern-valid-but-DENIED domains must be 403, not 400): a domain that is absent for a permitted caller must stay 404. The older open issue 20261029T172524Z-datazone-error-messages-diverge-from-aws-wording-in.md records this same step as previously answering 404 with the constant message 'Domain not found'; it covers only the message wording and does not cover the status/error-class divergence reported here.
Fix: CreateProject's domain-resolution path in the datazone operation handler — resolve the domain first and raise ResourceNotFoundException with the request's domainIdentifier formatted into the message; reserve AccessDeniedException for the authorization decision.
Traces: datazone-create-validation-project-family#task#4.

Cause 2 of 2: the datazone error body carries an awsJson-style `__type` discriminator and an EMPTY required message.
AWS: datazone is restJson1 (service DataZone, version 2018-05-10, protocols [restJson1]); the error shape is conveyed in the `x-amzn-errortype` header, and AccessDeniedException's `message` member is @required. Recorded behavior only for the body: the recorded 403 DeleteDomain response (DELETE /v2/domains/{identifier}, AccessDeniedException httpError 403) carries a body with NO `__type` member and no empty-string message member; the error class travels in the header.
Emulator: same status 403, but the body is `{"__type": "AccessDeniedException", "message": ""}` — it injects a `__type` key, which belongs to the AWS JSON 1.0/1.1 protocols and not to restJson1, and serializes the @required `message` as the empty string instead of the denial text datazone uses for this error class ("User is not permitted to perform operation: <OperationName>", fixed text plus the operation name).
Why: a client parsing the error body sees an unmodelled `__type` member, and every message-bearing path (SDK exception .message, user-facing surfacing, log triage) gets an empty string for a @required member, so the reason for the denial is unavailable to the caller. The stray discriminator also makes the body differ structurally from every other restJson1 service in the emulator.
Fix: the datazone restJson1 error serializer — drop the `__type` body member for this protocol and never serialize a @required message as empty; populate it from the error that was raised.
Traces: datazone-domain-lifecycle#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, 107 steps, 28 passed), expected -> actual:

Cause 1 — datazone-create-validation-project-family#task#4, CreateProject: status expected 404, got 403; $.message expected 'Domain dzd_bogus00000000001 not found', got 'User is not permitted to perform operation: CreateProject'; $headers.x-amzn-errortype expected 'ResourceNotFoundException', got 'AccessDeniedException'. The supplied domainIdentifier matches the modelled pattern ^dzd[-_][a-zA-Z0-9_-]{1,36}$, so no modelled input constraint was violated and the only fault is that the domain does not exist.

Cause 2 — datazone-domain-lifecycle#task#5, DeleteDomain: status 403 expected and returned (match); body $ expected {} , got {'__type': 'AccessDeniedException', 'message': ''}. The expected body carries neither a `__type` member nor an empty message member; x-amzn-errortype matched.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: fixture datazone-create-validation-project-family, step task#4 — CreateProject (POST /v2/domains/dzd_<nonexistent>/projects) as a permitted caller against a pattern-valid domain identifier that names no existing domain. Expect HTTP 404, header x-amzn-errortype: ResourceNotFoundException, body message 'Domain <domainIdentifier> not found'; observe HTTP 403, x-amzn-errortype: AccessDeniedException, message 'User is not permitted to perform operation: CreateProject'.
3. Cause 2: fixture datazone-domain-lifecycle, step task#5 — DeleteDomain (DELETE /v2/domains/{identifier}) on the denied path. Expect the recorded restJson1 403 error body with no `__type` member and no empty message member; observe {"__type": "AccessDeniedException", "message": ""}.
4. Compare status, x-amzn-errortype and the full error body for each step against the recorded response.
