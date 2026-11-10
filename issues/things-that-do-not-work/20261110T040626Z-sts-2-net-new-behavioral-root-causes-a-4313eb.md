Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.get-caller-identity-rejects-tampered-session-token sts.get-caller-identity-unsigned-request-rejected
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.get-caller-identity-rejects-tampered-session-token|run#14|AssertionError|assert code == "InvalidClientTokenId", (
Issue-Key: sts.get-caller-identity-unsigned-request-rejected|run#11|AssertionError|assert code == "MissingAuthenticationToken", (
Issue-Commit: 7eda33408fad28e5ce4a9dbc5d19eef9d00fdd67

# sts: 2 net-new behavioral root causes — a tampered session token is refused SignatureDoesNotMatch instead of InvalidClientTokenId, and an entirely unsigned GetCallerIdentity is refused InvalidClientTokenId instead of MissingAuthenticationToken

> _Note: this issue was identified against the code base at commit `7eda33408fad28e5ce4a9dbc5d19eef9d00fdd67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCallerIdentity

## Description
Two net-new root causes, both in the emulator's SigV4 authentication front door as it serves the STS endpoint. They are numbered so a fixer can close them one at a time; BOTH must be closed before submitting.

Related but NOT covering: open issue 20261029T182307Z-smoke-an-unsigned-request-to-the-sts-endpoint.md describes the unsigned-request path as reaching a 404 UnknownService because the router preferred X-Amz-Target; and 20261109T220008Z-smoke-query-string-presigned-sigv4-requests-are-refused.md describes presigned query-string SigV4 being refused AccessDenied. Neither carries the two replay keys below, and the failures here are a different observable outcome: the request IS now refused 403 by the auth layer, with the WRONG error code and message for the condition. Cause 2 below is the surviving classification half of that unsigned-request path.

=== Cause 1 of 2: a genuinely issued session token with one character flipped is refused SignatureDoesNotMatch, not InvalidClientTokenId ===
AWS: recorded behavior only — GetCallerIdentity is modelled with NO errors (Smithy com.amazonaws.sts#AWSSecurityTokenServiceV20110615, GetCallerIdentity errors: []), so these are framework SigV4 auth errors, not operation errors. AWS separates the two auth-failure classes: the credential/session token not being one the service issued (unknown, tampered, or belonging to no live session) is HTTP 403 with awsQuery `<Code>InvalidClientTokenId</Code>` and the message `The security token included in the request is invalid` — entirely fixed text, no variable parts. `SignatureDoesNotMatch` is reserved for the different condition where the credential IS recognized but the computed signature over the canonical request does not match (wrong secret key, wrong canonicalization). Reference: https://docs.aws.amazon.com/IAM/latest/UserGuide/create-signed-request.html
Emulator: the same call, signed with a session whose SessionToken has exactly one character replaced (AccessKeyId and SecretAccessKey untouched, taken from a live GetSessionToken response), is refused with error code `SignatureDoesNotMatch`. The untampered session authenticates fine immediately before and after, so the only input that changed is the session token. The emulator therefore classifies an unrecognizable session token as a signature mismatch: it is feeding the session token into the signature comparison instead of resolving it to a session first and refusing an unresolvable one as an invalid security token.
Also part of this cause: the message that accompanies InvalidClientTokenId must be exactly `The security token included in the request is invalid`, with no trailing period. The emulator's existing InvalidClientTokenId body (visible in cause 2) reads `The security token included in the request is invalid.` with a trailing period, so after the code is corrected the message will still diverge by that character.
Why: SDK credential providers key off the error code. `InvalidClientTokenId` (with `ExpiredToken`) is the class that tells a client its temporary credentials are no longer usable and must be re-fetched; `SignatureDoesNotMatch` tells it the request was signed wrongly, which botocore/other SDKs treat as a signing/clock problem and do NOT refresh credentials for. A client whose session token is corrupted in transit or in a cache will therefore loop on a signing error against the emulator where against AWS it would re-mint credentials and recover. It also makes the whole "invalid security token" negative path untestable against the emulator.
Fix: the shared SigV4 auth middleware in front of the STS service — the credential-resolution step must resolve the request's session token to an issued session BEFORE comparing signatures, and refuse an unresolvable/unknown session token 403 InvalidClientTokenId with the fixed message above; only a recognized credential whose signature fails may answer SignatureDoesNotMatch. The awsQuery ErrorResponse serializer for that refusal must emit the message without a trailing period.
Traces: sts.get-caller-identity-rejects-tampered-session-token

=== Cause 2 of 2: a request carrying NO authentication at all is refused InvalidClientTokenId instead of MissingAuthenticationToken ===
AWS: recorded behavior only (framework auth error; GetCallerIdentity models no errors). A POST to the STS endpoint with Content-Type application/x-www-form-urlencoded, body `Action=GetCallerIdentity&Version=2011-06-15`, no Authorization header and no SigV4 query credentials is refused HTTP 403 with awsQuery `<Code>MissingAuthenticationToken</Code>` and the message `Request is missing Authentication Token` — entirely fixed text, no variable parts. The ABSENCE of a credential is a distinct class from a PRESENT-but-invalid one: AWS reserves MissingAuthenticationToken for the former and InvalidClientTokenId for the latter.
Emulator: the unsigned request is refused with the right status (403) but the wrong class — body `<?xml version="1.0"?><ErrorResponse><Error><Type>Sender</Type><Code>InvalidClientTokenId</Code><Message>The security token included in the request is invalid.</Message></Error><RequestId>...</RequestId></ErrorResponse>`. Fixed text here is `The security token included in the request is invalid.`; the only variable part of the body is the per-run RequestId, which is not part of the requirement. So the auth layer collapses "no credential presented" into its invalid-token verdict rather than reporting the missing-credential class. (Good news for the fixer: the request is no longer answered 404 UnknownService and no identity is disclosed in the body, so the routing half of the earlier smoke issue no longer reproduces here — only the classification is wrong.)
Why: a client with an EMPTY credential chain (no profile, no env vars, no instance role) gets, from AWS, MissingAuthenticationToken — the one code that says "you sent no credentials". Against the emulator it gets InvalidClientTokenId, which says "your credentials were rejected", sending the operator to debug key validity and session expiry instead of credential discovery. SDK and gateway error handling that branches on MissingAuthenticationToken (e.g. to prompt for configuration rather than retry) never fires.
Fix: the shared SigV4 auth middleware's presence check must run before credential validation — when no Authorization header and no SigV4 query credential are present on an operation whose auth scheme is sigv4 and which is not @optionalAuth, refuse 403 with Code MissingAuthenticationToken and message `Request is missing Authentication Token`, serialized in the target service's protocol error shape (an awsQuery ErrorResponse document for STS).
Traces: sts.get-caller-identity-unsigned-request-rejected

## Evidence
Cause 1 — sts.get-caller-identity-rejects-tampered-session-token, [mismatch], at check_sts_get_caller_identity_rejects_tampered_session_token.py line 64:
  assert code == "InvalidClientTokenId", (...)
  AssertionError: expected InvalidClientTokenId for a tampered session token, got 'SignatureDoesNotMatch'
Sequence: GetSessionToken(DurationSeconds=900) -> a client signed with those three credentials calls GetCallerIdentity successfully (positive control) -> the SAME credentials with one character of SessionToken replaced call GetCallerIdentity.
  Expected: HTTP 403, Code `InvalidClientTokenId`, Message `The security token included in the request is invalid` (fixed text; no variable parts).
  Actual:   Code `SignatureDoesNotMatch`.

Cause 2 — sts.get-caller-identity-unsigned-request-rejected, [mismatch], at check_sts_get_caller_identity_unsigned_request_rejected.py line 51:
  assert code == "MissingAuthenticationToken", (...)
  AssertionError: wrong error code for an unsigned request: expected 'MissingAuthenticationToken', got 'InvalidClientTokenId' (body '<?xml version="1.0"?><ErrorResponse><Error><Type>Sender</Type><Code>InvalidClientTokenId</Code><Message>The security token included in the request is invalid.</Message></Error><RequestId>5a0bd86c-2e2b')
Request: raw POST to the STS endpoint, Content-Type: application/x-www-form-urlencoded, body `Action=GetCallerIdentity&Version=2011-06-15`, no Authorization header, no X-Amz-Target.
  Expected: HTTP 403, Code `MissingAuthenticationToken`, Message `Request is missing Authentication Token` (fixed text; no variable parts).
  Actual:   HTTP 403 (correct), Code `InvalidClientTokenId`, Message `The security token included in the request is invalid.`
The endpoint host/port and the RequestId are per-environment/per-run and are not part of either requirement. Both checks re-issue a normally signed GetCallerIdentity after the refusal and it still matches, so the endpoint stays healthy — the divergence is purely the error classification.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: call sts:GetSessionToken(DurationSeconds=900). With the returned AccessKeyId/SecretAccessKey/SessionToken, call GetCallerIdentity — it succeeds. Now flip a single character in the middle of SessionToken (leave the key id and secret untouched) and call GetCallerIdentity again. AWS refuses 403 InvalidClientTokenId with message `The security token included in the request is invalid`; the emulator refuses with Code `SignatureDoesNotMatch`. Replay sts.get-caller-identity-rejects-tampered-session-token to confirm.
3. Cause 2: POST to the STS endpoint with Content-Type: application/x-www-form-urlencoded and body `Action=GetCallerIdentity&Version=2011-06-15`, sending NO Authorization header and NO X-Amz-Target header. AWS answers 403 with Code `MissingAuthenticationToken` and message `Request is missing Authentication Token`; the emulator answers 403 with Code `InvalidClientTokenId` and message `The security token included in the request is invalid.` Replay sts.get-caller-identity-unsigned-request-rejected to confirm.
4. In both cases re-issue an ordinary signed GetCallerIdentity afterwards to confirm the endpoint is unaffected and the divergence is only in the refusal's code/message.
