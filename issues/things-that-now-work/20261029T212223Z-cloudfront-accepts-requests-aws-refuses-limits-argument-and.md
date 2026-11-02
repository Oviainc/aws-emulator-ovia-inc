Issue-Source: trace-sweep-divergence
Issue-Trace: cfdeep-request-local-limits
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfdeep-request-local-limits cloudfront-anycast-ip-list-create-limit-exceeded cloudfront-anycast-ip-list-error-paths cloudfront-anycast-ip-list-lifecycle cloudfront-continuous-deployment-policy-create-invalid-domain-error cloudfront-continuous-deployment-policy-errors cloudfront-continuous-deployment-policy-lifecycle cloudfront-field-level-encryption-invalid-reference-errors cloudfront-invalidation-create-get-list cfetag-field-level-encryption-config-etag-chain cfetag-field-level-encryption-profile-etag-chain cloudfront-field-level-encryption-duplicate-and-ifmatch-errors cloudfront-field-level-encryption-profile-workflow cfetag-public-key-etag-chain cloudfront-public-key-duplicate-and-notfound-errors cloudfront-public-key-lifecycle cloudfront-oac-error-paths
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateCachePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateCachePolicy|4xx->2xx|status
Issue-Key: cloudfront|CreateOriginRequestPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateOriginRequestPolicy|4xx->2xx|status
Issue-Key: cloudfront|CreateAnycastIpList|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateAnycastIpList|4xx->2xx|status
Issue-Key: cloudfront|CreateContinuousDeploymentPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateContinuousDeploymentPolicy|4xx->2xx|status
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|4xx->2xx|status
Issue-Key: cloudfront|CreateInvalidation|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateInvalidation|4xx->2xx|status
Issue-Key: cloudfront|UpdateFieldLevelEncryptionConfig|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|UpdateFieldLevelEncryptionConfig|4xx->2xx|status
Issue-Key: cloudfront|CreateFieldLevelEncryptionProfile|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateFieldLevelEncryptionProfile|4xx->2xx|status
Issue-Key: cloudfront|CreatePublicKey|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreatePublicKey|4xx->2xx|status
Issue-Key: cloudfront|CreateOriginAccessControl|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateOriginAccessControl|4xx->2xx|status
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront accepts requests AWS refuses: limits, argument and uniqueness validation missing

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCachePolicy

## Description
Ten causes, one shared fault: the emulator performs no request-level limit, argument or uniqueness validation on these create/update paths. Each one answers the success status (201/200/202) and persists a resource AWS would have refused outright with a 4xx.

Fix for every cause: add the missing precondition to that operation's validation layer, before the resource is written to state, and emit the modelled error.

Cause 1 of 10: CreateCachePolicy does not enforce the per-policy header limit.
AWS: 400 `TooManyHeadersInCachePolicy` with the fixed message `Your request contains more headers in the Cache Policy than are allowed.` (Smithy: CreateCachePolicy errors include TooManyHeadersInCachePolicy -> 400). Fixed text, no variable part.
Emulator: 201 Created with the policy stored.
Why: a client exercising the quota boundary gets a policy it cannot have in AWS, and code that handles TooManyHeadersInCachePolicy is never reached.
Fix: CreateCachePolicy validation (ParametersInCacheKeyAndForwardedToOrigin.HeadersConfig.Headers quantity).
Traces: cfdeep-request-local-limits#task#1 (also task#2, task#3).

Cause 2 of 10: CreateOriginRequestPolicy does not enforce the per-policy header limit.
AWS: 400 `TooManyHeadersInOriginRequestPolicy`, fixed message `Your request contains more headers in Origin Request Policy than are allowed.`
Emulator: 201 Created.
Why/Fix: as cause 1, in CreateOriginRequestPolicy validation.
Traces: cfdeep-request-local-limits#task#4.

