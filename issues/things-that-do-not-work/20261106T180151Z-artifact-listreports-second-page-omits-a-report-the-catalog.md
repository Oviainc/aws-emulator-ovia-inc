Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-list-pagination
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-list-pagination
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|ListReports|2xx->2xx|$.reports[arn="arn:aws:artifact:us-east-1::report/report-0QjvdIs6Bsw4uPxb:2"]
Issue-Commit: 1cef0270414d463e8e4cd5e638402f2f0968b2f1

# ListReports second page omits a report the catalog holds

> _Note: this issue was identified against the code base at commit `1cef0270414d463e8e4cd5e638402f2f0968b2f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListReports

## Description
One net-new root cause in the artifact READ plane: the continuation page of a ListReports enumeration answers HTTP 200 with a well-formed envelope whose $.reports collection does not contain a report the service is supposed to be holding, so a client paging the catalog to the end silently ends up with an incomplete catalog.

Cause 1 of 1 - ListReports paged enumeration is short a report (fixture artifact-list-pagination, step task#2).
AWS: ListReports (Smithy, services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json: restJson1 GET /v1/report/list, http code 200; input maxResults @httpQuery @range 1..300, nextToken @httpQuery @length 1..2048; output {reports: ReportsList, nextToken @length 1..2048}; @paginated inputToken=nextToken outputToken=nextToken pageSize=maxResults items=reports; @readonly; errors AccessDeniedException 403, ValidationException 400, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, InternalServerException 500). The operation enumerates the service's whole report catalog, one page at a time; every report the catalog holds must appear in exactly one page of the enumeration, carrying its full ReportSummary (Smithy ReportSummary: id @pattern ^report-[a-zA-Z0-9]{16}$, arn, name, series, category, companyName, productName, description, state: PublishedState, version @range min 1, periodStart/periodEnd @timestampFormat date-time, acceptanceType, uploadState, statusMessage), keyed by its arn (report id plus a ':<version>' suffix). Which reports exist is not bounded by the model, so membership is recorded behavior only: the recording's continuation page contains PUBLISHED, PUBLIC-visibility, PASSTHROUGH-acceptance reports of an accessibility-conformance (ACR/VPAT) series, each with its own arn/id/version/series/category/description and period bounds.
Emulator: HTTP 200 (status matches, no error), but the entry the continuation page must contain is absent - the replay reports '$.reports[arn="<report arn>"]: missing (expected {...full ReportSummary...}, got None)'. No extra entries and no nextToken divergence are reported, so the envelope and the paging cursor are right and the catalog behind them is short by this report.
Why: ListReports is the only discovery path for AWS Artifact reports - a client enumerates every page to find the report it then accepts terms for (GetTermForReport) and downloads (GetReport). A 200 page that simply omits a report is indistinguishable from that report not existing, so the client concludes the compliance document is unavailable and never requests it; a client that pages to the end and caches the result persists an incomplete catalog with no error, retry or fallback path.
Fix: the report catalog the ListReports handler enumerates - the catalog seed/materialisation in the artifact crate's state plus the ListReports handler that pages over it (services/service-com-amazonaws-artifact-artifact/src/ops/, src/state.rs). The paging envelope itself (nextToken emission) is not the defect.
Traces: artifact-list-pagination#task#2.

Note on neighbouring open issues: open issue 20261030T084723Z-artifact-list-operations-answer-200-with-the-recorded describes the same catalog-membership root cause, but its replay pointer is bound only to artifact-catalog-list-reports#task#1 (its Issue-Trace-Ids name 'artifact-list-pagination' with no step, so this cluster is not covered by its pointer and the replay reports this failure as unowned). Closing that issue therefore does not re-run this page; this issue exists so the continuation page of the enumeration is actually replayed. 20261029T112047Z-artifact-listreports-pagination concerns the pagination cursor's seal taking no server-side secret (a client-assembled cursor being honoured) - that is cursor integrity, not catalog membership, and sealing the cursor would still enumerate a short catalog. 20261029T192742Z-artifact-refusal-plane-answers-200-or-401-where covers refusal-plane causes where an expected 400 is answered 200/401, which this 200->200 payload-membership divergence is outside of.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, score_pct 76.7, 30 steps total, 23 passed, 7 mismatched, 7 divergence clusters; 6 of the 7 clusters are already owned by open issues (20261029T192742Z-artifact-refusal-plane-answers-200-or-401-where, 20261030T084723Z-artifact-list-operations-answer-200-with-the-recorded), this one is reported with owned_by empty and stub_path empty.

Fixture artifact-list-pagination, step task#2, operation ListReports (divergence_key service=artifact|ListReports|status=200->200|keys=$.reports[arn="..."]; verdict mismatch; affected_count 1):
  status: expected 200, got 200 (no status divergence, no error code, no error message involved)
  $.reports[arn="arn:aws:artifact:<region>::report/<reportId>:<version>"]: missing (expected the full ReportSummary for that report - acceptanceType PASSTHROUGH, category 'Certifications and Attestations', companyName AWS, productName AWS, series ACR, state PUBLISHED, visibility PUBLIC, the report's description, name, id, version and periodStart - got None).
  The arn, report id, version, name and period timestamp in the recording are that run's values filling the ReportSummary template; the requirement is that every report the catalog holds is enumerated on exactly one page with its full summary, not that one particular id exists.
  No unexpected/extra $.reports entries and no nextToken divergence were reported.

Smithy cited: ListReports GET /v1/report/list -> 200, @paginated items=reports inputToken/outputToken=nextToken pageSize=maxResults, @readonly, errors 403/400/404/402/429/500; ReportSummary members as listed in the description.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1, fixture artifact-list-pagination, through step task#2: the first step requests GET /v1/report/list with maxResults so the response carries a nextToken, and step task#2 requests GET /v1/report/list?nextToken=<the token returned> (plus the same maxResults).
3. Expect HTTP 200 whose $.reports enumerates the next slice of the catalog, each entry a full ReportSummary keyed by its arn; observe 200 with the recorded report arn absent from $.reports.
4. Union the $.reports entries across every page until nextToken is absent and compare with the catalog the recording enumerates: the union is short by the missing report, confirming it is catalog membership rather than one page's slicing.
