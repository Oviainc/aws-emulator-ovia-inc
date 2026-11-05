Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cfoac-tenant-only-distribution-reference-diagnosis
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cfoac-tenant-only-distribution-reference-diagnosis cloudfront-cfode-attach-rewrite-request-shape-mistakes cloudfront-cfode-rewrite-fix-function-state-and-etag-chain cfetag-field-level-encryption-config-etag-chain cloudfront-distribution-round-trips-declared-values-defaults-and-omissions cloudfront-list-response-headers-policies-managed-and-pagination cloudfront-icc-reference-invalidation-settles-completed
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetDistributionConfig|2xx->2xx|$.DistributionConfig.ViewerCertificate.MinimumProtocolVersion
Issue-Key: cloudfront|GetDistributionConfig|2xx->2xx|$.DistributionConfig.ViewerCertificate.SSLSupportMethod
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.ViewerCertificate.MinimumProtocolVersion
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.ViewerCertificate.SSLSupportMethod
Issue-Key: cloudfront|GetDistributionConfig|2xx->2xx|$.DistributionConfig.DefaultCacheBehavior.FunctionAssociations.Quantity
Issue-Key: cloudfront|GetDistributionConfig|2xx->2xx|$.DistributionConfig.DefaultRootObject
Issue-Key: cloudfront|UpdateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.QueryArgProfiles.Items
Issue-Key: cloudfront|GetCachePolicy|2xx->2xx|$.CachePolicy.LastModifiedTime
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.Comment
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentTypeOptions.Override
Issue-Key: cloudfront|PublishFunction|2xx->2xx|$.FunctionSummary.Status
Issue-Key: cloudfront|DescribeFunction|2xx->2xx|$.FunctionSummary.Status
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.InProgressInvalidationBatches
Issue-Related: 20261029T212223Z-cloudfront-distribution-config-s3-origin-default-omitted-and.md
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# CloudFront readback omits defaulted members and reports stale resource state

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistributionConfig

## Description
Ten net-new wire divergences in which a successful (2xx) CloudFront read returns a body whose members are missing, carry the wrong server-chosen default, or report state the emulator never advanced. The shared fault is that response serialization is not derived from the resource's settled state: server-defaulted members are dropped, managed/built-in resources carry invented values, and lifecycle state (function association status, invalidation progress) is never transitioned. Related but distinct open issues — 20261029T212223Z-cloudfront-omits-server-defaulted-and-empty-optional-members.md, 20261029T212223Z-cloudfront-distribution-tenant-domain-status-and-distribution-viewercertificate.md and 20261029T212223Z-cloudfront-distribution-config-s3-origin-default-omitted-and.md — own their own replay keys and do not cover the steps below.

Cause 1 of 10: GetDistributionConfig drops S3OriginConfig.OriginReadTimeout and returns the wrong ViewerCertificate defaults.
AWS: Smithy: S3OriginConfig has member OriginReadTimeout (integer, "Specifies how long, in seconds, CloudFront waits for a response from the origin"); it is server-defaulted and always serialized back. ViewerCertificate.MinimumProtocolVersion and SSLSupportMethod are both enums; recorded behavior for this distribution is SSLSupportMethod `sni-only` and MinimumProtocolVersion `TLSv1.2_2021`.
Emulator: omits $.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout entirely, and returns SSLSupportMethod `vip` and MinimumProtocolVersion `TLSv1`.
Why: a client that round-trips a config (Get, modify one field, Put) silently drops OriginReadTimeout, and is told the distribution accepts non-SNI viewers on a legacy TLS policy — the opposite of the recommended posture it configured. SDK deserializers that treat the member as present also see a gap.
Fix: the GetDistributionConfig serializer and the distribution state's default materialisation for S3OriginConfig and ViewerCertificate.
Traces: cloudfront-cfoac-tenant-only-distribution-reference-diagnosis#task#6.

Cause 2 of 10: GetDistribution has the same omission and the same wrong ViewerCertificate defaults.
AWS: as Cause 1, under the Distribution wrapper (Smithy: Distribution.DistributionConfig is @required).
Emulator: omits $.Distribution.DistributionConfig...S3OriginConfig.OriginReadTimeout, returns SSLSupportMethod `vip` and MinimumProtocolVersion `TLSv1`.
Why: as Cause 1 — the defect is in the shared DistributionConfig serialization, so both read paths are wrong and a client cannot obtain the true config by either.
Fix: the shared DistributionConfig serializer used by GetDistribution and GetDistributionConfig.
Traces: cloudfront-cfoac-tenant-only-distribution-reference-diagnosis#task#12.

