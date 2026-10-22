Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the Bedrock Data Automation emulator

_Filed: 2026-10-21T21:53:20Z_
_Operation: service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the service holds no state. Implement a FIRST stateful slice: the Blueprint lifecycle, which is this service's primary resource (`restJson1`, sigv4 signing name `bedrock`, endpoint prefix `bedrock-data-automation`).

### Happy path to implement (in order)

1. **CreateBlueprint** — `PUT /blueprints/` -> 201. Input: `blueprintName` (required, 1..128, `^[a-zA-Z0-9-_]+$`), `type` (required, AUDIO|DOCUMENT|IMAGE|VIDEO), `schema` (required, 1..100000), optional `blueprintStage` (DEVELOPMENT|LIVE, default DEVELOPMENT), `clientToken`, `encryptionConfiguration`, `tags`. Output: `blueprint` (Blueprint, required) — must be built from the record just stored, with a freshly minted `blueprintArn` of the form `arn:aws:bedrock:<region>:<account12>:blueprint/<uuid-12..36>`, `blueprintName`, `type`, `schema`, `blueprintStage`, and real `creationTime`/`lastModifiedTime` (`date-time`). Creating a second blueprint with a name that already exists must fail `ConflictException` (409).
2. **GetBlueprint** — `POST /blueprints/{blueprintArn}/` -> 200. Input: `blueprintArn` (@httpLabel, required), optional `blueprintVersion`, `blueprintStage`. Output: `blueprint` must be the stored record — same `schema`, `blueprintName`, `type`, `creationTime` as returned by CreateBlueprint. Unknown ARN -> `ResourceNotFoundException` (404).
3. **ListBlueprints** — `POST /blueprints/` -> 200, `@paginated(inputToken=nextToken, outputToken=nextToken, items=blueprints)`. Output `blueprints` (list<BlueprintSummary>, required) must enumerate the blueprints actually stored, each summary carrying `blueprintArn`, `blueprintName`, `blueprintVersion`, `blueprintStage`, `creationTime`, `lastModifiedTime`. Honour `maxResults` (1..1000) by truncating and returning an opaque `nextToken` that resumes where the page ended; honour the `blueprintArn` and `blueprintStageFilter` (ALL|DEVELOPMENT|LIVE) filters at least for the ACCOUNT `resourceOwner`.
4. **UpdateBlueprint** — `PUT /blueprints/{blueprintArn}/` -> 200. Input: `blueprintArn` (@httpLabel, required), `schema` (required), optional `blueprintStage`, `encryptionConfiguration`. Must mutate the stored record (new `schema`/`blueprintStage`, bumped `lastModifiedTime`, unchanged `creationTime`) and return it; a subsequent GetBlueprint must observe the new schema. Unknown ARN -> `ResourceNotFoundException` (404).
5. **DeleteBlueprint** — `DELETE /blueprints/{blueprintArn}/` -> 200, empty output. Input: `blueprintArn` (@httpLabel, required), optional `blueprintVersion` (@httpQuery). Must remove the record (or the single version when `blueprintVersion` is given); afterwards GetBlueprint must answer `ResourceNotFoundException` (404) and ListBlueprints must no longer include it.

Optional stretch inside this slice (only if cheap): **CreateBlueprintVersion** (`POST /blueprints/{blueprintArn}/versions/` -> 201) snapshotting the current schema under a monotonically increasing numeric `blueprintVersion` that GetBlueprint can retrieve with `blueprintVersion`.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, no minting an ARN that is never stored. `clientToken` on CreateBlueprint should make a repeat call return the same blueprint rather than creating a second one.

## State Model

Service state (behind `crate::state::service_state(&*self.engine)` guard) should hold at minimum:

- `blueprints: HashMap<String /*blueprintArn*/, Blueprint>` where `Blueprint { blueprint_arn: String, blueprint_name: String, r#type: Type, schema: String, blueprint_stage: BlueprintStage, creation_time: DateTime, last_modified_time: DateTime, kms_key_id: Option<String>, kms_encryption_context: Option<HashMap<String,String>>, versions: BTreeMap<u64 /*version*/, BlueprintVersionSnapshot{schema, created_at}>, tags: Vec<Tag> }` — exactly the members of the modeled `Blueprint` shape plus the version snapshots.
- `blueprint_names: HashMap<String /*blueprintName*/, String /*blueprintArn*/>` for the uniqueness check that drives `ConflictException`.
- `idempotency: HashMap<String /*clientToken*/, String /*blueprintArn*/>` so an `@idempotencyToken` replay is a no-op returning the original resource.
- `next_version: u64` per blueprint (or derive from `versions.keys().max()`), and a pagination helper that maps an opaque `nextToken` to an offset/last-seen-ARN over a stable ordering (e.g. by `creationTime` then ARN).

Operations outside this slice (projects, libraries, library ingestion jobs, blueprint optimization, tagging, CopyBlueprintStage) stay 501 and will be filed as individual gap issues later.

## Evidence

`OPERATIONS.md` at the crate root lists all 27 operations with their routes, input/output members and errors — CreateBlueprint (`PUT /blueprints/` -> 201, output `blueprint` required), GetBlueprint (`POST /blueprints/{blueprintArn}/` -> 200), ListBlueprints (`POST /blueprints/` -> 200, `@paginated` items=blueprints), UpdateBlueprint (`PUT /blueprints/{blueprintArn}/` -> 200), DeleteBlueprint (`DELETE /blueprints/{blueprintArn}/` -> 200), all declaring `ResourceNotFoundException` 404 / `ConflictException` 409 / `ValidationException` 400. `smithy_query(shape="Blueprint,BlueprintSummary")` gives the record fields cited in the State Model (blueprintArn, schema, type, creationTime, lastModifiedTime, blueprintName, blueprintVersion, blueprintStage, kmsKeyId, kmsEncryptionContext). `src/ops/create_blueprint.rs` lines 4 and 173 show the scaffold marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateBlueprint"))`; every other file in `src/ops/` is the same generated stub, so no operation holds state today. No tests assert any behaviour for these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `bedrock-data-automation` client at it.
2. CreateBlueprint with blueprintName="my-doc-bp", type="DOCUMENT", schema="{\"class\":\"invoice\"}" — expect 201 and a `blueprint` whose blueprintArn is a well-formed blueprint ARN and whose schema/name/type echo what was stored, with real creationTime/lastModifiedTime. Today: HTTP 501 NotImplemented.
3. GetBlueprint with that blueprintArn — expect 200 and the identical blueprint record (same schema and creationTime).
4. ListBlueprints with maxResults=1 after creating two blueprints — expect one summary plus a nextToken, and the second page to return the other blueprint and no token.
5. UpdateBlueprint on the first ARN with schema="{\"class\":\"receipt\"}" — expect 200, then GetBlueprint must return the new schema and a lastModifiedTime later than creationTime.
6. CreateBlueprint again with blueprintName="my-doc-bp" — expect 409 ConflictException.
7. DeleteBlueprint on the first ARN — expect 200; then GetBlueprint on it must return 404 ResourceNotFoundException and ListBlueprints must omit it.
8. GetBlueprint on a syntactically valid but never-created ARN — expect 404 ResourceNotFoundException, not 501.
