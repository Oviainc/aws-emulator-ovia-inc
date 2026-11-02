Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-name-and-arn-validation-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-name-and-arn-validation-errors s3tables-namespace-error-paths s3tables-tag-ops-malformed-arn
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|CreateTableBucket|4xx->4xx|$.message
Issue-Key: s3tables|CreateTableBucket|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|CreateNamespace|4xx->4xx|$.message
Issue-Key: s3tables|CreateNamespace|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|CreateTable|4xx->4xx|$.message
Issue-Key: s3tables|CreateTable|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|GetTableBucket|4xx->4xx|$.message
Issue-Key: s3tables|GetTableBucket|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|ListTagsForResource|4xx->4xx|$.message
Issue-Key: s3tables|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|TagResource|4xx->4xx|$.message
Issue-Key: s3tables|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|UntagResource|4xx->4xx|$.message
Issue-Key: s3tables|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 8715ba3f180c27028dee3267eb478eb6947c4fd2

# s3tables rejects invalid input with unmodeled ValidationException instead of BadRequestException

> _Note: this issue was identified against the code base at commit `8715ba3f180c27028dee3267eb478eb6947c4fd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTableBucket

## Description
Seven causes with one shared fault: input that violates a modeled constraint is refused by a GENERIC smithy-style validator rather than by s3tables' own BadRequestException, so the status is right (400) but the error type and the message are not. com.amazonaws.s3tables declares only AccessDeniedException, BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500), NotFoundException(404) and TooManyRequestsException(429) — there is NO ValidationException shape anywhere in the service model, and the recorded AWS responses carry x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/'. In every cause the AWS message is FIXED text with no variable part; the emulator instead emits the generated template "<n> validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", whose variable parts come from the request. Every cause must be closed before this issue is submitted.

Cause 1 of 7: CreateTableBucket with an invalid bucket name (fixture s3tables-name-and-arn-validation-errors, step task#2).
AWS: 400 BadRequestException, x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/', message exactly "The specified bucket name isn't valid. Specify a different bucket name, and then try again." (all fixed text). Smithy: CreateTableBucketRequest.name @length(3,63) @pattern(^[0-9a-z-]*$), error BadRequestException.
Emulator: 400 with x-amzn-errortype 'ValidationException' and message "1 validation error detected: Value 'Sim2real-probe-s3t-5b9478fd' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z-]*$".
Why: an SDK maps the unmodeled ValidationException to an unknown-error type, so `catch BadRequestException` does not fire and retry/validation logic written against AWS takes the wrong branch; error-message assertions also fail.
Fix: the CreateTableBucket handler's input validation — raise the service's BadRequestException with AWS's fixed message instead of the framework's generic constraint reporter.
Traces: s3tables-name-and-arn-validation-errors#task#2 (cluster also covers #task#3).

Cause 2 of 7: CreateNamespace with a multipart namespace (same fixture, step task#4).
AWS: 400 BadRequestException with message exactly "Multipart namespaces are not supported." (fixed text). Smithy: CreateNamespaceRequest.namespace is a NamespaceList with @length(min:1,max:1).
Emulator: 400 ValidationException "1 validation error detected: Value '2 items' at 'namespace' failed to satisfy constraint: Member must have length less than or equal to 1".
Why/Fix: as cause 1, in CreateNamespace's validation.
Traces: s3tables-name-and-arn-validation-errors#task#4 (cluster also covers #task#5 and s3tables-namespace-error-paths#task#4).

Cause 3 of 7: CreateTable with an invalid table name (same fixture, step task#7).
AWS: 400 BadRequestException with message exactly "The specified table name is not valid." (fixed text). Smithy: CreateTableRequest.name @pattern(^[0-9a-z_]*$).
Emulator: 400 ValidationException "1 validation error detected: Value 'Sim2real_tbl_5b9478fd' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z_]*$".
Why/Fix: as cause 1, in CreateTable's validation.
Traces: s3tables-name-and-arn-validation-errors#task#7 (cluster also covers #task#8).

Cause 4 of 7: GetTableBucket with a malformed table-bucket ARN (same fixture, step task#9).
AWS: 400 BadRequestException with message exactly "The specified table bucket ARN is not valid." (fixed text). Smithy: GetTableBucketRequest.tableBucketARN @pattern(^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})$).
Emulator: 400 ValidationException quoting the ARN and the regex.
Why/Fix: as cause 1, in GetTableBucket's ARN validation.
Traces: s3tables-name-and-arn-validation-errors#task#9.

Cause 5 of 7: ListTagsForResource with a malformed resourceArn (fixture s3tables-tag-ops-malformed-arn, step task#1).
AWS: 400 BadRequestException, x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/', message exactly "The specified resourceArn is not valid." (fixed text). Smithy: ListTagsForResourceRequest.resourceArn @pattern(^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/.+$).
Emulator: 400 ValidationException "1 validation error detected: Value 'not-a-valid-s3tables-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ...".
Why/Fix: as cause 1, in the tag operations' ARN validation.
Traces: s3tables-tag-ops-malformed-arn#task#1.

Cause 6 of 7: TagResource with a malformed resourceArn (same fixture, step task#2).
AWS/Emulator/Why/Fix: identical to cause 5 on TagResource.
Traces: s3tables-tag-ops-malformed-arn#task#2.

Cause 7 of 7: UntagResource with a malformed resourceArn (same fixture, step task#3).
AWS/Emulator/Why/Fix: identical to cause 5 on UntagResource.
Traces: s3tables-tag-ops-malformed-arn#task#3.

## Evidence
Replay of simparity-wire-aws-v1 (7 clusters, 11 steps; status 400 expected and returned in all of them, so only the error type and message diverge):
- CreateTableBucket task#2: expected $.message "The specified bucket name isn't valid. Specify a different bucket name, and then try again.", got "1 validation error detected: Value 'Sim2real-probe-s3t-5b9478fd' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z-]*$"; x-amzn-errortype 'ValidationException'.
- CreateNamespace task#4: expected 'Multipart namespaces are not supported.', got "1 validation error detected: Value '2 items' at 'namespace' failed to satisfy constraint: Member must have length less than or equal to 1".
- CreateTable task#7: expected 'The specified table name is not valid.', got the generic template.
- GetTableBucket task#9: expected 'The specified table bucket ARN is not valid.', got the generic template.
- ListTagsForResource/TagResource/UntagResource (s3tables-tag-ops-malformed-arn task#1/#2/#3): expected 'The specified resourceArn is not valid.' with x-amzn-errortype 'BadRequestException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/', got the generic template with x-amzn-errortype 'ValidationException'.
Model check (smithy_query, services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json): no ValidationException shape exists; BadRequestException is the modeled 400 for all of these operations.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Fixture s3tables-name-and-arn-validation-errors, steps task#2 (CreateTableBucket bad name), task#4 (CreateNamespace multipart), task#7 (CreateTable bad name), task#9 (GetTableBucket bad ARN) — read the response's x-amzn-errortype header and $.message.
3. Fixture s3tables-tag-ops-malformed-arn, steps task#1/#2/#3 (ListTagsForResource/TagResource/UntagResource with resourceArn 'not-a-valid-s3tables-arn') — same two fields.
