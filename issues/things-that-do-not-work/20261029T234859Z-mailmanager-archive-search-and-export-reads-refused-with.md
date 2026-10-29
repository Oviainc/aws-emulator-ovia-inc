Issue-Source: trace-sweep-divergence
Issue-Trace: mailmanager-archive-export-and-message-negative-surface
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mailmanager-archive-export-and-message-negative-surface mailmanager-archive-search-lifecycle
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|ListArchiveExports|2xx->4xx|$.Exports
Issue-Key: mailmanager|ListArchiveExports|2xx->4xx|status
Issue-Key: mailmanager|StartArchiveSearch|2xx->4xx|$.SearchId
Issue-Key: mailmanager|StartArchiveSearch|2xx->4xx|status
Issue-Key: mailmanager|ListArchiveSearches|2xx->4xx|$.Searches
Issue-Key: mailmanager|ListArchiveSearches|2xx->4xx|status
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# mailmanager archive search and export reads refused with 400 ValidationException instead of serving the modeled 200

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListArchiveExports

## Description
Three mailmanager archive-lifecycle operations refuse well-formed requests at the request boundary with HTTP 400 instead of answering the modeled 200 response. The refusal is produced BEFORE the operation handler runs: all three handlers are still scaffolded and would answer HTTP 501 NotImplemented (Scaffolded::not_implemented renders 501 — services/service-com-amazonaws-mailmanager-mailmanagersvc/src/scaffolded.rs:285), yet the replay observes 400, so the centralized request walk (deserialize -> validate -> handle) rejects the input during validation and never reaches the handler. The only input member the three operations have in common is ArchiveId (ArchiveId: @length min 3 max 66, @pattern ^a-[\w]{1,64}$). Hypothesis: boundary validation mis-evaluates that ArchiveId constraint (or a required-member presence test) and refuses an identifier AWS accepts; the model and the recordings only establish that AWS answers 200 for these requests. THIS ISSUE BUNDLES 3 CAUSES — all three must be fixed before it is closed.

