Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the AWS Security Incident Response (Security IR) emulator

_Filed: 2026-10-21T21:28:21Z_
_Operation: service-com-amazonaws-securityir-securityincidentresponse | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `services/service-com-amazonaws-securityir-securityincidentresponse/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. Nothing works end to end.

Implement a FIRST, SMALL slice: the case lifecycle of the service's primary resource (`case`), statefully, over the `restJson1` protocol (sigv4 signing name `security-ir`). Six operations, in this order:

1. **CreateCase** — `POST /v1/create-case -> 201`. Input: `resolverType` (AWS|Self), `title` (1..300), `description` (1..8000), `engagementType` (Investigation|Security Incident), `reportedIncidentStartDate`, `impactedAccounts`, `watchers`, `threatActorIpAddresses` (default []), `impactedServices` (default []), `impactedAwsRegions` (default []), optional `tags`, `clientToken` (@idempotencyToken). Must MINT and STORE a new case: allocate a fresh 10-digit `caseId` matching `^\d{10,32}`, persist every input field verbatim, set `caseStatus = "Submitted"`, `createdDate = lastUpdatedDate = now`, and build `caseArn = arn:aws:security-ir:<region>:<account-id>:case/<caseId>`. Output: `caseId` — the stored id, not a constant. Repeating the same `clientToken` must return the SAME `caseId` without creating a second case.
2. **GetCase** — `GET /v1/cases/{caseId}/get-case -> 200`. Must look the case up by `caseId` and return the persisted values: `title`, `description`, `caseArn`, `caseStatus`, `engagementType`, `resolverType`, `reportedIncidentStartDate`, `impactedAccounts`, `impactedAwsRegions`, `impactedServices`, `threatActorIpAddresses`, `watchers`, `createdDate`, `lastUpdatedDate`, plus `closureCode`/`closedDate` once closed. An unknown `caseId` must NOT return a zeroed 200 — refuse it (`Scaffolded::validation`, 400, since this operation models no errors).
3. **ListCases** — `POST /v1/list-cases -> 200`. Input `maxResults` (default 25) and `nextToken`. Must enumerate the cases actually stored, projecting each into a `ListCasesItem{caseId, caseArn, title, caseStatus, engagementType, resolverType, createdDate, lastUpdatedDate, closedDate, pendingAction}`, set `total` to the full stored count, and honour the `@paginated` contract: return at most `maxResults` items and an opaque `nextToken` that resumes where the page ended (absent on the last page).
4. **CreateCaseComment** — `POST /v1/cases/{caseId}/create-comment -> 201`. Input `body` (1..12000) plus `clientToken`. Must append a comment to the case's comment list, allocating a 6-digit `commentId` matching `^\d{6}$`, recording `createdDate`/`lastUpdatedDate` and `creator`; output the allocated `commentId`. Same `clientToken` twice = one comment.
5. **ListComments** — `POST /v1/cases/{caseId}/list-comments -> 200`. Must return the comments stored for that case as `ListCommentsItem{commentId, body, createdDate, lastUpdatedDate, creator, lastUpdatedBy}` with `total` = stored count, honouring `maxResults`/`nextToken`. The body posted in step 4 must come back byte-identical.
6. **CloseCase** — `POST /v1/cases/{caseId}/close-case -> 200`. Must MUTATE the stored case: set `caseStatus = "Closed"`, `closedDate = now`, bump `lastUpdatedDate`, and return `{caseStatus, closedDate}` read back from the stored case. A subsequent GetCase/ListCases must show `Closed` and the same `closedDate`.

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the request back, no minting an id that is never stored: CreateCase followed by GetCase must return exactly what was created, and CloseCase must be visible to every later read.

## State Model

In `src/state.rs`, on the service state struct:

- `cases: BTreeMap<String /* caseId */, Case>` — ordered by caseId so ListCases paginates deterministically.
- `next_case_id: u64` — monotonic counter rendered zero-padded to 10 digits (`^\d{10,32}`).
- `idempotency: HashMap<String /* clientToken */, String /* minted id */>` — replays CreateCase / CreateCaseComment.
- `Case { case_id: String, case_arn: String, title: String, description: String, case_status: CaseStatus /* starts Submitted */, engagement_type: EngagementType, resolver_type: ResolverType, reported_incident_start_date: DateTime, actual_incident_start_date: Option<DateTime>, impacted_accounts: Vec<String>, impacted_aws_regions: Vec<ImpactedAwsRegion>, impacted_services: Vec<String>, threat_actor_ip_addresses: Vec<ThreatActorIp>, watchers: Vec<Watcher>, pending_action: Option<PendingAction>, closure_code: Option<ClosureCode>, created_date: DateTime, last_updated_date: DateTime, closed_date: Option<DateTime>, tags: BTreeMap<String,String>, comments: BTreeMap<String /* commentId */, CaseComment>, next_comment_id: u32 }`
- `CaseComment { comment_id: String /* 6 digits */, body: String, creator: String, last_updated_by: String, created_date: DateTime, last_updated_date: DateTime }`

Done = a realistic client script (steps below) passes end to end with correct, stateful response bodies — not merely 2xx. The other 19 operations (memberships, attachments presigned URLs, UpdateCase/UpdateCaseStatus/UpdateResolverType, ListCaseEdits, ListInvestigations, SendFeedback, tagging, GetFindingMetrics) stay at 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for this crate (25 operations, protocol `restJson1`, SDK id `Security IR`, sigv4 name `security-ir`) documents each route, its input/output members and constraints: CreateCase `POST /v1/create-case -> 201` with required `resolverType`/`title`/`description`/`engagementType`/`reportedIncidentStartDate`/`impactedAccounts`/`watchers` and output `caseId` (len 10..32, pattern `^\d{10,32}`); GetCase `GET /v1/cases/{caseId}/get-case -> 200` returning the full case projection incl. `caseArn` pattern `^arn:aws:security-ir:...:case/[0-9]{10}$`; ListCases `POST /v1/list-cases -> 200` with `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)` and `maxResults` default 25; CreateCaseComment `POST /v1/cases/{caseId}/create-comment -> 201` output `commentId` pattern `^\d{6}$`; ListComments `POST /v1/cases/{caseId}/list-comments -> 200` paginated; CloseCase `POST /v1/cases/{caseId}/close-case -> 200` output `{caseStatus, closedDate}` where CaseStatus includes `Closed`. Item shapes confirmed from the Smithy model: `ListCasesItem{caseId*, caseArn, title, caseStatus, engagementType, resolverType, createdDate, lastUpdatedDate, closedDate, pendingAction}` and `ListCommentsItem{commentId*, body, creator, lastUpdatedBy, createdDate, lastUpdatedDate}`. Handler state: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-securityir-securityincidentresponse/src/ops/` matches all 25 files, e.g. `src/ops/create_case.rs:178` returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateCase"))`. No test asserts any case behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. CreateCase with resolverType=Self, title="Suspicious IAM activity", description="Unexpected AssumeRole calls", engagementType="Investigation", reportedIncidentStartDate=now, impactedAccounts=["000123123123"], watchers=[{email:"sec@example.com"}], and clientToken="tok-1" — expect 201 and a caseId matching ^\d{10,32} (today: 501)
2. Repeat the identical CreateCase with clientToken="tok-1" — expect the SAME caseId and still exactly one case
3. GetCase with that caseId — expect 200 with title/description/engagementType/resolverType/impactedAccounts/watchers exactly as submitted, caseStatus="Submitted", a caseArn of the form arn:aws:security-ir:<region>:<account>:case/<caseId>, and createdDate/lastUpdatedDate populated
4. ListCases with maxResults=25 — expect the new case present in items with matching caseId/title/caseStatus and total>=1
5. CreateCaseComment on that caseId with body="Containment started" — expect 201 and a 6-digit commentId
6. ListComments on that caseId — expect 200 with one item whose commentId matches step 5 and whose body is "Containment started" verbatim
7. CloseCase on that caseId — expect 200 with caseStatus="Closed" and a closedDate
8. GetCase again — expect caseStatus="Closed" and the same closedDate returned by CloseCase; ListCases must also report that case as Closed
9. GetCase with an unknown caseId such as 9999999999 — expect a 4xx refusal, not a 200 with empty fields
