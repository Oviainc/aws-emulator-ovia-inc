Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.batch-check-layer-availability-mixed ecr.batch-delete-image-partial-outcome ecr.batch-delete-image-untags-shared-digest ecr.complete-layer-upload-bad-uploadid-rejected ecr.complete-layer-upload-double-completion-rejected ecr.complete-layer-upload-empty-rejected ecr.complete-layer-upload-wrong-digest-rejected ecr.completed-upload-id-reuse-fails ecr.delete-repository-requires-force-when-not-empty ecr.describe-image-replication-status-image-not-found-rejected ecr.describe-image-signing-status-image-not-found-rejected ecr.download-url-for-layer-expires-after-ttl ecr.download-url-returns-exact-layer-bytes ecr.e2e-image-scan-lifecycle ecr.e2e-storage-class-and-replication-flow ecr.get-download-url-for-layer ecr.get-download-url-for-layer-url-expiry ecr.image-storage-class-transition ecr.image-storage-class-transition-state-machine ecr.image-storage-class-value-persists ecr.image-tag-immutable-mutation-rejected ecr.immutable-tag-exclusion-filter-scoped ecr.immutable-tag-mutability-blocks-retag ecr.layer-upload-lifecycle-complete ecr.lifecycle-policy-preview-expires-image ecr.lifecycle-preview-names-exact-expiring-image ecr.list-image-referrers-ordering-by-artifact-type ecr.list-image-referrers-ordering-reflects-push-order ecr.list-image-referrers-repository-not-found ecr.list-image-referrers-returns-oci-artifacts ecr.list-image-referrers-server-populated ecr.manifest-list-child-reference-integrity ecr.mutable-repo-allows-tag-overwrite ecr.oci-manifest-media-type-negotiation ecr.op-contract-batchCheckLayerAvailability ecr.op-contract-batchDeleteImage ecr.op-contract-batchGetImage ecr.op-contract-deleteRepository ecr.op-contract-listImages ecr.op-contract-putImageTagMutability ecr.push-image-tag-list-and-fetch ecr.push-untagged-then-multi-tag ecr.put-image-dangling-references-rejected ecr.put-image-digest-mismatch ecr.put-image-digest-mismatch-on-concurrent-push-rejected ecr.put-image-duplicate-identical-rejected ecr.put-image-immutable-tag-mutation-rejected ecr.put-image-layers-not-found ecr.put-image-tag-mutability-transition ecr.repository-delete-non-empty-rejected ecr.repository-policy-deny-blocks-layer-upload ecr.scan-not-found-before-scan-started ecr.start-image-scan-archived-image-blocked ecr.update-image-storage-class-double-archive-rejected ecr.update-image-storage-class-transition ecr.update-image-storage-class-unsupported-rejected ecr.update-image-storage-class-unsupported-transition-rejected ecr.update-storage-class-unsupported-rejected ecr.upload-layer-part-binary-blob-round-trip ecr.upload-layer-part-noncontiguous-range-rejected ecr.upload-layer-part-streaming-then-complete
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|InitiateLayerUpload|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ecr InitiateLayerUpload: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateLayerUpload

## Description
`InitiateLayerUpload` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/initiate_layer_upload.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

61 recorded unit(s) fail on their `InitiateLayerUpload` call, so each one unblocks only when `InitiateLayerUpload` works: `ecr.batch-check-layer-availability-mixed`, `ecr.batch-delete-image-partial-outcome`, `ecr.batch-delete-image-untags-shared-digest`, `ecr.complete-layer-upload-bad-uploadid-rejected`, `ecr.complete-layer-upload-double-completion-rejected`, `ecr.complete-layer-upload-empty-rejected`, `ecr.complete-layer-upload-wrong-digest-rejected`, `ecr.completed-upload-id-reuse-fails`, `ecr.delete-repository-requires-force-when-not-empty`, `ecr.describe-image-replication-status-image-not-found-rejected`, `ecr.describe-image-signing-status-image-not-found-rejected`, `ecr.download-url-for-layer-expires-after-ttl`, `ecr.download-url-returns-exact-layer-bytes`, `ecr.e2e-image-scan-lifecycle`, `ecr.e2e-storage-class-and-replication-flow`, `ecr.get-download-url-for-layer`, `ecr.get-download-url-for-layer-url-expiry`, `ecr.image-storage-class-transition`, `ecr.image-storage-class-transition-state-machine`, `ecr.image-storage-class-value-persists` (+41 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/initiate_layer_upload.rs`. Replaying the corpus, the emulator refused the `InitiateLayerUpload` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the InitiateLayerUpload operation: operation InitiateLayerUpload is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr emulator
Call InitiateLayerUpload and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