Cause 3 of 10: CreateAnycastIpList does not enforce the account quota.
AWS: 400 `EntityLimitExceeded` with the message `To request a limit increase for your Anycast static IP list, contact AWS Support: <service-quotas console URL for the cloudfront L-6A19EDFD quota>` - the sentence and the quota code are fixed; only the console host/region path is environmental.
Emulator: 202 Accepted, list created.
Why: the quota is unreachable, so clients never see the limit path; the probe then sees extra lists in later Lists.
Fix: CreateAnycastIpList validation/state (per-account list count).
Traces: cloudfront-anycast-ip-list-create-limit-exceeded#task#1 (also cloudfront-anycast-ip-list-error-paths#task#1, cloudfront-anycast-ip-list-lifecycle#task#1).

Cause 4 of 10: CreateContinuousDeploymentPolicy does not validate StagingDistributionDnsNames.
AWS: 400 `InvalidArgument` with the message `The parameter StagingDistributionDnsNames is not a default CloudFront domain name.` - fixed text naming the offending member.
Emulator: 201 Created with an arbitrary DNS name accepted.
Why: a continuous-deployment policy can be built pointing at a non-CloudFront domain, which AWS would never allow.
Fix: CreateContinuousDeploymentPolicy validation.
Traces: cloudfront-continuous-deployment-policy-create-invalid-domain-error#task#1 (also cloudfront-continuous-deployment-policy-errors#task#1, cloudfront-continuous-deployment-policy-lifecycle#task#2).

Cause 5 of 10: CreateFieldLevelEncryptionConfig accepts an empty QueryArgProfileConfig with an empty ContentTypeProfileConfig.
AWS: 400 `QueryArgProfileEmpty` with the fixed message `The query arg profile in field level encryption config cannot be empty when content type profile is empty or when no default profile id is given in any content type profile.` (Smithy models QueryArgProfileEmpty -> 400 on the FLE config operations).
Emulator: 201 Created.
Why: an unusable FLE config is persisted; the modelled refusal never fires.
Fix: CreateFieldLevelEncryptionConfig validation (cross-member rule between QueryArgProfileConfig and ContentTypeProfileConfig).
Traces: cloudfront-field-level-encryption-invalid-reference-errors#task#3.

Cause 6 of 10: CreateInvalidation accepts malformed invalidation paths.
AWS: 400 `InvalidArgument` with the fixed message `Your request contains one or more invalid invalidation paths.` (Smithy: CreateInvalidation errors include InvalidArgument -> 400).
Emulator: 201 Created, invalidation recorded.
Why: paths that AWS rejects (e.g. not starting with '/') become accepted invalidations, so client-side path validation is never exercised.
Fix: CreateInvalidation validation (InvalidationBatch.Paths.Items).
Traces: cloudfront-invalidation-create-get-list#task#5.

Cause 7 of 10: UpdateFieldLevelEncryptionConfig accepts an update that drops QueryArgProfileConfig.
AWS: 400 `IllegalUpdate` with the message `QueryArgProfileConfig is required for updates` (Smithy: UpdateFieldLevelEncryptionConfig errors include IllegalUpdate -> 400). Fixed text naming the required member.
Emulator: 200 OK and applies the update.
Why: the config silently loses its query-arg profile mapping, which AWS prevents.
Fix: UpdateFieldLevelEncryptionConfig validation.
Traces: cfetag-field-level-encryption-config-etag-chain#task#5.

Cause 8 of 10: CreateFieldLevelEncryptionProfile does not detect a duplicate profile name.
AWS: 409 `FieldLevelEncryptionProfileAlreadyExists` with the fixed message `The name that you are using to create a field level encryption profile is associated with another field level encryption profile in your account.`
Emulator: 201 Created - a second profile with the same name.
Why: name uniqueness is not a constraint in the emulator, so duplicate profiles accumulate and later lookups by name are ambiguous.
Fix: CreateFieldLevelEncryptionProfile uniqueness check in validation/state.
Traces: cfetag-field-level-encryption-profile-etag-chain#task#3 (also cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#3, cloudfront-field-level-encryption-profile-workflow#task#5).

