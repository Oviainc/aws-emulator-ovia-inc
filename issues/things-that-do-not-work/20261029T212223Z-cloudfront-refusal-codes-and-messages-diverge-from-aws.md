Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-get-public-key-wrong-resource-type-id-rejected
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-get-public-key-wrong-resource-type-id-rejected cloudfront-public-key-and-key-group-lifecycle cloudfront-public-key-duplicate-and-notfound-errors cloudfront-public-key-error-paths cfetag-public-key-etag-chain cloudfront-anycast-ip-list-invalid-ip-count cloudfront-distribution-tenant-invalidation cloudfront-distribution-validation-and-filtered-listing cloudfront-field-level-encryption-errors cloudfront-field-level-encryption-invalid-reference-errors cloudfront-field-level-encryption-lifecycle cloudfront-list-distributions-by-key-group-invalid-argument-rejected cloudfront-realtime-log-config-invalid-sampling-rate cloudfront-resource-policy-missing-resource-error
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetPublicKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|UpdatePublicKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateAnycastIpList|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetInvalidationForDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|ListDistributionsByRealtimeLogConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|ListDistributionsByKeyGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateRealtimeLogConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|PutResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|PutResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|PutResourcePolicy|4xx->4xx|status
Issue-Key: cloudfront|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetResourcePolicy|4xx->4xx|status
Issue-Related: 20261029T212223Z-cloudfront-duplicate-caller-reference-409-messages-do-not.md
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront refusal codes and messages diverge from AWS across not-found and invalid-argument paths

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicKey

## Description
Ten causes, one shared fault: the emulator refuses these requests, but with its own error code and/or its own sentence rather than the modelled code and AWS's fixed message for the class. Nothing about the request is mis-handled - only the refusal is unfaithful, which is what a client's error handling keys on.

Fix for every cause: the error-construction site in that operation's validation/handler.

Cause 1 of 10: GetPublicKey for a non-existent public key. AWS: 404 `NoSuchPublicKey` with the fixed message `The specified CloudFront public key does not exist.` (Smithy: GetPublicKey errors = AccessDenied 403, NoSuchPublicKey 404). Emulator: 404 but "The specified public key doesn't exist." - a different sentence that omits 'CloudFront'. Traces cloudfront-get-public-key-wrong-resource-type-id-rejected#task#3 (also cloudfront-public-key-and-key-group-lifecycle#task#10, cloudfront-public-key-duplicate-and-notfound-errors#task#3, cloudfront-public-key-error-paths#task#2).
Cause 2 of 10: UpdatePublicKey attempting to change immutable fields. AWS: 400 with the fixed message `You cannot modify encoded material and name of a public key once created.` Emulator: 400 with "You can't change the value of a public key." - a shorter sentence that does not say which fields are immutable. Traces cfetag-public-key-etag-chain#task#3.
Cause 3 of 10: CreateAnycastIpList with an invalid IpCount. AWS: 400 with the message `The parameter Quantity Invalid quantity provided. The quantity of IPs in an AnycastIpList should be either 3 or 21.` - fixed text naming the parameter and the two legal values. Emulator: 'An argument is invalid: IpCount must be one of 3, 21, not 0' - its own template that echoes the submitted value. Traces cloudfront-anycast-ip-list-invalid-ip-count#task#1.
Cause 4 of 10: GetInvalidationForDistributionTenant for an absent invalidation. AWS: 404 with Error.Code `EntityNotFound`. Emulator: 404 with `NoSuchInvalidation` - the wrong code for the tenant-scoped operation. Traces cloudfront-distribution-tenant-invalidation#task#5.
Cause 5 of 10: ListDistributionsByRealtimeLogConfig naming an unknown config. AWS: 400 with the fixed message `The specified real-time log configuration does not exist.` Emulator: 'An argument is invalid: no real-time log configuration answers to the Name or ARN the request names'. Traces cloudfront-distribution-validation-and-filtered-listing#task#4.
Cause 6 of 10: CreateFieldLevelEncryptionProfile referencing an absent public key. AWS: 404 with the fixed message `The specified CloudFront public key does not exist.` Emulator: "The specified public key doesn't exist." Traces cloudfront-field-level-encryption-errors#task#3 (also cloudfront-field-level-encryption-invalid-reference-errors#task#1, cloudfront-field-level-encryption-lifecycle#task#2).
Cause 7 of 10: ListDistributionsByKeyGroup for an absent key group. AWS: 404 with the fixed message `The specified key group does not exist.` Emulator: "The specified resource doesn't exist. Update your request and try again." - a generic sentence that does not name the resource kind. Traces cloudfront-list-distributions-by-key-group-invalid-argument-rejected#task#4.
Cause 8 of 10: CreateRealtimeLogConfig with SamplingRate out of range. AWS: 400 with the fixed message `The parameter SamplingRate must be an integer greater than 0 and less than or equal to 100.` Emulator: 'An argument is invalid: SamplingRate must be a whole number between 1 and 100 (inclusive) but the request submits <value>' - its own template echoing the submitted value. Traces cloudfront-realtime-log-config-invalid-sampling-rate#task#1 (also task#2).
Cause 9 of 10: PutResourcePolicy for a resource that does not exist. AWS: 404 `EntityNotFound` with the fixed message `The specified resource doesn't exist. Update your request and try again.` Emulator: 400 `InvalidArgument` "The specified resource type isn't valid. Specify a VPC origin and try again." - wrong status, wrong code, wrong message: it rejects the resource TYPE instead of reporting the resource absent. Traces cloudfront-resource-policy-missing-resource-error#task#1.
Cause 10 of 10: GetResourcePolicy for a resource that does not exist. AWS: 404 `EntityNotFound` with the same fixed message as cause 9. Emulator: 400 `InvalidArgument` 'The ResourceArn parameter is required. Specify a valid ARN and try again.' - it claims the supplied ResourceArn is missing. Traces cloudfront-resource-policy-missing-resource-error#task#2.

