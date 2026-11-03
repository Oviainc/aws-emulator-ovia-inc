Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: supplychain.bom-import-job-fails-without-s3-access
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: supplychain.bom-import-job-fails-without-s3-access|run#4|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}: {message}"
Issue-Commit: 0ebbd81840eb1d8236e49c909db275b2251d47fa

# Implement supplychain CreateBillOfMaterialsImportJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0ebbd81840eb1d8236e49c909db275b2251d47fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBillOfMaterialsImportJob

## Description
`CreateBillOfMaterialsImportJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/create_bill_of_materials_import_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateBillOfMaterialsImportJob` call, so each one unblocks only when `CreateBillOfMaterialsImportJob` works: `supplychain.bom-import-job-fails-without-s3-access`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/create_bill_of_materials_import_job.rs`. Replaying the corpus, the emulator refused the `CreateBillOfMaterialsImportJob` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_supplychain_bom_import_job_fails_without_s3_access.py", line 43, in run
    assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}: {message}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected AccessDeniedException, got NotImplemented: operation CreateBillOfMaterialsImportJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the supplychain emulator
Call CreateBillOfMaterialsImportJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