Cause 3 of 10: GetDistributionConfig omits OriginReadTimeout and reports a FunctionAssociations.Quantity of 1 after the association was refused.
AWS: Smithy: FunctionAssociations carries a @required Quantity that must equal the number of Items actually stored. When UpdateDistribution refuses a request, AWS leaves the distribution unchanged, so Quantity stays at its pre-request value (recorded: 0).
Emulator: omits OriginReadTimeout and reports FunctionAssociations.Quantity 1.
Why: a rejected update has been partially applied, so the distribution now claims a function association the service said it would not accept. The caller's view of the resource and the emulator's state permanently disagree.
Fix: UpdateDistribution's handler must not mutate stored state on a validation failure (atomic apply-after-validate), plus the serializer fix from Cause 1.
Traces: cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#12.

Cause 4 of 10: GetDistributionConfig loses DefaultRootObject and again reports a phantom function association.
AWS: Smithy: DistributionConfig.DefaultRootObject is an optional string that AWS stores and returns verbatim; a refused UpdateDistribution leaves both DefaultRootObject and FunctionAssociations.Quantity at their stored values (recorded: the submitted root object, and Quantity 0).
Emulator: returns DefaultRootObject as the empty string and FunctionAssociations.Quantity 1.
Why: a configured default root object silently disappears, which changes how the distribution would serve the origin root, and the phantom association is as in Cause 3.
Fix: the distribution state's write path for DefaultRootObject and the apply-after-validate ordering in UpdateDistribution.
Traces: cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#8.

Cause 5 of 10: UpdateFieldLevelEncryptionConfig drops the empty QueryArgProfiles.Items element.
AWS: Smithy: QueryArgProfileConfig has @required ForwardWhenQueryArgProfileIsUnknown plus QueryArgProfiles; AWS serializes the Items element even when the profile list is empty (recorded: an empty Items element).
Emulator: omits $.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.QueryArgProfiles.Items.
Why: REST-XML clients that navigate to Items to read or append profiles get a missing node instead of an empty one, so a read-modify-write of the FLE config fails or drops the member.
Fix: the FieldLevelEncryptionConfig XML serializer — emit empty collection container elements rather than suppressing them.
Traces: cfetag-field-level-encryption-config-etag-chain#task#6.

Cause 6 of 10: GetCachePolicy returns an invented LastModifiedTime for an AWS-managed cache policy.
AWS: recorded behavior only — for a built-in managed cache policy AWS reports LastModifiedTime as the epoch, `1970-01-01T00:00:00Z`. The model (CachePolicy.LastModifiedTime, timestamp) does not constrain the value; the recording does.
Emulator: returns `2020-05-31T00:00:00Z` (the API version date).
Why: a client that keys a cache of managed-policy definitions on LastModifiedTime, or that compares a managed policy's timestamp against its own, sees a value AWS never produces. The value is a fixed property of the managed policy, not a per-run timestamp, so it is a requirement.
Fix: the managed/built-in cache-policy seed data in CloudFront state.
Traces: cloudfront-distribution-round-trips-declared-values-defaults-and-omissions#task#3.

Cause 7 of 10: ListResponseHeadersPolicies returns wrong Comment text and ContentTypeOptions.Override for AWS-managed response-headers policies.
AWS: recorded behavior only — the managed CORS-with-preflight-and-security-headers and CORS-and-security-headers policies carry the Comments "Allows all origins for CORS requests, including preflight requests, and adds security headers" and "Allows all origins for simple CORS requests, and adds security headers", and their SecurityHeadersConfig.ContentTypeOptions.Override is `true`. These are fixed properties of the managed policies.
Emulator: returns longer paraphrased comments ("...and adds a set of security headers to every response") and ContentTypeOptions.Override `false` on three managed policies.
Why: Override `false` is a behavioural difference, not cosmetic — it says the managed security-headers policy will not override an origin-supplied X-Content-Type-Options, so a client asserting the managed policy's guarantees sees the wrong contract.
Fix: the managed response-headers-policy seed data in CloudFront state.
Traces: cloudfront-list-response-headers-policies-managed-and-pagination#task#3.

Cause 8 of 10: PublishFunction reports FunctionSummary.Status `ASSOCIATED` where AWS reports `UNASSOCIATED`.
AWS: Smithy: PublishFunction http {POST /2020-05-31/function/{Name}/publish, code 200}, output FunctionSummary; FunctionSummary.Status is documented "The status of the CloudFront function." Recorded behavior: a function that is published but not referenced by any live cache behavior reports `UNASSOCIATED`.
Emulator: reports `ASSOCIATED`.
Why: Status is the gate for DeleteFunction — an `ASSOCIATED` function cannot be deleted (FunctionInUse/409), so a client that publishes and then tears down is blocked, and a client that checks whether its association actually took effect is told yes when it did not.
Fix: the function-association bookkeeping in CloudFront state — Status must be derived from the set of distributions that actually reference the function ARN, and a refused UpdateDistribution must not create a reference.
Traces: cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#9.

