Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the macie2 emulator

_Filed: 2026-10-21T21:08:32Z_
_Operation: service-com-amazonaws-macie2-macie2 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 81 operations in `services/service-com-amazonaws-macie2-macie2/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no Macie2 client workflow can run at all. Implement a FIRST, SMALL happy-path slice: the Macie session lifecycle plus the custom-data-identifier CRUD loop, which is Macie2's primary customer-owned resource.

Happy path to implement, in order (fields from OPERATIONS.md):

1. **EnableMacie** — `POST /macie`. Input: `clientToken` (@idempotencyToken), `findingPublishingFrequency` (FIFTEEN_MINUTES|ONE_HOUR|SIX_HOURS), `status` (ENABLED|PAUSED). Empty output body (200). Must CREATE a persisted session record with `createdAt`/`updatedAt` set to now, `status` defaulting to ENABLED and `findingPublishingFrequency` defaulting to FIFTEEN_MINUTES when omitted, and a synthesized `serviceRole` ARN. Calling it when a session already exists must return `ConflictException` (HTTP 409), not a second success.
2. **GetMacieSession** — `GET /macie`. No input. Output `createdAt`, `findingPublishingFrequency`, `serviceRole`, `status`, `updatedAt` — all read back from the record stored by EnableMacie/UpdateMacieSession, never canned. With no session enabled it must return `AccessDeniedException` (HTTP 403) / `ResourceNotFoundException` (404) rather than an empty 200.
3. **CreateCustomDataIdentifier** — `POST /custom-data-identifiers`. Input: `name` (required), `regex` (required), `description`, `keywords`, `ignoreWords`, `maximumMatchDistance`, `severityLevels`, `tags`, `clientToken`. Output: `customDataIdentifierId` — a server-generated id that is STORED together with every input field and a computed `arn` (`arn:aws:macie2:<region>:<account>:custom-data-identifier/<id>`) and `createdAt`. Missing `name`/`regex` must be `ValidationException` (400).
4. **GetCustomDataIdentifier** — `GET /custom-data-identifiers/{id}`. Output must echo the STORED record: `id`, `arn`, `name`, `description`, `regex`, `keywords`, `ignoreWords`, `maximumMatchDistance`, `severityLevels`, `tags`, `createdAt`, `deleted`. Unknown id => `ResourceNotFoundException` (404).
5. **ListCustomDataIdentifiers** — `POST /custom-data-identifiers/list`. Input `maxResults`, `nextToken`; output `items` (list<CustomDataIdentifierSummary>: `id`, `arn`, `name`, `description`, `createdAt`) and `nextToken`. Must enumerate exactly the identifiers currently stored, honour `maxResults` as a page bound and return an opaque `nextToken` that resumes where the previous page ended (the model marks the op `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)`).
6. **DeleteCustomDataIdentifier** — `DELETE /custom-data-identifiers/{id}`. Empty output. Must remove the record so a following GetCustomDataIdentifier returns 404 and ListCustomDataIdentifiers no longer includes it. Unknown id => `ResourceNotFoundException` (404).

Optionally close the session lifecycle with **UpdateMacieSession** (`PATCH /macie`, sets `status`/`findingPublishingFrequency` and bumps `updatedAt`) and **DisableMacie** (`DELETE /macie`, removes the session so GetMacieSession fails again) — both trivial once the state model exists.

Every one of these must compute its response from persisted shared state reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back without storing it, no random id that is never saved. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each handler as it is implemented.

## State Model

In `src/state.rs` (`ComAmazonawsMacie2Macie2State`):

- `session: Option<MacieSession>` — the single per-account Macie session.
  - `MacieSession { status: MacieStatus /* ENABLED|PAUSED */, finding_publishing_frequency: FindingPublishingFrequency, service_role: String, created_at: DateTime, updated_at: DateTime }`
- `custom_data_identifiers: BTreeMap<String /* id */, CustomDataIdentifier>` — BTreeMap so list order is stable and token-based paging is deterministic.
  - `CustomDataIdentifier { id: String, arn: String, name: String, description: Option<String>, regex: String, keywords: Vec<String>, ignore_words: Vec<String>, maximum_match_distance: Option<i32>, severity_levels: Vec<SeverityLevel>, tags: BTreeMap<String,String>, created_at: DateTime, deleted: bool }`
- `idempotency_tokens: HashMap<String /* clientToken */, String /* created id */>` — so a retried CreateCustomDataIdentifier / EnableMacie with the same `clientToken` returns the original result instead of creating a duplicate.

Done = a client script can EnableMacie -> GetMacieSession -> CreateCustomDataIdentifier -> ListCustomDataIdentifiers -> GetCustomDataIdentifier -> DeleteCustomDataIdentifier -> (Get now 404) and every response body is derived from the state above. All other operations (classification jobs, findings, members/invitations, allow lists, automated discovery, reveal/export config, tagging, buckets) stay 501 and are filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-macie2-macie2/OPERATIONS.md` lists 81 operations for restJson1 service sdkId `Macie2` and gives the exact contracts cited above: EnableMacie (`POST /macie`, clientToken/findingPublishingFrequency/status, empty output), GetMacieSession (`GET /macie`, outputs createdAt/findingPublishingFrequency/serviceRole/status/updatedAt), CreateCustomDataIdentifier (`POST /custom-data-identifiers`, required `name`+`regex`, output `customDataIdentifierId`), GetCustomDataIdentifier (`GET /custom-data-identifiers/{id}` with the full record incl. `arn`, `createdAt`, `deleted`), ListCustomDataIdentifiers (`POST /custom-data-identifiers/list`, @paginated nextToken/items), DeleteCustomDataIdentifier (`DELETE /custom-data-identifiers/{id}`), UpdateMacieSession (`PATCH /macie`), DisableMacie (`DELETE /macie`). Each declares ValidationException (400), ResourceNotFoundException (404) and ConflictException (409) among its errors. A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` across `src/ops/` matches every handler file in the crate (e.g. `src/ops/enable_macie.rs`, `src/ops/create_custom_data_identifier.rs`, `src/ops/get_macie_session.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. No handler reads or writes `crate::state::service_state`, so no state exists today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI macie2 client at it.
2. `aws macie2 enable-macie --finding-publishing-frequency ONE_HOUR` — currently HTTP 501 NotImplemented; must return 200 and persist the session.
3. `aws macie2 get-macie-session` — must return status=ENABLED, findingPublishingFrequency=ONE_HOUR, a serviceRole and createdAt/updatedAt matching what enable-macie stored.
4. `aws macie2 create-custom-data-identifier --name demo-cdi --regex '[0-9]{3}-[0-9]{2}-[0-9]{4}' --keywords ssn --description 'demo'` — must return a generated customDataIdentifierId.
5. `aws macie2 list-custom-data-identifiers` — items must contain exactly one summary with that id, arn, name=demo-cdi and the stored createdAt.
6. `aws macie2 get-custom-data-identifier --id <id>` — must return name=demo-cdi, the same regex, keywords=[ssn], description='demo', deleted=false.
7. `aws macie2 create-custom-data-identifier --name demo-cdi-2 --regex 'x+'` then `aws macie2 list-custom-data-identifiers --max-results 1` — must return 1 item plus a nextToken, and passing that nextToken must return the second item.
8. `aws macie2 delete-custom-data-identifier --id <id>` then `aws macie2 get-custom-data-identifier --id <id>` — must fail with ResourceNotFoundException (HTTP 404), and list must no longer include it.
9. `aws macie2 create-custom-data-identifier --name no-regex` (omitting required regex) — must fail with ValidationException (HTTP 400).
10. `aws macie2 get-custom-data-identifier --id does-not-exist` — must fail with ResourceNotFoundException (HTTP 404), not a hollow 200.