Cause 1 of 3: ListArchiveExports (fixture mailmanager-archive-export-and-message-negative-surface, step task#2).
AWS: Smithy: ListArchiveExports input ArchiveId (required, @length 3..66, @pattern ^a-[\w]{1,64}$), NextToken (PaginationToken @length 1..2048), PageSize (@range 1..50); output ListArchiveExportsResponse with Exports (ExportSummaryList of ExportSummary{ExportId, Status}) and optional NextToken; @readonly, @paginated(items: Exports); modeled errors AccessDeniedException 403, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400. For an existing archive with no export jobs AWS answers HTTP 200 with Exports present as an empty list — a listing of zero items is a success, never a client error.
Emulator: HTTP 400. The response carries none of the modeled output members: $.Exports is absent where an empty list was recorded.
Why: a client paging export jobs cannot distinguish "no exports yet" from "bad request". SDK clients raise ValidationException on a request the service documents as valid, so every archive-export listing/polling flow aborts, and the 400 is not retried.
Fix: the boundary input validation for this operation's ArchiveId/pagination members (it must accept any value satisfying the modeled @length and @pattern), then the ListArchiveExports handler so it returns 200 with Exports (empty list when the archive has no export jobs).

Cause 2 of 3: StartArchiveSearch (fixture mailmanager-archive-search-lifecycle, step task#2).
AWS: Smithy: StartArchiveSearch input ArchiveId (required, @length 3..66, @pattern ^a-[\w]{1,64}$), FromTimestamp (Timestamp, required), ToTimestamp (Timestamp, required), MaxResults (SearchMaxResults, required, @range 0..1000), Filters (ArchiveFilters, optional); output StartArchiveSearchResponse with SearchId (@length 1..64); modeled errors AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400. AWS answers HTTP 200 and a freshly minted SearchId for a valid archive and timestamp range.
Emulator: HTTP 400. $.SearchId is absent (the recording asserts only that a SearchId is present and shaped, not its literal value).
Why: the search job is never created and no SearchId is handed back, so the whole archive-search lifecycle (GetArchiveSearch / ListArchiveSearches / GetArchiveSearchResults) is unreachable for a client that follows the documented flow.
Fix: the boundary input validation for this operation (it must admit the modeled required members and the ArchiveId pattern), then the StartArchiveSearch handler and the search-job state it records, returning 200 with a SearchId of 1..64 characters.

Cause 3 of 3: ListArchiveSearches (fixture mailmanager-archive-search-lifecycle, step task#5).
AWS: Smithy: ListArchiveSearches input ArchiveId (required, @length 3..66, @pattern ^a-[\w]{1,64}$), NextToken, PageSize (@range 1..50); output ListArchiveSearchesResponse with Searches (SearchSummaryList of SearchSummary{SearchId, Status: SearchStatus{SubmissionTimestamp, CompletionTimestamp, State (SearchState enum), ErrorMessage}}) and optional NextToken; @readonly, @paginated(items: Searches); modeled errors AccessDeniedException 403, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400. AWS answers HTTP 200 and lists every search job previously started on that archive, each with its SearchId and a Status whose State reflects the job's progress (COMPLETED once finished, with SubmissionTimestamp and CompletionTimestamp set).
Emulator: HTTP 400. $.Searches is absent where the recording expects one SearchSummary for the search started earlier in the same fixture, with Status.State == COMPLETED and both timestamps present (the SearchId and the timestamp values are per-run values, not requirements; the member presence and the State value are).
Why: a client polling for search completion gets a client error instead of job status, so it can neither observe COMPLETED nor fetch results; combined with Cause 2, the archive-search feature is entirely unusable.
Fix: the boundary input validation for this operation, then the ListArchiveSearches handler so it projects the search jobs held in this service's state into Searches with the modeled SearchStatus members.

Note: no open issue covers these steps (list_existing_issues reports no mailmanager issues at all), and none of the three divergences is attributed to an unimplemented-operation stub by the replay report (stub_path empty, owned_by empty), because the 400 is returned before the scaffolded handler's 501 can be.

Traces: mailmanager-archive-export-and-message-negative-surface#task#2 (ListArchiveExports), mailmanager-archive-search-lifecycle#task#2 (StartArchiveSearch), mailmanager-archive-search-lifecycle#task#5 (ListArchiveSearches).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up: true, bootstrap_signal: false, /__admin/health 200):

1) fixture mailmanager-archive-export-and-message-negative-surface, step task#2, operation ListArchiveExports, service mailmanager, verdict mismatch
   expected_status 200, actual_status 400
   detail: "status: expected 200, got 400" / "$.Exports: missing (expected [], got None)"
   divergence_key: service=mailmanager|ListArchiveExports|status=200->400|keys=$.Exports,status

2) fixture mailmanager-archive-search-lifecycle, step task#2, operation StartArchiveSearch, service mailmanager, verdict mismatch
   expected_status 200, actual_status 400
   detail: "status: expected 200, got 400" / "$.SearchId: missing (expected '<shaped-not-asserted>', got None)"
   divergence_key: service=mailmanager|StartArchiveSearch|status=200->400|keys=$.SearchId,status

3) fixture mailmanager-archive-search-lifecycle, step task#5, operation ListArchiveSearches, service mailmanager, verdict mismatch
   expected_status 200, actual_status 400
   detail: "status: expected 200, got 400" / "$.Searches: missing (expected [{'SearchId': '{{search_id}}', 'Status': {'CompletionTimestamp': '<shaped-not-asserted>', 'State': 'COMPLETED', 'SubmissionTimestamp': '<shaped-not-asserted>'}}], got None)"
   divergence_key: service=mailmanager|ListArchiveSearches|status=200->400|keys=$.Searches,status

Expected (AWS, per services/service-com-amazonaws-mailmanager-mailmanagersvc/smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json): HTTP 200 with the operation's modeled output — Exports (possibly empty) for ListArchiveExports, SearchId for StartArchiveSearch, Searches with SearchSummary.Status.State for ListArchiveSearches.
Actual (emulator): HTTP 400 for all three, with no modeled output member present. A 400 here is not even the scaffold signal: the handlers are still stubs returning Scaffolded::not_implemented, which renders 501 (services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/list_archive_exports.rs:166, src/ops/start_archive_search.rs:173, src/scaffolded.rs:285), so the request is refused during boundary validation before `handle`.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (admin health on /__admin/health, data listener accepting).
2. Replay the wire fixture corpus simparity-wire-aws-v1.
3. Cause 1: replay fixture mailmanager-archive-export-and-message-negative-surface and look at step task#2 — a ListArchiveExports call on the fixture's archive. Recorded 200 with Exports: []; emulator answers 400 with no Exports member.
4. Cause 2: replay fixture mailmanager-archive-search-lifecycle and look at step task#2 — a StartArchiveSearch call carrying ArchiveId, FromTimestamp, ToTimestamp and MaxResults. Recorded 200 with a SearchId; emulator answers 400 with no SearchId member.
5. Cause 3: same fixture, step task#5 — a ListArchiveSearches call on the same archive after the search completes. Recorded 200 with one SearchSummary whose Status.State is COMPLETED and whose SubmissionTimestamp/CompletionTimestamp are set; emulator answers 400 with no Searches member.
6. All three steps must replay green (status 200 and the modeled output members present) before this issue is closed.
