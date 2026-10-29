Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: _smoke.sigv4-unsigned-request-rejected
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: _smoke.sigv4-unsigned-request-rejected|run#7|AssertionError|assert rejected, (
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# _smoke: an UNSIGNED request to the STS endpoint is not rejected — it is answered 404 UnknownService, because the router resolves the service from X-Amz-Target instead of the awsQuery Action and no SigV4 enforcement runs

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCallerIdentity

## Description
NOTE ON ATTRIBUTION: this finding's check id is `_smoke.sigv4-unsigned-request-rejected`, whose leading token `_smoke` is a harness grouping and not an AWS service, so the per-service bootstrap gate could not place it. It is filed rather than deferred. The endpoint it exercises is the STS one (`ctx.client("sts").meta.endpoint_url`) and the two causes below live in the emulator's shared front door (request router and auth middleware), not in any single service crate.

Two net-new root causes, both observable in the single response this check captures. They are numbered so a fixer can close them one at a time; both must be closed before submitting.

=== Cause 1 of 2: a POST carrying an X-Amz-Target header is routed by that header even for an awsQuery service, so the STS endpoint answers 404 UnknownService ===
AWS: Smithy (com.amazonaws.sts#AWSSecurityTokenServiceV20110615) is an awsQuery service: the operation is selected from the form-encoded `Action` and `Version` parameters in the request body (`Action=GetCallerIdentity&Version=2011-06-15`), and GetCallerIdentity is bound on it. X-Amz-Target is an awsJson concept and carries no routing meaning for this service; AWS ignores it and dispatches on Action, so such a request reaches GetCallerIdentity and is answered by it (or refused by auth — see cause 2). AWS never answers 404 for a correctly addressed STS endpoint.
Emulator: the request is answered HTTP 404 with body `{"__type":"UnknownService","message":"no emulated service is registered for 'awssecuritytokenservicev20110615'"}`. The fixed text is `no emulated service is registered for '...'`; the variable part is the service token the router derived, here the X-Amz-Target prefix `AWSSecurityTokenServiceV20110615` lower-cased. So the router preferred X-Amz-Target over the body's Action, mapped that prefix to a service token nothing is registered under, and gave up — note the SAME endpoint serves the other 138 sts checks, so routing works whenever X-Amz-Target is absent. Both the status (404 vs the operation's own) and the error code (`UnknownService`, which is not an AWS error code) are wrong.
Why: any client, proxy or SDK middleware that attaches X-Amz-Target to a query-protocol request — and anything replaying a captured request with that header present — gets 404 UnknownService from a service that is in fact emulated. 404/UnknownService is also unretryable and uninterpretable: a client cannot tell a misrouted request from a missing service.
Fix: the shared request router's service-resolution step. For a service whose protocol is awsQuery/ec2Query the operation must be resolved from the body's `Action`/`Version`, and X-Amz-Target must not be consulted (or must be ignored when it does not resolve); the fallback for an unresolved target on an endpoint that IS registered must not be a 404 UnknownService.
Traces: _smoke.sigv4-unsigned-request-rejected

=== Cause 2 of 2: no SigV4 enforcement — a request with NO Authorization header is not rejected ===
AWS: GetCallerIdentity is a SigV4-signed operation (the STS service's authSchemes are sigv4; no operation-level @optionalAuth on it). A request to the STS endpoint with no Authorization header and no signature is refused before any handler runs, with HTTP 403 and, in the awsQuery error document, `<Code>MissingAuthenticationToken</Code>` and the message `Request is missing Authentication Token` — entirely fixed text with no variable parts. The check accepts any of the AWS auth-rejection classes (status 401/403, or a body naming MissingAuthenticationToken / IncompleteSignature / InvalidClientTokenId / SignatureDoesNotMatch / AccessDenied), so only the complete ABSENCE of a rejection fails it.
Emulator: the unsigned request is not rejected at all. It is answered 404 with the UnknownService body of cause 1 — a routing outcome, reached without any authentication check having been applied. Fixing cause 1 alone would therefore turn this into a 200 from GetCallerIdentity, which is the over-permissive outcome the check is named for; the two causes must both be closed.
Why: an emulator that serves unsigned requests cannot surface a client whose credential chain is empty or whose signing is broken — those bugs appear only in production, where every call is refused 403. It also means no negative-path test for missing or malformed credentials can be written against the emulator, and the whole auth-rejection class is unobservable.
Fix: the shared auth middleware in front of the router — a request for an operation whose auth scheme is sigv4 and which is not @optionalAuth must be refused 403 MissingAuthenticationToken when no Authorization header (and no SigV4 query credential) is present, in the error shape of the target service's protocol (an awsQuery ErrorResponse document for STS). This check is marked scope-contested by the harness (an emulator may defer full signature VALIDATION), but the presence check is the part it measures: a request carrying no credential at all must not be served.
Traces: _smoke.sigv4-unsigned-request-rejected

## Evidence
_smoke.sigv4-unsigned-request-rejected, [mismatch], at check line 46:
  assert rejected, (f"unsigned request was NOT rejected (status={status}) — over-permissive auth (SigV4 enforcement). body={text[:150]}")
  AssertionError: unsigned request was NOT rejected (status=404) — over-permissive auth (SigV4 enforcement). body={"__type":"UnknownService","message":"no emulated service is registered for 'awssecuritytokenservicev20110615'"}

The request the check issues (raw urllib, no signing):
  POST <sts endpoint>/ 
  Content-Type: application/x-www-form-urlencoded
  X-Amz-Target: AWSSecurityTokenServiceV20110615.GetCallerIdentity
  body: Action=GetCallerIdentity&Version=2011-06-15
  (no Authorization header)

Expected: HTTP 403 with awsQuery error Code `MissingAuthenticationToken`, message `Request is missing Authentication Token` (the check also accepts 401 or any of SignatureDoesNotMatch / IncompleteSignature / InvalidClientTokenId / InvalidSignature / AccessDenied).
Actual: HTTP 404, body `{"__type":"UnknownService","message":"no emulated service is registered for 'awssecuritytokenservicev20110615'"}` — a JSON error document from the router, not an awsQuery ErrorResponse, with a code (`UnknownService`) that is not an AWS error code. The endpoint host/port is per-environment and is not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and replay _smoke.sigv4-unsigned-request-rejected.
2. Cause 1: issue a raw POST to the STS endpoint with Content-Type: application/x-www-form-urlencoded, body `Action=GetCallerIdentity&Version=2011-06-15`, and header `X-Amz-Target: AWSSecurityTokenServiceV20110615.GetCallerIdentity` — but WITH a valid SigV4 Authorization header. AWS ignores X-Amz-Target and answers GetCallerIdentity; the emulator answers 404 `{"__type":"UnknownService",...}`. Repeat without the X-Amz-Target header to confirm the same signed request is served, isolating the header as the cause.
3. Cause 2: issue the same raw POST with NO Authorization header and NO X-Amz-Target header, so routing cannot be blamed. AWS answers 403 with Code MissingAuthenticationToken; the emulator serves the operation.
