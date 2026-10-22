Issue-Source: trace-sweep-divergence
Issue-Trace: elementalinference-feed-association-error-paths
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elementalinference-feed-association-error-paths elementalinference-fixtures-search-and-get
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elemental-inference|AssociateFeed|4xx->4xx|$.message
Issue-Key: elemental-inference|DisassociateFeed|4xx->4xx|$.message
Issue-Key: elemental-inference|SearchFixtures|4xx->4xx|$.message
Issue-Key: elemental-inference|GetFixture|4xx->4xx|$.message
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# ElementalInference error messages are rebuilt from request and state values

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateFeed

## Description
On four rejection paths the HTTP status is right (400 on all four) but the message body is constructed by the emulator rather than following the service's message rule: three paths where AWS returns fixed text get an emulator-composed sentence that leaks request and internal-state values, and one path where AWS's message is built from the request (it names the identifier that was not found) gets a fixed generic sentence instead. Four causes, all in the handlers' error construction.

Cause 1 of 4 - AssociateFeed on an already-associated feed.
AWS: Smithy: AssociateFeed models ValidationException (httpError 400) and ConflictException (409); the recorded refusal is 400 with the invariant sentence "The feed is already associated with a resource." - it contains no request or state value, so the rule is a constant string.
Emulator: 400 with "You can't associate this feed. It has status ACTIVE which indicate that it's already associated with the resource err-source-01. You can associate only one resource with each feed." - the feed's internal status and the name of the other association are interpolated in.
Why: the emulator discloses the existing association's resource name to a caller that supplied a different one, and clients/tests that match on the service's message text do not match.
Fix: the AssociateFeed handler's conflict rejection - emit the constant message.
Traces: elementalinference-feed-association-error-paths#task#4.

Cause 2 of 4 - DisassociateFeed naming a resource that is not the associated one.
AWS: Smithy: DisassociateFeed models ValidationException (400); the recorded refusal is 400 with the invariant sentence "You can't disassociate this feed because it isn't associated with the resource you specified. Specify the correct resource." - again no request/state value, i.e. a constant.
Emulator: 400 with "The resource does-not-match is not associated with this feed. The feed is associated with the resource err-source-01." - both the rejected input and the real association are interpolated in.
Why: same disclosure of the current association plus a message no client can match.
Fix: the DisassociateFeed handler's rejection path.
Traces: elementalinference-feed-association-error-paths#task#5.

Cause 3 of 4 - SearchFixtures with endDate before startDate.
AWS: Smithy: SearchFixtures.startDate (required) and .endDate both target FixtureDate (@pattern ^\\d{4}-\\d{2}-\\d{2}$) and the op models ValidationException (400); the recorded refusal is 400 with the invariant sentence "endDate must not be earlier than startDate."
Emulator: 400 with "endDate '2024-01-01' must not precede startDate '2024-01-10'." - the two request values are interpolated into a differently worded sentence.
Why: the cross-member ordering rule is reported in text no client can match on.
Fix: the SearchFixtures input validation.
Traces: elementalinference-fixtures-search-and-get#task#2.

Cause 4 of 4 - GetFixture for an unknown fixtureId omits the identifier AWS echoes.
AWS: Smithy: GetFixture.fixtureId is an httpLabel (FixtureId, @length 1-128) and the op models ValidationException (400) and ResourceNotFoundException (404); the recorded refusal is 400 and its message is built from the request - it names the fixtureId that could not be found and then tells the caller what to do: "Can't find fixture <fixtureId>. Use a different fixtureId and try again." The rule (message names the requested fixtureId) is the requirement; the surrounding wording is the example.
Emulator: 400 with a fixed generic sentence, "The fixtureId that you specified is not valid. Obtain a fixtureId from SearchFixtures, or from the clipping output of a feed.", which never names the value that was rejected.
Why: a client or operator debugging a batch of fixture lookups cannot tell which id failed from the error alone.
Fix: the GetFixture handler's not-found rejection - interpolate the requested fixtureId.
Traces: elementalinference-fixtures-search-and-get#task#3.

## Evidence
Replay of simparity-wire-aws-v1; all four steps answered the expected status (400) and diverged only on `$.message`. elementalinference-feed-association-error-paths#task#4 (AssociateFeed): expected 'The feed is already associated with a resource.', got "You can't associate this feed. It has status ACTIVE which indicate that it's already associated with the resource err-source-01. You can associate only one resource with each feed.". #task#5 (DisassociateFeed): expected "You can't disassociate this feed because it isn't associated with the resource you specified. Specify the correct resource.", got 'The resource does-not-match is not associated with this feed. The feed is associated with the resource err-source-01.'. elementalinference-fixtures-search-and-get#task#2 (SearchFixtures): expected 'endDate must not be earlier than startDate.', got "endDate '2024-01-01' must not precede startDate '2024-01-10'.". #task#3 (GetFixture): expected "Can't find fixture nonexistent-fixture-probe-000. Use a different fixtureId and try again.", got 'The fixtureId that you specified is not valid. Obtain a fixtureId from SearchFixtures, or from the clipping output of a feed.'.

## Steps to Reproduce
1. Replay elementalinference-feed-association-error-paths: associate a feed with a resource, then POST /v1/feed/{id}/associate again (task#4) and POST /v1/feed/{id}/disassociate with a different associatedResourceName (task#5) - both 400 bodies interpolate the feed's status and existing association.
2. Replay elementalinference-fixtures-search-and-get: POST /v1/fixtures with startDate 2024-01-10 and endDate 2024-01-01 (task#2) - the 400 message quotes both dates instead of the fixed sentence.
3. GET /v1/fixtures/nonexistent-fixture-probe-000 (task#3) - the 400 message never names the requested fixtureId.
