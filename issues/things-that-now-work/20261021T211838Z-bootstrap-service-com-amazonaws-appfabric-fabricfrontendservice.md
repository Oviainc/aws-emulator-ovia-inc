Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AppFabric emulator

_Filed: 2026-10-21T21:18:38Z_
_Operation: service-com-amazonaws-appfabric-fabricfrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 26 operations in `services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns HTTP 501 `NotImplemented`, and the crate holds no state. There is no working emulator for AppFabric (restJson1, sdkId `AppFabric`, sigv4 name `appfabric`).

Implement a FIRST, SMALL, stateful slice: the app-bundle / ingestion lifecycle, which is AppFabric's primary resource chain. These 6 operations must work end-to-end against persisted state:

1. **CreateAppBundle** — `POST /appbundles -> 201`. Input: `clientToken` (@idempotencyToken), `customerManagedKeyIdentifier`, `tags`. Output: `appBundle` (AppBundle{`arn` required, `customerManagedKeyArn`}). Mint a UUID and a real ARN (`arn:aws:appfabric:<region>:<account>:appbundle/<uuid>`), store the bundle (plus tags and the client token for idempotent replay), and return the stored record.
2. **GetAppBundle** — `GET /appbundles/{appBundleIdentifier} -> 200`. Input `appBundleIdentifier` (ARN **or** bare UUID — both forms must resolve to the same stored bundle). Output `appBundle` must be the exact record created in step 1, including `customerManagedKeyArn` when one was supplied. Unknown identifier -> `ResourceNotFoundException` (404).
3. **ListAppBundles** — `GET /appbundles -> 200`, @paginated(inputToken=nextToken, outputToken=nextToken, items=appBundleSummaryList). Output `appBundleSummaryList` (list<AppBundleSummary{arn}>) built from the store, honouring `maxResults` (1..100) and round-tripping `nextToken`; omit `nextToken` on the last page.
4. **CreateIngestion** — `POST /appbundles/{appBundleIdentifier}/ingestions -> 201`. Input: `app` (required), `tenantId` (required), `ingestionType` (`auditLog`, required), `clientToken`, `tags`. Output `ingestion` (Ingestion{`arn`, `appBundleArn`, `app`, `tenantId`, `createdAt`, `updatedAt`, `state`, `ingestionType` — all required). Must 404 with `ResourceNotFoundException` when the parent bundle does not exist, persist the ingestion under its bundle, set `state` to `disabled` on creation (IngestionState = enabled|disabled), and set `createdAt`/`updatedAt` to real timestamps.
5. **ListIngestions** — `GET /appbundles/{appBundleIdentifier}/ingestions -> 200`, @paginated(items=ingestions). Output `ingestions` (list<IngestionSummary{arn, app, tenantId, state}>) for that bundle only, derived from the store, with `maxResults`/`nextToken` honoured. (`GetIngestion` on the same record should return the full Ingestion; implement it alongside if cheap.)
6. **DeleteAppBundle** — `DELETE /appbundles/{appBundleIdentifier} -> 204`. Remove the bundle from the store so a subsequent `GetAppBundle` returns `ResourceNotFoundException` (404) and `ListAppBundles` no longer lists it.

NO canned, echoed or hardcoded responses: every field in every response above must be read back out of the store that the create calls wrote. A random ARN returned without storing the record is a failure of this issue, as is a `GetAppBundle` that echoes the path identifier. Identifier resolution must accept both the ARN and the bare UUID form permitted by the shared pattern `^arn:.+$|^[a-f0-9]{8}-...$`.

## State Model

In `src/state.rs` (`ComAmazonawsAppfabricFabricfrontendserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `app_bundles: HashMap<String /* bundle uuid */, AppBundleRecord>` where
  `AppBundleRecord { id: String, arn: String, customer_managed_key_arn: Option<String>, tags: Vec<Tag>, created_at: SystemTime, ingestions: HashMap<String /* ingestion uuid */, IngestionRecord> }`
- `IngestionRecord { id: String, arn: String, app_bundle_arn: String, app: String, tenant_id: String, ingestion_type: IngestionType, state: IngestionState, created_at: SystemTime, updated_at: SystemTime, tags: Vec<Tag>, destinations: HashMap<String, IngestionDestinationRecord> /* reserved for a later slice */ }`
- `client_tokens: HashMap<String /* clientToken */, String /* arn of the resource created */>` so the @idempotent creates replay the same resource instead of creating a duplicate.
- Insertion order (or a monotonic sequence on each record) must be kept so list pagination is stable and `nextToken` can encode a position.

The remaining 20 operations (app authorizations, ingestion destinations, Start/StopIngestion, user-access tasks, tagging) stay stubbed at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-appfabric-fabricfrontendservice/OPERATIONS.md` lists all 26 operations with their routes, required members and errors (CreateAppBundle POST /appbundles -> 201 with required output `appBundle`; GetAppBundle GET /appbundles/{appBundleIdentifier} -> 200 with ResourceNotFoundException 404; ListAppBundles @paginated with maxResults 1..100 and nextToken; CreateIngestion POST /appbundles/{appBundleIdentifier}/ingestions -> 201; ListIngestions @paginated items=ingestions; DeleteAppBundle DELETE -> 204). Shape detail from the Smithy model: `AppBundle{arn required, customerManagedKeyArn}`, `AppBundleSummary{arn required}`, `Ingestion{arn, appBundleArn, app, tenantId, createdAt, updatedAt, state, ingestionType — all required}`, `IngestionSummary{arn, app, tenantId, state}`, `IngestionState = enabled|disabled`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 26 handler files, i.e. the whole service answers 501 and persists nothing.

## Steps to Reproduce

1. Start the emulator and point an AppFabric client at it (restJson1, signing name `appfabric`).
2. `CreateAppBundle` with a `clientToken` and `tags` — expect 201 and an `appBundle.arn` of the form arn:aws:appfabric:<region>:<account>:appbundle/<uuid>; actual: HTTP 501 NotImplemented.
3. `GetAppBundle` with that ARN, then again with the bare UUID — both must return 200 with the identical `appBundle` record created above.
4. `ListAppBundles` — the new bundle must appear in `appBundleSummaryList`; repeat with `maxResults=1` and follow `nextToken` to page through.
5. `CreateIngestion` on that bundle with app="SLACK", tenantId="t-1", ingestionType="auditLog" — expect 201 with `ingestion.appBundleArn` equal to the bundle ARN, `state`="disabled" and non-null createdAt/updatedAt.
6. `ListIngestions` for the bundle — the ingestion must be listed with matching arn/app/tenantId/state; `ListIngestions` on a different bundle must not list it.
7. `CreateIngestion` against a non-existent bundle id — expect ResourceNotFoundException (404), not 501 or 200.
8. `DeleteAppBundle` — expect 204, then `GetAppBundle` on the same identifier must return ResourceNotFoundException (404) and `ListAppBundles` must no longer include it.
