Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-catalog-list-reports
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-catalog-list-reports artifact-list-pagination artifact-report-term-and-download artifact-compliance-inquiry-basic-workflow
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|ListReports|2xx->2xx|$.reports[arn="arn
Issue-Key: artifact|ListReports|2xx->2xx|$.reports[name="AWS Outposts Rack Information Security FAQ"].description
Issue-Key: artifact|ListComplianceInquiryQueries|2xx->2xx|$.queries[*]
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# artifact read plane serves an incomplete report catalog and an empty inquiry query list

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListReports

## Description
Three net-new root causes, all in the artifact READ plane: the report catalog ListReports projects is missing entries and misstates the content of entries it does hold, and ListComplianceInquiryQueries projects no queries for an inquiry that owns them. Each cause is independent and must be closed separately.

Cause 1 of 3 — ListReports omits published reports from the catalog (fixture artifact-catalog-list-reports, step task#1).
AWS: ListReports (Smithy: restJson1 GET /v1/report/list, output ListReportsResponse.reports: ReportsList of ReportDetail, paginated inputToken/outputToken=nextToken, pageSize=maxResults, items=reports) returns every PUBLISHED report version visible to the caller, each as a ReportDetail carrying id/arn/name/description/series/category/companyName/productName/state/version/visibility/acceptanceType and the period timestamps. The recording shows the catalog containing report versions the emulator does not serve at all — for example the ACR/VPAT series version, the UAE TDRA IAR assessment version, and a further ACR version — identified by their report ARNs of the form arn:aws:artifact:<region>::report/<reportId>:<version>.
Emulator: HTTP 200 with a reports list that does not contain those entries: $.reports[arn="arn:aws:artifact:us-east-1::report/report-7g5j2DTVeNXWx7St:1"] is missing, as are the entries for report-5eevElBv6MyRmQsD:7 and report-0c1IXmudeXuTlHJJ:3 (replay: "missing (expected {...}, got None)" for each ARN). The report ids, region and version numbers above are the recording's values filling the ARN template, not the requirement; the requirement is that the catalog the emulator seeds covers the report series AWS publishes, so no catalog ARN a client reads back is absent.
Why: a client that enumerates the catalog and then calls GetTermForReport/GetReport for a report it found cannot reach any report the catalog never lists — the whole term-acceptance and download flow for those series is unreachable, and compliance tooling that reconciles an expected report inventory reports false negatives. The same shortfall changes how many pages the listing has, so paginated walks over the catalog diverge too (this cluster also covers artifact-list-pagination#task#1 and #task#2).
Fix: the report catalog seed backing ListReports (the service crate's report state / catalog fixture), not the serializer — the entries are absent from state, not dropped on the wire.
Traces: artifact-catalog-list-reports#task#1 (representative); same cluster: artifact-list-pagination#task#1, artifact-list-pagination#task#2.

Cause 2 of 3 — a seeded report's description is emulator-authored prose instead of the published description (fixture artifact-report-term-and-download, step task#1).
AWS: ReportDetail.description (Smithy: LongStringAttribute, @length 1..2048, @pattern ^[^<>]*$) carries the report's full published description text as AWS authors it — for the Outposts rack information-security FAQ series the recorded text is the multi-sentence description beginning "The AWS Outposts Information Security FAQ document is designed to help customer teams responsible for information security evaluate the service against their organizational requirements..." (recorded behavior only; the model constrains the member's shape, not its content).
Emulator: HTTP 200, but $.reports[name="AWS Outposts Rack Information Security FAQ"].description is the summary string "Information security FAQ for AWS Outposts rack" — a hand-written one-line stand-in rather than the published description. The defect is general: catalog rows are seeded with abbreviated, emulator-authored descriptions instead of the service's own text.
Why: a client that displays or indexes report descriptions (a console listing, a compliance-document picker, a test asserting the catalog row it expects) renders text AWS never returns, and any consumer matching on description content fails to find the report.
Fix: the report catalog seed data backing ListReports/GetReportMetadata — the description field of each catalog row must hold the published description, not a summary.
Traces: artifact-report-term-and-download#task#1.

Cause 3 of 3 — ListComplianceInquiryQueries returns an empty queries list for an inquiry that owns queries (fixture artifact-compliance-inquiry-basic-workflow, step task#3).
AWS: ListComplianceInquiryQueries (Smithy: restJson1 GET /v1/compliance-inquiry/listQueries, output queries: QueriesList of QuerySummary; QuerySummary requires queryIdentifier, query, status, statusMessage, createdAt and may carry response, reviewType, citations: CitationList of Citation{sourceLabel,sourceContent,sourceLink}, updatedResponseVersions) lists one QuerySummary per query the named inquiry owns, each with the answer text and the citations that grounded it.
Emulator: HTTP 200 with no element at index 0 — $.queries[0] is missing where the recording has a completed QuerySummary whose citations carry sourceContent/sourceLabel/sourceLink triples for the source documents the response cites. So an inquiry created and processed through the recorded workflow projects zero queries.
Why: the query list is the only way a client reads an inquiry's answers and their citations; with an empty list the inquiry appears to have produced nothing, and every downstream step keyed on a queryIdentifier (PutComplianceInquiryFeedback, ExportComplianceInquiry's queryIdentifiers) has no identifier to name.
Fix: the compliance-inquiry state that materialises queries for an inquiry, and the ListComplianceInquiryQueries handler's projection of QuerySummary (including the citations member) — not pagination, since the first page itself is empty.
Traces: artifact-compliance-inquiry-basic-workflow#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (score 70.0%, 9 of 30 steps mismatched).

Cause 1 — fixture artifact-catalog-list-reports step task#1, ListReports, expected 200 / actual 200:
  $.reports[arn="arn:aws:artifact:us-east-1::report/report-7g5j2DTVeNXWx7St:1"]: missing (expected the PUBLISHED ACR/VPAT ReportDetail, got None)
  $.reports[arn="arn:aws:artifact:us-east-1::report/report-5eevElBv6MyRmQsD:7"]: missing (expected the UAE TDRA IAR ReportDetail, got None)
  $.reports[arn="arn:aws:artifact:us-east-1::report/report-0c1IXmudeXuTlHJJ:3"]: missing (expected an ACR ReportDetail, got None)
  cluster also covers artifact-list-pagination#task#1 and artifact-list-pagination#task#2 (affected_count 3)

Cause 2 — fixture artifact-report-term-and-download step task#1, ListReports, expected 200 / actual 200:
  $.reports[name="AWS Outposts Rack Information Security FAQ"].description: value_mismatch
    expected: 'The AWS Outposts Information Security FAQ document is designed to help customer teams responsible for information security evaluate the service against their organizational requirements. The document is aligned to common questions relating to people, process, and technology that this audience often ask. This document is reviewed and updated regularly with the latest information.'
    actual:   'Information security FAQ for AWS Outposts rack'

Cause 3 — fixture artifact-compliance-inquiry-basic-workflow step task#3, ListComplianceInquiryQueries, expected 200 / actual 200:
  $.queries[0]: missing (expected a QuerySummary with citations[] carrying sourceContent/sourceLabel/sourceLink for the cited sources, got None)

Smithy (services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json): ListReports -> ListReportsResponse.reports: ReportsList<ReportDetail>, @paginated items=reports; ReportDetail.description: LongStringAttribute @length 1..2048 @pattern ^[^<>]*$; ListComplianceInquiryQueries -> queries: QueriesList<QuerySummary>, QuerySummary.citations: CitationList<Citation{sourceLabel,sourceContent,sourceLink}>.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: GET /v1/report/list (ListReports) with no filter and compare the returned report ARNs against the recorded catalog — the ACR/VPAT, UAE TDRA IAR and further ACR report versions in fixture artifact-catalog-list-reports#task#1 are absent. Walk the same listing with maxResults set (artifact-list-pagination#task#1/#task#2) to see the page contents diverge for the same reason.
3. Cause 2: in the same listing, read the row whose name is the Outposts rack information-security FAQ (fixture artifact-report-term-and-download#task#1) and compare description against the recorded published text — the emulator serves a one-line summary.
4. Cause 3: run fixture artifact-compliance-inquiry-basic-workflow to create and process an inquiry, then GET /v1/compliance-inquiry/listQueries?complianceInquiryId=<inquiryId> (step task#3) — the queries list is empty where the recording returns a QuerySummary with citations.
