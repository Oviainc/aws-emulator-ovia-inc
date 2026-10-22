Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-distribution-tenant-create-and-invalidate
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-distribution-tenant-create-and-invalidate cloudfront-distribution-tenant-lifecycle cloudfront-distribution-tenant-webacl-association
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.AllowedMethods.CachedMethods
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.DefaultCacheBehavior.AllowedMethods.Items.Method[*]
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DomainName
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.TenantConfig
Issue-Related: 20261022T025800Z-cloudfront-distribution-payloads-omit-members-aws-always-serializes.md
Issue-Commit: b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10

# CloudFront multi-tenant distribution DomainName and AllowedMethods order diverge

> _Note: this issue was identified against the code base at commit `b0c0a3cf60eeb3c3c5e56cfd149cad2378448d10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Creating a distribution with TenantConfig (connection mode tenant-only) is served as if it were a standard distribution, and the echoed method lists come back in a different order than AWS emits.

Cause 1 of 2: a multi-tenant distribution is given a generated cloudfront.net domain instead of the placeholder AWS returns.
AWS: Smithy models Distribution.DomainName as required and DistributionConfig.TenantConfig as the multi-tenant-only member ('This field only supports multi-tenant distributions'), with ConnectionMode selecting direct vs tenant-only; for a tenant-only distribution the traffic domain belongs to the connection group, not the distribution. Recorded behavior: AWS returns DomainName '-' for a distribution created with TenantConfig. The model does not state the placeholder, so this is recorded behavior only — but '-' is a fixed literal, not a generated value.
Emulator: returns a generated distribution domain (d0lc535g268ho1.cloudfront.net) for the tenant-only distribution.
Why: a client cannot tell a multi-tenant distribution from a standard one by its DomainName, and tooling that would route tenants through a connection group instead resolves and uses a distribution domain that does not serve tenant traffic.
Fix: the CreateDistribution handler / Distribution projection in the cloudfront service crate must emit DomainName '-' when the stored config carries TenantConfig (tenant-only connection mode) instead of allocating a cloudfront.net name. The same two clusters also miss the always-present members covered by the separate Distribution-serialization issue (including TenantConfig itself, which is dropped from the echoed DistributionConfig).

Cause 2 of 2: AllowedMethods/CachedMethods Items are re-ordered in the response.
AWS: Smithy binds MethodsList as an ordered XML list of <Method> members; a restXml list's element order is part of the wire value. Recorded behavior: AWS emits Quantity 2 with Items.Method ['HEAD', 'GET'] (and the same order under CachedMethods).
Emulator: emits ['GET', 'HEAD'].
Why: a client that compares the returned cache behavior with the config it sent (drift detection, read-modify-write, idempotent re-apply) sees a difference that AWS does not report, and ordered-list assertions on the payload fail.
Fix: the cache-behavior serializer must preserve/normalize the method list in the order the service emits rather than re-sorting it.

Traces: cloudfront-distribution-tenant-create-and-invalidate#setup#1 (DomainName plus the Method order mismatch) and cloudfront-distribution-tenant-lifecycle#setup#1 (DomainName, with cloudfront-distribution-tenant-webacl-association#setup#1 in the same cluster).

## Evidence
cloudfront-distribution-tenant-create-and-invalidate#setup#1 (CreateDistribution 201->201): '$.Distribution.DomainName: value_mismatch (expected \'-\', got \'d0lc535g268ho1.cloudfront.net\')'; '$.Distribution.DistributionConfig.DefaultCacheBehavior.AllowedMethods.Items.Method[0]: value_mismatch (expected \'HEAD\', got \'GET\')' and Method[1] expected 'GET' got 'HEAD'; plus AllowedMethods.CachedMethods and the ActiveTrustedSigners/ActiveTrustedKeyGroups/default-member omissions. cloudfront-distribution-tenant-lifecycle#setup#1 (affected also cloudfront-distribution-tenant-webacl-association#setup#1): same DomainName divergence, and DistributionConfig.TenantConfig among the missing keys.

## Steps to Reproduce
1. Replay fixture cloudfront-distribution-tenant-create-and-invalidate step setup#1: POST /2020-05-31/distribution with a DistributionConfig carrying TenantConfig (tenant-only connection mode) — expect Distribution.DomainName '-', observe a generated d*.cloudfront.net name. 2. In the same response compare DefaultCacheBehavior.AllowedMethods.Items.Method: expected order HEAD,GET, observed GET,HEAD. 3. Replay cloudfront-distribution-tenant-lifecycle#setup#1 for the same DomainName divergence (TenantConfig is also dropped from the echoed config).
