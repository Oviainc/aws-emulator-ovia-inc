Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the support emulator

_Filed: 2026-10-21T22:16:16Z_
_Operation: service-com-amazonaws-support-awssupport-20130415 | Class: missing_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 20 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero persisted state, so no AWS Support client can do anything. Implement a FIRST SMALL SLICE: the support-case lifecycle, statefully, end-to-end. Scope is exactly these five operations (awsJson1_1, `POST /` with `X-Amz-Target: AWSSupport_20130415.<Op>`):

1. **CreateCase** — input `subject` (required), `communicationBody` (required, len 1..8000), `serviceCode` (pattern `^[0-9a-z\-_]+$`), `severityCode`, `categoryCode`, `ccEmailAddresses` (0..10), `language`, `issueType`. Must allocate and PERSIST a new case, returning `caseId` in the real AWS shape `case-<12-digit-account>-<realm>-<year>-<16 hex>` (e.g. `case-123456789012-muen-2024-abcdef0123456789`). The case must record a numeric `displayId`, `status` = `opened`, `timeCreated` (ISO 8601), `submittedBy`, and the initial `communicationBody` as its first communication. No canned/echoed caseId that is not stored.

2. **DescribeCases** — input `caseIdList` (0..100), `displayId`, `afterTime`/`beforeTime`, `includeResolvedCases` (default false), `includeCommunications`, `maxResults` (10..100), `nextToken`. Output `cases` (list<CaseDetails>: `caseId`, `displayId`, `subject`, `status`, `serviceCode`, `categoryCode`, `severityCode`, `submittedBy`, `timeCreated`, `ccEmailAddresses`, `language`, `recentCommunications`) + `nextToken`. Must return the cases actually created, filtered by `caseIdList`/`displayId`, must EXCLUDE cases whose `status` is `resolved` unless `includeResolvedCases` is true, must populate `recentCommunications.communications` with up to the five most recent communications when `includeCommunications` is absent/true, and must raise `CaseIdNotFound` (HTTP 400) for an unknown id in `caseIdList`.

3. **AddCommunicationToCase** — input `caseId`, `communicationBody` (required, len 1..8000), `ccEmailAddresses` (0..10). Must APPEND a `Communication{caseId, body, submittedBy, timeCreated, attachments:[]}` to the stored case and return `result: true`; unknown `caseId` must fail with `CaseIdNotFound` (HTTP 400). A hardcoded `result: true` that stores nothing is a failure.

4. **DescribeCommunications** — input `caseId` (required), `beforeTime`/`afterTime`, `maxResults` (10..100), `nextToken`. Output `communications` + `nextToken`. Must return every communication persisted for that case (the CreateCase body first, then each AddCommunicationToCase body) newest-first, honour `maxResults` by emitting an opaque `nextToken` and resuming from it, and raise `CaseIdNotFound` (HTTP 400) for an unknown case.

5. **ResolveCase** — input `caseId`. Must read the stored status, set the case's `status` to `resolved`, and return `initialCaseStatus` (the status before the call, e.g. `opened`) and `finalCaseStatus` = `resolved`, both computed from state; unknown `caseId` → `CaseIdNotFound` (HTTP 400). A subsequent DescribeCases without `includeResolvedCases` must no longer list the case, and with `includeResolvedCases: true` must list it with `status: resolved`.

NEVER STUBS: all five must compute their responses from the persisted store reached via `crate::state::service_state(&*self.engine)` — no hardcoded returns, no echoing the input back, no random id that is never stored. The remaining 15 operations (attachment upload/download set, Trusted Advisor checks, DescribeServices / DescribeSeverityLevels / DescribeSupportedLanguages / DescribeCreateCaseOptions) stay 501 for now and are tracked as separate gap issues.

## State Model

In `ComAmazonawsSupportAwssupport20130415State`:

- `cases: HashMap<String /* caseId */, Case>` — the primary resource store.
- `Case { case_id: String, display_id: String, subject: String, status: String /* opened | pending-customer-action | work-in-progress | resolved */, service_code: Option<String>, category_code: Option<String>, severity_code: Option<String>, issue_type: Option<String>, submitted_by: String, time_created: String /* ISO 8601 */, cc_email_addresses: Vec<String>, language: Option<String>, communications: Vec<CommunicationRecord> }`.
- `CommunicationRecord { case_id: String, body: String, submitted_by: String, time_created: String, attachment_set: Vec<AttachmentDetails> /* empty in this slice */ }` — append-only, ordered oldest→newest in storage; readers reverse/slice it.
- `next_display_id: u64` — monotonic counter backing each case's numeric `displayId`.
- Case ordering for DescribeCases: deterministic by `time_created` descending then `case_id`, so pagination tokens (index- or caseId-cursor based) are stable across calls.

## Evidence

`services/service-com-amazonaws-support-awssupport-20130415/OPERATIONS.md` lists all 20 operations with the input/output members and errors cited above (CreateCase lines 45-49, DescribeCases lines 63-68 incl. the `@paginated(inputToken=nextToken, outputToken=nextToken, items=cases)` trait, DescribeCommunications lines 70-75, AddCommunicationToCase lines 33-37, ResolveCase lines 143-147). `smithy_query shape=CaseDetails,Communication,RecentCaseCommunications` gives the exact response field names and the documented `status` value set (`opened`, `resolved`, `work-in-progress`, ...) and states that `recentCommunications` holds the five most recent communications plus a `nextToken`. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-support-awssupport-20130415/src/ops` matches all 20 handler files (create_case.rs, describe_cases.rs, add_communication_to_case.rs, describe_communications.rs, resolve_case.rs included), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No tests assert any case behaviour; only the generated `tests/state_roundtrip.rs` exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `support` client at it (awsJson1_1, target prefix `AWSSupport_20130415`).
2. CreateCase with subject='Emulator smoke test', communicationBody='initial body', serviceCode='amazon-elastic-compute-cloud-linux', severityCode='low', categoryCode='performance', language='en' → expect 200 with a stored `caseId`; today: HTTP 501 NotImplemented.
3. DescribeCases with caseIdList=[thatCaseId], includeCommunications=true → expect one CaseDetails with the subject/serviceCode/severityCode just submitted, status='opened', a numeric displayId, and recentCommunications containing the 'initial body' communication.
4. AddCommunicationToCase with caseId=thatCaseId, communicationBody='follow-up body' → expect 200 {"result": true}.
5. DescribeCommunications with caseId=thatCaseId → expect both communications ('follow-up body' newest-first, then 'initial body'), each with caseId, body, submittedBy and timeCreated.
6. DescribeCommunications with caseId='case-000000000000-xxxx-2024-0000000000000000' → expect CaseIdNotFound (HTTP 400), not 501.
7. ResolveCase with caseId=thatCaseId → expect initialCaseStatus='opened', finalCaseStatus='resolved'.
8. DescribeCases with caseIdList=[thatCaseId] and includeResolvedCases omitted → expect the case to be omitted; repeat with includeResolvedCases=true → expect it returned with status='resolved'.
