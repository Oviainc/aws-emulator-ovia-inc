Issue-Source: trace-sweep-divergence
Issue-Trace: rbin-invalid-parameter-and-notfound-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rbin-invalid-parameter-and-notfound-errors rbin-tag-operations-notfound-errors
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin|ListRules|4xx->4xx|$.Message
Issue-Key: rbin|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: rbin|TagResource|4xx->4xx|$.Message
Issue-Key: rbin|UntagResource|4xx->4xx|$.Message
Issue-Commit: 1aea0a827cbbaf06353cebaab6cc8375c4166c22

# rbin error messages are emulator-rendered rather than AWS's: ListRules strips the NextToken pattern anchors and the tag operations name the whole ARN in ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `1aea0a827cbbaf06353cebaab6cc8375c4166c22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRules

## Description
Two independent message-rendering root causes in the rbin crate. Both return the right HTTP status and the right error code; only the human-readable `Message` field diverges from AWS, so every client that matches on the message text (or that surfaces it to a user) sees text AWS never emits. THIS ISSUE HAS 2 CAUSES; the second one is reached through three operations and three fixture steps, all of which must be re-replayed before closing.

Cause 1 of 2 - ListRules: the reported @pattern is printed with its anchors stripped.
AWS: `ListRules$NextToken` is modelled with @pattern `^[A-Za-z0-9+/=]{1,2048}$` (Smithy: ListRulesRequest$NextToken pattern `^[A-Za-z0-9+/=]{1,2048}$`; error ValidationException, httpError 400). When a NextToken fails that pattern AWS answers HTTP 400 with code ValidationException and the standard constraint message `1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`, where `<pattern>` is the member's modelled pattern reproduced VERBATIM, anchors included. The fixed text of that message is `1 validation error detected: Value at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `; its variable parts are the member path (`nextToken`, from the offending request member) and the pattern string (from the model). No part of it comes from the submitted value.
Emulator: answers the correct 400/ValidationException but renders the pattern through a local 'strip the ^ and $ anchors' helper, producing `... Member must satisfy regular expression pattern: [A-Za-z0-9+/=]{1,2048}` - the two anchor characters AWS includes are missing.
Why: a client (or a test, or a user reading the error) comparing the reported pattern against the modelled one, or re-using the reported regex to pre-validate its own tokens, gets a regex that is NOT the service's - the unanchored form matches any string CONTAINING 1..2048 token characters, so the client accepts input the real service rejects. It also breaks any exact-match assertion on the AWS message.
Fix: the validation/message-rendering layer that formats a pattern constraint violation for every rbin operation (the `ViolationKind::Pattern` rendering reached from the crate's input-validation gate) must emit the modelled pattern unmodified instead of passing it through the anchor-stripping helper. Note this contradicts open LOW issue 20261028T050716Z-rbin-getrule-maintainability.md, which proposes consolidating that same anchor-stripping helper on the shared `service-common` copy: the recorded AWS response shows the stripping itself is wrong for this service, so the rendering must be corrected (and that crate's tests that pin the anchor-less text updated) rather than merely de-duplicated. That issue does not cover this divergence.
Traces: rbin-invalid-parameter-and-notfound-errors#task#4 (ListRules with an invalid NextToken).

