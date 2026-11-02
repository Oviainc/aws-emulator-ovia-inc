Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-capability-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-capability-error-paths b2bi-partnership-error-paths b2bi-profile-lifecycle b2bi-tagging-workflow b2bi-test-parsing-missing-file-error b2bi-transformer-job-error-paths
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|CreateCapability|4xx->4xx|$.Message
Issue-Key: b2bi|CreateCapability|4xx->4xx|$.__type
Issue-Key: b2bi|CreateCapability|4xx->4xx|status
Issue-Key: b2bi|GetCapability|4xx->4xx|status
Issue-Key: b2bi|DeleteCapability|4xx->4xx|status
Issue-Key: b2bi|CreatePartnership|4xx->4xx|status
Issue-Key: b2bi|GetPartnership|4xx->4xx|status
Issue-Key: b2bi|DeletePartnership|4xx->4xx|status
Issue-Key: b2bi|GetProfile|4xx->4xx|$
Issue-Key: b2bi|GetProfile|4xx->4xx|status
Issue-Key: b2bi|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: b2bi|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: b2bi|ListTagsForResource|4xx->4xx|status
Issue-Key: b2bi|TestParsing|4xx->4xx|$.message
Issue-Key: b2bi|TestParsing|4xx->4xx|status
Issue-Key: b2bi|GetTransformerJob|4xx->4xx|$.message
Issue-Key: b2bi|GetTransformerJob|4xx->4xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# b2bi client errors answer 404 with an unqualified __type instead of 400 and the modeled exception

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapability

## Description
Ten net-new causes, one shared fault: every modeled client-error refusal b2bi raises leaves the emulator with HTTP 404 and an error body whose __type is the bare shape name, where AWS answers HTTP 400 and a fully-qualified shape id; several also pick the wrong exception class and the wrong message member.

Shared AWS behavior (Smithy: services/service-com-amazonaws-b2bi-b2bi, protocol awsJson1_0; errors ResourceNotFoundException (errorKind client, member `message`, @length 10..1024) and the service-common ValidationException (errorKind client, member `Message`, @length 10..1024)). ResourceNotFoundException carries @httpError 404 in the model, but the recorded AWS wire responses answer **400** for BOTH ResourceNotFoundException and ValidationException on this service, so 400 is the requirement. The recorded `__type` is the fully-qualified shape id in the service's internal namespace — `com.amazonaws.visby#ResourceNotFoundException` / `com.amazonaws.visby#ValidationException` — not the bare name. The message member differs per class: ValidationException spells it `Message`, ResourceNotFoundException spells it `message`.

Shared emulator behavior: HTTP 404 on every one of these refusals, and where a body is compared, `__type` is the bare `ResourceNotFoundException` with no namespace. In four causes the emulator additionally raises ResourceNotFoundException where AWS raises ValidationException, so the `Message` member is absent entirely.

Why this matters: an SDK or CLI client keys retry/handling off the HTTP status and the error shape id. A 404 where AWS sends 400 makes generic HTTP error handling classify a modeled, non-retryable validation refusal as a missing endpoint/resource at the transport layer; code that branches on ValidationException vs ResourceNotFoundException takes the wrong branch, and code reading `Message` finds nothing because the emulator only ever writes `message`.

Fix: the service's error serializer (status mapping + __type rendering for awsJson1_0) for the shared half, and the per-operation validation/lookup ordering in each named operation's handler for the class and wording half.

Cause 1 of 10: CreateCapability with buckets the caller does not own. AWS: 400, __type com.amazonaws.visby#ValidationException, Message "Customer does not own input and/or output buckets." — fixed text, no variable parts. Emulator: 404, __type ResourceNotFoundException, no Message member. Fix: CreateCapability's input validation must test bucket ownership and raise ValidationException before resolving references. Trace: b2bi-capability-error-paths#task#1.

Cause 2 of 10: GetCapability for an unknown capabilityId. AWS: 400 (body matched). Emulator: 404. Fix: error-status mapping. Trace: b2bi-capability-error-paths#task#2.

Cause 3 of 10: DeleteCapability for an unknown capabilityId. AWS: 400 (body matched). Emulator: 404. Fix: error-status mapping. Trace: b2bi-capability-error-paths#task#4.

Cause 4 of 10: CreatePartnership naming a profile/capability that does not exist. AWS: 400 (body matched). Emulator: 404. Fix: error-status mapping. Trace: b2bi-partnership-error-paths#task#2.

