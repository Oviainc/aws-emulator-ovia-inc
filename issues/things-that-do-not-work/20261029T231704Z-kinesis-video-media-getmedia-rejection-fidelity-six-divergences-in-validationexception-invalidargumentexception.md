Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-media.get-media-continuation-token-selector-missing-token-rejected kinesis-video-media.get-media-fragment-number-missing-after-rejected kinesis-video-media.get-media-producer-timestamp-missing-start-rejected kinesis-video-media.get-media-server-timestamp-missing-start-rejected kinesis-video-media.get-media-after-fragment-number-non-numeric-rejected kinesis-video-media.get-media-malformed-stream-name-rejected kinesis-video-media.get-media-malformed-stream-arn-rejected kinesis-video-media.get-media-nonexistent-stream-rejected kinesis-video-media.get-media-continuation-token-invalid-rejected kinesis-video-media.get-media-unauthorized-role-denied-rejected
Issue-Shape-Id: com.amazonaws.kinesisvideomedia#AWSAcuityInletService
Issue-Key: kinesis-video-media.get-media-continuation-token-selector-missing-token-rejected|run#5|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-fragment-number-missing-after-rejected|run#5|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-producer-timestamp-missing-start-rejected|run#5|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-server-timestamp-missing-start-rejected|run#5|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-after-fragment-number-non-numeric-rejected|run#5|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-malformed-stream-name-rejected|run#5|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-malformed-stream-arn-rejected|run#5|AssertionError|assert message == (
Issue-Key: kinesis-video-media.get-media-nonexistent-stream-rejected|run#6|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-continuation-token-invalid-rejected|run#4|AssertionError|assert err["Code"] == "InvalidArgumentException", f"expected InvalidArgumentException, got {err!r}"
Issue-Key: kinesis-video-media.get-media-unauthorized-role-denied-rejected|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: 4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12

# GetMedia rejection fidelity: six divergences in ValidationException/InvalidArgumentException/ResourceNotFoundException messages, continuation-token validation and IAM deny enforcement

