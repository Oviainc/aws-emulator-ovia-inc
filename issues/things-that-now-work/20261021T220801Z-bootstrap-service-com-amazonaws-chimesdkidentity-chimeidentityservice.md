Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Chime SDK Identity emulator

_Filed: 2026-10-21T22:08:01Z_
_Operation: service-com-amazonaws-chimesdkidentity-chimeidentityservice | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 30 operations in `services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no client workflow can run. Implement a FIRST SMALL SLICE: the stateful lifecycle of the service's primary resources, the `AppInstance` and its `AppInstanceUser` (restJson1, sigv4 name `chime`, endpoint prefix `identity-chime`).

Happy path to implement, in order:

1. **CreateAppInstance** — `POST /app-instances` -> 201. Input `Name` (required, 1..256), `Metadata` (0..1024), `ClientRequestToken` (required, idempotency token), `Tags`. Must mint and PERSIST a new app instance keyed by a generated ARN of the form `arn:aws:chime:<region>:<account>:app-instance/<uuid>` and return it as `AppInstanceArn`. Repeating the same `ClientRequestToken` must return the SAME ARN rather than creating a second instance.
2. **DescribeAppInstance** — `GET /app-instances/{AppInstanceArn}` -> 200. Must return the stored `AppInstance { AppInstanceArn, Name, Metadata, CreatedTimestamp, LastUpdatedTimestamp }` with exactly the `Name`/`Metadata` supplied at create time. An unknown ARN must fail (`BadRequestException` 400 / `NotFoundException` where modeled), never a hollow 200.
3. **CreateAppInstanceUser** — `POST /app-instance-users` -> 201. Input `AppInstanceArn` (required), `AppInstanceUserId` (required, 1..64), `Name` (required, 1..100), `Metadata`, `ClientRequestToken` (required), `ExpirationSettings`. Must reject a non-existent parent `AppInstanceArn`, store the user under that instance, and return `AppInstanceUserArn` = `<app-instance-arn>/user/<AppInstanceUserId>`.
4. **DescribeAppInstanceUser** — `GET /app-instance-users/{AppInstanceUserArn}` -> 200. Must return the stored `AppInstanceUser { AppInstanceUserArn, Name, Metadata, CreatedTimestamp, LastUpdatedTimestamp, ExpirationSettings }`.
5. **ListAppInstanceUsers** — `GET /app-instance-users?app-instance-arn=...` -> 200. Must return one `AppInstanceUserSummary { AppInstanceUserArn, Name, Metadata }` per persisted user of that instance, echo `AppInstanceArn`, honour `MaxResults` (1..50) and emit/consume `NextToken` so a second page returns the remaining users and the final page omits the token.
6. **DeleteAppInstanceUser** — `DELETE /app-instance-users/{AppInstanceUserArn}` -> 204, and **DeleteAppInstance** — `DELETE /app-instances/{AppInstanceArn}` -> 204. Both must remove state: a subsequent Describe/List must no longer see the deleted resource, and deleting an app instance must cascade to its users.

Every one of these must compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`). No canned bodies, no echoed inputs, no generated ARN that is not stored. `ListAppInstances` (`GET /app-instances`) is a cheap seventh addition over the same map and is welcome in this slice.

## State Model

In `src/state.rs`, on the service state struct:

