Issue-Source: trace-sweep-divergence
Issue-Trace: cfdeep-request-local-limits
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfdeep-request-local-limits cfval-custom-error-response-code-invalid cfval-custom-error-response-error-code-invalid cfval-custom-error-response-page-path-relative cfval-default-root-object-invalid-chars cfval-geo-invalid-location-code cfval-geo-whitelist-empty cfval-query-string-cache-keys-without-query-string cfval-trusted-key-group-does-not-exist cfval-ttl-order-min-exceeds-max cloudfront-distribution-validation-and-filtered-listing
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateDistribution|4xx->2xx|status
Issue-Commit: b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10

# CloudFront CreateDistribution accepts configurations AWS rejects

> _Note: this issue was identified against the code base at commit `b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
CreateDistribution performs almost no semantic validation of DistributionConfig: ten recorded rejection probes all come back 201 Created with a live distribution, instead of the modeled 400/404 refusal.

Cause 1 of 2: per-distribution config validation is missing, so invalid configs are created (expected 400, got 201).
AWS: Smithy CreateDistribution (POST /2020-05-31/distribution, success 201) models a dedicated 400 client error for each of these rejections — TooManyOriginCustomHeaders, InvalidResponseCode / InvalidErrorCode / InvalidRelativePath (CustomErrorResponses), InvalidDefaultRootObject, InvalidLocationCode / InvalidGeoRestrictionParameter, InvalidQueryStringParameters (QueryStringCacheKeys without QueryString forwarding), TrustedKeyGroupDoesNotExist, InvalidTTLOrder. The recorded responses are 400 with the corresponding Code in the ErrorResponse envelope, e.g. Code 'TooManyOriginCustomHeaders', Type 'Sender', Message 'Your request contains more OriginCustomHeaders than allowed per distribution.'
Emulator: 201 Created with no ErrorResponse at all ('$.ErrorResponse: missing ... got None'); the single cluster covers 10 steps across 10 fixtures, one per rejection probe.
Why: a client that relies on CloudFront refusing an invalid configuration gets a distribution that AWS would never have created, so the defect only surfaces in production; negative-path tests of all ten validations pass against the emulator and fail against AWS.
Fix: validation in the CreateDistribution handler of the cloudfront service crate — custom-header count limit, CustomErrorResponse ResponseCode/ErrorCode/ResponsePagePath checks, DefaultRootObject character rules, GeoRestriction location codes and non-empty item list, QueryStringCacheKeys-requires-QueryString, trusted key-group existence, and MinTTL<=DefaultTTL<=MaxTTL ordering — each raising its modeled error.

Cause 2 of 2: a cache behavior whose TargetOriginId names no origin is accepted (expected 404 NoSuchOrigin, got 201).
AWS: Smithy CreateDistribution models NoSuchOrigin with httpError 404 ('No origin exists with the specified Origin Id.'); the recording is 404 with Code 'NoSuchOrigin', Type 'Sender', Message 'One or more of your cache behavior target origins or origin groups do not exist.'
Emulator: 201 Created, no error body.
Why: the distribution is created with a dangling TargetOriginId — a config AWS treats as unresolvable — so cross-reference validation a client depends on never happens.
Fix: the CreateDistribution handler must resolve every DefaultCacheBehavior/CacheBehavior TargetOriginId against Origins and OriginGroups and return NoSuchOrigin (404) when it does not resolve.

Traces: cause 1 — cfdeep-request-local-limits#task#5 plus the other nine steps in the same cluster (cfval-custom-error-response-code-invalid#task#1, cfval-custom-error-response-error-code-invalid#task#1, cfval-custom-error-response-page-path-relative#task#1, cfval-default-root-object-invalid-chars#task#1, cfval-geo-invalid-location-code#task#1, cfval-geo-whitelist-empty#task#1, cfval-query-string-cache-keys-without-query-string#task#1, cfval-trusted-key-group-does-not-exist#task#1, cfval-ttl-order-min-exceeds-max#task#1); cause 2 — cloudfront-distribution-validation-and-filtered-listing#task#2.

## Evidence
Cluster service=cloudfront|CreateDistribution|status=400->201 (representative cfdeep-request-local-limits#task#5, affected_count 10): 'status: expected 400, got 201' and '$.ErrorResponse: missing (expected {Error: {Type: Sender, Code: TooManyOriginCustomHeaders, Message: "Your request contains more OriginCustomHeaders than allowed per distribution."}, RequestId: <shaped-not-asserted>}, got None)'. Cluster service=cloudfront|CreateDistribution|status=404->201 (cloudfront-distribution-validation-and-filtered-listing#task#2): 'status: expected 404, got 201' and '$.ErrorResponse: missing (expected {Error: {Type: Sender, Code: NoSuchOrigin, Message: "One or more of your cache behavior target origins or origin groups do not exist."}}, got None)'.

## Steps to Reproduce
1. Replay fixture cfdeep-request-local-limits step task#5: POST /2020-05-31/distribution with more OriginCustomHeaders than the per-distribution limit — expect 400 TooManyOriginCustomHeaders, observe 201. 2. Replay any cfval-*#task#1 step (invalid custom-error ResponseCode/ErrorCode, relative ResponsePagePath, invalid DefaultRootObject characters, invalid geo location code, empty geo whitelist, QueryStringCacheKeys without QueryString, nonexistent trusted key group, MinTTL>MaxTTL) — each expects its modeled 400, all return 201. 3. Replay cloudfront-distribution-validation-and-filtered-listing#task#2: a cache behavior with a TargetOriginId that matches no Origin — expect 404 NoSuchOrigin, observe 201.