Cause 5 of 10: GetPartnership for an unknown partnershipId. AWS: 400 (body matched). Emulator: 404. Fix: error-status mapping. Trace: b2bi-partnership-error-paths#task#3.

Cause 6 of 10: DeletePartnership for an unknown partnershipId. AWS: 400 (body matched). Emulator: 404. Fix: error-status mapping. Trace: b2bi-partnership-error-paths#task#5.

Cause 7 of 10: GetProfile for a profile that was deleted. AWS: 400, __type com.amazonaws.visby#ResourceNotFoundException, message "Profile ID <profileId> not found." — fixed text "Profile ID " + the requested profileId from the request + " not found."; the emulator's wording already matches. Emulator: 404 and bare __type "ResourceNotFoundException", so the fixture's polling loop never settles. Fix: error-status mapping and __type qualification. Trace: b2bi-profile-lifecycle#teardown#7.

Cause 8 of 10: ListTagsForResource with a resourceArn that is not a well-formed b2bi ARN. AWS: 400, __type com.amazonaws.visby#ValidationException, Message "Invalid input ARN!" — fixed text, no variable parts (the offending ARN is NOT echoed). Emulator: 404, __type ResourceNotFoundException, no Message. Fix: ListTagsForResource must validate ARN syntax and raise ValidationException before it looks the resource up. Trace: b2bi-tagging-workflow#task#8.

Cause 9 of 10: TestParsing whose inputFile names an S3 key that does not exist. AWS: 400 with message "The S3 object s3://<bucket>/<key> does not exist" — fixed text "The S3 object ", the s3:// URI built from the request's bucketName/key, then " does not exist". Emulator: 404 with "The input file s3://<bucket>/<key> could not be read: NoSuchKey", i.e. its own wording plus the leaked dependency error code. Fix: TestParsing's handler wording and status. Trace: b2bi-test-parsing-missing-file-error#task#2.

Cause 10 of 10: GetTransformerJob for an unknown transformerJobId. AWS: 400 with message "Found no transformer job <transformerJobId> for customer <accountId>" — fixed text "Found no transformer job " + the requested transformerJobId + " for customer " + the calling account id. Emulator: 404 with "Transformer job with transformerJobId <id> not found for transformerId <id>", which words the refusal around the transformerId instead of the customer. Fix: GetTransformerJob's not-found wording and status. Trace: b2bi-transformer-job-error-paths#task#4.

## Evidence
Replay of simparity-wire-aws-v1 (10 fixtures, 56 steps, score 25%). Per cluster: b2bi-capability-error-paths#task#1 CreateCapability expected 400 got 404, $.__type expected 'com.amazonaws.visby#ValidationException' got 'ResourceNotFoundException', $.Message expected 'Customer does not own input and/or output buckets.' got None; #task#2 GetCapability expected 400 got 404; #task#4 DeleteCapability expected 400 got 404; b2bi-partnership-error-paths#task#2 CreatePartnership expected 400 got 404; #task#3 GetPartnership expected 400 got 404; #task#5 DeletePartnership expected 400 got 404; b2bi-profile-lifecycle#teardown#7 GetProfile expected 400 got 404 and poll gave {'__type': 'ResourceNotFoundException', ...} against expected {'__type': 'com.amazonaws.visby#ResourceNotFoundException', ...}; b2bi-tagging-workflow#task#8 ListTagsForResource expected 400 got 404, $.__type expected 'com.amazonaws.visby#ValidationException' got 'ResourceNotFoundException', $.Message expected 'Invalid input ARN!' got None; b2bi-test-parsing-missing-file-error#task#2 TestParsing expected 400 got 404, $.message expected 'The S3 object s3://<bucket>/nonexistent/input.edi does not exist' got 'The input file s3://<bucket>/nonexistent/input.edi could not be read: NoSuchKey'; b2bi-transformer-job-error-paths#task#4 GetTransformerJob expected 400 got 404, $.message expected 'Found no transformer job <jobId> for customer <accountId>' got 'Transformer job with transformerJobId <jobId> not found for transformerId <transformerId>'.

## Steps to Reproduce
1. Boot the emulator from mainline. 2. Replay the wire corpus simparity-wire-aws-v1. 3. Replay the representative step b2bi-capability-error-paths#task#1 and each step listed in trace_ids. 4. Observe HTTP 404 and a bare/absent __type where the recording has HTTP 400 and the com.amazonaws.visby#-qualified exception (plus the ValidationException Message for causes 1 and 8).
