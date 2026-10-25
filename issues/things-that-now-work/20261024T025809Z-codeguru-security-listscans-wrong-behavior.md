Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30b4aefb4b2ff0fa6acb9bcc02c838fcaca2cb1f

# ListScans returns EXPRESS scans, which the operation documents it never returns

_Filed: 2026-10-24T02:58:09Z_
_Operation: ListScans | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30b4aefb4b2ff0fa6acb9bcc02c838fcaca2cb1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents ListScans as "Returns a list of all scans in an account. Does not return EXPRESS scans." The emulator's handler lists every scan recorded in the store, with no filter on the scan type of the scan's run, so a scan created with `scanType: Express` appears in `summaries` exactly like a Standard scan.

Expected: a scan whose run was started as Express is omitted from the ListScans response entirely (it is reachable only through GetScan by name), so an account with N Express scans and M Standard scans sees exactly M summaries and pagination/`nextToken` is computed over the Standard scans only.

Actual: the handler iterates `store.scans` and pushes a `ScanSummary` for every scan that has a latest run, including runs whose `scan_type` is `Express`. Express scans therefore show up in the list, inflate page contents, and consume page slots / shift the `nextToken` cursor, so a client paging ListScans against the emulator sees entries real AWS never returns.

## Evidence

Spec: Smithy operation documentation for `com.amazonaws.codegurusecurity#ListScans` — "Returns a list of all scans in an account. Does not return EXPRESS scans." (smithy_query query=doc shape=ListScans). The scan type is available in state: `crate::state::ScanRun.scan_type` is stored by CreateScan (`src/ops/create_scan.rs` lines ~282-353) and `SCAN_TYPE_EXPRESS` is defined in `src/state.rs`; GetScan/ListScans already consult it for the `updatedAt` gate via `Scan::updated_at_for`.

Handler: `src/ops/list_scans.rs` lines 207-229 — the page loop only skips scans with no run (`let Some(run) = scan.latest_run() else { continue; }`) and then unconditionally pushes a `ScanSummary`; `run.scan_type` is read only for `updated_at_for`, never to exclude the scan.

Tests: `tests/all/scan_lifecycle.rs` exercises Express only for the settle span (`wait_for_settle`) and the scanType/analysisType default matrix (cases at lines ~594-600) and pages ListScans over Standard scans only (`list_scans_pages_every_scan_exactly_once_for_every_page_size`). No test asserts that an Express scan is absent from ListScans, so the divergence is uncovered.

## Steps to Reproduce

1. POST /uploadUrl with {"scanName": "express-scan"} and note the returned codeArtifactId.
2. POST /scans with {"scanName": "express-scan", "scanType": "Express", "resourceId": {"codeArtifactId": "<id>"}} -> 200.
3. POST /uploadUrl + POST /scans the same way for "standard-scan" with "scanType": "Standard".
4. GET /scans (ListScans) -> observe two summaries, one of them "express-scan". Expected: only "standard-scan" is returned, because ListScans does not return EXPRESS scans.
5. Also GET /scans?maxResults=1 and follow nextToken: the Express scan occupies a page slot and shifts the cursor, so every page boundary differs from real AWS.
