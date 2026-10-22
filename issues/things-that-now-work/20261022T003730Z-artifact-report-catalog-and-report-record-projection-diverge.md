Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-catalog-list-reports
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-catalog-list-reports artifact-list-pagination artifact-report-term-and-download artifact-reports-catalog-browsing
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|ListReports|2xx->2xx|$.reports[arn="arn
Issue-Key: artifact|ListReports|2xx->2xx|$.nextToken
Issue-Key: artifact|ListReports|2xx->2xx|$.reports[name="AWS Outposts Rack Information Security FAQ"]
Issue-Key: artifact|ListReports|2xx->2xx|$.reports[*]
Issue-Key: artifact|GetReportMetadata|2xx->2xx|$.reportDetails.createdAt
Issue-Key: artifact|GetReportMetadata|2xx->2xx|$.reportDetails.periodEnd
Issue-Key: artifact|GetReportMetadata|2xx->2xx|$.reportDetails.periodStart
Issue-Key: artifact|GetReportMetadata|2xx->2xx|$.reportDetails.visibility
Issue-Key: artifact|ListReportVersions|2xx->2xx|$.reports[arn="arn
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Artifact report catalog and report-record projection diverge from AWS

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListReports

## Description
Six causes, all in the artifact emulator's report read path (sdkId Artifact, restJson1). Every one of them is a 200 response whose body does not match what AWS Artifact returns: the seeded catalog is not AWS's published-report set, the pagination token is not opaque, and the ReportSummary/ReportDetail projection serializes timestamps and members wrongly. The replay reported state_reset_failures=0, so none of these are cross-fixture state bleed.

Cause 1 of 6 - ListReports does not serve AWS's published report catalog.
AWS: Smithy: ListReports -> ListReportsResponse.reports : list<ReportSummary>, GET /v1/report/list, 200. The catalog contents themselves are recorded behavior only: the recording returns the AWS Artifact published catalog, with report resources whose id matches ReportSummary.id @pattern ^report-[a-zA-Z0-9]{16}$ and whose arn has the form arn:aws:artifact:<region>::report/<reportId>:<version> (e.g. id report-7g5j2DTVeNXWx7St, series ACR, version 1; id report-5eevElBv6MyRmQsD, series 'UAE TDRA IAR', version 7).
Emulator: answers 200 but with a small placeholder catalog (ids of the form report-cloudsecalliance, report-iso270012022cert - not matching the modeled 16-char id pattern), so every recorded report arn is absent: '$.reports[arn="arn:aws:artifact:us-east-1::report/report-7g5j2DTVeNXWx7St:1"]: missing ... got None', likewise for report-5eevElBv6MyRmQsD:7 and report-0c1IXmudeXuTlHJJ:3.
Why: a client that lists the catalog and then drives GetReportMetadata / GetTermForReport / GetReport for a specific report id or series cannot find any real report, so every downstream agreement-acceptance and download flow is unreachable against the emulator.
Fix: the seeded catalog state behind the ListReports handler.
Traces: artifact-catalog-list-reports#task#1.

Cause 2 of 6 - the pagination token is a raw report id, not an opaque token, and the paged page is missing recorded reports.
AWS: Smithy: ListReportsResponse.nextToken : NextTokenAttribute @length 1..2048, echoed back via ListReportsRequest.nextToken @httpQuery nextToken. AWS returns an opaque continuation token; the fixture asserts only its shape, not its text.
Emulator: returns the identifier of a catalog entry as the token - '$.nextToken: value_mismatch (expected shaped token, got "report-iso270012022cert")' - and the page also omits the recorded report arn:aws:artifact:us-east-1::report/report-073F0fUCdVVNKJrF:3.
Why: the token leaks internal keying and is not shaped like an AWS pagination token, so clients that validate or persist tokens (and any test that asserts token shape) break; combined with cause 1 the page contents are wrong as well.
Fix: the ListReports handler's pagination/token encoding, plus the same seeded catalog state as cause 1.
Traces: artifact-list-pagination#task#1.

Cause 3 of 6 - same catalog gap observed by report name on the term/download path.
AWS: recorded behavior only - ListReports returns the report named 'AWS Outposts Rack Information Security FAQ' (id report-073F0fUCdVVNKJrF, version 3, series 'AWS Services', category 'Alignment Documents').
Emulator: that entry is absent from the 200 body ('$.reports[name=...]: missing ... got None'), so the fixture cannot pick up a report to request a term for.
Why: the term-acceptance and download sequence (ListReports -> GetTermForReport -> GetReport) cannot be exercised at all for any real AWS report.
Fix: the seeded catalog state behind the ListReports handler.
Traces: artifact-report-term-and-download#task#1.

Cause 4 of 6 - ListReports entries carry a non-AWS member set: epoch-number timestamps, no visibility, and extra upload members.
AWS: Smithy: ReportSummary.periodStart / periodEnd target TimestampAttribute with @timestampFormat date-time, so they serialize as ISO-8601 date-time strings ('2024-04-01T00:00:00.000Z' in the recording). The recording additionally shows a 'visibility' member (value PUBLIC) on each entry - recorded behavior only, since that member is not in the on-disk model - and shows no uploadState / statusMessage on published PUBLIC catalog entries.
Emulator: emits periodStart/periodEnd as epoch integers (1704067200 / 1735689600), omits visibility entirely, and adds uploadState='COMPLETE' and statusMessage='Document upload is complete.' - reported as both 'missing' and 'extra' on $.reports[0].
Why: SDK clients deserialize @timestampFormat date-time members from strings; an epoch number either fails deserialization or yields a 1970-era date, and code that filters the catalog on visibility sees the field as absent for every report.
Fix: the ListReports serializer for ReportSummary (timestamp formatting and projected member set). The epoch-number half of this is the same class of defect as the open platform issue 20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md, which does not carry any artifact replay key, so this cause is filed separately and must be re-replayed against artifact.
Traces: artifact-reports-catalog-browsing#task#1.

Cause 5 of 6 - GetReportMetadata reportDetails has the same timestamp and visibility defects.
AWS: Smithy: GetReportMetadataResponse.reportDetails : ReportDetail, whose createdAt / periodStart / periodEnd target TimestampAttribute with @timestampFormat date-time (ISO-8601 strings on the wire). The recording also carries a 'visibility' member on reportDetails - recorded behavior only.
Emulator: '$.reportDetails.createdAt: got 1735689600', '$.reportDetails.periodEnd: got 1735689600', '$.reportDetails.periodStart: got 1704067200' and '$.reportDetails.visibility: missing'.
Why: same as cause 4, on the single-report metadata read a client uses to decide whether a report version is current.
Fix: the GetReportMetadata serializer for ReportDetail.
Traces: artifact-reports-catalog-browsing#task#2.

Cause 6 of 6 - ListReportVersions entries have the same timestamp and visibility defects.
AWS: Smithy: ListReportVersions -> ListReportVersionsResponse.reports : list<ReportSummary>, GET /v1/report/listVersions, with the same @timestampFormat date-time members; the recording again carries visibility per entry (recorded behavior only).
Emulator: for arn arn:aws:artifact:us-east-1::report/report-cloudsecalliance it returns periodEnd=1735689600, periodStart=1704067200 as numbers and omits visibility.
Why: version-selection logic that compares report periods reads nonsense dates.
Fix: the ListReportVersions serializer for ReportSummary (shared with cause 4).
Traces: artifact-reports-catalog-browsing#task#3.

## Evidence
Cause 1: fixture artifact-catalog-list-reports step task#1, ListReports, expected 200 / actual 200; detail '$.reports[arn="arn:aws:artifact:us-east-1::report/report-7g5j2DTVeNXWx7St:1"]: missing (expected {...}, got None)' and the same for report-5eevElBv6MyRmQsD:7 and report-0c1IXmudeXuTlHJJ:3.
Cause 2: fixture artifact-list-pagination step task#1 (cluster also covers task#2), ListReports, 200/200; '$.nextToken: value_mismatch (expected shaped token, got "report-iso270012022cert")' and '$.reports[arn=".../report-073F0fUCdVVNKJrF:3"]: missing'.
Cause 3: fixture artifact-report-term-and-download step task#1, ListReports, 200/200; '$.reports[name="AWS Outposts Rack Information Security FAQ"]: missing ... got None'.
Cause 4: fixture artifact-reports-catalog-browsing step task#1, ListReports, 200/200; expected entry members acceptanceType, arn, category, companyName, description, id, name, periodEnd, periodStart, productName, series, state, version, visibility; actual entry {'id': 'report-cloudsecalliance', ..., 'uploadState': 'COMPLETE', 'periodStart': 1704067200, 'periodEnd': 1735689600, 'statusMessage': 'Document upload is complete.'} with no visibility.
Cause 5: fixture artifact-reports-catalog-browsing step task#2, GetReportMetadata, 200/200; createdAt=1735689600, periodEnd=1735689600, periodStart=1704067200, visibility missing.
Cause 6: fixture artifact-reports-catalog-browsing step task#3, ListReportVersions, 200/200; periodEnd=1735689600, periodStart=1704067200, visibility missing.
Model citations: ReportSummary.periodStart/periodEnd and ReportDetail.createdAt/periodStart/periodEnd target TimestampAttribute with timestampFormat 'date-time'; ReportSummary.id @pattern ^report-[a-zA-Z0-9]{16}$; ListReportsResponse.nextToken @length 1..2048; neither ReportSummary nor ReportDetail declares 'visibility' (recorded behavior only).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture artifact-catalog-list-reports, step task#1 (GET /v1/report/list) and compare the returned report arns with the recorded ones.
3. Cause 2: replay artifact-list-pagination steps task#1/task#2 and inspect $.nextToken and the page's report arns.
4. Cause 3: replay artifact-report-term-and-download step task#1 and look for the report named 'AWS Outposts Rack Information Security FAQ'.
5. Causes 4-6: replay artifact-reports-catalog-browsing steps task#1 (ListReports), task#2 (GET /v1/report/getMetadata) and task#3 (GET /v1/report/listVersions) and inspect periodStart/periodEnd/createdAt (numbers vs ISO-8601 strings), the absent 'visibility' member, and the extra uploadState/statusMessage members.
