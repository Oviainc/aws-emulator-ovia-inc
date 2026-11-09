Issue-Source: trace-sweep-divergence
Issue-Trace: cfdeep-invalidation-pagination-walk
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfdeep-invalidation-pagination-walk cloudfront-distribution-create-invalidate-monitor-webacl cloudfront-icc-caller-reference-retry-and-conflict cloudfront-icc-distribution-identifier-mistakes cloudfront-icc-path-format-mistakes cloudfront-icc-per-object-batch-too-large cloudfront-icc-readback-identifier-and-paging-mistakes cloudfront-icc-reference-invalidation-settles-completed cloudfront-icc-sdk-batch-construction-mistakes cloudfront-invalidation-create-get-list cloudfront-invalidation-lifecycle cloudfront-invalidation-workflow cloudfront-distribution-tenant-create-and-invalidate cloudfront-distribution-tenant-invalidation cloudfront-connection-function-errors cfval-origin-oai-and-oac-both-set cloudfront-distribution-tenant-lifecycle
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateInvalidation|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateInvalidationForDistributionTenant|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateConnectionFunction|2xx->2xx|$headers.location
Issue-Key: cloudfront|ListDistributionTenants|2xx->2xx|$.ListDistributionTenantsResult.DistributionTenantList.DistributionTenantSummary.ETag
Issue-Related: 20261029T212223Z-cloudfront-distribution-config-s3-origin-default-omitted-and.md
Issue-Related: 20261029T212223Z-cloudfront-distribution-tenant-domain-status-and-distribution-viewercertificate.md
Issue-Related: 20261109T120741Z-cloudfront-distribution-and-connection-group-responses-publish-aws-production.md
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# CloudFront invalidation and connection-function Location URIs name an unserved host and distribution readbacks lose derived values

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidation

## Description
Six causes grouped by one theme: values CloudFront derives at create/read time - the Location URI of an invalidation or connection function, the invalidation id inside it, the S3 origin's read timeout and a tenant summary's ETag - are not derived the way AWS derives them. The three Location causes fail with verdict `host_not_own`: the URI is rooted at the literal production host `cloudfront.amazonaws.com` instead of the endpoint the request reached, i.e. the emulator announces an address other than its own (same root fault as the two bundles filed alongside this one).

Cause 1 of 6: CreateInvalidation's Location names an unserved host and carries a mis-shaped invalidation id.
AWS: 201 with `Location: <service endpoint>/2020-05-31/distribution/<distribution id>/invalidation/<invalidation id>` (Smithy: CreateInvalidation output Location is an httpHeader; http {POST /2020-05-31/distribution/{DistributionId}/invalidation, 201}), where the invalidation id has CloudFront's invalidation-id form - the long uppercase alphanumeric token seen in the recording (Smithy: Invalidation.Id, 'The identifier for the invalidation request').
Emulator: the URI is rooted at `cloudfront.amazonaws.com`, and its last segment is a short 14-character distribution-style id rather than an invalidation id of AWS's length/form.
Why: a client that follows Location to poll GetInvalidation is sent off the emulator, and a client that validates or stores the invalidation id gets a token of the wrong shape (ids that collide with the distribution-id space).
Fix: CreateInvalidation's response construction (Location derivation) and the invalidation-id generator in state.
Traces: cfdeep-invalidation-pagination-walk#task#2 (14 affected steps, incl. cloudfront-invalidation-create-get-list#task#2, cloudfront-invalidation-lifecycle#task#2).

Cause 2 of 6: CreateInvalidationForDistributionTenant - same fault on the tenant route.
AWS: 201 with `Location: <service endpoint>/2020-05-31/distribution-tenant/<tenant id>/invalidation/<invalidation id>`, invalidation id in AWS's form.
Emulator: production host, and a short 14-character id.
Why/Fix: as cause 1, in CreateInvalidationForDistributionTenant.
Traces: cloudfront-distribution-tenant-create-and-invalidate#task#5 (2 affected steps).

Cause 3 of 6: CreateConnectionFunction's Location names an unserved host.
AWS: recorded behavior only - the recording for this step carries NO Location header at all for CreateConnectionFunction; whatever is emitted must at minimum be rooted at the endpoint the caller reached.
Emulator: emits `Location: https://cloudfront.amazonaws.com/2020-05-31/connection-function/<id>` - a header the recording does not contain, naming a host the emulator does not serve.
Why: a client that follows the header leaves the emulator; a client that keys off the header's presence sees a create contract AWS did not return.
Fix: CreateConnectionFunction's response construction (drop the header, or root it at the request's own endpoint).
Traces: cloudfront-connection-function-errors#setup#1.

