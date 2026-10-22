Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Amazon Connect emulator

_Filed: 2026-10-21T21:38:04Z_
_Operation: service-com-amazonaws-connect-amazonconnectservice | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-connect-amazonconnectservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented` while holding no state. Nothing in the service works end-to-end.

Implement a FIRST, SMALL, stateful slice: the instance -> queue lifecycle, which is Amazon Connect's primary resource loop (REST-JSON 1.1, routes as modeled).

1. **CreateInstance** — `PUT /instance`. Input: `IdentityManagementType` (required, CONNECT_MANAGED|EXISTING_DIRECTORY|SAML), `InboundCallsEnabled` (required bool), `OutboundCallsEnabled` (required bool), `InstanceAlias` (1..45, pattern `^(?!d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$`), `DirectoryId`, `ClientToken`, `Tags`. Must mint a fresh UUID `Id`, build `Arn` = `arn:aws:connect:<region>:<account>:instance/<Id>`, persist the whole record (alias, identity type, call flags, tags, CreatedTime, InstanceStatus=ACTIVE, ServiceRole, InstanceAccessUrl) and return `{Id, Arn}` — the Id must come from storage, not be echoed or random-without-storage.
2. **DescribeInstance** — `GET /instance/{InstanceId}`. Must look the instance up by Id and return the persisted `Instance` shape (`Id`, `Arn`, `IdentityManagementType`, `InstanceAlias`, `CreatedTime`, `InstanceStatus`, `InboundCallsEnabled`, `OutboundCallsEnabled`, `InstanceAccessUrl`, `Tags`) with exactly the values supplied at create time. Unknown Id -> `ResourceNotFoundException` (HTTP 404).
3. **ListInstances** — `GET /instance`. Returns `InstanceSummaryList` of `InstanceSummary` (`Id`, `Arn`, `IdentityManagementType`, `InstanceAlias`, `CreatedTime`, `InstanceStatus`, `InboundCallsEnabled`, `OutboundCallsEnabled`) built from the stored map — every created instance appears exactly once, deleted ones do not. Honour `MaxResults` (1..10) and `NextToken` (@paginated inputToken=NextToken/outputToken=NextToken/items=InstanceSummaryList): return a token when truncated and resume from it, omit it on the last page.
4. **CreateQueue** — `PUT /queues/{InstanceId}`. Input: `Name` (required, 1..127), `HoursOfOperationId` (required), `Description`, `OutboundCallerConfig`, `MaxContacts`, `QuickConnectIds`, `Tags`. Must reject an unknown `InstanceId` with `ResourceNotFoundException` (404), reject a duplicate queue `Name` within the same instance with `DuplicateResourceException` (409), persist the queue under that instance and return `{QueueId, QueueArn}` with QueueArn = `<instanceArn>/queue/<QueueId>`.
5. **DescribeQueue** — `GET /queues/{InstanceId}/{QueueId}`. Must return the stored `Queue` (`QueueId`, `QueueArn`, `Name`, `Description`, `HoursOfOperationId`, `MaxContacts`, `OutboundCallerConfig`, `Status`=ENABLED, `Tags`, `LastModifiedTime`, `LastModifiedRegion`) with the create-time values. Unknown instance or queue -> 404.
6. **ListQueues** — `GET /queues-summary/{InstanceId}`. Returns `QueueSummaryList` of `QueueSummary` (`Id`, `Arn`, `Name`, `QueueType`=STANDARD, `LastModifiedTime`) for the instance's stored queues, filtered by the `queueTypes` query param when present, paginated via `maxResults` (1..1000) / `nextToken`. Unknown instance -> 404.
7. **DeleteQueue** / **DeleteInstance** — `DELETE /queues/{InstanceId}/{QueueId}` and `DELETE /instance/{InstanceId}`. Must remove the record so the matching Describe returns `ResourceNotFoundException` (404) and the List no longer includes it; deleting an unknown id is itself a 404.

No canned or echoed responses: each of these operations must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`, so a create is observable by every later read in the same process. Remove the `SCAFFOLDED, NOT IMPLEMENTED` marker only from the handlers actually implemented.

## State Model

In `src/state.rs` (`ComAmazonawsConnectAmazonconnectserviceState`):

- `instances: BTreeMap<String /* instance id */, Instance>` where `Instance { id, arn, identity_management_type, instance_alias: Option<String>, directory_id: Option<String>, inbound_calls_enabled: bool, outbound_calls_enabled: bool, instance_status: String /* ACTIVE */, service_role: String, instance_access_url: String, created_time: SystemTime, tags: BTreeMap<String,String>, client_token: Option<String>, queues: BTreeMap<String /* queue id */, Queue> }`
- `Queue { id, arn, name, description: Option<String>, hours_of_operation_id, max_contacts: Option<i32>, outbound_caller_config: Option<OutboundCallerConfig>, outbound_email_config: Option<OutboundEmailConfig>, quick_connect_ids: Vec<String>, queue_type: String /* STANDARD */, status: String /* ENABLED */, tags: BTreeMap<String,String>, last_modified_time: SystemTime, last_modified_region: String }`
- `queue_name_index: per-instance map name -> queue id` (or derive from `queues`) to enforce `DuplicateResourceException` on same-name creates.
- `client_tokens: BTreeMap<String /* ClientToken */, String /* instance id */>` so a repeated `CreateInstance` ClientToken is idempotent rather than creating a second instance.
- Pagination: a deterministic order (BTreeMap iteration) plus an opaque `NextToken` encoding the last-returned key, used by both ListInstances and ListQueues.

All other Connect operations (contacts, users, routing profiles, flows, metrics, …) stay 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate: CreateInstance (line 372, `PUT /instance`, out `Id`/`Arn`), DeleteInstance (671, `DELETE /instance/{InstanceId}`), CreateQueue (436, `PUT /queues/{InstanceId}`, out `QueueId`/`QueueArn`, errors include DuplicateResourceException 409 and ResourceNotFoundException 404), DeleteQueue (721), DescribeInstance (959, out `Instance`), DescribeQueue (1016, out `Queue`), ListInstances (1624, @paginated items=InstanceSummaryList, MaxResults 1..10), ListQueues (1726, @paginated items=QueueSummaryList, queueTypes filter, MaxResults 1..1000). Field lists for the `Instance`, `InstanceSummary`, `Queue` and `QueueSummary` structures come from smithy_query on the model at services/service-com-amazonaws-connect-amazonconnectservice/smithy-model/com-amazonaws-connect-amazonconnectservice.json. Stub status confirmed by the `//! SCAFFOLDED, NOT IMPLEMENTED:` header in src/ops/create_instance.rs (line 4), which the crate README documents as the census marker for every generated handler.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `connect` client at it.
2. `CreateInstance` with IdentityManagementType=CONNECT_MANAGED, InstanceAlias=myorg, InboundCallsEnabled=true, OutboundCallsEnabled=true -> expect 200 with a stored Id and Arn (today: HTTP 501 NotImplemented).
3. `DescribeInstance` with that Id -> expect 200 whose Instance.InstanceAlias=myorg, InboundCallsEnabled=true, IdentityManagementType=CONNECT_MANAGED, InstanceStatus=ACTIVE.
4. `ListInstances` -> expect the new instance in InstanceSummaryList; with MaxResults=1 and two instances created, expect a NextToken that yields the second on the follow-up call.
5. `CreateQueue` on that InstanceId with Name=support, HoursOfOperationId=hop-1 -> expect 200 with QueueId/QueueArn; repeat the same Name -> expect DuplicateResourceException (409); use a bogus InstanceId -> expect ResourceNotFoundException (404).
6. `DescribeQueue` with the returned QueueId -> expect Name=support and HoursOfOperationId=hop-1 as created.
7. `ListQueues` for the instance -> expect one QueueSummary with Id/Arn/Name=support/QueueType=STANDARD.
8. `DeleteQueue` then `DescribeQueue` -> expect ResourceNotFoundException (404) and an empty QueueSummaryList.
9. `DeleteInstance` then `DescribeInstance` -> expect ResourceNotFoundException (404) and the instance gone from ListInstances.
