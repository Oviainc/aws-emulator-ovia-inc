Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the AmplifyUIBuilder emulator

_Filed: 2026-10-21T22:16:24Z_
_Operation: service-com-amazonaws-amplifyuibuilder-amplifyuibuilder | Class: missing_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 28 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no client workflow can run. Implement a FIRST, SMALL slice: the stateful lifecycle of the service's primary resource, the Component (restJson1, sigv4 name `amplifyuibuilder`). Everything else (Forms, Themes, codegen jobs, tagging, Figma token exchange, Export*) stays stubbed and will be filed as individual gap issues later.

### Happy path to implement (in order)

1. **CreateComponent** — `POST /app/{appId}/environment/{environmentName}/components?clientToken=...`. Input: `appId`, `environmentName` (@httpLabel, required) and the `componentToCreate` payload (`CreateComponentData`: required `name`, `componentType`, `properties`, `variants`, `overrides`, `bindingProperties`; optional `sourceId`, `children`, `collectionProperties`, `tags`, `events`, `schemaVersion`). Must mint a fresh UUID `id`, set `createdAt` to now, store the component under (appId, environmentName, id), and return the stored record as the `entity` (`Component`) payload — including required `appId`, `environmentName`, `id`, `name`, `componentType`, `properties`, `variants`, `overrides`, `bindingProperties`, `createdAt`. Must NOT return a canned/echoed body that is not also persisted.
2. **GetComponent** — `GET /app/{appId}/environment/{environmentName}/components/{id}`. Must look up the stored record and return it as the `component` payload, byte-for-byte equal to what CreateComponent persisted (same `id`, `createdAt`, all properties/variants/overrides). Unknown `id` (or unknown app/environment) must fail with `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListComponents** — `GET /app/{appId}/environment/{environmentName}/components?maxResults=&nextToken=`. Must return one `ComponentSummary` (`appId`, `environmentName`, `id`, `name`, `componentType`) per component actually stored for that (appId, environmentName) — never a hardcoded list. Honour `maxResults` (1..100) as a page bound and emit/accept an opaque `nextToken` that resumes where the previous page ended; omit `nextToken` on the last page.
4. **UpdateComponent** — `PATCH /app/{appId}/environment/{environmentName}/components/{id}`. Input payload `updatedComponent` (`UpdateComponentData`: all members optional). Must merge only the members present onto the stored record, leave the others untouched, set `modifiedAt` to now, keep `id`/`createdAt` stable, persist the merged record, and return it as `entity`. A subsequent GetComponent must show the merged values. Unknown `id` must fail with `ResourceNotFoundException` (404).
5. **DeleteComponent** — `DELETE /app/{appId}/environment/{environmentName}/components/{id}`. Must remove the record and return an empty 2xx (output has no members). A following GetComponent must then return `ResourceNotFoundException` (404) and ListComponents must no longer include the id. Deleting an unknown id must return `ResourceNotFoundException` (404).

All five must compute their responses from the shared engine store reached via `crate::state::service_state(&*self.engine)` — no canned responses, no echoed inputs, and no "random id without storage". Removing the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker from these five files is part of the change.

## State Model

In `src/state.rs`, on `ComAmazonawsAmplifyuibuilderAmplifyuibuilderState`:

- `components: HashMap<(String /*appId*/, String /*environmentName*/), HashMap<String /*id (uuid)*/, StoredComponent>>`
- `StoredComponent { app_id: String, environment_name: String, id: String, name: String, component_type: String, properties: ComponentProperties, variants: ComponentVariants, overrides: ComponentOverrides, binding_properties: ComponentBindingProperties, children: Option<ComponentChildList>, collection_properties: Option<ComponentCollectionProperties>, events: Option<ComponentEvents>, source_id: Option<String>, schema_version: Option<String>, tags: Option<HashMap<String,String>>, created_at: SystemTime, modified_at: Option<SystemTime> }`
- `idempotency: HashMap<String /*clientToken*/, String /*component id*/>` so a repeated CreateComponent with the same `@idempotencyToken` returns the already-created component instead of a duplicate (CreateComponent/UpdateComponent/DeleteComponent are `@idempotent` in the model).
- Insertion order for a given (appId, environmentName) preserved (e.g. an auxiliary `Vec<String>` of ids or an ordered map) so ListComponents pagination is stable across pages.

Done = an integration test that drives CreateComponent -> GetComponent -> ListComponents -> UpdateComponent -> GetComponent -> DeleteComponent -> GetComponent(404) and asserts the response bodies, not just 2xx status codes.

## Evidence

OPERATIONS.md for this crate lists all 28 operations with routes and members: CreateComponent (`POST /app/{appId}/environment/{environmentName}/components`, payload `componentToCreate`: CreateComponentData, output `entity`: Component, errors InvalidParameterException 400 / ResourceConflictException 409 / ServiceQuotaExceededException 402 / InternalServerException 500), GetComponent (`GET .../components/{id}`, output `component`, ResourceNotFoundException 404), ListComponents (`@paginated(inputToken=nextToken, outputToken=nextToken, items=entities)`, `maxResults` range 1..100, output `entities`: list<ComponentSummary> required), UpdateComponent (`PATCH .../components/{id}`, payload `updatedComponent`: UpdateComponentData), DeleteComponent (`DELETE .../components/{id}`, no output members, ResourceNotFoundException 404). Shape detail from the Smithy model via smithy_query: `Component` requires appId, environmentName, id (Uuid), name, componentType, properties, variants, overrides, bindingProperties, createdAt; `UpdateComponentData` has every member optional (merge semantics); `ComponentSummary` carries appId, environmentName, id, name, componentType. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 28 handler files (create_component.rs, get_component.rs, list_components.rs, update_component.rs, delete_component.rs among them), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. There are no tests beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK amplifyuibuilder client at it (restJson1, signing name `amplifyuibuilder`).
2. CreateComponent(appId="d1234abcd", environmentName="staging", componentToCreate={name:"Hero", componentType:"View", properties:{}, variants:[], overrides:{}, bindingProperties:{}}) — expect 200 with entity.id a UUID, entity.createdAt set; actual: HTTP 501 NotImplemented.
3. GetComponent(appId="d1234abcd", environmentName="staging", id=<id from step 2>) — expect the same component (name "Hero", componentType "View", identical id/createdAt).
4. ListComponents(appId="d1234abcd", environmentName="staging", maxResults=1) — expect entities to contain exactly the created component's summary, with nextToken absent (or honoured when more exist).
5. UpdateComponent(appId, environmentName, id, updatedComponent={name:"HeroV2"}) — expect entity.name == "HeroV2", componentType still "View", modifiedAt set, id/createdAt unchanged; then GetComponent returns the merged record.
6. DeleteComponent(appId, environmentName, id) — expect empty 2xx; then GetComponent(same id) returns ResourceNotFoundException (HTTP 404) and ListComponents no longer lists it.
7. GetComponent with an id that was never created — expect ResourceNotFoundException (HTTP 404).
