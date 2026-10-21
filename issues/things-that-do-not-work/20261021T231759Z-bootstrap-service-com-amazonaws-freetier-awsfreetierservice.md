Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the AWS Free Tier (AWSFreeTierService) emulator

_Filed: 2026-10-21T23:17:59Z_
_Operation: service-com-amazonaws-freetier-awsfreetierservice | Class: stub_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 5 operations in `services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/` is an unmodified scaffold carrying `//! SCAFFOLDED, NOT IMPLEMENTED:` and answering HTTP 501 `NotImplemented`, so no Free Tier client can make a single successful call. Implement a FIRST, SMALL happy-path slice statefully; the responses must be computed from persisted service state, never canned, echoed or randomly generated without storage.

The service has no "create" API — its primary resources (the account plan, the activity catalog, the Free Tier usage records) are account-scoped singletons/collections that the emulator must seed per account and then read/mutate. The happy-path loop is therefore read -> mutate -> re-read -> list -> get-one.

Wire protocol (README/OPERATIONS.md): `awsJson1_0`, `POST /`, `X-Amz-Target: AWSFreeTierService.<OperationName>`, sigv4 signing name `freetier`.

### Happy path (implement these, in this order)

1. **GetAccountPlanState** (@readonly, no input members) — return the stored plan for the caller's account: `accountId` (required, `^[0-9]{12}$`, from the request credentials/context, not a constant), `accountPlanType` (required, FREE|PAID), `accountPlanStatus` (required, ACTIVE|EXPIRED|NOT_STARTED), plus optional `accountPlanRemainingCredits` (`MonetaryAmount{amount: double (default 0, required), unit: CurrencyCode (required)}`) and `accountPlanExpirationDate` (timestamp). On first call, lazily seed a default FREE/ACTIVE plan for the account and persist it, so repeated calls are identical.
2. **UpgradeAccountPlan** — input `accountPlanType` (required, FREE|PAID). Must MUTATE the persisted plan: store the new type, set `accountPlanStatus` accordingly (ACTIVE for a PAID upgrade), and return `accountId`, `accountPlanType`, `accountPlanStatus` reflecting what was stored. A following GetAccountPlanState MUST report the upgraded plan — this is the state-persistence proof for the slice. Reject a value outside the enum with `ValidationException` (HTTP 400).
3. **ListAccountActivities** (@readonly, @paginated items=`activities`) — input `filterActivityStatuses` (list<ActivityStatus>), `maxResults` (1..1000, default 10), `nextToken` (1..8192), `languageCode`. Return `activities` (required list<`ActivitySummary{activityId (32 chars, ^[a-zA-Z0-9]+$), title, reward: ActivityReward union{credit: MonetaryAmount}, status: ActivityStatus}`>) read from the seeded activity catalog, honoring `filterActivityStatuses` (only the listed statuses), truncating at `maxResults`, and emitting `nextToken` only when more items remain — the token must resume exactly where the previous page stopped and must not be echoed back when the page is the last one.
4. **GetAccountActivity** (@readonly) — input `activityId` (required, exactly 32 chars, `^[a-zA-Z0-9]+$`), optional `languageCode`. Look the activity up in the SAME catalog ListAccountActivities paged over and return its full record: `activityId`, `title`, `description`, `status`, `instructionsUrl`, `reward` (all required) plus optional `estimatedTimeToCompleteInMinutes`, `expiresAt`, `startedAt`, `completedAt`. An id that is 32 valid chars but unknown -> `ResourceNotFoundException` (HTTP 404); an id of the wrong length or with illegal characters -> `ValidationException` (HTTP 400).
5. **GetFreeTierUsage** (@paginated items=`freeTierUsages`) — input `maxResults` (1..1000, default 10), `nextToken`, optional `filter` (`Expression`). Return `freeTierUsages` (required list<`FreeTierUsage{service, operation, usageType, region, actualUsageAmount, forecastedUsageAmount, limit, unit, description, freeTierType}`>) from the stored usage records, paginated the same way as above. For the first slice the `filter` Expression may support just the simple `Dimensions{Key, Values, MatchOptions}` form (SERVICE / REGION / USAGE_TYPE with EQUALS and CONTAINS); nested And/Or/Not may stay unimplemented, but the parameter must NOT be silently ignored when the simple form is given.

## State Model

Store everything in `ComAmazonawsFreetierAwsfreetierserviceState` (`src/state.rs`), reached via `crate::state::service_state(&*self.engine)` and mutated under the lock — never a fresh engine:

