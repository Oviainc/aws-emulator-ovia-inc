Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the ssm-incidents (SSM Incident Manager) emulator

_Filed: 2026-10-21T22:16:44Z_
_Operation: service-com-amazonaws-ssmincidents-ssmincidents | Class: missing_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 31 operations in `services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all.

Implement a FIRST SLICE of 6 operations that make the primary resource lifecycle (response plan -> incident record) work end to end on the `restJson1` wire protocol (sigv4 signing name `ssm-incidents`). Every one of these must compute its response from persisted state held in the shared engine store (`crate::state::service_state(&*self.engine)`) — no canned bodies, no echoing the input back, no minting an ARN that is never stored.

### Happy path (in order)

1. **CreateResponsePlan** — `POST /createResponsePlan -> 201`. Input: `name` (required, pattern `^[a-zA-Z0-9-_]*$`), `incidentTemplate` (required: `title`, `impact` 1..5, optional `summary`, `dedupeString`, `notificationTargets`, `incidentTags`), optional `displayName`, `chatChannel`, `engagements`, `actions`, `integrations`, `tags`, `clientToken`. Must mint and STORE a response-plan ARN (`arn:aws:ssm-incidents::<account>:response-plan/<name>`) keyed so later reads see it, and return that same `arn` in the output. Re-creating the same `name` with different contents must raise `ConflictException` (409).
2. **StartIncident** — `POST /startIncident -> 200`. Input: `responsePlanArn` (required), optional `title`, `impact`, `triggerDetails`, `relatedItems`, `clientToken`. Must look the response plan up in state and return `ResourceNotFoundException` (404) when it is absent. On success, create and persist a new IncidentRecord whose `title`/`impact`/`summary`/`dedupeString`/`notificationTargets`/`chatChannel` default from the plan's `incidentTemplate` and are overridden by the request's `title`/`impact`. Set `status=OPEN`, `creationTime`/`lastModifiedTime` to now, `lastModifiedBy` and `incidentRecordSource.createdBy` to the caller principal, `incidentRecordSource.source` (e.g. `aws.ssm-incidents` / manual). Return the newly stored `incidentRecordArn`.
3. **GetIncidentRecord** — `GET /getIncidentRecord?arn=... -> 200`. Must return the stored `incidentRecord` with EVERY required member populated from state (`arn`, `title`, `status`, `impact`, `creationTime`, `lastModifiedTime`, `lastModifiedBy`, `incidentRecordSource`, `dedupeString`) plus optional `summary`, `resolvedTime`, `chatChannel`, `notificationTargets`. Unknown ARN -> `ResourceNotFoundException` (404).
4. **ListIncidentRecords** — `POST /listIncidentRecords -> 200`. Must return one `IncidentRecordSummary` (`arn`, `title`, `status`, `impact`, `creationTime`, `resolvedTime`, `incidentRecordSource`) per stored incident — derived from the same records `GetIncidentRecord` serves, never a fixed list. Honour `maxResults` (1..100) and round-trip `nextToken` so paging terminates.
5. **UpdateIncidentRecord** — `POST /updateIncidentRecord -> 204`. Input: `arn` (required), optional `title`, `summary`, `impact` (1..5), `status` (`OPEN`|`RESOLVED`), `chatChannel`, `notificationTargets`. Must MUTATE the stored record (only the supplied fields), bump `lastModifiedTime`/`lastModifiedBy`, and set `resolvedTime` when `status` transitions to `RESOLVED`. A following `GetIncidentRecord` must observe the change. Unknown ARN -> `ResourceNotFoundException` (404).
6. **DeleteIncidentRecord** — `POST /deleteIncidentRecord -> 204`. Must remove the record from state so a subsequent `GetIncidentRecord` returns `ResourceNotFoundException` (404) and `ListIncidentRecords` no longer lists it. Idempotent (`@idempotent`): deleting an already-absent ARN succeeds with 204.

Shared requirement for all six: reject input violating the modeled constraints with `ValidationException` (400) — missing required members, `impact` outside 1..5, `status` outside `OPEN|RESOLVED`, `name` failing its pattern, ARNs failing `^arn:aws(-cn|-us-gov)?:[a-z0-9-]*:[a-z0-9-]*:([0-9]{12})?:.+$`.

## State Model

Store in the per-service state struct (`src/state.rs`), reached via `crate::state::service_state`:

- `response_plans: HashMap<String /* plan arn */, ResponsePlan>` where `ResponsePlan { arn, name, display_name: Option<String>, incident_template: IncidentTemplate { title, impact, summary, dedupe_string, notification_targets, incident_tags }, chat_channel, engagements: Vec<String>, actions: Vec<Action>, integrations: Vec<Integration>, tags: HashMap<String,String>, created_time }`
- `incident_records: HashMap<String /* incident arn */, IncidentRecordEntry>` where `IncidentRecordEntry { arn, title, summary: Option<String>, status: IncidentRecordStatus, impact: i32, creation_time, resolved_time: Option<Timestamp>, last_modified_time, last_modified_by, dedupe_string, incident_record_source: { created_by, invoked_by, resource_arn, source }, chat_channel: Option<ChatChannel>, notification_targets: Vec<NotificationTargetItem>, response_plan_arn, timeline_events: Vec<TimelineEvent>, related_items: Vec<RelatedItem> }`
- `idempotency: HashMap<String /* clientToken */, String /* arn returned the first time */>` so a replayed `clientToken` on CreateResponsePlan / StartIncident returns the SAME ARN instead of creating a duplicate.
- A monotonic counter / ULID source for incident ARNs (`arn:aws:ssm-incidents::<account>:incident-record/<plan-name>/<id>`) — the generated id must be written into the map, not returned ad hoc.

`timeline_events` and `related_items` are reserved in the struct now (so the follow-up slice needs no migration) but the timeline/related-item/replication-set/resource-policy/tagging/findings operations stay 501 in this slice and will be filed as individual gap issues later.

## Done when

A realistic client script (AWS CLI or SDK) runs CreateResponsePlan -> StartIncident -> GetIncidentRecord -> ListIncidentRecords -> UpdateIncidentRecord(status=RESOLVED) -> GetIncidentRecord -> DeleteIncidentRecord -> GetIncidentRecord(404) with correct, stateful bodies at every step — not merely 2xx status codes.

## Evidence

OPERATIONS.md (crate root) declares 31 operations for `restJson1` / sdkId `SSM Incidents`, including the routes and members quoted above: CreateResponsePlan (`POST /createResponsePlan -> 201`, requires `name` + `incidentTemplate`, outputs required `arn`), StartIncident (`POST /startIncident -> 200`, requires `responsePlanArn`, outputs required `incidentRecordArn`, declares ResourceNotFoundException/ConflictException), GetIncidentRecord (`GET /getIncidentRecord -> 200`, `arn` via @httpQuery, outputs required `incidentRecord`), ListIncidentRecords (`POST /listIncidentRecords -> 200`, @paginated on nextToken/incidentRecordSummaries, maxResults range 1..100), UpdateIncidentRecord (`POST /updateIncidentRecord -> 204`, `status` one of OPEN|RESOLVED, `impact` range 1..5), DeleteIncidentRecord (`POST /deleteIncidentRecord -> 204`, @idempotent). Smithy model shapes confirm the response fields that must come from state: IncidentRecord requires arn/title/status/impact/creationTime/lastModifiedTime/lastModifiedBy/incidentRecordSource/dedupeString; IncidentRecordSummary requires arn/title/status/impact/creationTime/incidentRecordSource; IncidentRecordSource requires createdBy/source; IncidentTemplate requires title/impact and carries summary/dedupeString/notificationTargets/incidentTags that StartIncident inherits. Current implementation: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/` matches all 31 handler files (create_response_plan.rs, start_incident.rs, get_incident_record.rs, list_incident_records.rs, update_incident_record.rs, delete_incident_record.rs among them), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs` state-API example, and `list_existing_issues` reports no issues filed for this service.

## Steps to Reproduce

1. Start the emulator and point an SSM Incidents client at it (signing name `ssm-incidents`).
2. aws ssm-incidents create-response-plan --name my-plan --incident-template '{"title":"DB down","impact":2}' -> expect 201 with a response-plan ARN; today returns HTTP 501 NotImplemented.
3. aws ssm-incidents start-incident --response-plan-arn <arn from step 2> --title 'DB down now' --impact 1 -> expect 200 with incidentRecordArn.
4. aws ssm-incidents get-incident-record --arn <incidentRecordArn> -> expect incidentRecord.status=OPEN, title='DB down now', impact=1, dedupeString/creationTime/lastModifiedBy/incidentRecordSource populated from state.
5. aws ssm-incidents list-incident-records -> expect the new incident's summary in incidentRecordSummaries.
6. aws ssm-incidents update-incident-record --arn <incidentRecordArn> --status RESOLVED --summary 'failover complete' -> expect 204, then get-incident-record shows status=RESOLVED, the new summary, a resolvedTime and a bumped lastModifiedTime.
7. aws ssm-incidents delete-incident-record --arn <incidentRecordArn> -> expect 204; get-incident-record on the same ARN must then fail with ResourceNotFoundException (404) and list-incident-records must no longer include it.
8. Negative check: get-incident-record --arn arn:aws:ssm-incidents::111122223333:incident-record/nope/1 -> expect ResourceNotFoundException (404), and start-incident with --impact 9 -> expect ValidationException (400).
