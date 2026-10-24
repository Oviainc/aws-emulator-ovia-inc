Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.e2e-image-push-scan-replicate-cleanup ecr.image-scan-on-archived-image-rejected ecr.image-storage-class-archival-blocks-mutating-ops ecr.start-image-scan-transitions-to-findings
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|StartImageScan|NotImplemented
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Implement ecr StartImageScan: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartImageScan

## Description
`StartImageScan` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/start_image_scan.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `StartImageScan` call, so each one unblocks only when `StartImageScan` works: `ecr.e2e-image-push-scan-replicate-cleanup`, `ecr.image-scan-on-archived-image-rejected`, `ecr.image-storage-class-archival-blocks-mutating-ops`, `ecr.start-image-scan-transitions-to-findings`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/start_image_scan.rs`. Replaying the corpus, the emulator refused the `StartImageScan` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartImageScan operation: operation StartImageScan is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr emulator
Call StartImageScan and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
