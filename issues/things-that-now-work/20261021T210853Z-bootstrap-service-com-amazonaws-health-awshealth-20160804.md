Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the AWS Health (AWSHealth_20160804) emulator

_Filed: 2026-10-21T21:08:53Z_
_Operation: service-com-amazonaws-health-awshealth-20160804 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 15 operations in `services/service-com-amazonaws-health-awshealth-20160804/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501. `src/state.rs` holds `ComAmazonawsHealthAwshealth20160804State { /* TODO: add service-specific state fields */ }` — zero state. A client cannot complete a single Health call.

This issue defines the FIRST, SMALL slice: the event-discovery read loop that every Health client (console, `aws health describe-events`, Personal Health Dashboard pollers) performs. Health is a read-only API — there is no Create* operation — so the emulator's "create" step is a seeded event catalog held in service state (populated when the state struct is constructed, and deterministic across calls). Every operation below must compute its response BY FILTERING/AGGREGATING that persisted catalog. No canned bodies, no echoing the request filter back, no randomly generated ARNs that are not also stored.

Happy path to implement, in order (protocol: awsJson1_1, `POST /` with `X-Amz-Target: AWSHealth_20160804.<Op>`):

1. **DescribeEventTypes** — in: `filter` (EventTypeFilter: `eventTypeCodes`, `services`, `eventTypeCategories`), `maxResults` (10..100), `nextToken`. Out: `eventTypes` (list<EventType> with `service`, `code`, `category`), `nextToken`. Must be derived from the distinct (service, eventTypeCode, category) triples present in the seeded catalog, and must honour `filter.services` / `filter.eventTypeCodes` / `filter.eventTypeCategories`.
2. **DescribeEvents** — in: `filter` (EventFilter: at minimum `eventArns`, `services`, `regions`, `eventTypeCodes`, `eventTypeCategories`, `eventStatusCodes`), `maxResults` (1..100), `nextToken`, `locale`. Out: `events` (list<Event>: `arn`, `service`, `eventTypeCode`, `eventTypeCategory`, `region`, `startTime`, `endTime`, `lastUpdatedTime`, `statusCode`, `eventScopeCode`), `nextToken`. Results sorted by `lastUpdatedTime` descending (documented in the model: "Results are sorted by lastModifiedTime, starting with the most recent event"). `maxResults` must truncate and emit an opaque `nextToken` matching `^[a-zA-Z0-9=/+_.-]{4,10000}$`; a token that was never issued must fail with `InvalidPaginationToken` (HTTP 400), and an unsupported `locale` (anything other than `en`) with `UnsupportedLocale` (HTTP 400). Omit `nextToken` on the last page.
3. **DescribeEventDetails** — in: `eventArns` (required, 1..10), `locale`. Out: `successfulSet` (list<EventDetails>: `event`, `eventDescription.latestDescription`, `eventMetadata`), `failedSet` (list<EventDetailsErrorItem>: `eventArn`, `errorName`, `errorMessage`). The `event` returned MUST be byte-for-byte the same Event that DescribeEvents returned for that ARN (same store). An ARN not in the catalog goes to `failedSet` with `errorName` `EventNotFound` — NOT to a top-level error, and NOT silently dropped.
4. **DescribeAffectedEntities** — in: `filter` (EntityFilter, required; `eventArns` required 1..10, plus `entityArns`, `entityValues`, `statusCodes`), `maxResults` (1..100), `nextToken`, `locale`. Out: `entities` (list<AffectedEntity>: `entityArn`, `eventArn`, `entityValue`, `awsAccountId`, `lastUpdatedTime`, `statusCode`), `nextToken`. Entities come from the stored per-event entity list; `filter.statusCodes` / `entityValues` must actually filter; same pagination + locale error rules as DescribeEvents.
5. **DescribeEntityAggregates** — in: `eventArns` (1..50). Out: `entityAggregates` (list<EntityAggregate>: `eventArn`, `count`, `statuses`). `count` must equal the number of stored entities for that event (so it agrees with what DescribeAffectedEntities returns), and `statuses` the per-`entityStatusCode` tally. A hardcoded count is a failure of this issue.
6. **DescribeEventAggregates** — in: `filter` (EventFilter), `aggregateField` (required, `eventTypeCategory`), `maxResults` (10..100), `nextToken`. Out: `eventAggregates` (list<EventAggregate>: `aggregateValue`, `count`). Counts must be computed by grouping the SAME filtered event set DescribeEvents would return, so the per-category counts sum to the number of matching events.

## State Model

Add to `ComAmazonawsHealthAwshealth20160804State`:

- `events: BTreeMap<String /* eventArn */, HealthEvent>` — insertion-independent, iterated and then sorted by `last_updated_time` desc for DescribeEvents.
  - `HealthEvent { arn, service, event_type_code, event_type_category, region, availability_zone: Option<String>, start_time, end_time: Option<Timestamp>, last_updated_time, status_code /* open|closed|upcoming */, event_scope_code /* PUBLIC|ACCOUNT_SPECIFIC|NONE */, description: String, event_metadata: BTreeMap<String,String>, affected_accounts: Vec<String> }`
  - ARNs follow the modeled pattern `^arn:aws:health:<region>::event/<SERVICE>/<EVENT_TYPE_CODE>/<EVENT_TYPE_CODE>_<id>$`.
- `entities: Vec<HealthEntity>` (or `BTreeMap<eventArn, Vec<HealthEntity>>`)
  - `HealthEntity { entity_arn, event_arn, entity_value, entity_url: Option<String>, aws_account_id, last_updated_time, status_code /* IMPAIRED|UNIMPAIRED|UNKNOWN|PENDING|RESOLVED */, tags: BTreeMap<String,String>, entity_metadata: BTreeMap<String,String> }`
- `pagination_tokens: HashMap<String /* token */, PageCursor { op: &'static str, offset: usize, filter_digest: u64 }>` — tokens are issued by the server and validated on reuse; unknown/garbage token => `InvalidPaginationToken`.
- Seed data: a small deterministic catalog (e.g. 3-4 events across EC2/RDS in two regions, categories `issue` / `scheduledChange` / `accountNotification`, with 2-3 affected entities on at least one ACCOUNT_SPECIFIC event) installed when the state is first created. Event types for DescribeEventTypes are DERIVED from this catalog, not a second hardcoded list.

Done = a client script can run steps 1-6 against the emulator and see self-consistent results: the ARN listed by DescribeEvents resolves in DescribeEventDetails, its entities appear in DescribeAffectedEntities, DescribeEntityAggregates' `count` equals that entity count, and DescribeEventAggregates' category counts sum to the DescribeEvents result size. Pagination with `maxResults=1` walks the whole catalog without repeats or omissions.

The remaining 9 operations (DescribeServiceLifecycle, all *ForOrganization operations, Describe/Enable/DisableHealthServiceAccessForOrganization) stay at 501 and will be filed as individual gap issues later — that is expected and in scope for a later pass, not this one.

## Evidence

OPERATIONS.md at `services/service-com-amazonaws-health-awshealth-20160804/OPERATIONS.md` lists all 15 operations with their input/output members and errors (awsJson1_1, X-Amz-Target `AWSHealth_20160804.<Op>`, sigv4 name `health`), including the required members and the `@paginated(inputToken=nextToken, outputToken=nextToken)` traits and `InvalidPaginationToken`/`UnsupportedLocale` errors cited above. Shape member names (Event, EventFilter, AffectedEntity, EntityFilter, EventDetails, EntityAggregate, EventAggregate, EventType, EventTypeFilter) were taken from the Smithy model via smithy_query on `smithy-model/com-amazonaws-health-awshealth-20160804.json`; the DescribeEvents sort order ("Results are sorted by lastModifiedTime, starting with the most recent event") and the DescribeEventDetails/DescribeAffectedEntities split come from the modeled operation docs reproduced in `src/ops/describe_events.rs`. Stub proof: `src/ops/describe_events.rs` lines 4 and 178 — `//! SCAFFOLDED, NOT IMPLEMENTED:` and `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeEvents"))`; all 12 files in `src/ops/` are identical scaffolds. State proof: `src/state.rs` line 8 — `// TODO: add service-specific state fields`. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `health` (awsJson1_1, target prefix `AWSHealth_20160804`).
2. Call DescribeEventTypes with no filter: expect a non-empty `eventTypes` list derived from the seeded catalog (today: HTTP 501 NotImplemented).
3. Call DescribeEvents with `{"filter":{"eventTypeCategories":["issue"]},"maxResults":1}`: expect one Event plus a `nextToken`; re-issue with that token and expect the next distinct event, ordered by `lastUpdatedTime` descending.
4. Call DescribeEvents with `{"nextToken":"bogustoken"}`: expect HTTP 400 `InvalidPaginationToken`. Call it with `{"locale":"fr"}`: expect HTTP 400 `UnsupportedLocale`.
5. Take the `arn` from step 3 and call DescribeEventDetails with `{"eventArns":[<arn>]}`: expect `successfulSet[0].event` identical to the summary from DescribeEvents plus a non-empty `eventDescription.latestDescription`; add a non-existent ARN and expect it in `failedSet` with `errorName=EventNotFound`.
6. Call DescribeAffectedEntities with `{"filter":{"eventArns":[<arn>]}}`: expect the stored AffectedEntity records (entityArn/entityValue/awsAccountId/statusCode) for that event; repeat with `filter.statusCodes=["IMPAIRED"]` and verify the list shrinks accordingly.
7. Call DescribeEntityAggregates with `{"eventArns":[<arn>]}`: expect `count` equal to the number of entities returned in step 6 and `statuses` tallying their status codes.
8. Call DescribeEventAggregates with `{"aggregateField":"eventTypeCategory"}`: expect per-category counts whose sum equals the total number of events returned by an unfiltered DescribeEvents.
