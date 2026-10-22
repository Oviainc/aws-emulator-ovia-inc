Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-distribution-etag-chain
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-distribution-etag-chain cfval-caller-reference-reuse-conflict cfval-create-distribution-empty-body cloudfront-distribution-alias-association-error cfval-custom-error-response-code-invalid cfval-custom-error-response-error-code-invalid cfval-custom-error-response-page-path-relative cfval-default-root-object-invalid-chars cfval-geo-invalid-location-code cfval-geo-whitelist-empty cfval-query-string-cache-keys-without-query-string cfval-trusted-key-group-does-not-exist cfval-ttl-order-min-exceeds-max cloudfront-invalidation-workflow
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.ActiveTrustedKeyGroups
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.ActiveTrustedSigners
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Aliases
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.CacheBehaviors
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.ContinuousDeploymentPolicyId
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.CustomErrorResponses
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.AllowedMethods
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.Compress
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.FieldLevelEncryptionId
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.FunctionAssociations
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.GrpcConfig
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.LambdaFunctionAssociations
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.SmoothStreaming
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.TrustedKeyGroups
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.TrustedSigners
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultRootObject
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.HttpVersion
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.IsIPV6Enabled
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Logging
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.OriginGroups
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.ConnectionAttempts
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.ConnectionTimeout
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomHeaders
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomOriginConfig.OriginKeepaliveTimeout
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomOriginConfig.OriginReadTimeout
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomOriginConfig.OriginSslProtocols
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.OriginAccessControlId
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.OriginPath
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.OriginShield
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.PriceClass
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Restrictions
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.Staging
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.ViewerCertificate
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.WebACLId
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$headers.location
Issue-Key: cloudfront|UpdateDistribution|2xx->2xx|$.Distribution.ActiveTrustedKeyGroups
Issue-Key: cloudfront|UpdateDistribution|2xx->2xx|$.Distribution.ActiveTrustedSigners
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.ActiveTrustedKeyGroups
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.ActiveTrustedSigners
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Aliases
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.CacheBehaviors
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.ContinuousDeploymentPolicyId
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.CustomErrorResponses
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.AllowedMethods
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.Compress
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.FieldLevelEncryptionId
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.FunctionAssociations
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.GrpcConfig
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.LambdaFunctionAssociations
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.SmoothStreaming
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.TrustedKeyGroups
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.TrustedSigners
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultRootObject
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.HttpVersion
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.IsIPV6Enabled
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Logging
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.OriginGroups
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.ConnectionAttempts
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.ConnectionTimeout
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomHeaders
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomOriginConfig.OriginKeepaliveTimeout
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomOriginConfig.OriginReadTimeout
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.CustomOriginConfig.OriginSslProtocols
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.OriginAccessControlId
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.OriginPath
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Origins.Items.Origin.OriginShield
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.PriceClass
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Restrictions
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.Staging
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.ViewerCertificate
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.WebACLId
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.CustomErrorResponses.Items.CustomErrorResponse.ErrorCachingMinTTL
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.CustomErrorResponses.Items.CustomErrorResponse.ErrorCachingMinTTL
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.DefaultTTL
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.ForwardedValues.Headers
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.MaxTTL
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.DefaultTTL
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.ForwardedValues.Headers
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.MaxTTL
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.ForwardedValues.QueryStringCacheKeys
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.ForwardedValues.QueryStringCacheKeys
Issue-Commit: b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10

# CloudFront Distribution payloads omit members AWS always serializes

