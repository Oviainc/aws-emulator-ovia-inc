Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-compliance-inquiry-basic-workflow
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-compliance-inquiry-basic-workflow
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|CreateComplianceInquiry|2xx->2xx|$.complianceInquirySummary.createdAt
Issue-Key: artifact|CreateComplianceInquiry|2xx->2xx|$.complianceInquirySummary.name
Issue-Key: artifact|GetComplianceInquiryMetadata|2xx->2xx|$.complianceInquiryDetail.createdAt
Issue-Key: artifact|GetComplianceInquiryMetadata|2xx->2xx|$.complianceInquiryDetail.name
Issue-Key: artifact|GetComplianceInquiryMetadata|2xx->2xx|$.complianceInquiryDetail.statusMessage
Issue-Key: artifact|GetComplianceInquiryMetadata|2xx->2xx|$.complianceInquiryDetail.updatedAt
Issue-Commit: 5e32f6bad4f368147eb90818a03953ca666b028c

# Artifact compliance-inquiry projection returns epoch timestamps and the wrong terminal status message

> _Note: this issue was identified against the code base at commit `5e32f6bad4f368147eb90818a03953ca666b028c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComplianceInquiry

## Description
TWO causes in the artifact emulator (sdkId Artifact, restJson1, sigv4), both in the compliance-inquiry resource projection that CreateComplianceInquiry and GetComplianceInquiryMetadata serve from one stored record. They are bundled because they share one service and one projection; each must be closed on its own. Neither is covered by an open issue: the open WIRE issue 20261022T003730Z-artifact-report-catalog-and-report-record-projection-diverge.md owns the REPORT catalog/record projection (ListReports, GetReportMetadata, ListReportVersions) and carries no replay key for either step below, and the open behavioral issue 20261022T012757Z-artifact-report-and-compliance-inquiry-error-paths-and.md covers compliance-inquiry ERROR paths only.

Cause 1 of 2: compliance-inquiry timestamps are serialized as epoch-seconds numbers instead of ISO-8601 date-time strings.
AWS: Smithy: InquirySummary.createdAt @required -> TimestampAttribute, and InquiryDetail.createdAt @required / InquiryDetail.updatedAt -> TimestampAttribute, where com.amazonaws.artifact#TimestampAttribute carries @timestampFormat("date-time"). On restJson1 that member must be rendered as an RFC3339/ISO-8601 date-time STRING, not as a JSON number. The corpus corroborates this: recorded Artifact responses in the same trace set render TimestampAttribute members as strings such as '2024-11-01T00:00:00.000Z' / '2024-11-01T00:00:00Z'. Status is unchanged: CreateComplianceInquiry answers HTTP 202, GetComplianceInquiryMetadata HTTP 200, and both the emulator and the recording agree on those.
Emulator: the same members come back as JSON numbers of epoch seconds with a fractional part — $.complianceInquirySummary.createdAt was a bare number (observed example 1792659338.4471648) on the 202, and $.complianceInquiryDetail.createdAt / $.complianceInquiryDetail.updatedAt likewise on the 200, where the recording has a date-time string. The concrete instant is environment-specific and is NOT the requirement; the JSON type and lexical form are.
Why: an AWS SDK deserializing a @timestampFormat date-time member expects a string, so a numeric value is a parse/type failure rather than a value difference — the client gets a deserialization error on an otherwise successful 202/200, and the created inquiry becomes unreadable through the SDK. Tooling that passes the field through as a formatted date also breaks.
Fix: the restJson1 response serializer's timestamp rendering for this service's TimestampAttribute members — it must honour the shape's @timestampFormat("date-time") rather than falling back to restJson1's default epoch-seconds encoding — exercised through the InquirySummary / InquiryDetail projections.
Traces: artifact-compliance-inquiry-basic-workflow#setup#1 (CreateComplianceInquiry, 202) and artifact-compliance-inquiry-basic-workflow#task#2 (GetComplianceInquiryMetadata, 200). Projection code: services/service-com-amazonaws-artifact-artifact/src/state.rs:502 (to_summary createdAt) and :517-518 (to_detail createdAt/updatedAt), which build ::aws_smithy_types::DateTime values that the serializer then renders numerically.

Cause 2 of 2: a text compliance inquiry reaches its terminal state with the "one or more queries encountered errors" status message instead of plain completion.
AWS: Smithy: InquiryDetail.statusMessage @required -> InquiryStatusMessage, a closed ENUM whose values include both 'Compliance inquiry processing is complete.' and 'Compliance inquiry processing is complete. One or more queries encountered errors during processing.'. Because the member is a modeled enum, the value is part of the contract rather than free-form message text. Recorded behavior: a single-query TEXT inquiry (CreateComplianceInquiry with inquiryContent.query, supportMode AI_ONLY) reaches its terminal state with statusMessage 'Compliance inquiry processing is complete.' — i.e. the query yielded a response and nothing errored.
Emulator: GetComplianceInquiryMetadata reports statusMessage 'Compliance inquiry processing is complete. One or more queries encountered errors during processing.' for exactly that inquiry. By construction (state.rs materialise) every enumerated query is driven to QUERY FAILED on the clock-owed transition, so the inquiry-level message always selects the query-error variant; the clean-completion enum value is unreachable for any inquiry that owns a query.
Why: the terminal-state signal is wrong on the success path. A client that polls GetComplianceInquiryMetadata and branches on statusMessage to decide whether the inquiry's answers are usable will treat every successfully completed inquiry as partially failed — retrying, alerting, or discarding results — and no test or workflow can ever observe the clean-completion value the model declares.
Why (hypothesis): Hypothesis: the emulator deliberately refuses to synthesise model-generated compliance answers, and the status message is derived from that absence; the fix is to let a text inquiry complete cleanly without inventing answer prose, not to fabricate a response body.
Fix: the compliance-inquiry terminal-state transition in this service's state (the query-status and inquiry-status/status-message selection applied when processing becomes due), plus whatever GetComplianceInquiryMetadata reads from it.
Traces: artifact-compliance-inquiry-basic-workflow#task#2 (GetComplianceInquiryMetadata, 200). Transition code: services/service-com-amazonaws-artifact-artifact/src/state.rs:479-489 (materialise) with the enum constants at :130-140.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0; 30 steps, 11 passed, 19 mismatched, score 36.7%).

Cause 1, fixture artifact-compliance-inquiry-basic-workflow step setup#1 — CreateComplianceInquiry, expected HTTP 202, actual HTTP 202 (verdict mismatch):
  $.complianceInquirySummary.createdAt: value_mismatch (expected a date-time-shaped value, got 1792659338.4471648)
Same fixture, step task#2 — GetComplianceInquiryMetadata, expected HTTP 200, actual HTTP 200 (verdict mismatch):
  $.complianceInquiryDetail.createdAt: value_mismatch (expected a date-time-shaped value, got 1792659338.4471648)
  $.complianceInquiryDetail.updatedAt: value_mismatch (expected a date-time-shaped value, got 1792659338.9471648)
The recorded Artifact payloads in the same trace set render TimestampAttribute members as strings, e.g. 'periodStart': '2024-11-01T00:00:00.000Z' and 'periodEnd': '2025-10-31T00:00:00Z' (fixtures artifact-catalog-list-reports, artifact-list-pagination).

Cause 2, fixture artifact-compliance-inquiry-basic-workflow step task#2 — GetComplianceInquiryMetadata, 200 vs 200 (verdict mismatch):
  $.complianceInquiryDetail.statusMessage: value_mismatch (expected 'Compliance inquiry processing is complete.', got 'Compliance inquiry processing is complete. One or more queries encountered errors during processing.')
Both values are members of the modeled InquiryStatusMessage enum, so this is an enum-value divergence, not a free-text message difference.

Excluded from this issue as not an emulator defect: the same two steps also report $.complianceInquirySummary.name / $.complianceInquiryDetail.name differing (expected 'What compliance certifications does AWS S3 hold?', got 'sim2real-probe-artifact-inquiry-d13e4491'). The handler echoes the request's name verbatim (services/service-com-amazonaws-artifact-artifact/src/ops/create_compliance_inquiry.rs:237, 'name: input.name'), and the replayed request carried the harness-generated probe name, so the echo is correct and the difference is request-derived.

Model citations used: com.amazonaws.artifact#TimestampAttribute (timestamp, @timestampFormat date-time); InquirySummary.{createdAt @required}; InquiryDetail.{createdAt @required, updatedAt}; InquiryStatusMessage enum values; CreateComplianceInquiry http {POST /v1/compliance-inquiry/create, code 202}; GetComplianceInquiryMetadata http {GET /v1/compliance-inquiry/getMetadata, code 200}.

## Steps to Reproduce
1. Clone this checkout and boot the emulator; replay trace set simparity-wire-aws-v1, fixture artifact-compliance-inquiry-basic-workflow (service artifact).
2. Cause 1, step setup#1: POST /v1/compliance-inquiry/create with a name, inquiryContent.query and supportMode=AI_ONLY. Assert the 202 body's $.complianceInquirySummary.createdAt is a JSON STRING in ISO-8601/RFC3339 date-time form (per TimestampAttribute @timestampFormat date-time); observe a bare JSON number of epoch seconds.
3. Cause 1, step task#2: GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=<the id from step 2> and assert the 200 body's $.complianceInquiryDetail.createdAt and $.complianceInquiryDetail.updatedAt are the same date-time strings; observe bare JSON numbers. Do not assert the instants themselves — only the JSON type and lexical form.
4. Cause 2, step task#2: poll the same GetComplianceInquiryMetadata until the inquiry leaves PROCESSING, then compare $.complianceInquiryDetail.statusMessage with the modeled enum value 'Compliance inquiry processing is complete.'; observe 'Compliance inquiry processing is complete. One or more queries encountered errors during processing.'. A single-query TEXT inquiry must reach the clean-completion value.
5. Re-replay both steps of artifact-compliance-inquiry-basic-workflow after the fix; both must pass. Ignore the $..name difference, which is the replayed request's own value.
