Issue-Source: trace-sweep-divergence
Issue-Trace: sts-assume-role-session-name-length-bounds
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sts-assume-role-session-name-length-bounds sts-assume-role-unauthorized-role sts-assume-role-with-web-identity-invalid-token sts-delegated-access-token-exchange sts-delegated-access-token-invalid-trade-in sts-federation-token-name-length-bounds sts-decode-authorization-message-invalid sts-get-federation-token-malformed-policy
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts|AssumeRole|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sts|AssumeRoleWithWebIdentity|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sts|GetDelegatedAccessToken|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sts|GetFederationToken|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sts|DecodeAuthorizationMessage|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sts|DecodeAuthorizationMessage|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sts|GetFederationToken|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 0ebbd81840eb1d8236e49c909db275b2251d47fa

# STS awsQuery error envelopes omit RequestId and carry emulator-authored messages

> _Note: this issue was identified against the code base at commit `0ebbd81840eb1d8236e49c909db275b2251d47fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
The STS error path diverges from AWS on every refusal replayed in this corpus. The shared fault is the awsQuery error serializer: the <ErrorResponse> envelope it writes has an <Error> child but no <RequestId> sibling, so no refusal is traceable. Two refusals additionally carry a message the emulator authored rather than the one AWS words for that error class. 7 causes, all on service sts.

Cause 1 of 7: AssumeRole refusals omit ErrorResponse.RequestId.
AWS: Smithy: STS is protocol awsQuery (sdkId STS, api version 2011-06-15). Every awsQuery refusal is serialized as <ErrorResponse><Error><Type>Sender</Type><Code>...</Code><Message>...</Message></Error><RequestId>...</RequestId></ErrorResponse>; RequestId is always present and is a per-request value (a UUID the service mints), never a constant.
Emulator: status 400 and the Error child match, but $.ErrorResponse.RequestId is missing entirely (expected a shaped value, got None).
Why: a client or support workflow that logs the RequestId of a refusal, and any SDK that surfaces it as the request id of the failed call, gets nothing to correlate against; retry/diagnostic tooling that requires the field sees a malformed envelope.
Fix: the sts crate's awsQuery error serializer — the component that renders ErrorResponse must emit the RequestId element for every refusal, taking the value from the per-request id already assigned to the invocation.
Traces: sts-assume-role-session-name-length-bounds#task#2 (also task#4, task#5).

Cause 2 of 7: AssumeRole authorization refusal (403) omits ErrorResponse.RequestId.
AWS: same envelope rule as Cause 1; it applies to the 403 refusal class as well as 400 (Smithy: STS error shapes carry awsQueryError httpResponseCode 400/403, e.g. RegionDisabledException -> 403).
Emulator: status 403 is right, Error child is right, $.ErrorResponse.RequestId missing.
Why: as Cause 1, on the deny path where correlation matters most.
Fix: same serializer as Cause 1 — the fix must cover non-400 refusal statuses, not only 400.
Traces: sts-assume-role-unauthorized-role#task#2.

Cause 3 of 7: AssumeRoleWithWebIdentity invalid-token refusal omits ErrorResponse.RequestId.
AWS: Smithy: AssumeRoleWithWebIdentity errors include InvalidIdentityTokenException (client, httpError 400). The refusal envelope carries RequestId as in Cause 1.
Emulator: status 400 and Error match; $.ErrorResponse.RequestId missing (the recording shows a per-run UUID there; the requirement is that the element exists and carries this request's id, not that recorded value).
Fix: same serializer; it must be reached from the anonymous/optionalAuth operations too.
Traces: sts-assume-role-with-web-identity-invalid-token#task#1.

Cause 4 of 7: GetDelegatedAccessToken refusals omit ErrorResponse.RequestId.
AWS: Smithy: GetDelegatedAccessToken errors ExpiredTradeInTokenException (400), PackedPolicyTooLarge (400), RegionDisabledException (403); same envelope rule.
Emulator: status 400 and Error match; $.ErrorResponse.RequestId missing on the exchange refusal and on the invalid trade-in refusal.
Fix: same serializer.
Traces: sts-delegated-access-token-exchange#task#1 (also task#2) and sts-delegated-access-token-invalid-trade-in#task#1.

Cause 5 of 7: GetFederationToken bounds refusals omit ErrorResponse.RequestId.
AWS: Smithy: GetFederationToken errors MalformedPolicyDocument (400), PackedPolicyTooLarge (400), RegionDisabledException (403); same envelope rule. Name bounds come from GetFederationToken.Name (@length 2-32, pattern ^[\w+=,.@-]*$).
Emulator: status 400 and Error match; $.ErrorResponse.RequestId missing.
Fix: same serializer.
Traces: sts-federation-token-name-length-bounds#task#2 (also task#4).

Cause 6 of 7: DecodeAuthorizationMessage validates the EncodedMessage WITHOUT base64-decoding it, so the refusal message lists the wrong bytes (and omits RequestId).
AWS: Smithy: DecodeAuthorizationMessage.EncodedMessage is required, @length 1-10240, and the only modeled error is InvalidAuthorizationMessageException (client, awsQueryError code InvalidAuthorizationMessageException, httpResponseCode 400). AWS base64-decodes the encoded message first and, when the decoded payload is not a valid authorization message, words the refusal 'Message is not valid: ' followed by the signed-byte listing of the DECODED bytes. Fixed text: 'Message is not valid: '; variable part: the byte listing derived from base64-decoding the request's EncodedMessage.
Emulator: status 400 and code are right, but the message lists the signed bytes of the request's raw ASCII characters instead of its base64-decoded payload — in the replay, the recorded message listed 34 decoded bytes while the emulator listed 46 bytes that spell the literal input string character-for-character. $.ErrorResponse.RequestId is also missing.
Why: a caller that inspects or decodes the reported bytes to understand why its authorization message was rejected is shown its own undecoded input, so the diagnostic is useless; the emulator also accepts/rejects on the wrong input, meaning a message that is valid base64 of an invalid payload and one that is not base64 at all are not distinguished.
Fix: the DecodeAuthorizationMessage handler's validation — base64-decode EncodedMessage before validating, and build the message from the decoded bytes; plus the shared error serializer for RequestId.
Traces: sts-decode-authorization-message-invalid#task#1.

Cause 7 of 7: GetFederationToken's malformed-policy refusal leaks the JSON parser's exception text instead of the MalformedPolicyDocument message (and omits RequestId).
AWS: Smithy: GetFederationToken error MalformedPolicyDocumentException (client, awsQueryError code MalformedPolicyDocument, httpResponseCode 400), documented as 'the policy document was malformed'. For a syntactically invalid session policy AWS answers 400 MalformedPolicyDocument with the fixed message 'Syntax errors in policy.' — the whole message is fixed text, with no part taken from the request.
Emulator: status 400, but the message is the internal deserializer's diagnostic: 'Unexpected IOException: Unexpected end-of-input: expected close marker for Array\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 40]'. $.ErrorResponse.RequestId is also missing.
Why: the message is not AWS's wording, names an internal library and a source offset that no AWS message contains, and a client matching on the documented text (or showing it to a user) breaks; it also leaks implementation internals across the wire.
Fix: the GetFederationToken handler's policy validation — catch the parse failure and raise MalformedPolicyDocument with AWS's fixed message, instead of propagating the parser's exception string into Error.Message; plus the shared error serializer for RequestId.
Traces: sts-get-federation-token-malformed-policy#task#1.

Related but NOT covering this: the open issues 'IAM error responses omit the ResponseMetadata RequestId element' (20261029T184359Z-iam-...) and 'SES v1 Query error envelope omits the RequestId element on every refusal' (20261029T180607Z-email-...) report the same class of defect in other crates' serializers; neither carries an sts replay key, so the sts serializer is still unfixed.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false).

Cause 1 — sts-assume-role-session-name-length-bounds#task#2, AssumeRole, expected 400 / actual 400: '$.ErrorResponse.RequestId: missing (expected <shaped-not-asserted>, got None)'; same cluster covers task#4 and task#5.
Cause 2 — sts-assume-role-unauthorized-role#task#2, AssumeRole, expected 403 / actual 403: '$.ErrorResponse.RequestId: missing'.
Cause 3 — sts-assume-role-with-web-identity-invalid-token#task#1, AssumeRoleWithWebIdentity, expected 400 / actual 400: '$.ErrorResponse.RequestId: missing (expected a per-run UUID, got None)'.
Cause 4 — sts-delegated-access-token-exchange#task#1, GetDelegatedAccessToken, expected 400 / actual 400: '$.ErrorResponse.RequestId: missing'; cluster also covers sts-delegated-access-token-exchange#task#2 and sts-delegated-access-token-invalid-trade-in#task#1.
Cause 5 — sts-federation-token-name-length-bounds#task#2, GetFederationToken, expected 400 / actual 400: '$.ErrorResponse.RequestId: missing'; cluster also covers task#4.
Cause 6 — sts-decode-authorization-message-invalid#task#1, DecodeAuthorizationMessage, expected 400 / actual 400: '$.ErrorResponse.Error.Message: value_mismatch (expected 'Message is not valid: [-78, 41, -74, -83, ...]' — 34 decoded bytes — got 'Message is not valid: [115, 105, 109, 50, ...]', the ASCII bytes of the literal input string)' and '$.ErrorResponse.RequestId: missing'.
Cause 7 — sts-get-federation-token-malformed-policy#task#1, GetFederationToken, expected 400 / actual 400: '$.ErrorResponse.Error.Message: value_mismatch (expected 'Syntax errors in policy.', got 'Unexpected IOException: Unexpected end-of-input: expected close marker for Array\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 40]')' and '$.ErrorResponse.RequestId: missing'.

In every cause the HTTP status and the Error Type/Code matched; only the envelope's RequestId element and (causes 6-7) the message text diverged.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the sts divergence clusters.
3. Cause 1: replay sts-assume-role-session-name-length-bounds step task#2 — POST the awsQuery AssumeRole form with a RoleSessionName outside the modeled length bounds; the response is 400 with <ErrorResponse><Error>...</Error></ErrorResponse> and no <RequestId> child.
4. Cause 2: replay sts-assume-role-unauthorized-role step task#2 (AssumeRole against a role whose trust policy denies the caller) — 403, no RequestId element.
5. Cause 3: replay sts-assume-role-with-web-identity-invalid-token step task#1 — 400 InvalidIdentityToken, no RequestId element.
6. Cause 4: replay sts-delegated-access-token-exchange steps task#1/task#2 and sts-delegated-access-token-invalid-trade-in step task#1 — 400, no RequestId element.
7. Cause 5: replay sts-federation-token-name-length-bounds steps task#2/task#4 — 400, no RequestId element.
8. Cause 6: replay sts-decode-authorization-message-invalid step task#1 — call DecodeAuthorizationMessage with an EncodedMessage that is valid base64 of a non-authorization payload; compare Error.Message's byte listing against the base64-DECODED bytes of the request (emulator lists the raw input characters) and note the missing RequestId.
9. Cause 7: replay sts-get-federation-token-malformed-policy step task#1 — call GetFederationToken with a Policy that is truncated JSON; expect 400 MalformedPolicyDocument 'Syntax errors in policy.', observe the Jackson IOException text and the missing RequestId.
10. All nine steps must pass before this issue is closed.