Cause 9 of 10: CreatePublicKey does not detect a duplicate public-key name.
AWS: 409 `PublicKeyAlreadyExists` with the fixed message `The name that you are using to create a CloudFront public key is associated with another public key in your account.`
Emulator: 201 Created.
Why/Fix: as cause 8, in CreatePublicKey.
Traces: cfetag-public-key-etag-chain#task#2 (also cloudfront-public-key-duplicate-and-notfound-errors#task#2, cloudfront-public-key-lifecycle#task#4).

Cause 10 of 10: CreateOriginAccessControl does not detect a duplicate OAC name on this path.
AWS: 409 `OriginAccessControlAlreadyExists` with the fixed message `An origin access control with the same name already exists.` (Smithy: CreateOriginAccessControl errors include OriginAccessControlAlreadyExists -> 409).
Emulator: 201 Created.
Why/Fix: as cause 8, in CreateOriginAccessControl. Note this is a *missing* conflict, distinct from the wrong-message conflict filed separately.
Traces: cloudfront-oac-error-paths#task#2.

## Evidence
Each cluster diverges on `status` plus a wholly missing $.ErrorResponse.

1. CreateCachePolicy: expected 400 / TooManyHeadersInCachePolicy 'Your request contains more headers in the Cache Policy than are allowed.', got 201 with no ErrorResponse.
2. CreateOriginRequestPolicy: expected 400 / TooManyHeadersInOriginRequestPolicy 'Your request contains more headers in Origin Request Policy than are allowed.', got 201.
3. CreateAnycastIpList: expected 400 / EntityLimitExceeded 'To request a limit increase for your Anycast static IP list, contact AWS Support: .../services/cloudfront/quotas/L-6A19EDFD', got 202.
4. CreateContinuousDeploymentPolicy: expected 400 / InvalidArgument 'The parameter StagingDistributionDnsNames is not a default CloudFront domain name.', got 201.
5. CreateFieldLevelEncryptionConfig: expected 400 / QueryArgProfileEmpty 'The query arg profile in field level encryption config cannot be empty when content type profile is empty or when no default profile id is given in any content type profile.', got 201.
6. CreateInvalidation: expected 400 / InvalidArgument 'Your request contains one or more invalid invalidation paths.', got 201.
7. UpdateFieldLevelEncryptionConfig: expected 400 / IllegalUpdate 'QueryArgProfileConfig is required for updates', got 200.
8. CreateFieldLevelEncryptionProfile: expected 409 / FieldLevelEncryptionProfileAlreadyExists 'The name that you are using to create a field level encryption profile is associated with another field level encryption profile in your account.', got 201.
9. CreatePublicKey: expected 409 / PublicKeyAlreadyExists 'The name that you are using to create a CloudFront public key is associated with another public key in your account.', got 201.
10. CreateOriginAccessControl: expected 409 / OriginAccessControlAlreadyExists 'An origin access control with the same name already exists.', got 201.

All ten messages above are AWS's fixed text; none has a variable part except cause 3's console URL path and cause 4/7's member name, which come from the quota code and the request's own member.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Cause 1/2: CreateCachePolicy / CreateOriginRequestPolicy with more whitelisted headers than the per-policy quota. 3: CreateAnycastIpList past the account quota. 4: CreateContinuousDeploymentPolicy with a StagingDistributionDnsNames value that is not a *.cloudfront.net domain. 5: CreateFieldLevelEncryptionConfig with both profile configs empty. 6: CreateInvalidation with a malformed path. 7: UpdateFieldLevelEncryptionConfig omitting QueryArgProfileConfig. 8/9/10: create the resource, then repeat the Create* with the same Name.
3. Each returns success and persists the resource instead of the modelled 4xx.
4. Representative step: fixture cfdeep-request-local-limits, step task#1 (CreateCachePolicy).
