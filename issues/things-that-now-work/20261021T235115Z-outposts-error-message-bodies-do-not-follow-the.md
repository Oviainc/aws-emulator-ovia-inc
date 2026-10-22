Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-decommission-and-delete-conflicts
Issue-Step: teardown#6
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-decommission-and-delete-conflicts outposts-error-paths-not-found-and-validation outposts-site-lifecycle
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|DeleteOutpost|4xx->4xx|$.Message
Issue-Key: outposts|GetSite|4xx->4xx|$.Message
Issue-Key: outposts|GetOutpost|4xx->4xx|$.Message
Issue-Key: outposts|GetConnection|4xx->4xx|$.message
Issue-Key: outposts|CreateQuote|4xx->4xx|$.message
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# Outposts error message bodies do not follow the recorded AWS message rules

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSite

## Description
Status codes and error codes match on these five steps; only the Message/message body differs — but in each case the recordings show a deterministic rule built from the request, and the emulator uses a different rule. Two rules are involved.

Rule A (Causes 1-3): the outposts NotFoundException body is the identifier echo '<memberName>=<value as supplied>', not an English sentence. Three recordings agree on this (DeleteOutpost, GetSite, GetOutpost), including that the value is echoed exactly as the client sent it — the DeleteOutpost recording echoes the full ARN the request used, not the bare id.
Rule B (Causes 4-5): the outposts ValidationException body is the standard framework text '<N> validation error(s) detected: Value at '<member>' failed to satisfy constraint: <constraint>' with the offending value OMITTED. Two independent recordings agree (connectionId pattern, countryCode length+pattern), and the countryCode recording shows the omission applied to each of two joined errors.

Cause 1 of 5: DeleteOutpost not-found message is prose instead of the identifier echo.
AWS: Smithy: DeleteOutpost -> NotFoundException(404), member Message : ErrorMessage. Recorded rule A: body $.Message = 'outpostId=<value as supplied>' (the recording's request supplied an ARN, and the ARN is echoed).
Emulator: 404 (status and code correct) with $.Message = 'the Outpost op-... does not exist' — prose, and it normalises the supplied ARN down to the bare id.
Why: a client or test that parses the identifier out of the message, or that round-trips the exact identifier it sent, gets neither.
Fix: the outposts NotFoundException message construction for DeleteOutpost (echo the request's identifier member and value).
Traces: outposts-decommission-and-delete-conflicts#teardown#6.

Cause 2 of 5: GetSite not-found message is prose instead of 'siteId=<value>'.
AWS: Smithy: GetSite -> NotFoundException(404). Recorded rule A: $.Message = 'siteId=os-00000000000000000'.
Emulator: 404 with $.Message = 'the site os-00000000000000000 does not exist'.
Why/Fix: as Cause 1, in the GetSite handler's not-found path.
Traces: outposts-error-paths-not-found-and-validation#task#1 (also outposts-site-lifecycle#teardown#10).

Cause 3 of 5: GetOutpost not-found message is prose instead of 'outpostId=<value>'.
AWS: Smithy: GetOutpost -> NotFoundException(404). Recorded rule A: $.Message = 'outpostId=op-00000000000000000'.
Emulator: 404 with $.Message = 'the Outpost op-00000000000000000 does not exist'.
Why/Fix: as Cause 1, in the GetOutpost handler's not-found path.
Traces: outposts-error-paths-not-found-and-validation#task#2.

Cause 4 of 5: GetConnection validation message echoes the offending value.
AWS: Smithy: GetConnection.ConnectionId @length 1..1024 @pattern ^[a-zA-Z0-9+/=]{1,1024}$, error ValidationException(400). Recorded rule B: "1 validation error detected: Value at 'connectionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9+/=]{1,1024}$" — member and constraint named, value omitted.
Emulator: 400/ValidationException but the body interpolates the value: "Value 'conn-does-not-exist' at 'connectionId' ...".
Why: the emulator reflects client-supplied input back into the error body where AWS does not, so assertions on AWS's exact validation text fail and any value a client considers sensitive is echoed.
Fix: the shared outposts validation-error message formatter (drop the value interpolation).
Traces: outposts-error-paths-not-found-and-validation#task#6.

Cause 5 of 5: CreateQuote validation message echoes the offending value, in both joined errors.
AWS: Smithy: CreateQuote.CountryCode required, @length min 2 max 2, @pattern ^[A-Z]{2}$, error ValidationException(400). Recorded rule B: "2 validation errors detected: Value at 'countryCode' failed to satisfy constraint: Member must have length less than or equal to 2; Value at 'countryCode' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Z]{2}$".
Emulator: 400/ValidationException with "Value 'USA' at 'countryCode' ..." in both clauses.
Why/Fix: same formatter as Cause 4; confirms the rule is not op-specific.
Traces: outposts-error-paths-not-found-and-validation#task#11.

## Evidence
Cause 1 — DeleteOutpost, 404/404: $.Message expected 'outpostId=arn:aws:outposts:us-west-2:000000000000:outpost/op-e77c84b180a57e0a0', got 'the Outpost op-e77c84b180a57e0a0 does not exist'.
Cause 2 — GetSite, 404/404: $.Message expected 'siteId=os-00000000000000000', got 'the site os-00000000000000000 does not exist'.
Cause 3 — GetOutpost, 404/404: $.Message expected 'outpostId=op-00000000000000000', got 'the Outpost op-00000000000000000 does not exist'.
Cause 4 — GetConnection, 400/400: $.message expected "1 validation error detected: Value at 'connectionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9+/=]{1,1024}$", got the same text with "Value 'conn-does-not-exist' at 'connectionId'".
Cause 5 — CreateQuote, 400/400: $.message expected "2 validation errors detected: Value at 'countryCode' failed to satisfy constraint: Member must have length less than or equal to 2; Value at 'countryCode' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Z]{2}$", got the same text with "Value 'USA'" inserted in each clause.
Status and x-amzn-errortype agree with the recordings on all five steps; the divergence is the message-construction rule (identifier echo for NotFoundException, value-omitting framework text for ValidationException), which multiple independent recordings establish.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay outposts-decommission-and-delete-conflicts step teardown#6 — DeleteOutpost on an already-deleted Outpost ARN.
3. Cause 2: replay outposts-error-paths-not-found-and-validation step task#1 — GetSite with siteId os-00000000000000000.
4. Cause 3: same fixture, step task#2 — GetOutpost with outpostId op-00000000000000000.
5. Cause 4: same fixture, step task#6 — GetConnection with a connectionId that violates the pattern.
6. Cause 5: same fixture, step task#11 — CreateQuote with countryCode 'USA'.