- `app_instances: BTreeMap<String /* AppInstanceArn */, AppInstance>` where `AppInstance { arn: String, name: String, metadata: Option<String>, created_timestamp: SystemTime, last_updated_timestamp: SystemTime, tags: BTreeMap<String,String>, users: BTreeMap<String /* AppInstanceUserArn */, AppInstanceUser>, admins: BTreeMap<String /* AppInstanceAdminArn */, SystemTime>, retention_settings: Option<AppInstanceRetentionSettings> }`
- `AppInstanceUser { arn: String, app_instance_arn: String, user_id: String, name: String, metadata: Option<String>, created_timestamp: SystemTime, last_updated_timestamp: SystemTime, expiration_settings: Option<ExpirationSettings>, endpoints: BTreeMap<String /* EndpointId */, Endpoint> }`
- `idempotency: BTreeMap<String /* ClientRequestToken */, String /* created ARN */>` so repeated create calls with one token are idempotent.
- `users_by_arn: BTreeMap<String, String>` (user ARN -> owning instance ARN) or equivalent lookup so `DescribeAppInstanceUser`/`DeleteAppInstanceUser`, which are keyed only by the user ARN, resolve in one hop.
- Ordering: `BTreeMap` iteration order gives deterministic, stable list pages; `NextToken` can be the last-returned key (opaque base64) so paging is resumable.

Done = a client script can CreateAppInstance -> DescribeAppInstance -> CreateAppInstanceUser -> DescribeAppInstanceUser -> ListAppInstanceUsers -> DeleteAppInstanceUser -> DeleteAppInstance end-to-end, with every response body reflecting what was actually created, and reads after delete failing instead of succeeding.

The remaining 23 operations (bots, admins, endpoints, tagging, retention/expiration settings, the Update* family) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for this crate (30 operations, restJson1, sdkId `Chime SDK Identity`) documents the routes and members cited above: CreateAppInstance `POST /app-instances -> 201` with required `Name`+`ClientRequestToken` returning `AppInstanceArn`; DescribeAppInstance `GET /app-instances/{AppInstanceArn} -> 200` returning `AppInstance`; CreateAppInstanceUser `POST /app-instance-users -> 201` with required `AppInstanceArn`, `AppInstanceUserId`, `Name`, `ClientRequestToken`; DescribeAppInstanceUser `GET /app-instance-users/{AppInstanceUserArn} -> 200`; ListAppInstanceUsers `GET /app-instance-users -> 200` @paginated(inputToken=NextToken, outputToken=NextToken) with `MaxResults` range 1..50; DeleteAppInstanceUser and DeleteAppInstance -> 204. The `AppInstance`, `AppInstanceUser`, `AppInstanceSummary` and `AppInstanceUserSummary` member lists (incl. `CreatedTimestamp`/`LastUpdatedTimestamp`) come from smithy_query on the model at smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 30 handler files, including create_app_instance.rs, describe_app_instance.rs, create_app_instance_user.rs, describe_app_instance_user.rs, list_app_instance_users.rs, delete_app_instance_user.rs and delete_app_instance.rs — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exist beyond the generated tests/state_roundtrip.rs.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client for `chime-sdk-identity` at it.
2. Call CreateAppInstance with Name="demo-app", Metadata="m1", ClientRequestToken="tok-1" — expect 201 and an AppInstanceArn; today: HTTP 501 NotImplemented.
3. Repeat the identical CreateAppInstance call with ClientRequestToken="tok-1" — expect the SAME AppInstanceArn (idempotent), not a second instance.
4. Call DescribeAppInstance with that ARN — expect 200 with AppInstance.Name=="demo-app", Metadata=="m1", and populated CreatedTimestamp/LastUpdatedTimestamp.
5. Call CreateAppInstanceUser with that AppInstanceArn, AppInstanceUserId="user1", Name="User One", ClientRequestToken="tok-2" — expect 201 with an AppInstanceUserArn derived from the instance ARN.
6. Call DescribeAppInstanceUser with that user ARN — expect 200 with Name=="User One".
7. Create a second user "user2", then call ListAppInstanceUsers?app-instance-arn=<arn>&max-results=1 — expect exactly 1 summary plus a NextToken; pass the NextToken back and expect the other user and no NextToken.
8. Call DeleteAppInstanceUser on user1 — expect 204, then DescribeAppInstanceUser on user1 must fail and ListAppInstanceUsers must no longer include it.
9. Call DeleteAppInstance on the instance — expect 204, then DescribeAppInstance must fail and ListAppInstances must not include it; its remaining users must be gone too.
