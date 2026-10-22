Issue-Source: trace-sweep-divergence
Issue-Trace: evs-tagging-missing-resource
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: evs-tagging-missing-resource
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs|TagResource|4xx->4xx|$.message
Issue-Key: evs|TagResource|4xx->4xx|$.resourceArn
Issue-Key: evs|TagResource|4xx->4xx|status
Issue-Key: evs|ListTagsForResource|4xx->4xx|$.message
Issue-Key: evs|ListTagsForResource|4xx->4xx|$.resourceArn
Issue-Key: evs|ListTagsForResource|4xx->4xx|status
Issue-Key: evs|UntagResource|4xx->4xx|$.message
Issue-Key: evs|UntagResource|4xx->4xx|$.resourceArn
Issue-Key: evs|UntagResource|4xx->4xx|status
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Tagging operations on a missing resource return 404 and omit resourceArn

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource, ListTagsForResource, UntagResource

## Description
The three evs tagging operations answer a missing resource with the environment-lookup not-found error: wrong wire status (404 for the recorded 400), the environment-lookup message instead of the tagging not-found message, and no resourceArn member in the body. One shared defect in the tagging handlers' not-found path, observed on three operations.

Shared AWS basis: Smithy binds ResourceNotFoundException to all three tagging operations (TagResource errors = [ResourceNotFoundException, ServiceQuotaExceededException, TagPolicyException, TooManyTagsException]; ListTagsForResource errors = [ResourceNotFoundException]; UntagResource errors = [ResourceNotFoundException, TagPolicyException]), and each input takes resourceArn (required, pattern ^arn:aws:evs:[a-z]{2}-[a-z]+-[0-9]:[0-9]{12}:environment/[a-zA-Z0-9_-]+$). ResourceNotFoundException carries @httpError(404) in the model, but the RECORDED wire status is 400, so 400 is the requirement for this awsJson1_0 service. Recorded behavior only (the model's ResourceNotFoundException declares message/resourceId/resourceType and no resourceArn): for the tagging surface the recorded body carries a resourceArn member echoing the resourceArn from the request, and a fixed, resource-agnostic message 'Resource cannot be found.' — i.e. the tagging not-found error is NOT the environment-lookup error. The error code/__type matched in every step below; status, message and the resourceArn member diverge.

Cause 1 of 3: TagResource against an ARN for an environment that does not exist.
AWS: HTTP 400, ResourceNotFoundException, message 'Resource cannot be found.', resourceArn = the request's resourceArn.
Emulator: HTTP 404, message 'Environment <arn> does not exist.', no resourceArn member.

Cause 2 of 3: ListTagsForResource against the same non-existent ARN.
AWS: HTTP 400, ResourceNotFoundException, message 'Resource cannot be found.', resourceArn echoed.
Emulator: HTTP 404, message 'Environment <arn> does not exist.', no resourceArn member.

Cause 3 of 3: UntagResource against the same non-existent ARN.
AWS: HTTP 400, ResourceNotFoundException, message 'Resource cannot be found.', resourceArn echoed.
Emulator: HTTP 404, message 'Environment <arn> does not exist.', no resourceArn member.

Why: two separate client breakages. The 404 is classified by generic HTTP middleware and retry layers as a routing/endpoint failure rather than the modelled client error, so correct handling of AWS's 400 ResourceNotFoundException does not trigger. And the missing resourceArn member means a client that batches tagging calls and reads resourceArn off the error to learn WHICH resource was missing gets nothing to correlate on; the recorded message is deliberately resource-agnostic, so the ARN is only available in that member.

Fix: the not-found path of the TagResource/ListTagsForResource/UntagResource handlers plus the evs error serializer — raise the tagging-specific not-found error (fixed message, resourceArn populated from the request) instead of delegating to the environment-lookup error, and serialize it with the status the service returns on the wire (400) rather than the modelled 404.

Traces: evs-tagging-missing-resource#task#1 (TagResource), evs-tagging-missing-resource#task#2 (ListTagsForResource), evs-tagging-missing-resource#task#3 (UntagResource).

## Evidence
Fixture evs-tagging-missing-resource.
Step task#1 (TagResource): expected status 400, got 404; $.message expected 'Resource cannot be found.', got 'Environment arn:aws:evs:us-west-2:000000000000:environment/env-0000000000 does not exist.'; $.resourceArn expected 'arn:aws:evs:us-west-2:000000000000:environment/env-0000000000', got absent.
Step task#2 (ListTagsForResource): identical status, message and missing-resourceArn divergence.
Step task#3 (UntagResource): identical status, message and missing-resourceArn divergence.
Error code/__type matched in all three.

## Steps to Reproduce
1. Boot the emulator and replay fixture evs-tagging-missing-resource.
2. Steps task#1/task#2/task#3 call TagResource, ListTagsForResource and UntagResource with resourceArn = arn:aws:evs:us-west-2:000000000000:environment/env-0000000000, an environment that was never created.
3. Each response is HTTP 404 with the environment-lookup message and no resourceArn member; the recording shows HTTP 400, message 'Resource cannot be found.' and resourceArn echoing the request.
