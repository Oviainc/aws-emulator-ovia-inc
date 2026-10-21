Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the Amazon Connect Contact Lens emulator

_Filed: 2026-10-21T21:58:08Z_
_Operation: service-com-amazonaws-connectcontactlens-amazonconnectcontactlens | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate `services/service-com-amazonaws-connectcontactlens-amazonconnectcontactlens` is a fresh scaffold: its single handler, `src/ops/list_realtime_contact_analysis_segments.rs`, still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), holding no state. Nothing in this service works end-to-end today.

This service models exactly ONE operation, so the happy path is that operation plus the state it must read. The first slice is:

1. **ListRealtimeContactAnalysisSegments** (`POST /realtime-contact-analysis/analysis-segments`, restJson1, success 200) — input `InstanceId` (required, 1..256, `\S`), `ContactId` (required, 1..256, `\S`), `MaxResults` (1..100), `NextToken` (1..131070, `\S`); output `Segments` (required list of `RealtimeContactAnalysisSegment`, 0..100) and optional `NextToken`. The response MUST be computed by looking up the analysis session stored for `(InstanceId, ContactId)` in service state and slicing its segment list — never a canned or echoed body.
2. **Unknown instance/contact** — when no analysis session exists for the `(InstanceId, ContactId)` pair, return the modeled `ResourceNotFoundException` (HTTP 404), not an empty 200 and not 501.
3. **Pagination over the stored segments** — `MaxResults` caps the page size (default to the full page, max 100 per the `Segments` length constraint); when more stored segments remain, return an opaque `NextToken` whose next call resumes exactly after the last returned segment, with no duplicates and no skips; when the session is complete and drained, OMIT `NextToken` (per the modeled doc: absent `NextToken` means analysis is completed and there is nothing more to retrieve). A `NextToken` that does not correspond to a live cursor for that contact must be rejected with `InvalidRequestException` (HTTP 400).
4. **Segment fidelity** — a stored segment must round-trip its real shape: a `Transcript` with `Id`, `ParticipantId`, `ParticipantRole`, `Content`, `BeginOffsetMillis`, `EndOffsetMillis`, optional `Sentiment` (POSITIVE|NEUTRAL|NEGATIVE) and `IssuesDetected`; and/or `Categories` with `MatchedCategories` + `MatchedDetails` (category name -> `CategoryDetails{PointsOfInterest[{BeginOffsetMillis,EndOffsetMillis}]}`); and/or `PostContactSummary{Content, Status, FailureCode}`. Returned values must be the ones held in state, byte-for-byte.
5. **Seeding hook** — because Contact Lens has no public write API (segments are produced by Amazon Connect voice analysis), the emulator needs an internal/admin seeding path so a test can register an analysis session with a known segment list for a given `(InstanceId, ContactId)`. Without it there is no way for a client to observe real state. This is the create side of the lifecycle and is in scope for the bootstrap.
6. **Retention** — the operation documents that voice data is retained for 24 hours; a session seeded with a start time older than 24h should behave as absent (`ResourceNotFoundException`).

Every one of the above must compute its response from persisted state. Hardcoded segment lists, echoing the request, or minting a `NextToken` that no stored cursor backs are failures of this issue.

## State Model

- `ContactLensState { sessions: HashMap<(InstanceId, ContactId), AnalysisSession> }` — the single store reached via `crate::state::service_state(&*self.engine)`.
- `AnalysisSession { instance_id: String, contact_id: String, started_at: SystemTime, completed: bool, segments: Vec<StoredSegment> }` — `completed=false` means more segments may arrive later, which is what makes a `NextToken` be returned on a drained-but-open session; `completed=true` + drained means omit `NextToken`. `started_at` drives the 24-hour retention check.
- `StoredSegment { transcript: Option<Transcript>, categories: Option<Categories>, post_contact_summary: Option<PostContactSummary>, extracted_information: Option<ExtractedInformation> }` — mirrors the modeled `RealtimeContactAnalysisSegment` members so output serialization is a direct map of stored values.
- `Transcript { id, participant_id, participant_role, content, begin_offset_millis, end_offset_millis, sentiment: Option<SentimentValue>, issues_detected: Vec<IssueDetected{character_offsets{begin_offset_char,end_offset_char}}> }`.
- `Categories { matched_categories: Vec<String>, matched_details: BTreeMap<String, CategoryDetails{points_of_interest: Vec<PointOfInterest{begin_offset_millis,end_offset_millis}>}> }`.
- Pagination cursors: `cursors: HashMap<String /*NextToken*/, Cursor{instance_id, contact_id, next_index: usize}>`, so a token is validatable and resolvable rather than parsed from client input. Segments are returned in stored (chronological, ascending `BeginOffsetMillis`) order.

Anything outside this slice (none of the other operations exist in this model) is out of scope; later gap passes cover refinements such as `ExtractedInformation` population and throttling behaviour.

## Evidence

1) `services/.../OPERATIONS.md` lists exactly 1 operation — `ListRealtimeContactAnalysisSegments`, `POST /realtime-contact-analysis/analysis-segments -> 200`, restJson1, sigv4 name `connect`, endpoint prefix `contact-lens`; input `InstanceId`/`ContactId` required, `MaxResults` range 1..100, `NextToken` 1..131070; output `Segments` required (len 0..100) + `NextToken`; errors AccessDeniedException 403, InternalServiceException 500, InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429. 2) `src/ops/list_realtime_contact_analysis_segments.rs` lines 3-6 carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and line 176 returns `Err(crate::scaffolded::Scaffolded::not_implemented("ListRealtimeContactAnalysisSegments"))` — the only handler in `src/ops/`, so the service is all-stubs. 3) Smithy model closure for the operation: `@paginated(inputToken=NextToken, outputToken=NextToken, pageSize=MaxResults)`; `RealtimeContactAnalysisSegment` members `Transcript`/`Categories`/`PostContactSummary`/`ExtractedInformation`; `Transcript` requires `Id`, `ParticipantId`, `ParticipantRole`, `Content`, `BeginOffsetMillis`, `EndOffsetMillis`; `SentimentValue` = POSITIVE|NEUTRAL|NEGATIVE; `PostContactSummaryStatus` = FAILED|COMPLETED. 4) Modeled operation doc (handler header lines 52-53, 69-75): voice data retained 24 hours; absent `NextToken` means analysis completed with no more segments, present `NextToken` means either more segments now or more possibly later. No tests exist for this crate.

## Steps to Reproduce

1. Start the emulator and seed an analysis session for InstanceId="inst-1", ContactId="contact-1" with 3 transcript segments (ascending BeginOffsetMillis) via the emulator's seeding path, session not yet completed.
2. POST /realtime-contact-analysis/analysis-segments with {"InstanceId":"inst-1","ContactId":"contact-1","MaxResults":2} — expect HTTP 200 with Segments[0..1] exactly as seeded (Transcript.Id/ParticipantRole/Content/offsets/Sentiment preserved) and a non-empty NextToken. Today: HTTP 501 NotImplemented.
3. Repeat the call passing the returned NextToken — expect the 3rd segment only, no duplicate of the first two.
4. Mark the seeded session completed and page to the end — expect the final response to OMIT NextToken.
5. POST the same request for ContactId="does-not-exist" — expect HTTP 404 ResourceNotFoundException, not an empty 200.
6. POST with NextToken="garbage" — expect HTTP 400 InvalidRequestException.
7. POST with MaxResults=0 or MaxResults=101 — expect a 400 (modeled range 1..100 refused before the handler), never a 200 or 501.