> _Note: this issue was identified against the code base at commit `b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
The emulator answers CreateDistribution / UpdateDistribution / GetDistribution with the right status code but a structurally incomplete <Distribution> payload: every member the request did not set, and every member the service itself materialises, is dropped from the XML instead of being emitted with its service-applied value. 16 replayed clusters across 10 fixtures diverge on exactly this, on all three operations.

Cause 1 of 1: Distribution / DistributionConfig serialization drops absent-but-always-present members.
AWS: Smithy (com.amazonaws.cloudfront, 2020-05-31) models Distribution with required members Id, ARN, Status, LastModifiedTime, InProgressInvalidationBatches, DomainName, DistributionConfig plus optional ActiveTrustedSigners / ActiveTrustedKeyGroups; CreateDistribution (POST /2020-05-31/distribution, 201), UpdateDistribution and GetDistribution (GET /2020-05-31/distribution/{Id}, 200) all return Distribution as the httpPayload. The model alone does not state which optional members the service fills in, so for those this is recorded behavior only: the recordings show AWS always serializing the fully-resolved configuration — ActiveTrustedSigners and ActiveTrustedKeyGroups as {Enabled false, Quantity 0}; empty-string DefaultRootObject, Origin.OriginPath and Origin.OriginAccessControlId; Quantity-0 wrappers for Aliases, CustomHeaders, OriginGroups, CacheBehaviors, CustomErrorResponses, LambdaFunctionAssociations, FunctionAssociations, TrustedSigners, TrustedKeyGroups; the CustomOriginConfig defaults OriginSslProtocols {TLSv1, TLSv1.1, TLSv1.2}, OriginReadTimeout 30, OriginKeepaliveTimeout 5; Origin.ConnectionAttempts 3, Origin.ConnectionTimeout 10, Origin.OriginShield {Enabled false}; and the distribution-level defaults PriceClass, HttpVersion, IsIPV6Enabled, Staging, Logging, Restrictions, ViewerCertificate, WebACLId, ContinuousDeploymentPolicyId, DefaultCacheBehavior.AllowedMethods/CachedMethods, SmoothStreaming, Compress, FieldLevelEncryptionId, GrpcConfig and (where ForwardedValues is used) Headers, QueryStringCacheKeys, DefaultTTL, MaxTTL.
Emulator: those elements are simply missing from the body (replay reports each as 'missing (expected ..., got None)'), on CreateDistribution 201, UpdateDistribution 200 and GetDistribution 200 alike, so the response echoes only what the caller sent.
Why: an SDK client deserializes the payload into a Distribution whose sub-structures are null. Reading distribution.DistributionConfig.DefaultCacheBehavior.AllowedMethods, Restrictions, ViewerCertificate or ActiveTrustedKeyGroups NPEs or yields None, a read-modify-write round trip through UpdateDistribution silently loses the service-applied defaults, and config diffing/drift tooling (CDK/Terraform style) sees every unset member as removed.
Fix: the cloudfront service crate's Distribution/DistributionConfig response serializer and the state projection feeding it (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31) must apply the service defaults when storing the config and then emit every member, not only the ones present in the request body.
Traces: cfetag-distribution-etag-chain#task#1 (CreateDistribution 201) and #task#3 (UpdateDistribution 200); cfval-create-distribution-empty-body#task#3, cfval-custom-error-response-error-code-invalid#task#3, cfval-default-root-object-invalid-chars#task#3, cfval-geo-invalid-location-code#task#3, cfval-query-string-cache-keys-without-query-string#task#3, cfval-trusted-key-group-does-not-exist#task#3, cfval-ttl-order-min-exceeds-max#task#3 (GetDistribution 200); cfval-custom-error-response-code-invalid#task#2, cfval-default-root-object-invalid-chars#task#2, cfval-geo-invalid-location-code#task#2, cfval-query-string-cache-keys-without-query-string#task#2, cfval-trusted-key-group-does-not-exist#task#2, cfval-ttl-order-min-exceeds-max#task#2, cloudfront-invalidation-workflow#task#1 (CreateDistribution 201). The same omission also shows up inside the tenant clusters filed separately.

## Evidence
Cluster service=cloudfront|CreateDistribution|status=201->201 (representative cfetag-distribution-etag-chain#task#1, 4 affected steps): expected $.Distribution.ActiveTrustedSigners {'Enabled': 'false', 'Quantity': '0'} / ActiveTrustedKeyGroups {'Enabled': 'false', 'Quantity': '0'} / DistributionConfig.Aliases {'Quantity': '0'} / DefaultRootObject '' / Origins.Items.Origin.OriginPath '' / Origin.CustomHeaders {'Quantity': '0'} / CustomOriginConfig.OriginSslProtocols {'Quantity': '3', Items {'SslProtocol': ['TLSv1','TLSv1.1','TLSv1.2']}} / OriginReadTimeout '30' / OriginKeepaliveTimeout '5' / ConnectionAttempts '3' / ConnectionTimeout '10' / OriginShield {'Enabled': 'false'} / OriginAccessControlId '' / OriginGroups {'Quantity': '0'} / DefaultCacheBehavior.TrustedSigners / TrustedKeyGroups / AllowedMethods {'Quantity': '2', Items {'Method': ['HEAD','GET']}, CachedMethods ...} — each reported 'got None'. Same key set on UpdateDistribution 200 (cfetag-distribution-etag-chain#task#3, ActiveTrustedSigners/ActiveTrustedKeyGroups) and on GetDistribution 200 (cfval-*#task#3 clusters, 35+ missing keys incl. PriceClass, HttpVersion, IsIPV6Enabled, Logging, Restrictions, Staging, ViewerCertificate, WebACLId, ContinuousDeploymentPolicyId, GrpcConfig, Compress, SmoothStreaming, FieldLevelEncryptionId, FunctionAssociations, LambdaFunctionAssociations).

## Steps to Reproduce
1. Replay fixture cfetag-distribution-etag-chain step task#1: POST /2020-05-31/distribution with a minimal DistributionConfig (one custom origin, default cache behavior, Enabled). 2. Status is 201 as expected, but the returned <Distribution> contains no ActiveTrustedSigners/ActiveTrustedKeyGroups and none of the service-applied DistributionConfig defaults. 3. Replay task#3 (UpdateDistribution) and any cfval-*#task#3 (GetDistribution) to see the same members missing on the read and update paths.
