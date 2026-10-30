Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: freetier.get-free-tier-usage-conflicting-root-operators-rejected freetier.get-free-tier-usage-empty-filter-rejected freetier.get-freetier-usage-multiple-root-operators-rejected freetier.get-free-tier-usage-dimension-values-pattern-rejected freetier.get-free-tier-usage-malformed-next-token-error freetier.get-freetier-usage-multiple-match-options-rejected
Issue-Shape-Id: com.amazonaws.freetier#AWSFreeTierService
Issue-Key: freetier.get-free-tier-usage-conflicting-root-operators-rejected|run#9|AssertionError|assert error["Message"] == (
Issue-Key: freetier.get-free-tier-usage-empty-filter-rejected|run#6|AssertionError|assert error["Message"] == (
Issue-Key: freetier.get-freetier-usage-multiple-root-operators-rejected|run#8|AssertionError|assert error["Message"] == (
Issue-Key: freetier.get-free-tier-usage-dimension-values-pattern-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: freetier.get-free-tier-usage-malformed-next-token-error|run#6|AssertionError|assert error["Message"] == "[Message = The next token is invalid.]", error
Issue-Key: freetier.get-freetier-usage-multiple-match-options-rejected|run#7|AssertionError|assert error["Message"] == "[Message = Exactly one match option must be provided]", error
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# freetier GetFreeTierUsage refusal messages are emulator-authored prose instead of the service's fixed text (4 causes)

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFreeTierUsage

## Description
GetFreeTierUsage refuses every one of these requests with the right error code (ValidationException, HTTP 400 per the model's @httpError on com.amazonaws.freetier#ValidationException), but with message text this crate wrote itself rather than the text AWS emits. The code is right and the refusal happens, so only the message diverges — and a message is a requirement, not a cosmetic. FOUR distinct causes are bundled here; all four live in this one operation's validation path.

Cause 1 of 4: root-operator cardinality message.
AWS: Smithy: GetFreeTierUsage.filter targets Expression, whose documentation states "you must specify exactly one of the following root operators" (members Or, And, Not, Dimensions). The modeled refusal is ValidationException (service-common error, @httpError 400). Recorded behavior: AWS words EVERY violation of that cardinality — zero root operators set, two set, or two conflicting ones set — with ONE fixed string carrying no variable parts at all: "[Message = Only one root operator is allowed for each expression: And, Or, Not, Dimensions]". The literal "[Message = " prefix and the closing "]" are part of AWS's text, as is the operator order "And, Or, Not, Dimensions".
Emulator: emits three different hand-written sentences that name the offending members back to the caller — "filter: an expression must set exactly one of Or, And, Not, Dimensions, but this one sets Or, And" (And+Or), "... but this one sets none" (filter={}), "... but this one sets And, Dimensions" (And+Dimensions). None of these is AWS's string, and the emulator varies the text with the request where AWS does not vary it at all.
Why: a client that branches on, logs or asserts the refusal reason sees a message that never occurs on AWS; code that greps for "Only one root operator is allowed" (the documented phrase) never matches, and code that parses the emulator's trailing "sets X, Y" clause finds nothing to parse against real AWS.
Fix: the filter/Expression validation in this operation's handler (services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_free_tier_usage.rs) must raise the single fixed string for all three cardinality cases instead of composing a per-request sentence.
Traces: freetier.get-free-tier-usage-conflicting-root-operators-rejected, freetier.get-free-tier-usage-empty-filter-rejected, freetier.get-freetier-usage-multiple-root-operators-rejected.

Cause 2 of 4: Dimensions.Values constraint message is reported per member instead of as the collection-level aggregate.
AWS: Smithy: DimensionValues.Values targets the list Values (@length min 1), whose member Value carries @length min 1 / max 20 and @pattern ^[ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*$. Recorded behavior: AWS reports a bad entry against the COLLECTION, rendering the whole list as the offending value and listing the member's constraints as a nested aggregate: "1 validation error detected: Value '[<the list, comma-joined, in brackets>]' at 'filter.dimensions.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 20, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*]". Fixed text: everything except the bracketed rendering of the submitted list. Note AWS prints the pattern WITHOUT the ^ and $ anchors and lists all three member constraints regardless of which one was broken.
Emulator: reports the single offending member with an indexed path and only the constraint that failed, anchors included: "1 validation error detected: Value 'Amazon!EC2' at 'filter.dimensions.values.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*$". Wrong member path shape, wrong value rendering, wrong constraint list, wrong pattern rendering.
Why: the member path is the machine-readable part of an AWS validation message; a client that maps 'filter.dimensions.values' to the field it must re-prompt for gets an indexed sub-path AWS never emits, and anything that reproduces the documented charset from the message picks up anchors that are not in AWS's text.
Fix: the input-validation layer for this operation's filter must aggregate list-member constraint failures at the list path (services/.../src/ops/get_free_tier_usage.rs validation, and whatever shared validation-message builder it calls).
Traces: freetier.get-free-tier-usage-dimension-values-pattern-rejected.

Cause 3 of 4: unissued nextToken message.
AWS: Smithy: GetFreeTierUsageRequest.nextToken targets NextPageToken (@length 1..8192); a token the service did not issue is refused with ValidationException. Recorded behavior: the message is the fixed string "[Message = The next token is invalid.]" — no variable parts, including the "[Message = " wrapper and the trailing period inside the bracket.
Emulator: "nextToken: the pagination token is not one GetFreeTierUsage issued" — emulator-authored prose that also leaks the operation name into the text.
Why: a paginating client that distinguishes "token expired/invalid, restart the scan" from other validation failures by the message never recognizes the condition.
Fix: the nextToken decode-failure branch of this operation's handler (services/.../src/ops/get_free_tier_usage.rs, which today raises that string around crate::state::decode_page_token).
Traces: freetier.get-free-tier-usage-malformed-next-token-error.

Cause 4 of 4: MatchOptions cardinality message.
AWS: Smithy: DimensionValues.MatchOptions is required and its documentation states "You can specify only one of these values in the array." The refusal is ValidationException. Recorded behavior: the message is the fixed string "[Message = Exactly one match option must be provided]" — no variable parts, no count, no member path.
Emulator: "filter.Dimensions.MatchOptions: exactly one match option is allowed, got 2" — a different sentence that additionally reports the submitted count, which AWS's text does not contain.
Why: same as above — the reason string a client matches on does not occur on AWS.
Fix: the MatchOptions cardinality check in this operation's filter validation (services/.../src/ops/get_free_tier_usage.rs).
Traces: freetier.get-freetier-usage-multiple-match-options-rejected.

All four causes are message-text defects only: the error code and status already match, and each check's positive control (a single-root-operator filter, a well-formed value, a plain unpaginated call) succeeds against the emulator, so the refusal paths themselves are reached. Related but distinct: the fixed issue 20261024T000352Z-freetier-getfreetierusage-validation.md made the multi-root-operator case refuse at all; that refusal now happens, and what remains is its wording. The open issue 20261028T211319Z-freetier-listaccountactivities-pagination.md covers a token whose offset runs past the end being answered 200 — a different defect from the wording of the refusal raised here.

## Evidence
All four causes replayed against the emulator; expected values are what the recorded AWS behavior and the Smithy model require.

Cause 1 (root-operator cardinality), three variants, all ValidationException/400:
  filter={"And":[region],"Or":[other_region]}
    expected Message: "[Message = Only one root operator is allowed for each expression: And, Or, Not, Dimensions]"
    actual   Message: "filter: an expression must set exactly one of Or, And, Not, Dimensions, but this one sets Or, And"
  filter={}
    expected Message: "[Message = Only one root operator is allowed for each expression: And, Or, Not, Dimensions]"
    actual   Message: "filter: an expression must set exactly one of Or, And, Not, Dimensions, but this one sets none"
  filter={"And":[region],"Dimensions":{...}}
    expected Message: "[Message = Only one root operator is allowed for each expression: And, Or, Not, Dimensions]"
    actual   Message: "filter: an expression must set exactly one of Or, And, Not, Dimensions, but this one sets And, Dimensions"
  (AWS's string is identical in all three; the emulator's differs in all three.)

Cause 2 (Dimensions.Values charset), filter={"Dimensions":{"Key":"SERVICE","Values":["Amazon!EC2"],"MatchOptions":["CONTAINS"]}}:
  expected Message: "1 validation error detected: Value '[Amazon!EC2]' at 'filter.dimensions.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 20, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*]"
  actual   Message: "1 validation error detected: Value 'Amazon!EC2' at 'filter.dimensions.values.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*$"
  Both are Code=ValidationException. The identical filter with Values=["Amazon-EC2"] succeeds on the emulator (positive control), so only the message of the rejection diverges.

Cause 3 (unissued nextToken), nextToken="not-a-real-pagination-token":
  expected Message: "[Message = The next token is invalid.]"
  actual   Message: "nextToken: the pagination token is not one GetFreeTierUsage issued"
  Both Code=ValidationException; the plain call with no token succeeds either side.

Cause 4 (MatchOptions), filter={"Dimensions":{"Key":"REGION","Values":["us-east-1"],"MatchOptions":["EQUALS","STARTS_WITH"]}}:
  expected Message: "[Message = Exactly one match option must be provided]"
  actual   Message: "filter.Dimensions.MatchOptions: exactly one match option is allowed, got 2"
  Both Code=ValidationException; MatchOptions=["EQUALS"] succeeds either side.

## Steps to Reproduce
1. Boot the emulator with the freetier service mounted and build a botocore freetier client against it.
2. Cause 1: call GetFreeTierUsage with filter={"And":[{"Dimensions":{"Key":"REGION","Values":["us-east-1"],"MatchOptions":["EQUALS"]}}],"Or":[{"Dimensions":{"Key":"REGION","Values":["us-west-2"],"MatchOptions":["EQUALS"]}}]}; then with filter={}; then with filter={"And":[region_dim],"Dimensions":{"Key":"SERVICE","Values":["AWSLambda"],"MatchOptions":["EQUALS"]}}. Each returns ValidationException; compare Error.Message with "[Message = Only one root operator is allowed for each expression: And, Or, Not, Dimensions]".
3. Cause 2: call GetFreeTierUsage with filter={"Dimensions":{"Key":"SERVICE","Values":["Amazon!EC2"],"MatchOptions":["CONTAINS"]}}, maxResults=10 and compare Error.Message with the collection-level aggregate above. Re-run with Values=["Amazon-EC2"] to confirm the call otherwise succeeds.
4. Cause 3: call GetFreeTierUsage(maxResults=5) (succeeds), then GetFreeTierUsage(nextToken="not-a-real-pagination-token") and compare Error.Message with "[Message = The next token is invalid.]".
5. Cause 4: call GetFreeTierUsage with filter={"Dimensions":{"Key":"REGION","Values":["us-east-1"],"MatchOptions":["EQUALS","STARTS_WITH"]}} and compare Error.Message with "[Message = Exactly one match option must be provided]".
6. Re-replay the checks in trace_ids to confirm all four causes are closed.
