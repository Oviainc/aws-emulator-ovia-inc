Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.concurrent-associate-web-acl-race cloudfront.copy-distribution-independent-clone cloudfront.create-invalidation-for-tenant-batch-too-large-fails cloudfront.delete-distribution-tenant-in-use-connection-group-fails cloudfront.disassociate-web-acl-invalid-argument-rejected cloudfront.distribution-disable-then-delete-removes-resource cloudfront.distribution-tag-untag-lifecycle cloudfront.distribution-with-tags-lifecycle cloudfront.invalidation-for-tenant-lifecycle cloudfront.invalidation-idempotent-same-batch-returns-same-id cloudfront.list-distribution-tenants-pagination-consistent-ordering cloudfront.monitoring-subscription-lifecycle cloudfront.op-contract-create-distribution cloudfront.op-contract-deleteMonitoringSubscription cloudfront.op-contract-get-distribution-config cloudfront.tag-resource-on-cache-policy-arn-via-distribution cloudfront.update-distribution-tenant-cname-conflict-fails cloudfront.update-distribution-tenant-duplicate-customization-fails cloudfront.web-acl-association-enforced-at-edge cloudfront.web-acl-association-invalid-webacl-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|TagResource|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `cloudfront.concurrent-associate-web-acl-race`, `cloudfront.copy-distribution-independent-clone`, `cloudfront.create-invalidation-for-tenant-batch-too-large-fails`, `cloudfront.delete-distribution-tenant-in-use-connection-group-fails`, `cloudfront.disassociate-web-acl-invalid-argument-rejected`, `cloudfront.distribution-disable-then-delete-removes-resource`, `cloudfront.distribution-tag-untag-lifecycle`, `cloudfront.distribution-with-tags-lifecycle`, `cloudfront.invalidation-for-tenant-lifecycle`, `cloudfront.invalidation-idempotent-same-batch-returns-same-id`, `cloudfront.list-distribution-tenants-pagination-consistent-ordering`, `cloudfront.monitoring-subscription-lifecycle`, `cloudfront.op-contract-create-distribution`, `cloudfront.op-contract-deleteMonitoringSubscription`, `cloudfront.op-contract-get-distribution-config`, `cloudfront.tag-resource-on-cache-policy-arn-via-distribution`, `cloudfront.update-distribution-tenant-cname-conflict-fails`, `cloudfront.update-distribution-tenant-duplicate-customization-fails`, `cloudfront.web-acl-association-enforced-at-edge`, `cloudfront.web-acl-association-invalid-webacl-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
