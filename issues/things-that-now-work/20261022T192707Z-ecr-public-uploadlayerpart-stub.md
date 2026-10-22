Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.batch-check-layer-availability ecr-public.batch-delete-image-partial-failure ecr-public.chunked-layer-upload-offsets-are-enforced-byte-exactly ecr-public.complete-layer-upload-digest-mismatch-rejected ecr-public.complete-layer-upload-not-found ecr-public.concurrent-complete-layer-upload-same-uploadid-converges ecr-public.data-plane-accepts-registry-alias-and-normalizes-to-account-id ecr-public.delete-repository-force ecr-public.delete-repository-not-empty ecr-public.describe-image-tags-pagination-partitions-the-tag-set-exactly ecr-public.describe-images-not-found ecr-public.e2e-image-publish-and-catalog-flow ecr-public.e2e-image-push-pull-tag-delete-flow ecr-public.e2e-put-image-missing-layers-rejected ecr-public.image-digest-mismatch-rejected ecr-public.image-push-lifecycle ecr-public.image-pushed-at-is-a-live-clock-stamp-that-reads-do-not-move ecr-public.layer-blob-byte-equality-round-trip ecr-public.manifest-list-image-size-is-the-max-of-its-children ecr-public.put-image-duplicate-rejected ecr-public.put-image-duplicate-tag-moves-tag ecr-public.put-image-missing-layer-rejected ecr-public.put-image-rejects-manifest-with-unuploaded-layer ecr-public.put-image-with-a-digest-mismatched-manifest-rejected-and-nothing-stored ecr-public.response-shapes-omit-private-ecr-only-members ecr-public.server-side-manifest-schema-validation-enforced ecr-public.server-side-public-repository-always-readable ecr-public.upload-layer-part-binary-blob-preserved ecr-public.upload-layer-part-exceeds-max-size-rejected ecr-public.upload-layer-part-offset-gap-rejected ecr-public.upload-layer-part-sequential-ordering-enforced ecr-public.upload-layer-part-unknown-upload-id-rejected
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|UploadLayerPart|NotImplemented
Issue-Key: ecr-public.upload-layer-part-sequential-ordering-enforced|run#12|AssertionError|assert (err.get("Code"), err.get("Message")) == expected, f"expected {expected}, got {err}"
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement ecr-public UploadLayerPart: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UploadLayerPart

## Description
`UploadLayerPart` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecrpublic-spencerfrontendservice/src/ops/upload_layer_part.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

32 recorded unit(s) fail on their `UploadLayerPart` call, so each one unblocks only when `UploadLayerPart` works: `ecr-public.batch-check-layer-availability`, `ecr-public.batch-delete-image-partial-failure`, `ecr-public.chunked-layer-upload-offsets-are-enforced-byte-exactly`, `ecr-public.complete-layer-upload-digest-mismatch-rejected`, `ecr-public.complete-layer-upload-not-found`, `ecr-public.concurrent-complete-layer-upload-same-uploadid-converges`, `ecr-public.data-plane-accepts-registry-alias-and-normalizes-to-account-id`, `ecr-public.delete-repository-force`, `ecr-public.delete-repository-not-empty`, `ecr-public.describe-image-tags-pagination-partitions-the-tag-set-exactly`, `ecr-public.describe-images-not-found`, `ecr-public.e2e-image-publish-and-catalog-flow`, `ecr-public.e2e-image-push-pull-tag-delete-flow`, `ecr-public.e2e-put-image-missing-layers-rejected`, `ecr-public.image-digest-mismatch-rejected`, `ecr-public.image-push-lifecycle`, `ecr-public.image-pushed-at-is-a-live-clock-stamp-that-reads-do-not-move`, `ecr-public.layer-blob-byte-equality-round-trip`, `ecr-public.manifest-list-image-size-is-the-max-of-its-children`, `ecr-public.put-image-duplicate-rejected` (+12 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecrpublic-spencerfrontendservice/src/ops/upload_layer_part.rs`. Replaying the corpus, the emulator refused the `UploadLayerPart` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UploadLayerPart operation: operation UploadLayerPart is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr-public emulator
Call UploadLayerPart and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
