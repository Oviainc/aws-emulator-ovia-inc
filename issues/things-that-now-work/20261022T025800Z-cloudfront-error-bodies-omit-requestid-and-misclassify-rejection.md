Issue-Source: trace-sweep-divergence
Issue-Trace: cfval-create-distribution-empty-body
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfval-create-distribution-empty-body cfetag-distribution-etag-chain cfval-caller-reference-reuse-conflict cloudfront-distribution-not-found-error-paths cloudfront-invalidation-workflow cloudfront-distribution-not-found-errors cloudfront-distribution-validation-and-filtered-listing cloudfront-list-distributions-by-web-acl-id-invalid-max-items-rejected cloudfront-not-found-error-paths
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|CreateDistribution|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateDistribution|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudfront|DeleteDistribution|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteDistribution|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudfront|GetDistribution|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetDistribution|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudfront|GetInvalidation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetInvalidation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudfront|CreateInvalidation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateInvalidation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudfront|ListDistributions|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|ListDistributions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|ListDistributions|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudfront|ListDistributionsByWebACLId|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|ListDistributionsByWebACLId|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|ListDistributionsByWebACLId|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10

# CloudFront error bodies omit RequestId and misclassify rejection codes

> _Note: this issue was identified against the code base at commit `b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Every replayed CloudFront error path diverges in the error envelope itself, and two of them return the wrong error code. 11 clusters over 7 fixtures and 6 operations.

Cause 1 of 4: ErrorResponse never carries RequestId.
AWS: CloudFront is restXml; its error envelope is <ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>. Every recorded error response in the corpus carries a RequestId element (its value is a per-request id and is not a requirement; its presence is).
Emulator: $.ErrorResponse.RequestId is absent on every error cluster replayed (reported 'missing ... got None') — 409 DistributionAlreadyExists, 412 PreconditionFailed, 404 NoSuchDistribution, 400 MalformedXML/InvalidArgument/InconsistentQuantities alike.
Why: clients and support tooling that log or surface the request id for correlation get null; any client asserting the envelope's shape fails to parse the error.
Fix: the cloudfront crate's restXml error serializer must emit RequestId in the ErrorResponse envelope for all modeled errors.

Cause 2 of 4: non-numeric MaxItems is rejected with the wrong error code and a leaked internal parse message.
AWS: Smithy ListDistributions (GET /2020-05-31/distribution, MaxItems httpQuery integer) and ListDistributionsByWebACLId (GET /2020-05-31/distributionsByWebACLId/{WebACLId}) model exactly one 400 error for this, InvalidArgument ('An argument is invalid.'). The recordings show Code InvalidArgument with the request-derived message 'MaxItems must be a non-zero positive integer.'
Emulator: 400 with Code 'ValidationException' and Message 'invalid i32 `not-a-number`: invalid digit found in string' — a Rust integer-parse error surfaced to the wire; ValidationException is not a CloudFront error shape at all.
Why: clients branch on the error code; InvalidArgument is modeled and matchable, ValidationException is not, so SDK error matching falls through to a generic/unknown error and retry or input-correction logic never triggers.
Fix: query-parameter coercion in the ListDistributions / ListDistributionsByWebACLId handlers must map an unparsable MaxItems to the modeled InvalidArgument (400) instead of letting the parse error escape as ValidationException.

Cause 3 of 4: CreateDistribution with no DistributionConfig body returns the wrong error code.
AWS: Smithy CreateDistribution marks DistributionConfig required and httpPayload; the recording for an empty body is 400 with Code 'MalformedXML' (message: "1 validation error detected: Value null at 'distributionConfig' failed to satisfy constraint: Member must not be null" — the rule being: the missing member is the payload root).
Emulator: 400 with Code 'ValidationException' and the member path drilled one level too deep ('distributionConfig.callerReference').
Why: a client cannot distinguish a malformed/absent body from a field-level validation failure; the modeled MalformedXML code is never produced.
Fix: body/payload binding in the CreateDistribution handler must raise MalformedXML when the DistributionConfig payload is absent, before per-member validation runs.

Cause 4 of 4: not-found / conflict / precondition / quantity messages are rebuilt with request detail instead of AWS's fixed text.
AWS: these messages are fixed service text, documented on the error shapes themselves — NoSuchDistribution 'The specified distribution does not exist.', DistributionNotDisabled, PreconditionFailed 'The precondition in one or more of the request fields evaluated to false.' (wire text: "The request failed because it didn't meet the preconditions in one or more request-header fields."), InconsistentQuantities "The value of Quantity and the size of Items don't match." (wire text: 'The specified quantity of Origins (2) does not match the actual quantity supplied (1).'), DistributionAlreadyExists 'The caller reference you attempted to create the distribution with is associated with another distribution.' (wire text: 'The caller reference that you are using to create a distribution is associated with another distribution. Already exists: <distId>'). The status and code match; only the message text is rebuilt.
Emulator: appends or substitutes internal detail — 'The specified distribution does not exist: EDFDVBD6EXAMPLE', 'The caller reference <ref> is already associated with distribution <id>', "The If-Match version is not the distribution's current version (<etag>)", "The value of Quantity and the size of Items don't match: Origins states Quantity 2 but carries 1 items".
Why: clients and test suites that match on the documented message text (and users reading it) see a different sentence, and the emulator leaks internal identifiers into a message AWS keeps generic.
Fix: the cloudfront error constructors must use the modeled fixed message per error shape rather than formatting the offending id/version/quantity into it.

Traces: cfval-create-distribution-empty-body#task#1 (cause 3+1); cloudfront-distribution-not-found-errors#task#1 and cloudfront-list-distributions-by-web-acl-id-invalid-max-items-rejected#task#1 (cause 2+1); cfetag-distribution-etag-chain#task#2 (409), #task#4 (412), #task#8 (404), cloudfront-distribution-not-found-error-paths#task#2 (GetInvalidation 404), #task#6 (DeleteDistribution 404), #task#8 (CreateInvalidation 404), cloudfront-distribution-validation-and-filtered-listing#task#1 (400 InconsistentQuantities), cloudfront-not-found-error-paths#task#1 (GetDistribution 404) (cause 4+1).

## Evidence
cfval-create-distribution-empty-body#task#1: $.ErrorResponse.Error.Code expected 'MalformedXML', got 'ValidationException'; Message expected "1 validation error detected: Value null at 'distributionConfig' failed to satisfy constraint: Member must not be null", got "... at 'distributionConfig.callerReference' ..."; $.ErrorResponse.RequestId missing. cloudfront-distribution-not-found-errors#task#1 (ListDistributions) and cloudfront-list-distributions-by-web-acl-id-invalid-max-items-rejected#task#1: Code expected 'InvalidArgument', got 'ValidationException'; Message expected 'MaxItems must be a non-zero positive integer.', got 'invalid i32 `not-a-number`: invalid digit found in string'; RequestId missing. cfetag-distribution-etag-chain#task#2 (409): Message expected 'The caller reference that you are using to create a distribution is associated with another distribution. Already exists: E7IYCGPKTANSF3', got 'The caller reference sim2real-probe-cfetag-dist-0ccc9d75 is already associated with distribution E7IYCGPKTANSF3'; RequestId missing. #task#4 (412): expected "The request failed because it didn't meet the preconditions in one or more request-header fields.", got "The If-Match version is not the distribution's current version (EGZNI4PND7H2S3)". cloudfront-distribution-not-found-error-paths#task#2/#task#6/#task#8 and cloudfront-not-found-error-paths#task#1: expected 'The specified distribution does not exist.', got 'The specified distribution does not exist: EDFDVBD6EXAMPLE'. cloudfront-distribution-validation-and-filtered-listing#task#1: expected 'The specified quantity of Origins (2) does not match the actual quantity supplied (1).', got "The value of Quantity and the size of Items don't match: Origins states Quantity 2 but carries 1 items".

## Steps to Reproduce
1. GET /2020-05-31/distribution?MaxItems=not-a-number (fixture cloudfront-distribution-not-found-errors step task#1): expect 400 Code InvalidArgument, observe 400 Code ValidationException with a Rust parse message and no RequestId. 2. POST /2020-05-31/distribution with an empty body (cfval-create-distribution-empty-body#task#1): expect Code MalformedXML, observe ValidationException. 3. GET /2020-05-31/distribution/EDFDVBD6EXAMPLE (cloudfront-not-found-error-paths#task#1): 404 message has the id appended. 4. Any error response above: ErrorResponse has no RequestId element.