```
FreeTierState {
    // account id -> plan singleton
    plans: HashMap<String /*accountId (12 digits)*/, AccountPlan>,
    // account id -> activity catalog, insertion-ordered for stable pagination
    activities: HashMap<String /*accountId*/, Vec<Activity>>,
    // account id -> free tier usage records, insertion-ordered
    usages: HashMap<String /*accountId*/, Vec<UsageRecord>>,
}

AccountPlan {
    account_id: String,          // ^[0-9]{12}$
    plan_type: AccountPlanType,  // FREE | PAID
    plan_status: AccountPlanStatus, // ACTIVE | EXPIRED | NOT_STARTED
    remaining_credits: Option<MonetaryAmount{amount: f64, unit: CurrencyCode}>,
    expiration_date: Option<Timestamp>,
}

Activity {                       // serves BOTH list and get
    activity_id: String,         // exactly 32 [a-zA-Z0-9]
    title: String,
    description: String,
    instructions_url: String,
    status: ActivityStatus,      // COMPLETED | EXPIRING | IN_PROGRESS | NOT_STARTED
    reward: ActivityReward,      // union { credit: MonetaryAmount }
    estimated_time_to_complete_in_minutes: Option<i32>,
    expires_at: Option<Timestamp>,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
}

UsageRecord {
    service: String, operation: String, usage_type: String, region: String,
    actual_usage_amount: f64, forecasted_usage_amount: f64, limit: f64,
    unit: String, description: String, free_tier_type: String,
}
```

Pagination tokens are opaque strings (e.g. an encoded offset into the ordered `activities`/`usages` vector) that are validated: a malformed or unknown token -> `ValidationException` (400), not a panic and not a silent reset to page 1.

Done = a client script can walk GetAccountPlanState -> UpgradeAccountPlan(PAID) -> GetAccountPlanState (now PAID) -> ListAccountActivities (paged, filtered) -> GetAccountActivity(id from the list) -> GetFreeTierUsage (paged) with correct, mutually consistent bodies. The remaining long tail (full nested `Expression` filtering, `languageCode` localization of titles/descriptions, plan expiration/credit accounting, AccessDeniedException/ThrottlingException paths) stays out of this slice and will be filed as individual gap issues.

## Evidence

OPERATIONS.md (crate root) lists exactly 5 bound operations — GetAccountActivity, GetAccountPlanState, GetFreeTierUsage, ListAccountActivities, UpgradeAccountPlan — with the input/output members, required flags, constraints (activityId len 32..32 `^[a-zA-Z0-9]+$`; accountId `^[0-9]{12}$`; maxResults range 1..1000 default 10; nextToken len 1..8192), @paginated traits (inputToken/outputToken=nextToken; items=activities / freeTierUsages) and errors (ValidationException 400, ResourceNotFoundException 404, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500) cited above. The Smithy model supplied the member shapes quoted in the State Model: ActivitySummary, ActivityReward (union with `credit`), MonetaryAmount (amount default 0 + unit, both required), FreeTierUsage, Expression (Dimensions/And/Or/Not). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-freetier-awsfreetierservice/src/ops` matches all 5 handler files (get_account_activity.rs, get_account_plan_state.rs, get_free_tier_usage.rs, list_account_activities.rs, upgrade_account_plan.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. There are no tests asserting any of this behavior (only the generated tests/state_roundtrip.rs).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `freetier` client at it (awsJson1_0, POST /, X-Amz-Target: AWSFreeTierService.<Op>).
2. Call GetAccountPlanState with an empty body: expect 200 with accountId (12 digits), accountPlanType and accountPlanStatus; actual = HTTP 501 NotImplemented.
3. Call UpgradeAccountPlan {"accountPlanType":"PAID"}: expect 200 echoing the stored accountId plus accountPlanType=PAID and the new accountPlanStatus; actual = HTTP 501.
4. Call GetAccountPlanState again: expect accountPlanType=PAID, proving the upgrade was persisted across calls (not an echoed response).
5. Call ListAccountActivities {"maxResults":1}: expect 200 with exactly 1 ActivitySummary (activityId/title/reward/status) and a nextToken; call again with that nextToken and expect the NEXT activity, with nextToken absent on the final page; actual = HTTP 501.
6. Call ListAccountActivities {"filterActivityStatuses":["NOT_STARTED"]}: expect only NOT_STARTED activities returned.
7. Call GetAccountActivity with an activityId taken from the list: expect 200 with the same activityId/title/status/reward plus description and instructionsUrl; actual = HTTP 501.
8. Call GetAccountActivity with a 32-char id that does not exist: expect ResourceNotFoundException (HTTP 404). Call it with a 5-char id: expect ValidationException (HTTP 400).
9. Call GetFreeTierUsage {"maxResults":1}: expect 200 with 1 FreeTierUsage record and a nextToken that pages to the next record; actual = HTTP 501.
10. Call GetFreeTierUsage with filter {"Dimensions":{"Key":"SERVICE","Values":["AWSLambda"],"MatchOptions":["CONTAINS"]}}: expect only matching usage records, not the unfiltered list.