Cause 4 of 6: CreateDistribution drops the S3 origin's read timeout (and announces unserved names).
AWS: 201 whose `Origins.Items.Origin.S3OriginConfig` reports `OriginReadTimeout` (Smithy: S3OriginConfig.OriginReadTimeout, integer, 'Specifies how long, in seconds, CloudFront waits for a response from the origin'; the recording shows the value defaulted to 30 when the request omits it), with DomainName and Location on the serving endpoint.
Emulator: `S3OriginConfig.OriginReadTimeout` is absent, DomainName is a `*.cloudfront.net` name and Location is on `cloudfront.amazonaws.com`.
Why: a client that round-trips the returned config into UpdateDistribution silently loses the origin read timeout, and sees no value where AWS always reports one.
Fix: the S3OriginConfig default in distribution state plus CreateDistribution's response construction. (The OriginReadTimeout omission in isolation is described in 20261029T212223Z-cloudfront-distribution-config-s3-origin-default-omitted-and.md, which does not carry this cluster's replay key.)
Traces: cfval-origin-oai-and-oac-both-set#task#4.

Cause 5 of 6: GetDistribution reads back the same losses.
AWS: 200 with `S3OriginConfig.OriginReadTimeout` present and a served DomainName.
Emulator: OriginReadTimeout absent, DomainName a `*.cloudfront.net` name.
Why/Fix: as cause 4, on GetDistribution's readback path.
Traces: cfval-origin-oai-and-oac-both-set#task#5.

Cause 6 of 6: ListDistributionTenants summaries carry an ETag that is not the tenant's.
AWS: recorded behavior only - 200 where each `DistributionTenantSummary.ETag` is the current ETag of the tenant it summarizes, i.e. the same value Get/CreateDistributionTenant returns for that tenant, so a client can use it directly as If-Match.
Emulator: reports an unrelated ETag value for the summary, different from the one the tenant's own responses carry in the same fixture. (The sibling `Domains.member.Status` failure in this cluster is the tenant-domain-status cause owned by 20261029T212223Z-cloudfront-distribution-tenant-domain-status-and-distribution-viewercertificate.md.)
Why: a client that takes the ETag from a list response and sends it as If-Match on UpdateDistributionTenant/DeleteDistributionTenant is refused PreconditionFailed, so list-then-modify cannot work.
Fix: ListDistributionTenants' summary projection (it must read the tenant's stored ETag rather than mint one).
Traces: cloudfront-distribution-tenant-lifecycle#task#4.

The concrete ids, ETags and generated labels quoted from the recording are per-run values; the requirements are the URI authority, the id/ETag provenance and the presence of the defaulted member.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). Six unowned mismatch clusters:
1. CreateInvalidation 201->201 - cfdeep-invalidation-pagination-walk#task#2: $headers.location host_not_own, expected 'https://cloudfront.amazonaws.com/2020-05-31/distribution/E7IYCGPKTANSF3/invalidation/IBM5B2ULLUSKHZAOLO538EHQTE', got '.../invalidation/IGZNI4PND7H2S3' - production authority, and a 14-char id where the recording has a 26-char invalidation id (14 affected steps).
2. CreateInvalidationForDistributionTenant 201->201 - cloudfront-distribution-tenant-create-and-invalidate#task#5: expected '.../distribution-tenant/dt_5NI6ZNXJB8DO0/invalidation/I7C8ZPV5CUORWZJ8H49T7EODL8', got '.../invalidation/IRKD77TQ89JF12' (2 affected).
3. CreateConnectionFunction 201->201 - cloudfront-connection-function-errors#setup#1: $headers.location expected None, got 'https://cloudfront.amazonaws.com/2020-05-31/connection-function/cf_7IYCGPKTANSF3'.
4. CreateDistribution 201->201 - cfval-origin-oai-and-oac-both-set#task#4: $...S3OriginConfig.OriginReadTimeout missing (expected '30', got None); $.Distribution.DomainName got 'd2pysi4346dfp3.cloudfront.net'; $headers.location got 'https://cloudfront.amazonaws.com/2020-05-31/distribution/EZGEITDQ1HLFY0'.
5. GetDistribution 200->200 - cfval-origin-oai-and-oac-both-set#task#5: same OriginReadTimeout missing + DomainName host_not_own.
6. ListDistributionTenants 200->200 - cloudfront-distribution-tenant-lifecycle#task#4: $...DistributionTenantSummary.ETag value_mismatch (expected 'EJZ50Z92T9DP30', got 'E89FKDDMX3R403'), alongside the Domains.member.Status 'inactive'->'active' cause owned by an existing issue.
None of the six carried a stub_path or an owned_by owner.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, replay the wire corpus (simparity-wire-aws-v1).
2. Representative step: fixture cfdeep-invalidation-pagination-walk, step task#2 (CreateInvalidation) - inspect the Location header: production authority and a short distribution-style id in the invalidation position.
3. cloudfront-distribution-tenant-create-and-invalidate#task#5 (CreateInvalidationForDistributionTenant) - same.
4. cloudfront-connection-function-errors#setup#1 (CreateConnectionFunction) - a Location header is emitted that the recording does not have, on cloudfront.amazonaws.com.
5. cfval-origin-oai-and-oac-both-set#task#4 then #task#5 (CreateDistribution, GetDistribution) - S3OriginConfig has no OriginReadTimeout member.
6. cloudfront-distribution-tenant-lifecycle#task#4 (ListDistributionTenants) - compare the summary ETag with the ETag the tenant's own Get/Create returned in the same fixture; they differ.
