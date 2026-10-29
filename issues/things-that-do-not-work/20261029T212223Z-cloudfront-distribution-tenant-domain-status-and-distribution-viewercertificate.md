Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-distribution-tenant-create-and-invalidate
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-distribution-tenant-create-and-invalidate cloudfront-distribution-tenant-customization-unicode-domain-preserved cloudfront-distribution-tenant-delete-requires-disabled cloudfront-distribution-tenant-errors-and-filters cloudfront-distribution-tenant-lifecycle cloudfront-distribution-tenant-unicode-customization-round-trip cloudfront-distribution-tenant-webacl-association
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetDistributionTenant|2xx->2xx|$.DistributionTenant.Domains.member.Status
Issue-Key: cloudfront|CreateDistributionTenant|2xx->2xx|$.DistributionTenant.Domains.member.Status
Issue-Key: cloudfront|ListDistributionTenants|2xx->2xx|$.ListDistributionTenantsResult.DistributionTenantList.DistributionTenantSummary.Domains.member.Status
Issue-Key: cloudfront|UpdateDistributionTenant|2xx->2xx|$.DistributionTenant.Domains.member.Status
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.ViewerCertificate.SSLSupportMethod
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.TenantConfig
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront distribution tenant domain status and distribution ViewerCertificate defaults are wrong

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistributionTenant

## Description
Six causes, one shared fault: the server-derived state the emulator assigns to a freshly created distribution tenant and to a tenant-capable distribution does not match what AWS assigns.

Causes 1-4 (DistributionTenant domain status).
AWS: a tenant domain starts out `inactive` - it only becomes active once the domain is validated and served (Smithy: DomainResult.Status targets the DomainStatus enum - 'Whether the domain is active or inactive'). The recorded create, get, list and update responses all report inactive for a newly created tenant.
Emulator: reports `active` on every one of those paths.
Why: a client that waits for a tenant domain to become active has nothing to wait for and will treat an unvalidated domain as ready to serve; the inactive->active transition can never be observed.
Fix: the distribution-tenant state's initial domain status (and the transition that promotes it), shared by all four serialization paths.
Cause 1 of 6: GetDistributionTenant - traces cloudfront-distribution-tenant-create-and-invalidate#task#3 (also #teardown#7, cloudfront-distribution-tenant-customization-unicode-domain-preserved#task#3, cloudfront-distribution-tenant-delete-requires-disabled#task#4, cloudfront-distribution-tenant-errors-and-filters#teardown#7 and #teardown#9, cloudfront-distribution-tenant-lifecycle#task#3, cloudfront-distribution-tenant-unicode-customization-round-trip#task#3, cloudfront-distribution-tenant-webacl-association#task#5).
Cause 2 of 6: CreateDistributionTenant - traces cloudfront-distribution-tenant-create-and-invalidate#setup#2 (also cloudfront-distribution-tenant-lifecycle#task#2, cloudfront-distribution-tenant-unicode-customization-round-trip#task#2, cloudfront-distribution-tenant-webacl-association#setup#2).
Cause 3 of 6: ListDistributionTenants - the same status in each DistributionTenantSummary. Traces cloudfront-distribution-tenant-create-and-invalidate#task#4 (also cloudfront-distribution-tenant-lifecycle#task#4).
Cause 4 of 6: UpdateDistributionTenant - traces cloudfront-distribution-tenant-delete-requires-disabled#task#5.

Causes 5-6 (CreateDistribution ViewerCertificate/TenantConfig).
AWS: for these configurations the returned ViewerCertificate.SSLSupportMethod is `sni-only`, and a distribution created without tenant parameter definitions has NO TenantConfig element at all.
Emulator: returns SSLSupportMethod `vip`, and (cause 6) emits a TenantConfig element containing an empty ParameterDefinitions.
Why: `vip` is the dedicated-IP SSL method - a materially different and billable configuration from sni-only - so a client reading the distribution back believes it provisioned dedicated IP SSL; and a TenantConfig that was never requested appears in the config a client round-trips into UpdateDistribution.
Fix: CreateDistribution's ViewerCertificate defaulting in validation/state, and the distribution serializer (do not emit an empty TenantConfig).
Cause 5 of 6: CreateDistribution - SSLSupportMethod only. Traces cloudfront-distribution-tenant-create-and-invalidate#setup#1.
Cause 6 of 6: CreateDistribution - SSLSupportMethod plus the spurious TenantConfig. Traces cloudfront-distribution-tenant-lifecycle#setup#1 (also cloudfront-distribution-tenant-webacl-association#setup#1).

## Evidence
Causes 1-4 (status matches 200/201):
1. GetDistributionTenant: $.DistributionTenant.Domains.member.Status expected 'inactive', got 'active' (9 affected steps).
2. CreateDistributionTenant: same path, expected 'inactive', got 'active' (4 affected steps).
3. ListDistributionTenants: $.ListDistributionTenantsResult.DistributionTenantList.DistributionTenantSummary.Domains.member.Status expected 'inactive', got 'active'.
4. UpdateDistributionTenant: same member, expected 'inactive', got 'active'.

Causes 5-6 (status matches 201):
5. CreateDistribution: $.Distribution.DistributionConfig.ViewerCertificate.SSLSupportMethod expected 'sni-only', got 'vip'.
6. CreateDistribution: the same SSLSupportMethod divergence plus $.Distribution.DistributionConfig.TenantConfig type_mismatch (expected absent/empty, got {'ParameterDefinitions': ''}).

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Causes 1-4: CreateDistributionTenant with a domain, then GetDistributionTenant, ListDistributionTenants and UpdateDistributionTenant; Domains[].Status reads 'active' immediately in all four.
3. Causes 5-6: CreateDistribution for a tenant-capable distribution without specifying SSLSupportMethod and without TenantConfig; the response reports SSLSupportMethod 'vip' and, in the tenant-lifecycle shape, an empty TenantConfig element.
4. Representative step: fixture cloudfront-distribution-tenant-create-and-invalidate, step task#3 (GetDistributionTenant).
