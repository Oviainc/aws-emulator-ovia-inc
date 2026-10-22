Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Timestream Query emulator

_Filed: 2026-10-21T21:18:45Z_
_Operation: service-com-amazonaws-timestreamquery-timestream-20181101 | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 15 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` declares `ComAmazonawsTimestreamqueryTimestream20181101State` with a single `// TODO: add service-specific state fields` comment. Nothing is stateful yet.

The FIRST slice is the scheduled-query lifecycle — Timestream Query's only durable resource — plus the endpoint-discovery call every AWS SDK makes before any other Timestream request. Protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: Timestream_20181101.<OperationName>` (see README/OPERATIONS.md wire-protocol section).

Implement these 6, in this order:

1. **DescribeEndpoints** — no input. Output `Endpoints` (required, list<Endpoint> of `{Address, CachePeriodInMinutes}`). Must return the emulator's own reachable address so SDK endpoint discovery resolves back to the emulator instead of real AWS; otherwise every subsequent SDK call is sent off-box. Not canned AWS hostnames — derive `Address` from the serving endpoint/host.

2. **CreateScheduledQuery** — input `Name` (required, 1..64, pattern `^[a-zA-Z0-9|!\-_*'\(\)]([a-zA-Z0-9]|[!\-_*'\(\)\/.])+$`), `QueryString` (required, 1..262144), `ScheduleConfiguration` (required; `ScheduleExpression` required), `NotificationConfiguration` (required; `SnsConfiguration` required), `ScheduledQueryExecutionRoleArn` (required), `ErrorReportConfiguration` (required; `S3Configuration` required), optional `TargetConfiguration`, `KmsKeyId`, `Tags` (0..200), `ClientToken` (32..128, @idempotencyToken). Output `Arn` (required). Must mint and PERSIST a record keyed by a generated ARN of the form `arn:aws:timestream:<region>:<account>:scheduled-query/<Name>-<suffix>`, record `CreationTime` and initial `State = ENABLED`, store the tags, and store the full request so Describe can replay it. The op is `@idempotent`: a repeat with the same `ClientToken` returns the same `Arn`; a different definition reusing a live `Name` is `ConflictException` (409).

3. **DescribeScheduledQuery** — input `ScheduledQueryArn` (required). Output `ScheduledQuery` (required, `ScheduledQueryDescription`) built FROM the stored record: `Arn`, `Name`, `QueryString`, `CreationTime`, `State`, `ScheduleConfiguration`, `NotificationConfiguration`, `TargetConfiguration`, `ScheduledQueryExecutionRoleArn`, `KmsKeyId`, `ErrorReportConfiguration`, plus `NextInvocationTime` computed from the stored `ScheduleExpression`. Unknown/deleted ARN → `ResourceNotFoundException` (404), not an empty success.

4. **ListScheduledQueries** — input `MaxResults` (1..1000), `NextToken`. Output `ScheduledQueries` (required, list<ScheduledQuery> summaries: `Arn`, `Name`, `CreationTime`, `State`, `PreviousInvocationTime`, `NextInvocationTime`, `ErrorReportConfiguration`, `TargetDestination`, `LastRunStatus`) + `NextToken`. Must page over the stored map in a stable order, truncate at `MaxResults`, emit `NextToken` only when more remain, and resume exactly from an opaque token (absent `NextToken` when the last page is returned).

5. **UpdateScheduledQuery** — input `ScheduledQueryArn` (required), `State` (required, ENABLED|DISABLED). Empty output. Must mutate the stored record's `State` so the next Describe/List reports the new value; unknown ARN → `ResourceNotFoundException` (404).

6. **DeleteScheduledQuery** — input `ScheduledQueryArn` (required). Empty output. Must remove the record (and its tags) so a following Describe returns `ResourceNotFoundException` (404) and List no longer includes it. `@idempotent` per the model, but a second delete of an already-removed ARN must still report 404 as real Timestream does.

Every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no ARN minted without a stored record. Required output members must be populated from state (e.g. `Arn` from the created record, `ScheduledQueries` from the map), and validation of the modeled length/pattern/range constraints must produce `ValidationException` (400).

## State Model

Add to `ComAmazonawsTimestreamqueryTimestream20181101State`:

- `scheduled_queries: BTreeMap<String /* Arn */, ScheduledQueryRecord>` — BTreeMap so List ordering and token resumption are deterministic.
- `ScheduledQueryRecord { arn: String, name: String, query_string: String, schedule_expression: String, notification_configuration: NotificationConfiguration, target_configuration: Option<TargetConfiguration>, target_destination: Option<TargetDestination>, error_report_configuration: ErrorReportConfiguration, execution_role_arn: String, kms_key_id: Option<String>, state: ScheduledQueryState, creation_time: SystemTime, previous_invocation_time: Option<SystemTime>, next_invocation_time: Option<SystemTime>, last_run_status: Option<ScheduledQueryRunStatus>, last_run_summary: Option<ScheduledQueryRunSummary>, recently_failed_runs: Vec<ScheduledQueryRunSummary>, tags: BTreeMap<String, String> }`
- `names: BTreeMap<String /* Name */, String /* Arn */>` — enforces the one-live-query-per-name ConflictException.
- `create_tokens: BTreeMap<String /* ClientToken */, String /* Arn */>` — makes CreateScheduledQuery's `@idempotencyToken` replay return the same ARN.
- `endpoints: Vec<Endpoint { address: String, cache_period_in_minutes: i64 }>` — seeded from the serving address for DescribeEndpoints.

Out of scope for this slice (stay 501, filed later as individual gap issues): Query, CancelQuery, PrepareQuery, ExecuteScheduledQuery, TagResource, UntagResource, ListTagsForResource, DescribeAccountSettings, UpdateAccountSettings. The `tags` field on the record is included now so the tagging ops can be layered on without a state migration.

## Evidence

OPERATIONS.md (crate root) lists all 15 operations with their inputs/outputs/errors: CreateScheduledQuery requires Name/QueryString/ScheduleConfiguration/NotificationConfiguration/ScheduledQueryExecutionRoleArn/ErrorReportConfiguration and returns required `Arn`; DescribeScheduledQuery takes `ScheduledQueryArn` and returns required `ScheduledQuery` with `ResourceNotFoundException (HTTP 404)` modeled; ListScheduledQueries is `@paginated(inputToken=NextToken, outputToken=NextToken, items=ScheduledQueries)` with `MaxResults` range 1..1000; UpdateScheduledQuery requires `State` (ENABLED|DISABLED); DeleteScheduledQuery is @idempotent with ResourceNotFoundException; DescribeEndpoints returns required `Endpoints`. Shape detail for ScheduledQuery / ScheduledQueryDescription / ScheduleConfiguration / NotificationConfiguration / ErrorReportConfiguration / TargetConfiguration came from smithy_query on the model at services/service-com-amazonaws-timestreamquery-timestream-20181101/smithy-model/com-amazonaws-timestreamquery-timestream-20181101.json. All-stub status: `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 15 handler files (cancel_query, create_scheduled_query, delete_scheduled_query, describe_account_settings, describe_endpoints, describe_scheduled_query, execute_scheduled_query, list_scheduled_queries, list_tags_for_resource, prepare_query, query, tag_resource, untag_resource, update_account_settings, update_scheduled_query); src/state.rs has no fields. Endpoint discovery being mandatory for Timestream SDK clients is well-established service behavior (the model declares InvalidEndpointException, HTTP 421, on nearly every operation).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK Timestream Query client at it.
2. Call DescribeEndpoints — expect a non-empty `Endpoints` list whose `Address` points back at the emulator (today: HTTP 501 NotImplemented).
3. Call CreateScheduledQuery with Name="demo-query", QueryString="SELECT 1", ScheduleConfiguration.ScheduleExpression="rate(1 hour)", NotificationConfiguration.SnsConfiguration.TopicArn="arn:aws:sns:us-east-1:123456789012:t", ScheduledQueryExecutionRoleArn="arn:aws:iam::123456789012:role/r", ErrorReportConfiguration.S3Configuration.BucketName="errors" — expect 200 with an `Arn`.
4. Call DescribeScheduledQuery with that Arn — expect `ScheduledQuery.Name == "demo-query"`, `QueryString == "SELECT 1"`, `State == ENABLED`, the ScheduleConfiguration/NotificationConfiguration/ErrorReportConfiguration exactly as submitted, and a populated CreationTime.
5. Call ListScheduledQueries with MaxResults=1 — expect the created query in `ScheduledQueries`; create a second query and confirm paging with MaxResults=1 returns one item plus a NextToken that resumes at the second item and then returns no NextToken.
6. Call UpdateScheduledQuery with State=DISABLED, then DescribeScheduledQuery again — expect `State == DISABLED`.
7. Call DeleteScheduledQuery with that Arn, then DescribeScheduledQuery — expect ResourceNotFoundException (HTTP 404), and ListScheduledQueries no longer contains the ARN.
8. Re-run CreateScheduledQuery with the same ClientToken as step 3 — expect the same Arn, not a second stored record.