Cause 2 of 2 - TagResource / UntagResource / ListTagsForResource: ResourceNotFoundException names the whole ARN instead of the rule identifier.
AWS: all three tag operations take `ResourceArn` as an @httpLabel targeting `RuleArn` (@pattern `^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}$`) and model ResourceNotFoundException with httpError 404 (Smithy: TagResource/UntagResource/ListTagsForResource errors -> ResourceNotFoundException, 404). When the ARN is well-formed but resolves to no rule, AWS answers 404 ResourceNotFoundException with the message `Recycle Bin Rule '<identifier>' does not exist.` - the fixed text is `Recycle Bin Rule '` ... `' does not exist.` and the single variable part is the rule IDENTIFIER, i.e. only the `rule/<Identifier>` segment of the submitted ARN, not the ARN itself.
Emulator: returns the correct 404 and the correct ResourceNotFoundException code, but interpolates the full request ARN into that slot, e.g. `Recycle Bin Rule 'arn:aws:rbin:<region>:<account>:rule/<Identifier>' does not exist.` where AWS writes `Recycle Bin Rule '<Identifier>' does not exist.`
Why: the message is an AWS requirement, so any client, SDK test or log-scraper that asserts on or parses the not-found message - including anything that extracts the identifier from it to retry or report - fails against the emulator. It also leaks the emulator's own region/account values into a message AWS keeps identifier-only, making emulator responses environment-dependent where AWS's are not.
Fix: the not-found error construction shared by the three ARN-addressed tag handlers (TagResource, UntagResource, ListTagsForResource) must format the message from the identifier parsed out of the ARN's `rule/<Identifier>` resource segment, not from the raw `ResourceArn` input.
Traces: rbin-tag-operations-notfound-errors#task#1 (ListTagsForResource), rbin-tag-operations-notfound-errors#task#2 (TagResource), rbin-tag-operations-notfound-errors#task#3 (UntagResource).

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 fixtures, 40 steps, 36 passed, 4 mismatched, score 90%). All four steps matched status and error code; only `$.Message` diverged.

Cause 1 - fixture rbin-invalid-parameter-and-notfound-errors, step task#4, ListRules, expected 400 / actual 400:
  $.Message expected: "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9+/=]{1,2048}$"
  $.Message actual:   "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9+/=]{1,2048}"
  Diverging parts: the leading `^` and trailing `$` of the modelled pattern are absent. Fixed text matches; the variable part (the pattern, taken from the model) is wrong.
  Model: ListRulesRequest$NextToken @pattern ^[A-Za-z0-9+/=]{1,2048}$ ; ValidationException httpError 400.

Cause 2 - fixture rbin-tag-operations-notfound-errors, steps task#1 (ListTagsForResource), task#2 (TagResource), task#3 (UntagResource), expected 404 / actual 404 in every case:
  $.Message expected: "Recycle Bin Rule 'AAAAAAAAAAA' does not exist."
  $.Message actual:   "Recycle Bin Rule 'arn:aws:rbin:us-west-2:111111111111:rule/AAAAAAAAAAA' does not exist."
  Diverging part: the quoted slot. Fixed text `Recycle Bin Rule '`/`' does not exist.` matches; the variable part must be the rule identifier from the ARN's rule/<Identifier> segment, while the emulator substitutes the entire submitted ResourceArn. (The region/account values above are this recording's; they are not part of the requirement - the requirement is that NO ARN prefix appears at all.)
  Model: TagResource/UntagResource/ListTagsForResource $ResourceArn -> RuleArn @httpLabel @required; ResourceNotFoundException httpError 404.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and POST /list-rules to the rbin endpoint with a body whose ResourceType is valid and whose NextToken contains a character outside [A-Za-z0-9+/=] (e.g. {"ResourceType":"EBS_SNAPSHOT","NextToken":"not a token!"}).
2. Observe HTTP 400 with code ValidationException (correct) and Message "... Member must satisfy regular expression pattern: [A-Za-z0-9+/=]{1,2048}".
3. AWS returns the same status/code with the pattern anchored: "... Member must satisfy regular expression pattern: ^[A-Za-z0-9+/=]{1,2048}$". Re-replay fixture rbin-invalid-parameter-and-notfound-errors step task#4 to confirm.

Cause 2:
1. Without creating any rule, GET /tags/<a well-formed RuleArn whose identifier matches ^[0-9a-zA-Z]{11}$ and which no rule owns>, percent-encoded as the label requires.
2. Observe HTTP 404 ResourceNotFoundException (correct) with Message "Recycle Bin Rule '<the whole ARN>' does not exist."; AWS writes "Recycle Bin Rule '<the 11-character identifier>' does not exist."
3. Repeat with POST /tags/<same ARN> (TagResource, body {"Tags":[{"Key":"k","Value":"v"}]}) and DELETE /tags/<same ARN>?tagKeys=k (UntagResource) - both show the same substitution.
4. Re-replay fixture rbin-tag-operations-notfound-errors steps task#1, task#2 and task#3 to confirm all three handlers.
