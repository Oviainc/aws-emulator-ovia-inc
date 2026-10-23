Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.associate-distribution-tenant-web-acl-entity-not-found-rejected cloudfront.associate-distribution-tenant-web-acl-limit-exceeded-fails cloudfront.associate-distribution-tenant-web-acl-nonexistent-tenant-rejected cloudfront.associate-distribution-tenant-web-acl-not-found-rejected cloudfront.create-invalidation-for-tenant-batch-too-large-rejected cloudfront.delete-distribution-tenant-stale-etag-rejected cloudfront.delete-distribution-tenant-stale-version-rejected cloudfront.disassociate-tenant-web-acl-when-none-associated-invalid-argument cloudfront.distribution-tenant-delete-stale-etag-rejected cloudfront.distribution-tenant-lifecycle cloudfront.distribution-tenant-unicode-customization-round-trip cloudfront.distribution-tenant-web-acl-association-entity-not-found-rejected cloudfront.distribution-tenant-web-acl-association-toggles-state cloudfront.domain-conflict-detection-flow cloudfront.e2e-distribution-tenant-invalidation-scoped-to-tenant cloudfront.list-distribution-tenants-by-customization-filter cloudfront.list-distribution-tenants-by-customization-filters-correctly cloudfront.list-distribution-tenants-pagination-stable-ordering cloudfront.list-distribution-tenants-reflects-created-tenants cloudfront.list-distributions-by-owned-resource-not-found-rejected cloudfront.list-invalidations-for-distribution-tenant-ordering-most-recent-first cloudfront.list-invalidations-for-tenant-most-recent-first cloudfront.op-contract-associate-distribution-tenant-web-acl cloudfront.op-contract-create-distribution-tenant cloudfront.op-contract-deleteDistributionTenant cloudfront.op-contract-listDistributionTenants cloudfront.update-domain-association-not-found-rejected cloudfront.verify-dns-configuration-entity-not-found-rejected cloudfront.verify-dns-configuration-invalid-domain-rejected cloudfront.verify-dns-configuration-malformed-domain-invalid-argument-rejected cloudfront.verify-dns-configuration-mismatch-detected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistributionTenant|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront CreateDistributionTenant: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistributionTenant

## Description
`CreateDistributionTenant` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_distribution_tenant.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

31 recorded unit(s) fail on their `CreateDistributionTenant` call, so each one unblocks only when `CreateDistributionTenant` works: `cloudfront.associate-distribution-tenant-web-acl-entity-not-found-rejected`, `cloudfront.associate-distribution-tenant-web-acl-limit-exceeded-fails`, `cloudfront.associate-distribution-tenant-web-acl-nonexistent-tenant-rejected`, `cloudfront.associate-distribution-tenant-web-acl-not-found-rejected`, `cloudfront.create-invalidation-for-tenant-batch-too-large-rejected`, `cloudfront.delete-distribution-tenant-stale-etag-rejected`, `cloudfront.delete-distribution-tenant-stale-version-rejected`, `cloudfront.disassociate-tenant-web-acl-when-none-associated-invalid-argument`, `cloudfront.distribution-tenant-delete-stale-etag-rejected`, `cloudfront.distribution-tenant-lifecycle`, `cloudfront.distribution-tenant-unicode-customization-round-trip`, `cloudfront.distribution-tenant-web-acl-association-entity-not-found-rejected`, `cloudfront.distribution-tenant-web-acl-association-toggles-state`, `cloudfront.domain-conflict-detection-flow`, `cloudfront.e2e-distribution-tenant-invalidation-scoped-to-tenant`, `cloudfront.list-distribution-tenants-by-customization-filter`, `cloudfront.list-distribution-tenants-by-customization-filters-correctly`, `cloudfront.list-distribution-tenants-pagination-stable-ordering`, `cloudfront.list-distribution-tenants-reflects-created-tenants`, `cloudfront.list-distributions-by-owned-resource-not-found-rejected` (+11 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_distribution_tenant.rs`. Replaying the corpus, the emulator refused the `CreateDistributionTenant` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDistributionTenant operation: operation CreateDistributionTenant is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call CreateDistributionTenant and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
