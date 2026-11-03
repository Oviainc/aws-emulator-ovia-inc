Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisanalyticsv2.studio-presigned-url-for-a-ready-notebook-rejected
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2.studio-presigned-url-for-a-ready-notebook-rejected|run#8|AssertionError|assert got == expected, (
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# Implement kinesisanalyticsv2 CreateApplicationPresignedUrl: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationPresignedUrl

## Description
`CreateApplicationPresignedUrl` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/create_application_presigned_url.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateApplicationPresignedUrl` call, so each one unblocks only when `CreateApplicationPresignedUrl` works: `kinesisanalyticsv2.studio-presigned-url-for-a-ready-notebook-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/create_application_presigned_url.rs`. Replaying the corpus, the emulator refused the `CreateApplicationPresignedUrl` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_kinesisanalyticsv2_studio_presigned_url_for_a_ready_notebook_rejected.py", line 39, in run
    assert got == expected, (
           ^^^^^^^^^^^^^^^
AssertionError: CreateApplicationPresignedUrl(ZEPPELIN_UI_URL) on a READY notebook answered ('NotImplemented', 'operation CreateApplicationPresignedUrl is scaffolded but not implemented in this emulator'), expected ('ResourceInUseException', "CreateApplicationPresignedUrl operation cannot be performed for Application in 'READY' state")

## Steps to Reproduce
Boot the kinesisanalyticsv2 emulator
Call CreateApplicationPresignedUrl and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
