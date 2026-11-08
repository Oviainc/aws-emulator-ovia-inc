Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-dashboard-create-update-and-invalid-body
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-dashboard-create-update-and-invalid-body cloudwatch-dashboard-crud-round-trip cloudwatch-dashboard-delete-frees-name-for-a-new-body cloudwatch-dashboard-empty-widgets-array-preserved cloudwatch-dashboard-round-trip-then-delete-confirmed-gone monitoring-dashboard-lifecycle monitoring-dashboard-put-get-delete-lifecycle monitoring-get-dashboard-and-notfound-rejected monitoring-dashboard-tags-and-not-found-boundary
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetDashboard|2xx->2xx|$.DashboardBody
Issue-Key: monitoring|ListDashboards|2xx->2xx|$.DashboardEntries[*]
Issue-Key: monitoring|ListDashboards|2xx->2xx|$.DashboardEntries[*].Size
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch dashboard bodies are re-serialized, breaking byte fidelity and Size

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboard

## Description
3 causes, one shared fault: PutDashboard's DashboardBody is parsed into a JSON value and re-serialized on the way out instead of being stored and returned as the opaque string AWS keeps, so the body text changes and every byte-derived field is wrong.

Cause 1 of 3: GetDashboard after PutDashboard. AWS: DashboardBody is returned as the EXACT string that was submitted — Smithy: GetDashboard output DashboardBody targets DashboardBody, a string; the service treats it as an opaque document and does not normalise its whitespace or key order. Emulator: returns a re-serialized form with a space inserted after every ':' and ',' (e.g. '{"widgets": [{"type": "text", …' for a submitted '{"widgets":[{"type":"text",…'). Why: a client that round-trips a dashboard (GetDashboard -> diff -> PutDashboard) sees a spurious change on every read, so drift detection and infrastructure-as-code reconcilers report a permanent diff and rewrite the dashboard forever. Fix: dashboard state in the CloudWatch crate must store the submitted body verbatim, and the GetDashboard serializer must emit it unchanged. Trace: cloudwatch-dashboard-create-update-and-invalid-body#task#2.

Cause 2 of 3: ListDashboards' DashboardEntry.Size. AWS: Size is the size of the stored dashboard IN BYTES — Smithy: DashboardEntry.Size targets Size (long), 'The size of the dashboard, in bytes' — i.e. the byte length of the body as submitted. Emulator: reports the byte length of its re-serialized body, which differs from the submitted body's length (the whitespace it inserts is not in the stored document; here 125 against the recorded 149). Why: a client budgeting against the dashboard size quota, or comparing Size to detect a change, reads a number that does not correspond to anything it submitted. Fix: the same verbatim storage, plus computing Size from the stored bytes (not from a re-render). Trace: cloudwatch-dashboard-create-update-and-invalid-body#task#3.

Cause 3 of 3: the same wrong DashboardEntry.Size on the put/get/delete lifecycle and the tags/not-found boundary reads (141 against the recorded 165). Same component as Cause 2 but reached through a different fixture family, so it must be re-replayed to confirm. Trace: monitoring-dashboard-put-get-delete-lifecycle#task#3.

Note on values: the recorded LastModified timestamps and the specific 149/165 byte counts come from the recorded bodies; the requirement is that Size equals the byte length of the body as submitted and that the body round-trips unchanged — not those numbers.

## Evidence
Replay (simparity-wire-aws-v1), 3 clusters.
1. GetDashboard, cloudwatch-dashboard-create-update-and-invalid-body#task#2: $.DashboardBody expected '{"widgets":[{"type":"text","x":0,"y":0,"width":6,"height":3,"properties":{"markdown":"## Original Dashboard"}}]}', got '{"widgets": [{"type": "text", "x": 0, "y": 0, "width": 6, "height": 3, "properties": {"markdown": "## Original Dashboard"}}]}'. 11 affected steps incl. cloudwatch-dashboard-crud-round-trip#task#2, monitoring-dashboard-lifecycle#task#2, monitoring-get-dashboard-and-notfound-rejected#task#2.
2. ListDashboards, cloudwatch-dashboard-create-update-and-invalid-body#task#3: $.DashboardEntries[0] expected Size 149, got Size 125 (DashboardName and DashboardArn match). 3 affected steps.
3. ListDashboards, monitoring-dashboard-put-get-delete-lifecycle#task#3: $.DashboardEntries[0].Size expected 165, got 141. (also monitoring-dashboard-tags-and-not-found-boundary#task#2)

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutDashboard a body with no whitespace between tokens, then GetDashboard it (cloudwatch-dashboard-create-update-and-invalid-body#task#2) and byte-compare DashboardBody against what was submitted.
3. ListDashboards (same fixture, task#3) and compare DashboardEntries[0].Size with the byte length of the submitted body.
4. Repeat the ListDashboards check on monitoring-dashboard-put-get-delete-lifecycle#task#3.

## Disposition

Closed with every cause DECLINED: the emulator already implements the requirement this report
states, and the recorded numbers count bytes the replay never submits to it.

* The dashboard document is stored and reported as the exact text the request carried
  (`PutDashboard` stores `DashboardBody` verbatim, `GetDashboard` returns it unchanged), and
  `DashboardEntry$Size` is the byte length of that stored text
  (`src/ops/list_dashboards.rs`). Both are pinned by `tests/all/opaque_document_fidelity.rs`
  and `tests/all/dashboard_body_byte_fidelity.rs`.
* Cause 1 is not a defect: the separator-spaced text the report calls a "re-serialized form"
  is the text the recorded `PutDashboard` request carries and the emulator holds; the compact
  string it calls "expected" is the corpus's minified rendering of the recorded response (the
  replay grader compares `DashboardBody` as JSON, not as text). The key for it replays clean.
* Causes 2 and 3 cannot be derived: the recorded `Size` exceeds the document the replayed
  request carries by a constant within a fixture family and a different amount between
  families (24 bytes for the three families of one generator; another value for the
  tags/not-found fixture, whose 106-byte document with one tag matches neither the document's
  bytes, nor those plus 24, nor those plus the tag's text, JSON member, key/value map,
  modelled `{Key,Value}` object, `TagList` array or a flat per-tag cost — each tried by
  replay). An excess independent of the document, its length, the dashboard's name and its
  tags is not a function of the request or of stored state, so reproducing it would mean
  writing a recorded number into the handler.
* Consequence: those replay units still diverge on `$.DashboardEntries[*].Size` only, by the
  byte counts above. Nothing else in them diverges.