> _Note: this issue was identified against the code base at commit `4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMedia

## Description
This issue bundles SIX distinct net-new root causes in `kinesis-video-media:GetMedia`, grounded in the 10 failing behavioral checks that are NOT owned by the open issue `20261022T180000Z-kinesis-video-media-getmedia-cannot-see-a-stream-createstream-made.md`. That open issue covers only one cause (the split stream registry making a created stream invisible to GetMedia) and it does NOT cover any of the six below; where a cause is additionally masked by that registry split, this is said explicitly. A fixer must close ALL SIX before submitting.

Model used throughout: `services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/smithy-model/com-amazonaws-kinesisvideomedia-awsacuityinletservice.json`, operation `GetMedia` (POST /getMedia, success 200; errors ClientLimitExceededException 400, ConnectionLimitExceededException 400, InvalidArgumentException 400, InvalidEndpointException 400, NotAuthorizedException 401, ResourceNotFoundException 404).

=== Cause 1 of 6: InvalidArgumentException message for a StartSelector missing its companion member uses the emulator's own wording ===
Failing checks: kinesis-video-media.get-media-fragment-number-missing-after-rejected, kinesis-video-media.get-media-producer-timestamp-missing-start-rejected, kinesis-video-media.get-media-server-timestamp-missing-start-rejected, kinesis-video-media.get-media-continuation-token-selector-missing-token-rejected.
AWS: Smithy models `StartSelector$StartSelectorType` as required with values FRAGMENT_NUMBER|SERVER_TIMESTAMP|PRODUCER_TIMESTAMP|NOW|EARLIEST|CONTINUATION_TOKEN, and the companion members `AfterFragmentNumber` (FragmentNumberString), `StartTimestamp` (Timestamp) and `ContinuationToken` as optional, so the companion requirement is enforced in the handler, not by the schema; the model states only InvalidArgumentException 400 "The value for this input parameter is invalid". The message text is recorded behavior only: AWS answers HTTP 400 `InvalidArgumentException` whose message renders the service's own Java `StartSelector` toString. Fixed text per class, with the variable parts taken from the request's StartSelector: (a) FRAGMENT_NUMBER with no AfterFragmentNumber -> "FRAGMENT_NUMBER must be specified for startSelector : StartSelector(startSelectorType=FRAGMENT_NUMBER, afterFragmentNumber=null, startTimestamp=null, continuationToken=null)"; (b) PRODUCER_TIMESTAMP or SERVER_TIMESTAMP with no StartTimestamp -> "Start Timestamp must be specified for StartSelector : StartSelector(startSelectorType=<the selector type from the request>, afterFragmentNumber=null, startTimestamp=null, continuationToken=null)"; (c) CONTINUATION_TOKEN with no ContinuationToken -> "ContinuationToken must be specified for StartSelector : StartSelector(startSelectorType=CONTINUATION_TOKEN, afterFragmentNumber=null, startTimestamp=null, continuationToken=null)". The parenthesised rendering echoes the four StartSelector members in that fixed order, each either the value supplied in the request or the literal `null`; note the leading noun differs per class ("FRAGMENT_NUMBER" / "Start Timestamp" / "ContinuationToken") and the preposition case differs too ("for startSelector" in (a), "for StartSelector" in (b) and (c)).
Emulator: returns InvalidArgumentException (code and 400 are right) with its own sentences — "StartSelector.AfterFragmentNumber is required when StartSelectorType is FRAGMENT_NUMBER.", "StartSelector.StartTimestamp is required when StartSelectorType is PRODUCER_TIMESTAMP.", "StartSelector.StartTimestamp is required when StartSelectorType is SERVER_TIMESTAMP.", "StartSelector.ContinuationToken is required when StartSelectorType is CONTINUATION_TOKEN.".
Why: a client that matches, logs or asserts on the message text of this error class (the only field that distinguishes which companion member is missing, since all four classes share one error code) sees text that never occurs on AWS, so message-driven branching and golden-file tests pass against AWS and fail against the emulator.
Fix: the companion-field validation branch of the GetMedia handler — the `invalid_argument(..)` call sites in the StartSelector `match` — must render AWS's message template, including the StartSelector toString rendering, which is shared by all four classes and so belongs in one helper rather than four literals.
Traces: the four check ids above; handler citations under Traces at the end of this issue.

=== Cause 2 of 6: ValidationException message echoes the rejected value; AWS omits it ===
Failing checks: kinesis-video-media.get-media-after-fragment-number-non-numeric-rejected, kinesis-video-media.get-media-malformed-stream-name-rejected, kinesis-video-media.get-media-malformed-stream-arn-rejected.
AWS: pattern-constrained GetMedia inputs are `StreamName` (@length 1..256, @pattern ^[a-zA-Z0-9_.-]+$), `StreamARN`->ResourceARN (@length 1..1024, @pattern ^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$) and `StartSelector$AfterFragmentNumber`->FragmentNumberString (@length 1..128, @pattern ^[0-9]+$). A value violating one is refused with HTTP 400 `ValidationException` (the generic AWS front-end constraint error, not one of the operation's modeled errors) whose message is, as recorded: "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". The fixed text is "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; the variable parts are the member path in wire (lowerCamel, dotted for nested) form — 'streamName', 'streamARN', 'startSelector.afterFragmentNumber' — and the pattern. Critically, AWS does NOT interpolate the offending value: there is no "Value '<x>' at" form for these members.
Emulator: interpolates the rejected value, e.g. "1 validation error detected: Value 'not a valid stream name!' at 'streamName' failed to satisfy constraint: ...", "Value 'not-a-fragment-number' at 'startSelector.afterFragmentNumber' failed to satisfy constraint: ...", "Value 'arn:aws:not-a-real-service:us-east-1:123456789012:whatever' at 'streamARN' failed to satisfy constraint: ...". Code (ValidationException) and status are right; only the message diverges. (The quoted values are each check's own input and are not part of the requirement — the requirement is that no value appears at all.)
Why: besides breaking message assertions, interpolating the value makes the emulator leak request data AWS deliberately withholds here, so a caller that logs emulator errors records media identifiers AWS keeps out of its error channel.
Fix: the centralized request-validation step of the smithy server runtime that renders the "1 validation error detected" message for pattern violations (the validate stage of the deserialize -> validate -> handle walk, upstream of this operation's `handle`); it must emit the member path only, never the value.
Traces: the three check ids above.

=== Cause 3 of 6: ValidationException reports a pattern text that is not the one AWS reports ===
Failing checks: kinesis-video-media.get-media-malformed-stream-name-rejected, kinesis-video-media.get-media-malformed-stream-arn-rejected.
AWS: for the same ValidationException class, the pattern AWS prints is its service-side constraint, which is NOT always the public model's @pattern string. Recorded: for 'streamName' AWS prints "[a-zA-Z0-9_.-]+" (unanchored, while the model trait is ^[a-zA-Z0-9_.-]+$); for 'streamARN' AWS prints "arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+" — unanchored AND partition-generic (`arn:[a-z\\d-]+:`), while the model trait hard-codes ^arn:aws:. For 'startSelector.afterFragmentNumber' AWS does print the model's anchored "^[0-9]+$" (see Cause 2's check), so the rule is per-member, not a blanket strip of anchors.
Emulator: prints the model trait verbatim for every member — "^[a-zA-Z0-9_.-]+$" for streamName and "^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$" for streamARN.
Why: a client reading the pattern out of the message to repair its input is told the wrong rule. Hypothesis (stated as such because neither the model nor these recordings show it directly): because the emulator enforces the model's hard-coded `arn:aws:` prefix rather than AWS's `arn:[a-z\\d-]+:`, a well-formed stream ARN in a non-`aws` partition would also be refused with ValidationException where AWS accepts it.
Fix: the per-member constraint text used by that same validation stage for this service — the pattern reported (and enforced) for streamName and streamARN must be AWS's service-side pattern, not the model trait, and must stay anchored where AWS anchors it (afterFragmentNumber).
Traces: the two check ids above.

=== Cause 4 of 6: ResourceNotFoundException message names the stream instead of AWS's fixed sentence ===
Failing check: kinesis-video-media.get-media-nonexistent-stream-rejected.
AWS: the model documents ResourceNotFoundException as HTTP 404, "Status Code: 404, The stream with the given name does not exist". Recorded: GetMedia for a stream name that does not exist answers HTTP 404 `ResourceNotFoundException` with the message "The requested stream is not found or not active." — the entire message is fixed text with NO variable part; AWS does not echo the requested StreamName or StreamARN, and the same sentence covers a stream that exists but is not ACTIVE.
Emulator: answers 404 ResourceNotFoundException (code and status right) with "No such stream: <the name or ARN from the request>." — a template that both differs in wording and interpolates the requested identifier.
Why: a client matching the documented sentence does not recognise the emulator's error, and the emulator's variant implies a distinction (named stream absent) that AWS deliberately collapses with the not-active case.
Fix: the `resource_not_found(..)` construction in the GetMedia handler's stream-resolution step — it must produce the fixed sentence with no interpolation. This is distinct from the registry-split issue above: that issue is about WHICH streams resolve, this one is about the message emitted when resolution legitimately fails, and this check fails with the correct code on a genuinely absent stream.
Traces: check id above; `services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/src/ops/get_media.rs` lines 441-446.

=== Cause 5 of 6: an undecodable ContinuationToken is not rejected ===
Failing check: kinesis-video-media.get-media-continuation-token-invalid-rejected.
AWS: `StartSelector$ContinuationToken` is @length 1..128 @pattern ^[a-zA-Z0-9_\\.\\-]+$, so a token such as the check's "sim2real-bogus-continuation-token-000" SATISFIES the shape constraint and is not a ValidationException. Recorded behavior: AWS then fails to decode the token server-side and answers HTTP 400 `InvalidArgumentException` with the message "Illegal embedded sign character" — the whole message is fixed text, with no variable part and no echo of the token. It is NOT a 404: a syntactically valid but unresolvable continuation token is an argument fault, not a missing resource.
Emulator: the call never reaches any continuation-token decoding — it answers HTTP 404 `ResourceNotFoundException` "No such stream: <name>." for the stream the check created and polled to ACTIVE on the control plane. That 404 is the registry split described in the open issue above, so Cause 5 is MASKED by it and will still be open after that issue is fixed: there is no continuation-token decode/validation step in the handler at all, so once the stream resolves the bogus token will be treated as an unknown position rather than refused with InvalidArgumentException.
Why: a client that mishandles or truncates a continuation token gets a retriable-looking 404 "stream missing" instead of the 400 argument fault AWS returns, so it retries or re-creates the stream instead of restarting the session from a fresh selector.
Fix: the CONTINUATION_TOKEN branch of the GetMedia handler's selector resolution — decode the token and refuse an undecodable one with InvalidArgumentException and AWS's fixed message, before any stream-position lookup.
Traces: check id above.

=== Cause 6 of 6: GetMedia enforces no IAM authorization, so an explicit deny never produces AccessDeniedException ===
Failing check: kinesis-video-media.get-media-unauthorized-role-denied-rejected (verdict mismatch, "Check timed out after 60.0s").
AWS: authorization is evaluated before the operation runs. With a role that holds an inline Allow for `kinesisvideo:GetMedia`, GetMedia succeeds (HTTP 200); after an inline Deny for the same action on the same role is attached, the same assumed-role caller is refused with the IAM-layer error code `AccessDeniedException` (HTTP 400 for this family's JSON error channel), which is surfaced ahead of the stream-level `NotAuthorizedException` (modeled 401) semantics. Message/status for the deny leg are recorded behavior only — the check asserts on the error CODE, so the requirement is the code `AccessDeniedException`, not a message template.
Emulator: the handler's own doc comment states the authorization behaviour (`NotAuthorizedException`) is not modelled, and nothing consults IAM; the caller's identity is ignored entirely, so the explicit deny has no effect and the check's bounded poll for `AccessDeniedException` never converges (the run exhausts its 60s budget). The same check's earlier positive-control leg is additionally blocked by the registry split in the open issue above, which is why the timeout — not an assertion — is what surfaces; the authorization gap stands independently of it.
Why: this is an authorization bypass, the most damaging class here. A caller validating least-privilege policies, or a test proving a deny is effective, gets a successful GetMedia (or an unrelated 404) from the emulator and concludes the deny works when it does not; media a policy forbids is served to any identity that can reach the endpoint.
Fix: the request-authorization path in front of this operation's handler (the IAM evaluation middleware the data-plane services share), so GetMedia refuses a caller whose effective policy denies `kinesisvideo:GetMedia` with `AccessDeniedException`, and reserves the modeled `NotAuthorizedException` (401) for the stream-level/expired-token case.
Traces: check id above.

## Evidence
All evidence is from the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout; kinesis-video-media scored 0/14 (0%). The four checks OWNED by 20261022T180000Z-kinesis-video-media-getmedia-cannot-see-a-stream-createstream-made.md are excluded.

Cause 1 — expected vs actual error message (code InvalidArgumentException, HTTP 400, correct in all four):
  FRAGMENT_NUMBER, no AfterFragmentNumber
    expected: "FRAGMENT_NUMBER must be specified for startSelector : StartSelector(startSelectorType=FRAGMENT_NUMBER, afterFragmentNumber=null, startTimestamp=null, continuationToken=null)"
    actual:   "StartSelector.AfterFragmentNumber is required when StartSelectorType is FRAGMENT_NUMBER."
  PRODUCER_TIMESTAMP, no StartTimestamp
    expected: "Start Timestamp must be specified for StartSelector : StartSelector(startSelectorType=PRODUCER_TIMESTAMP, afterFragmentNumber=null, startTimestamp=null, continuationToken=null)"
    actual:   "StartSelector.StartTimestamp is required when StartSelectorType is PRODUCER_TIMESTAMP."
  SERVER_TIMESTAMP, no StartTimestamp
    expected: "Start Timestamp must be specified for StartSelector : StartSelector(startSelectorType=SERVER_TIMESTAMP, afterFragmentNumber=null, startTimestamp=null, continuationToken=null)"
    actual:   "StartSelector.StartTimestamp is required when StartSelectorType is SERVER_TIMESTAMP."
  CONTINUATION_TOKEN, no ContinuationToken
    expected: "ContinuationToken must be specified for StartSelector : StartSelector(startSelectorType=CONTINUATION_TOKEN, afterFragmentNumber=null, startTimestamp=null, continuationToken=null)"
    actual:   "StartSelector.ContinuationToken is required when StartSelectorType is CONTINUATION_TOKEN."

Cause 2 — code ValidationException correct, message interpolates the value:
  startSelector.afterFragmentNumber = a non-numeric value
    expected: "1 validation error detected: Value at 'startSelector.afterFragmentNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$"
    actual:   "1 validation error detected: Value 'not-a-fragment-number' at 'startSelector.afterFragmentNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$"
  streamName = a value with spaces/'!'
    expected: "1 validation error detected: Value at 'streamName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
    actual:   "1 validation error detected: Value 'not a valid stream name!' at 'streamName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"
  streamARN = a non-kinesisvideo ARN
    expected: "1 validation error detected: Value at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+"
    actual:   "1 validation error detected: Value 'arn:aws:not-a-real-service:us-east-1:123456789012:whatever' at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$"
  (the quoted inputs are each check's own; the requirement is that NO value is interpolated)

Cause 3 — same two messages, pattern text only: expected "[a-zA-Z0-9_.-]+" vs actual "^[a-zA-Z0-9_.-]+$" for streamName; expected "arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+" vs actual "^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$" for streamARN. For afterFragmentNumber both sides print "^[0-9]+$", which is why this is a per-member text defect and not an anchor-stripping rule.

Cause 4 — GetMedia on a never-created stream name: code ResourceNotFoundException and HTTP 404 correct;
    expected message: "The requested stream is not found or not active."
    actual message:   "No such stream: <requested stream name>."

Cause 5 — GetMedia(StartSelectorType=CONTINUATION_TOKEN, ContinuationToken=<a token matching ^[a-zA-Z0-9_\\.\\-]+$ but not issued by the service>) on a stream created and polled to ACTIVE:
    expected: HTTP 400, Code "InvalidArgumentException", Message "Illegal embedded sign character"
    actual:   HTTP 404, {'Code': 'ResourceNotFoundException', 'Message': 'No such stream: <stream name>.'} — the token is never examined.

Cause 6 — role with inline Allow kinesisvideo:GetMedia, then inline Deny for the same action:
    expected: GetMedia by that assumed-role caller fails with Code "AccessDeniedException" once the deny is attached (and succeeds before it).
    actual:   the error code never becomes AccessDeniedException; the emulator ignores caller identity (the handler's own module doc records that the NotAuthorizedException/authorization behaviour is "not modelled here"), and the check exhausts its 60s budget: "Check timed out after 60.0s".

Code citations (for orientation only):
  services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/src/ops/get_media.rs lines 379-399 — the StartSelector companion-field branches emitting the Cause 1 strings.
  same file lines 160-167 — the `invalid_argument` helper.
  same file lines 437-446 — stream resolution and the Cause 4 "No such stream: {}." message.
  same file lines 16-22 — the module doc stating authorization (NotAuthorizedException) is not modelled (Cause 6).
  The "1 validation error detected: ..." text of Causes 2-3 is produced by the shared runtime validate stage, not by this file.

## Steps to Reproduce
Boot the emulator from this checkout, then, for each cause, against the endpoint returned by kinesisvideo.get_data_endpoint(APIName='GET_MEDIA', ...):

Setup used by causes 1, 2, 5, 6: kinesisvideo.create_stream(StreamName=<name>, DataRetentionInHours=1), poll kinesisvideo.describe_stream until StreamInfo.Status == 'ACTIVE', then kinesisvideo.get_data_endpoint(APIName='GET_MEDIA', StreamName=<name>) and build a kinesis-video-media client on that endpoint.

Cause 1: call get_media(StreamName=<name>, StartSelector={'StartSelectorType': 'FRAGMENT_NUMBER'}); then again with 'PRODUCER_TIMESTAMP', 'SERVER_TIMESTAMP' and 'CONTINUATION_TOKEN'. Each returns 400 InvalidArgumentException; compare the Message with the AWS template in the evidence.

Cause 2 and 3: call get_media(StreamName=<name>, StartSelector={'StartSelectorType': 'FRAGMENT_NUMBER', 'AfterFragmentNumber': 'not-a-fragment-number'}); get_media(StreamName='not a valid stream name!', StartSelector={'StartSelectorType': 'EARLIEST'}); get_media(StreamARN='arn:aws:not-a-real-service:us-east-1:123456789012:whatever', StartSelector={'StartSelectorType': 'EARLIEST'}). Each returns ValidationException; observe the interpolated "Value '<x>' at" (cause 2) and the pattern text printed for streamName/streamARN (cause 3).

Cause 4: call get_media(StreamName=<a name never created>, StartSelector={'StartSelectorType': 'EARLIEST'}) -> 404 ResourceNotFoundException with Message 'No such stream: <that name>.' instead of 'The requested stream is not found or not active.'

Cause 5: call get_media(StreamName=<the ACTIVE stream>, StartSelector={'StartSelectorType': 'CONTINUATION_TOKEN', 'ContinuationToken': 'sim2real-bogus-continuation-token-000'}) -> the token (which satisfies ^[a-zA-Z0-9_\\.\\-]+$) is never decoded; expect 400 InvalidArgumentException 'Illegal embedded sign character'. Fixing the registry-split issue is a prerequisite for observing this cause's own failure mode, which is that no continuation-token decode step exists.

Cause 6: iam.create_role(<role trusting the account root>), iam.put_role_policy(<inline Allow for kinesisvideo:GetMedia on '*'>), sts.assume_role and build a kinesis-video-media client with those credentials against the same data endpoint; confirm get_media succeeds. Then iam.put_role_policy(<inline Deny for kinesisvideo:GetMedia on '*'>) on the same role and poll get_media with the same credentials: the error code never becomes 'AccessDeniedException' — the call behaves exactly as before, identity-independently.