Cause 9 of 10: DescribeFunction reports FunctionSummary.Status `ASSOCIATED` where AWS reports `DEPLOYED`.
AWS: Smithy: DescribeFunction http {GET /2020-05-31/function/{Name}/describe}, Stage httpQuery of FunctionStage (DEVELOPMENT|LIVE). Recorded behavior: the function reports `DEPLOYED` at this point in the lifecycle.
Emulator: reports `ASSOCIATED`.
Why: the status a client polls to decide the function is live never reaches its expected value, so a deploy-and-wait loop either times out or proceeds on a status AWS does not use here.
Fix: the function state machine behind DescribeFunction — the same derivation as Cause 8.
Traces: cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#14.

Cause 10 of 10: GetDistribution reports InProgressInvalidationBatches 1 after the invalidation has completed.
AWS: Smithy: Distribution.InProgressInvalidationBatches is @required integer, "The number of invalidation batches currently in progress." Recorded behavior: once the referenced invalidation has settled to Completed the counter is 0.
Emulator: still reports 1.
Why: the counter never drains, so the TooManyInvalidationsInProgress limit is approached monotonically and a client that waits for in-progress batches to clear before issuing the next invalidation waits forever.
Fix: the invalidation state machine — completing an invalidation must decrement the owning distribution's in-progress batch count that GetDistribution serializes.
Traces: cloudfront-icc-reference-invalidation-settles-completed#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 78.2%, 102 divergence clusters, all cloudfront).

Cause 1 — cloudfront-cfoac-tenant-only-distribution-reference-diagnosis#task#6 GetDistributionConfig (200/200): $.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout missing (expected '30', got None); $.DistributionConfig.ViewerCertificate.SSLSupportMethod expected 'sni-only', got 'vip'; MinimumProtocolVersion expected 'TLSv1.2_2021', got 'TLSv1'.
Cause 2 — #task#12 GetDistribution: identical three mismatches under $.Distribution.DistributionConfig.
Cause 3 — cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#12 GetDistributionConfig: OriginReadTimeout missing (expected '30'); $.DistributionConfig.DefaultCacheBehavior.FunctionAssociations.Quantity expected '0', got '1'.
Cause 4 — cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#8 GetDistributionConfig: DefaultRootObject expected 'index.html', got ''; FunctionAssociations.Quantity expected '0', got '1'.
Cause 5 — cfetag-field-level-encryption-config-etag-chain#task#6 UpdateFieldLevelEncryptionConfig: $.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.QueryArgProfiles.Items missing (expected '', got None).
Cause 6 — cloudfront-distribution-round-trips-declared-values-defaults-and-omissions#task#3 GetCachePolicy: $.CachePolicy.LastModifiedTime expected '1970-01-01T00:00:00Z', got '2020-05-31T00:00:00Z'.
Cause 7 — cloudfront-list-response-headers-policies-managed-and-pagination#task#3 ListResponseHeadersPolicies: summary[2] Comment expected 'Allows all origins for CORS requests, including preflight requests, and adds security headers', got '...and adds a set of security headers to every response'; summary[3] Comment expected 'Allows all origins for simple CORS requests, and adds security headers'; summary[2]/[3]/[4] SecurityHeadersConfig.ContentTypeOptions.Override expected 'true', got 'false'.
Cause 8 — cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#9 PublishFunction: $.FunctionSummary.Status expected 'UNASSOCIATED', got 'ASSOCIATED'.
Cause 9 — #task#14 DescribeFunction: $.FunctionSummary.Status expected 'DEPLOYED', got 'ASSOCIATED'.
Cause 10 — cloudfront-icc-reference-invalidation-settles-completed#task#3 GetDistribution: $.Distribution.InProgressInvalidationBatches expected '0', got '1'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cloudfront-cfoac-tenant-only-distribution-reference-diagnosis and inspect steps task#6 (representative) and task#12.
4. Replay cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#12; cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#8, #task#9 and #task#14; cfetag-field-level-encryption-config-etag-chain#task#6; cloudfront-distribution-round-trips-declared-values-defaults-and-omissions#task#3; cloudfront-list-response-headers-policies-managed-and-pagination#task#3; cloudfront-icc-reference-invalidation-settles-completed#task#3.
5. Diff each 200 response body against the recorded one. All ten causes must match before closing.
