Issue-Source: trace-sweep-divergence
Issue-Trace: cfdeep-request-local-limits
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfdeep-request-local-limits cfval-custom-error-response-code-invalid cfval-custom-error-response-error-code-invalid cfval-custom-error-response-page-path-relative cfval-default-root-object-invalid-chars cfval-geo-invalid-location-code cfval-geo-whitelist-empty cfval-query-string-cache-keys-without-query-string cfval-trusted-key-group-does-not-exist cfval-ttl-order-min-exceeds-max cloudfront-distribution-validation-and-filtered-listing cloudfront-cfode-rewrite-fix-function-state-and-etag-chain
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|4xx->2xx|$headers.location
Issue-Key: cloudfront|UpdateDistribution|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|UpdateDistribution|4xx->2xx|status
Issue-Related: 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# CloudFront CreateDistribution and UpdateDistribution accept distribution configurations AWS refuses

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Three causes, one shared fault: the distribution write path performs no semantic validation of DistributionConfig, so requests AWS refuses with a modeled 4xx error are accepted and a distribution is created or updated instead.

Cause 1 of 3: CreateDistribution answers 201 for configurations AWS refuses with a modeled 400.
AWS: 400 with the modeled refusal for the offending config member, e.g. for too many origin custom headers `Code: TooManyOriginCustomHeaders`, `Type: Sender`, `Message: 'Your request contains more OriginCustomHeaders than allowed per distribution.'` and NO Location header (Smithy: CreateDistribution errors include TooManyOriginCustomHeaders@400, InvalidResponseCode@400, InvalidErrorCode@400, InvalidRelativePath@400, InvalidDefaultRootObject@400, InvalidLocationCode@400, InvalidGeoRestrictionParameter@400, InvalidQueryStringParameters@400, TrustedKeyGroupDoesNotExist@400, InvalidTTLOrder@400; TooManyOriginCustomHeaders doc: 'Your request contains too many origin custom headers'). The message's fixed text is the sentence above; the variable parts of the sibling refusals are the member path and the offending value taken from the request.
Emulator: 201 Created with a full Distribution body and a Location header - no ErrorResponse at all. The cluster's affected steps cover nine further invalid configurations in the same shape (custom error response code, custom error response error code, relative custom-error page path, invalid DefaultRootObject characters, invalid geo location code, empty geo whitelist, query-string cache keys without query-string forwarding, a trusted key group that does not exist, MinTTL above MaxTTL) - Hypothesis: one unvalidated code path, since all of them answer 201 identically.
Why: a client's negative tests pass vacuously, invalid distributions enter the emulator's state and are then read back as valid, and code that relies on AWS rejecting a malformed config ships the config to production unchecked.
Fix: CreateDistribution's input validation (DistributionConfig semantic checks) before the handler mutates state.
Traces: cfdeep-request-local-limits#task#5 (10 affected steps: cfval-custom-error-response-code-invalid#task#1, cfval-custom-error-response-error-code-invalid#task#1, cfval-custom-error-response-page-path-relative#task#1, cfval-default-root-object-invalid-chars#task#1, cfval-geo-invalid-location-code#task#1, cfval-geo-whitelist-empty#task#1, cfval-query-string-cache-keys-without-query-string#task#1, cfval-trusted-key-group-does-not-exist#task#1, cfval-ttl-order-min-exceeds-max#task#1).

Cause 2 of 3: CreateDistribution answers 201 where a cache behavior targets an origin that does not exist.
AWS: 404 with `Code: NoSuchOrigin`, `Type: Sender`, `Message: 'One or more of your cache behavior target origins or origin groups do not exist.'` and no Location header (Smithy: CreateDistribution error NoSuchOrigin, httpError 404, doc 'No origin exists with the specified Origin Id'). The message is fixed text with no variable part.
Emulator: 201 Created with a Location header.
Why: a distribution whose TargetOriginId resolves to nothing is accepted and then served/readback as if valid, so the referential check AWS guarantees at create time never happens.
Fix: CreateDistribution's validation - resolve every cache behavior's TargetOriginId against the config's Origins/OriginGroups before create.
Traces: cloudfront-distribution-validation-and-filtered-listing#task#2.

Cause 3 of 3: UpdateDistribution answers 200 for a function association naming an unpublished function.
AWS: 400 with `Code: InvalidArgument`, `Type: Sender`, `Message: 'The parameter FunctionAssociationArn is invalid; <function name or ARN from the request> was not found or is not published.'` (Smithy: UpdateDistribution error InvalidArgument, httpError 400, doc 'An argument is invalid'). Fixed text is 'The parameter FunctionAssociationArn is invalid; ' and ' was not found or is not published.'; the variable part is the function identifier taken from the request.
Emulator: 200 OK - the association is accepted and stored.
Why: a distribution ends up referencing a function that was never published, so the emulator's state diverges from any AWS account's and the client's negative test passes vacuously. The open issue 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md mentions this behavior in prose but does not carry this cluster's replay key.
Fix: UpdateDistribution's validation - resolve each FunctionAssociations item's ARN and require a published LIVE function.
Traces: cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#5.

Related open issues that do NOT carry these replay keys: 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md and 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). Three unowned mismatch clusters:
1. CreateDistribution - cfdeep-request-local-limits#task#5: status expected 400, got 201; $.ErrorResponse missing (expected {'Error': {'Type': 'Sender', 'Code': 'TooManyOriginCustomHeaders', 'Message': 'Your request contains more OriginCustomHeaders than allowed per distribution.'}}); $headers.location expected None, got 'https://cloudfront.amazonaws.com/2020-05-31/distribution/E7IYCGPKTANSF3'. 10 affected steps, each a different invalid DistributionConfig.
2. CreateDistribution - cloudfront-distribution-validation-and-filtered-listing#task#2: status expected 404, got 201; $.ErrorResponse missing (expected Code 'NoSuchOrigin', Message 'One or more of your cache behavior target origins or origin groups do not exist.'); a Location header was emitted where the recording has none.
3. UpdateDistribution - cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#5: status expected 400, got 200; $.ErrorResponse missing (expected Code 'InvalidArgument', Message 'The parameter FunctionAssociationArn is invalid; <function> was not found or is not published.').
No stub_path and no owned_by on any of the three.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, replay the wire corpus (simparity-wire-aws-v1).
2. Representative step: fixture cfdeep-request-local-limits, step task#5 - CreateDistribution with more origin custom headers than a distribution allows; the emulator answers 201 instead of 400 TooManyOriginCustomHeaders.
3. Replay any cfval-* fixture's task#1 in the same cluster (e.g. cfval-ttl-order-min-exceeds-max, cfval-geo-whitelist-empty) - each invalid config is likewise accepted 201.
4. cloudfront-distribution-validation-and-filtered-listing#task#2 - CreateDistribution whose default cache behavior targets a nonexistent origin id: 201 instead of 404 NoSuchOrigin.
5. cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#5 - UpdateDistribution attaching an unpublished function: 200 instead of 400 InvalidArgument.