Fix summary: causes 1/6 share the public-key not-found message constructor; causes 9/10 are the resource-policy handlers' resolve-then-refuse order; the rest are per-operation message/code sites.

## Evidence
1. GetPublicKey 404->404: expected 'The specified CloudFront public key does not exist.', got "The specified public key doesn't exist."
2. UpdatePublicKey 400->400: expected 'You cannot modify encoded material and name of a public key once created.', got "You can't change the value of a public key."
3. CreateAnycastIpList 400->400: expected 'The parameter Quantity Invalid quantity provided. The quantity of IPs in an AnycastIpList should be either 3 or 21.', got 'An argument is invalid: IpCount must be one of 3, 21, not 0'.
4. GetInvalidationForDistributionTenant 404->404: Error.Code expected 'EntityNotFound', got 'NoSuchInvalidation'.
5. ListDistributionsByRealtimeLogConfig 400->400: expected 'The specified real-time log configuration does not exist.', got 'An argument is invalid: no real-time log configuration answers to the Name or ARN the request names'.
6. CreateFieldLevelEncryptionProfile 404->404: expected 'The specified CloudFront public key does not exist.', got "The specified public key doesn't exist."
7. ListDistributionsByKeyGroup 404->404: expected 'The specified key group does not exist.', got "The specified resource doesn't exist. Update your request and try again."
8. CreateRealtimeLogConfig 400->400: expected 'The parameter SamplingRate must be an integer greater than 0 and less than or equal to 100.', got 'An argument is invalid: SamplingRate must be a whole number between 1 and 100 (inclusive) but the request submits 150'.
9. PutResourcePolicy: expected 404 / EntityNotFound / "The specified resource doesn't exist. Update your request and try again."; got 400 / InvalidArgument / "The specified resource type isn't valid. Specify a VPC origin and try again."
10. GetResourcePolicy: expected 404 / EntityNotFound / same message; got 400 / InvalidArgument / 'The ResourceArn parameter is required. Specify a valid ARN and try again.'

The '0' and '150' in causes 3 and 8 are the probes' submitted values; AWS's messages state the rule (3 or 21; 1..100) and do not echo the submission.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Cause 1/6/7: call GetPublicKey, CreateFieldLevelEncryptionProfile (PublicKeyId of an absent key) and ListDistributionsByKeyGroup against ids that do not exist.
3. Cause 2: UpdatePublicKey changing Name/EncodedKey. Cause 3: CreateAnycastIpList with IpCount not in {3,21}. Cause 8: CreateRealtimeLogConfig with SamplingRate > 100.
4. Cause 4: GetInvalidationForDistributionTenant for an invalidation id that does not exist on an existing tenant. Cause 5: ListDistributionsByRealtimeLogConfig naming an unknown config.
5. Causes 9/10: PutResourcePolicy / GetResourcePolicy with a well-formed ResourceArn for a resource that does not exist; both answer 400 InvalidArgument instead of 404 EntityNotFound.
6. Representative step: fixture cloudfront-get-public-key-wrong-resource-type-id-rejected, step task#3 (GetPublicKey).
