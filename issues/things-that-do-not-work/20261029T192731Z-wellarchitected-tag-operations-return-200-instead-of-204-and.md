Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-workload-tagging-envelopes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-workload-tagging-envelopes
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|TagResource|2xx->2xx|status
Issue-Key: wellarchitected|UntagResource|2xx->2xx|status
Issue-Key: wellarchitected|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: wellarchitected|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|TagResource|4xx->4xx|$.Message
Issue-Key: wellarchitected|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Tag operations return 200 instead of 204 and a non-AWS not-found message

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
This bundle has 4 causes, all in the Well-Architected tag operation family: the success envelope carries the wrong HTTP status, and the ResourceNotFound envelope carries the wrong message.

Cause 1 of 4: TagResource success status.
AWS: TagResource (POST /tags/{WorkloadArn}) has an EMPTY output shape (Smithy: TagResourceOutput has no members) and AWS answers it with HTTP 204 No Content; the on-disk model's http code is the protocol default 200 because no explicit code is set, so this is recorded behavior: the recording shows 204 with no body.
Emulator: answers 200.
Why: a client or proxy that keys on 204 for 'no content' (or asserts the documented status) treats the response as carrying a body it must parse; a strict status assertion fails against the emulator but passes against AWS.
Fix: the TagResource handler's success response status.
Traces: wellarchitected-workload-tagging-envelopes#task#3.

Cause 2 of 4: UntagResource success status.
AWS: UntagResource (DELETE /tags/{WorkloadArn}?tagKeys=...) likewise has an empty output shape and answers 204 No Content in the recording.
Emulator: answers 200.
Why/Fix: as cause 1, in the UntagResource handler.
Traces: wellarchitected-workload-tagging-envelopes#task#5.

Cause 3 of 4: ListTagsForResource ResourceNotFound message.
AWS: ListTagsForResource (GET /tags/{WorkloadArn}, errors ResourceNotFoundException httpError 404) answers an unknown workload ARN with 404 and the fixed message 'Resource was not found' — no '[ResourceNotFound]' prefix and no variable part at all on this tag path.
Emulator: 404 with '[ResourceNotFound] No workload with ARN <workloadArn> was found in account <accountId>.' and x-amzn-errortype 'ResourceNotFoundException'.
Why: the message is a requirement of the error class; a client matching or displaying AWS's text sees a string AWS never emits, and the emulator additionally discloses the resolved account id.
Fix: the tag-path ResourceNotFoundException message in the ListTagsForResource handler (it must use the tag family's fixed message, not the workload-store message).
Traces: wellarchitected-workload-tagging-envelopes#task#7.

Cause 4 of 4: TagResource ResourceNotFound message.
AWS: TagResource against an unknown workload ARN answers 404 with the same fixed message 'Resource was not found'.
Emulator: the same '[ResourceNotFound] No workload with ARN <workloadArn> was found in account <accountId>.' text.
Why/Fix: as cause 3, in the TagResource handler.
Traces: wellarchitected-workload-tagging-envelopes#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1, 4 mismatch clusters, all service=wellarchitected, fixture wellarchitected-workload-tagging-envelopes.

Cause 1 — task#3, TagResource: status expected 204, got 200.
Cause 2 — task#5, UntagResource: status expected 204, got 200.
Cause 3 — task#7, ListTagsForResource, 404/404: $.Message expected 'Resource was not found', got '[ResourceNotFound] No workload with ARN arn:aws:wellarchitected:us-west-2:000000000000:workload/00000000000000000000000000000000 was found in account 000000000000.'; $headers.x-amzn-errortype expected (shaped), got 'ResourceNotFoundException'.
Cause 4 — task#8, TagResource, 404/404: $.Message expected 'Resource was not found', got the same workload-store sentence; x-amzn-errortype got 'ResourceNotFoundException'.

Fixed text in causes 3-4 is the whole message 'Resource was not found' — there is no variable part; the ARN and account id the emulator interpolates are not part of AWS's message for this error on the tag paths.

Smithy: TagResource POST /tags/{WorkloadArn}, output TagResourceOutput with no members, errors InternalServerException(500)/ResourceNotFoundException(404); UntagResource DELETE /tags/{WorkloadArn} with TagKeys httpQuery 'tagKeys', output with no members; ListTagsForResource GET /tags/{WorkloadArn} -> {Tags}, errors ResourceNotFoundException(404).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 and look at fixture wellarchitected-workload-tagging-envelopes.
3. Create a workload, then POST /tags/{workloadArn} with a tag map and observe HTTP 200 where 204 is expected (cause 1).
4. DELETE /tags/{workloadArn}?tagKeys=<key> and observe 200 where 204 is expected (cause 2).
5. GET /tags/{arn} for a workload ARN that does not exist and read $.Message (cause 3).
6. POST /tags/{arn} for that same non-existent ARN and read $.Message (cause 4).
