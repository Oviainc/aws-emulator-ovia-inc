Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the notifications emulator

_Filed: 2026-10-21T22:08:19Z_
_Operation: service-com-amazonaws-notifications-notifications | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 40 operations under `services/service-com-amazonaws-notifications-notifications/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. Nothing works end to end.

The first slice is the lifecycle of the service's primary resource, the NotificationConfiguration (wire protocol `restJson1`, signing name `notifications`). Implement exactly these five operations, statefully, in this order:

1. **CreateNotificationConfiguration** — `POST /notification-configurations` -> 201. Input: `name` (required, len 1..64, `^[A-Za-z0-9_\-]+$`), `description` (required, len 0..256), `aggregationDuration` (LONG|NONE|SHORT, optional), `tags` (map, optional). Must mint a NEW arn matching `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$` (27 lowercase-alphanumeric id), persist the record (name, description, aggregationDuration, tags, creationTime=now, status=ACTIVE, subtype=ACCOUNT) and return `{arn, status}` read back from the stored record — not an echo of the input.
2. **GetNotificationConfiguration** — `GET /notification-configurations/{arn}` -> 200. Must look the arn up in the store and return `arn`, `name`, `description`, `status`, `creationTime` (all required), plus `aggregationDuration` and `subtype` exactly as persisted by the create call. Unknown arn -> `ResourceNotFoundException` (HTTP 404), never a canned 200.
3. **ListNotificationConfigurations** — `GET /notification-configurations` -> 200. Must return one `NotificationConfigurationStructure` per stored configuration (never a fixed list), honour the `status` and `subtype` query filters, and honour `maxResults`/`nextToken` pagination: truncate at `maxResults`, emit an opaque `nextToken` (len 1..4096, `^[\w+-/=]+$`) when more remain, and resume from it — omitting `nextToken` on the final page.
4. **UpdateNotificationConfiguration** — `PUT /notification-configurations/{arn}` -> 200. Input: `arn` (httpLabel), optional `name`, `description`, `aggregationDuration`. Must mutate the stored record in place (leaving omitted members untouched) and return `{arn}`; a subsequent Get must observe the new values. Unknown arn -> `ResourceNotFoundException` (404).
5. **DeleteNotificationConfiguration** — `DELETE /notification-configurations/{arn}` -> 200, empty body. Must remove the record so a following Get returns `ResourceNotFoundException` (404) and List no longer includes it. Unknown arn -> `ResourceNotFoundException` (404).

No canned, hardcoded or echoed responses, and no random-ID-without-storage: every field in every response above must be computed from the persisted record. Each handler reaches the store via `crate::state::service_state(&*self.engine)` as the stub docs describe, and must have its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line deleted when implemented.

## State Model

```text
NotificationsState {
    // arn -> configuration; insertion-ordered (IndexMap/BTreeMap) so List is stable
    configurations: Map<String /*arn*/, NotificationConfigurationRecord>,
}

NotificationConfigurationRecord {
    arn: String,                       // arn:aws:notifications::<account>:configuration/<27-char id>
    name: String,                      // from create, mutable via update
    description: String,               // from create, mutable via update
    aggregation_duration: Option<AggregationDuration>, // LONG | NONE | SHORT
    status: NotificationConfigurationStatus,           // ACTIVE on create, DELETING while deleting
    subtype: Option<NotificationConfigurationSubtype>, // ACCOUNT for self-created
    creation_time: Timestamp,          // set once at create, never rewritten
    tags: Map<String, String>,         // from create's `tags`, 0..200 entries
}
```

The arn is the single key for lookup, update and delete; `tags` is persisted now so a later slice can implement TagResource/UntagResource/ListTagsForResource against the same record. Event rules, channel associations, notification hubs, managed notifications, organizational access and notification events stay stubbed at 501 and are filed as individual gap issues later.

## Evidence

OPERATIONS.md (services/service-com-amazonaws-notifications-notifications/OPERATIONS.md) lists all 40 operations with their routes, members and errors: CreateNotificationConfiguration `POST /notification-configurations -> 201` with required `name`/`description` and output required `arn`+`status`; GetNotificationConfiguration `GET /notification-configurations/{arn} -> 200` with required `arn`, `name`, `description`, `status`, `creationTime` and `ResourceNotFoundException (HTTP 404)`; ListNotificationConfigurations `GET /notification-configurations -> 200`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=notificationConfigurations)`; UpdateNotificationConfiguration `PUT /notification-configurations/{arn} -> 200` returning required `arn`; DeleteNotificationConfiguration `DELETE /notification-configurations/{arn} -> 200` with no output members. Handler evidence: services/service-com-amazonaws-notifications-notifications/src/ops/create_notification_configuration.rs lines 4 and 178 show the `SCAFFOLDED, NOT IMPLEMENTED` marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateNotificationConfiguration"))`; all 40 files in src/ops/ are the same generated stub. No tests exist for any of these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `notifications` client at it.
2. CreateNotificationConfiguration(name='my-config', description='demo config', aggregationDuration='SHORT', tags={'env':'dev'}) -> expect 201 with a freshly minted arn matching ^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$ and status='ACTIVE'; actual: HTTP 501 NotImplemented.
3. GetNotificationConfiguration(arn=<arn from step 2>) -> expect 200 with name='my-config', description='demo config', aggregationDuration='SHORT', status='ACTIVE', subtype='ACCOUNT' and a creationTime.
4. ListNotificationConfigurations(maxResults=1) -> expect the created configuration in notificationConfigurations and, if more than one exists, a nextToken that resumes the listing; the final page must omit nextToken.
5. UpdateNotificationConfiguration(arn=<arn>, description='updated demo') -> expect 200 with the same arn, then GetNotificationConfiguration(arn) must report description='updated demo' with name unchanged.
6. DeleteNotificationConfiguration(arn=<arn>) -> expect 200, then GetNotificationConfiguration(arn) must fail with ResourceNotFoundException (HTTP 404) and ListNotificationConfigurations must no longer include the arn.
7. GetNotificationConfiguration(arn='arn:aws:notifications::123456789012:configuration/aaaaaaaaaaaaaaaaaaaaaaaaaaa') for a never-created arn -> expect ResourceNotFoundException (HTTP 404).
