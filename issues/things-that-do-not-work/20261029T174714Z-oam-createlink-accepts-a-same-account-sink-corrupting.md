Issue-Source: trace-sweep-divergence
Issue-Trace: oam-sink-crud-and-same-account-link-rejection
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: oam-sink-crud-and-same-account-link-rejection
Issue-Shape-Id: com.amazonaws.oam#oamservice
Issue-Key: oam|CreateLink|4xx->2xx|$.message
Issue-Key: oam|CreateLink|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: oam|CreateLink|4xx->2xx|status
Issue-Key: oam|ListAttachedLinks|2xx->2xx|$.Items[*]
Issue-Key: oam|DeleteSink|2xx->4xx|$
Issue-Key: oam|DeleteSink|2xx->4xx|status
Issue-Key: oam|GetSink|4xx->2xx|$.Message
Issue-Key: oam|GetSink|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: oam|GetSink|4xx->2xx|status
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# oam CreateLink accepts a same-account sink, corrupting sink/link lifecycle

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLink

## Description
The oam emulator does not enforce the rule that a monitoring link may not point at a sink owned by the SAME account. Because the invalid link is created and persisted, three further operations on the sink/link pair then diverge as well. This issue bundles 4 causes; all four must be closed.

Cause 1 of 4 - CreateLink does not reject a sink in the caller's own account (fixture oam-sink-crud-and-same-account-link-rejection, step task#6).
AWS: CreateLink against a SinkIdentifier whose sink belongs to the calling account is rejected with HTTP 400 and the modeled client error InvalidParameterException (Smithy: CreateLink errors = ConflictException 409, InternalServiceFault 500, InvalidParameterException 400, MissingRequiredParameterException 400, ServiceQuotaExceededException 429; InvalidParameterException members = message, amznErrorType bound to header x-amzn-ErrorType). The recorded response body carries the member `message` with the fixed text "Cannot create a link to a sink in the same account" - that string has no variable parts - and sets the x-amzn-errortype header to the exception name. Hypothesis: the account comparison is made between the caller's account id and the account id embedded in the resolved sink ARN.
Emulator: returns HTTP 200 with a normal CreateLinkOutput; no `message` member and no x-amzn-errortype header are present at all.
Why: a client that relies on AWS refusing a same-account link sees the call succeed, so a configuration error (monitoring account == source account) is silently accepted and the client believes a cross-account observability link exists where AWS would have none.
Fix: sink-ownership/account validation in the CreateLink handler of the oam service crate, before the link is written to state.

Cause 2 of 4 - ListAttachedLinks reports the link that should never have been created (fixture oam-sink-crud-and-same-account-link-rejection, step task#7).
AWS: ListAttachedLinks on the sink returns HTTP 200 with the required `Items` list EMPTY, because the preceding same-account CreateLink was rejected and no link was attached (Smithy: ListAttachedLinksOutput.Items required, paginated on NextToken/MaxResults).
Emulator: returns HTTP 200 with one extra element in Items, describing the link that cause 1 wrongly created (Label/LinkArn/ResourceTypes populated; the ARN and label values themselves are per-run and not part of the requirement).
Why: a client enumerating links attached to a sink sees phantom attachments, so reconciliation and teardown logic acts on a link AWS would not report.
Fix: follows from cause 1 (no link should be in state); the link store read by the ListAttachedLinks handler must contain no entry for a rejected CreateLink.

Cause 3 of 4 - DeleteSink fails with ConflictException because of that phantom link (fixture oam-sink-crud-and-same-account-link-rejection, step task#8).
AWS: DeleteSink on a sink with no attached links returns HTTP 200 with an EMPTY body ({}) - DeleteSinkOutput has no members. ConflictException (409) is modeled for DeleteSink, but applies only when the sink still has attached links.
Emulator: returns HTTP 409 with body {"__type": "ConflictException", "Message": "this sink still has 1 attached link(s); delete every link to the sink before deleting the sink"}. Note also that the emulator's own conflict message is lower-cased free prose unlike AWS's sentence-cased messages (ConflictException member is `Message`), and the count in it is a value from state, not fixed text.
Why: a client cannot tear down its sink: the delete is permanently blocked by a link AWS would never have created, so cleanup and test teardown fail.
Fix: follows from cause 1; the DeleteSink handler's attached-link precondition must see an empty link set, and its conflict message wording should match AWS's.

Cause 4 of 4 - GetSink still returns the sink after the delete (fixture oam-sink-crud-and-same-account-link-rejection, step task#9).
AWS: GetSink for an identifier that does not exist (here, because DeleteSink already removed it) returns HTTP 404 with the modeled ResourceNotFoundException; the recorded body carries the member `Message` with the fixed text "Resource not found." (no variable parts) and sets x-amzn-errortype to the exception name (Smithy: GetSink errors include ResourceNotFoundException 404, members Message + amznErrorType on header x-amzn-ErrorType).
Emulator: returns HTTP 200 with a GetSinkOutput for the still-present sink; no `Message` member and no x-amzn-errortype header.
Why: a client polling for deletion never observes the resource going away and cannot distinguish "deleted" from "still live", so it loops or reports the sink as existing.
Fix: follows from cause 3 (the delete must succeed and remove the sink from state); the GetSink handler must then raise ResourceNotFoundException with the above status, code and message.

Traces: oam-sink-crud-and-same-account-link-rejection#task#6 (CreateLink), #task#7 (ListAttachedLinks), #task#8 (DeleteSink), #task#9 (GetSink). No open oam issue covers any of these.

## Evidence
Cause 1 - CreateLink (task#6): expected status 400, got 200; $.message missing (expected 'Cannot create a link to a sink in the same account', got None); $headers.x-amzn-errortype missing.
Cause 2 - ListAttachedLinks (task#7): status 200 both, but $.Items[0] extra (expected no item, got a link object with Label/LinkArn/ResourceTypes).
Cause 3 - DeleteSink (task#8): expected status 200, got 409; body expected {} , got {'__type': 'ConflictException', 'Message': 'this sink still has 1 attached link(s); delete every link to the sink before deleting the sink'}.
Cause 4 - GetSink (task#9): expected status 404, got 200; $.Message missing (expected 'Resource not found.', got None); $headers.x-amzn-errortype missing.
Smithy (com.amazonaws.oam): CreateLink errors include InvalidParameterException(400); DeleteSinkOutput has no members; GetSink errors include ResourceNotFoundException(404) with member Message; ConflictException member is Message.

## Steps to Reproduce
1. Replay fixture oam-sink-crud-and-same-account-link-rejection against the emulator.
2. Step task#6: CreateSink in the account, then CreateLink with SinkIdentifier = that sink's ARN (same account). Observe HTTP 200 instead of 400 InvalidParameterException with message 'Cannot create a link to a sink in the same account'.
3. Step task#7: ListAttachedLinks for that sink - Items contains the phantom link instead of being empty.
4. Step task#8: DeleteSink for that sink - HTTP 409 ConflictException instead of 200 with an empty body.
5. Step task#9: GetSink for the same identifier - HTTP 200 instead of 404 ResourceNotFoundException with Message 'Resource not found.'
