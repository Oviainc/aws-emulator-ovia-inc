Issue-Source: trace-sweep-divergence
Issue-Trace: states-resource-tagging-workflow
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: states-resource-tagging-workflow states-tagging-operations
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: states|ListTagsForResource|2xx->4xx|status
Issue-Key: states|TagResource|4xx->4xx|$.message
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# ListTagsForResource answers 404 for a live state machine and tag-limit rejection text diverges

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
The tagging surface diverges on both the read path and the tag-limit rejection. 2 causes.

Cause 1 of 2: ListTagsForResource on an existing, fully untagged state machine answers 404 instead of 200 with an empty list.
AWS: Smithy: ListTagsForResource input resourceArn (required), output tags -> TagList, errors InvalidArn (400) and ResourceNotFound (404, 'Could not find the referenced resource'). A resource that exists but has no tags is not a not-found: the recording answers HTTP 200 with tags [].
Emulator: HTTP 404 with no tags member, i.e. the resource is treated as absent once its tags are gone.
Why: a client reading tags after an UntagResource that removed the last tag sees the state machine as deleted; 404 is also not retry-equivalent to an empty 200, so tag-reconciliation loops break.
Fix: the ListTagsForResource handler / tag state in the sfn service - resolve the resource independently of whether a tag entry exists, and return an empty TagList.
Traces: states-resource-tagging-workflow#task#13.

Cause 2 of 2: TooManyTags message is rebuilt from request values instead of the fixed service text.
AWS: Smithy: TagResource errors include TooManyTags (client, HTTP 400), "You've exceeded the number of tags allowed for a resource". The recorded message is the fixed string 'Too many tags provided.' - it carries no request values.
Emulator: 400 TooManyTags with 'a Step Functions resource accepts at most 50 tags, got 60 in one request' - synthesised from the request.
Why: AWS's text is a constant here, so a client asserting on it diverges even though status and error code match; the emulator's text also varies with the request where AWS's does not.
Fix: the tag-limit check in the TagResource handler - return the fixed service text.
Traces: states-resource-tagging-workflow#task#14 (also states-tagging-operations#task#11).

## Evidence
- states-resource-tagging-workflow#task#13: expected status 200 with $.tags=[]; actual status 404 with tags absent.
- states-resource-tagging-workflow#task#14 (400 both sides, TooManyTags both sides): expected $.message='Too many tags provided.'; actual 'a Step Functions resource accepts at most 50 tags, got 60 in one request' (2 affected steps).

## Steps to Reproduce
1. Replay fixture states-resource-tagging-workflow; at step task#13 ListTagsForResource is called on the state machine created earlier in the fixture after its tags were removed -> emulator answers 404 instead of 200 with tags [].
2. At step task#14 TagResource is called with 60 tags -> status 400 TooManyTags with a request-derived message instead of 'Too many tags provided.'
