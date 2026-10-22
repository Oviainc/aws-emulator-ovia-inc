Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 78a47c5beb190b609d38a363e04d7dff9ff2596d

# Bootstrap the MTurk (MTurkRequesterServiceV20170117) emulator

_Filed: 2026-10-21T23:33:16Z_
_Operation: service-com-amazonaws-mturk-mturkrequesterservicev20170117 | Class: missing_

> _Note: this issue was identified against the code base at commit `78a47c5beb190b609d38a363e04d7dff9ff2596d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 39 operations in `services/service-com-amazonaws-mturk-mturkrequesterservicev20170117/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no MTurk requester client can do anything at all. Implement a FIRST, SMALL, stateful slice: the HIT lifecycle, which is the service's primary resource and core workflow.

Happy path to implement, in order (wire protocol: awsJson1_1, `POST /` with `X-Amz-Target: MTurkRequesterServiceV20170117.<Op>`):

1. **CreateHIT** — input `LifetimeInSeconds` (long, required), `AssignmentDurationInSeconds` (long, required), `Reward` (string, pattern `^[0-9]+(\.)?[0-9]{0,2}$`, required), `Title` (required), `Description` (required), plus optional `MaxAssignments`, `AutoApprovalDelayInSeconds`, `Keywords`, `Question`, `RequesterAnnotation`, `QualificationRequirements`, `UniqueRequestToken`. Must mint a new `HITId` matching `^[A-Z0-9]+$` (len 1..64) AND an implicit `HITTypeId` (same pattern) derived from the HIT-type fields (Title/Description/Reward/Keywords/AssignmentDurationInSeconds/AutoApprovalDelayInSeconds/QualificationRequirements — identical field sets should reuse the same HITTypeId), persist the HIT, and return the full `HIT` structure populated from what was stored: `HITId`, `HITTypeId`, `CreationTime` (now), `Expiration` (now + LifetimeInSeconds), `Title`, `Description`, `Question`, `Keywords`, `Reward`, `MaxAssignments` (default 1), `AssignmentDurationInSeconds`, `AutoApprovalDelayInSeconds`, `RequesterAnnotation`, `QualificationRequirements`, `HITStatus` = `Assignable`, `HITReviewStatus` = `NotReviewed`, `NumberOfAssignmentsPending` = 0, `NumberOfAssignmentsAvailable` = MaxAssignments, `NumberOfAssignmentsCompleted` = 0. A repeated `UniqueRequestToken` must return the already-created HIT rather than a second one.
2. **GetHIT** — input `HITId` (required). Returns the `HIT` structure read back out of the store, byte-for-byte consistent with what CreateHIT returned (same HITId, HITTypeId, CreationTime, Expiration, Reward, counters). Unknown/deleted HITId => `RequestError` (HTTP 400).
3. **ListHITs** — input `MaxResults` (1..100, default 10) and `NextToken` (len 1..255). Returns `HITs` (list<HIT>) containing exactly the HITs created so far (newest-first by `CreationTime` is MTurk's observable order), `NumResults` = number of items in THIS page, and `NextToken` only when more remain. Pagination must be real: MaxResults must truncate, and the returned NextToken must resume after the last item without duplicating or dropping HITs.
4. **UpdateExpirationForHIT** — input `HITId` (required), `ExpireAt` (timestamp, required). Mutates the stored HIT's `Expiration`; a subsequent GetHIT must show the new value. Setting `ExpireAt` to the past (the standard "expire a HIT now" idiom) must flip `HITStatus` to `Reviewable` and `NumberOfAssignmentsAvailable` to 0. Empty output body `{}`.
5. **DeleteHIT** — input `HITId` (required). Removes the HIT from the store; only permitted once the HIT is expired/`Reviewable` (a still-`Assignable` HIT must fail with `RequestError` HTTP 400). After success, GetHIT on that id must fail with `RequestError` and the HIT must no longer appear in ListHITs. Being `@idempotent`, a second DeleteHIT on a HIT that no longer exists should not panic — return `RequestError` as the real service does for an unknown id.
6. **GetAccountBalance** — no input. Must return `AvailableBalance`/`OnHoldBalance` from the emulator's persisted account record (sandbox-style seeded balance), not a constant literal computed in the handler, so that later SendBonus work can debit it.

Every one of these must compute its response from persisted state held in the shared engine store (reached via `crate::state::service_state(&*self.engine)`), NOT from canned or echoed values: no hardcoded HITId, no echoing input back without storing it, no random id that is never persisted. CreateHIT followed by GetHIT/ListHITs must return what was created, and DeleteHIT must be observable by both readers.

## State Model

In `src/state.rs`, on `ComAmazonawsMturkMturkrequesterservicev20170117State`:

- `hits: BTreeMap<String /*HITId*/, Hit>` where `Hit { hit_id: String, hit_type_id: String, hit_group_id: Option<String>, hit_layout_id: Option<String>, creation_time: SystemTime, expiration: SystemTime, title: String, description: String, question: Option<String>, keywords: Option<String>, reward: String, max_assignments: i32, assignment_duration_in_seconds: i64, auto_approval_delay_in_seconds: Option<i64>, requester_annotation: Option<String>, qualification_requirements: Vec<QualificationRequirement>, hit_status: HitStatus, hit_review_status: HitReviewStatus, num_pending: i32, num_available: i32, num_completed: i32 }`
- `hit_types: BTreeMap<String /*HITTypeId*/, HitType>` where `HitType { hit_type_id, title, description, keywords, reward, assignment_duration_in_seconds, auto_approval_delay_in_seconds, qualification_requirements }` — keyed so that CreateHIT/CreateHITType dedupe identical type definitions onto one id.
- `unique_request_tokens: HashMap<String /*token*/, String /*HITId*/>` for CreateHIT idempotency.
- `account: Account { available_balance: String, on_hold_balance: String }` seeded once at state construction, for GetAccountBalance.
- `next_id: u64` (or equivalent counter) used to mint `^[A-Z0-9]+$` entity ids deterministically.

Done = an integration test that drives CreateHIT -> GetHIT -> ListHITs -> UpdateExpirationForHIT -> GetHIT -> DeleteHIT -> GetHIT(404-equivalent RequestError) over the awsJson1_1 wire and asserts the response BODIES (ids stable across calls, Expiration honoured, status transitions, HIT absent after delete), not merely 2xx.

All other operations (CreateHITType, CreateHITWithHITType, qualification types and requests, assignments/approve/reject, worker blocks, bonuses, notifications, the remaining List* ops) stay scaffolded at 501 for now and will be filed as individual gap issues.

## Evidence

`OPERATIONS.md` for this crate lists 39 operations, protocol `awsJson1_1`, target prefix `MTurkRequesterServiceV20170117`; it gives CreateHIT's required inputs (`LifetimeInSeconds`, `AssignmentDurationInSeconds`, `Reward`, `Title`, `Description`) and output `HIT`, GetHIT (`HITId` -> `HIT`), ListHITs (`MaxResults` 1..100 / `NextToken` 1..255 -> `HITs`, `NumResults`, `NextToken`, @paginated), UpdateExpirationForHIT (`HITId`, `ExpireAt`), DeleteHIT (`HITId`, @idempotent), GetAccountBalance (`AvailableBalance`, `OnHoldBalance`), and the common errors `RequestError (HTTP 400)` / `ServiceFault (HTTP 500)`. `smithy_query shape=HIT` confirms the HIT member set to populate (HITId, HITTypeId, CreationTime, Expiration, HITStatus with values Assignable|Unassignable|Reviewable|Reviewing|Disposed, HITReviewStatus, NumberOfAssignmentsPending/Available/Completed). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 39 handler files, including `src/ops/create_hit.rs`, `get_hit.rs`, `list_hi_ts.rs`, `update_expiration_for_hit.rs`, `delete_hit.rs` and `get_account_balance.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and touches no state. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an MTurk requester client (boto3 `mturk` or raw awsJson1_1 POST / with X-Amz-Target) at it.
2. CreateHIT with Title='Test HIT', Description='demo', Reward='0.05', LifetimeInSeconds=3600, AssignmentDurationInSeconds=600, MaxAssignments=2 — expect a HIT with a fresh HITId/HITTypeId, CreationTime, Expiration ~= now+3600, HITStatus=Assignable, NumberOfAssignmentsAvailable=2; today: HTTP 501 NotImplemented.
3. GetHIT with the returned HITId — expect the identical HIT structure read from state; today: HTTP 501.
4. ListHITs with MaxResults=1 after creating two HITs — expect one HIT plus a NextToken, and the second HIT on the follow-up page; today: HTTP 501.
5. UpdateExpirationForHIT with ExpireAt in the past, then GetHIT — expect the new Expiration and HITStatus=Reviewable; today: HTTP 501.
6. DeleteHIT with that HITId, then GetHIT and ListHITs — expect the HIT gone (RequestError on GetHIT, absent from ListHITs); today: HTTP 501.
7. GetAccountBalance — expect AvailableBalance/OnHoldBalance from the persisted account record; today: HTTP 501.
