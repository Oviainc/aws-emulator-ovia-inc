Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# Every scan run settles to Failed with an emulator-specific errorMessage, so scanState Successful is unreachable

_Filed: 2026-10-24T02:28:10Z_
_Operation: GetScan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a scan created over an uploaded code artifact runs and completes successfully — GetScan (and ListScans / a later CreateScan read-back) reports scanState=InProgress and then Successful, with no errorMessage, and errorMessage is populated only when the scan genuinely failed ("Details about the error that causes a scan to fail to be retrieved", GetScanResponse$errorMessage).

Actual: the emulator hard-codes failure for every run. src/state.rs declares SCAN_NOT_ANALYSED_MESSAGE ("Why every scan run this emulator records ends in `Failed` rather than `Successful`") and Store::settle_due_runs unconditionally sets run.scan_state = SCAN_STATE_FAILED plus that fixed errorMessage once the run's settle instant passes. No code path can ever produce the modelled `Successful` state: grep for SCAN_STATE_SUCCESSFUL / "Successful" in src/ finds only the enum value list. So the canonical workflow (CreateUploadUrl -> CreateScan -> GetScan until done) always ends in a failed scan carrying a message no AWS response would contain, and a client that branches on scanState=="Successful" (or that treats Failed as fatal) can never proceed.

## Evidence

Smithy model: ScanState = Failed | InProgress | Successful (GetScanResponse$scanState: "Returns either InProgress, Successful, or Failed"); GetScanResponse$errorMessage documents an error that caused the scan to fail. Handler: src/ops/get_scan.rs lines 215-233 reports run.scan_state / run.error_message verbatim; the write is src/state.rs Store::settle_due_runs (run.scan_state = SCAN_STATE_FAILED, run.error_message = Some(SCAN_NOT_ANALYSED_MESSAGE)). Tests ASSERT the divergent behavior rather than the real one: tests/all/scan_lifecycle.rs lines 398-409 require scanState == SCAN_STATE_FAILED and a non-empty errorMessage, so no test covers a successful scan.

## Steps to Reproduce

1. POST /uploadUrl with {"scanName":"demo"} and note codeArtifactId
2. POST /scans with that resourceId.codeArtifactId and scanName=demo (scanType Standard) -> 200, scanState InProgress
3. Wait >500ms, then GET /scans/demo
4. Observe scanState="Failed" and an errorMessage about the emulator having no S3 data plane; real CodeGuru Security reports Successful with no errorMessage
