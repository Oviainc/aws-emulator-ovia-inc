Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the supplychain emulator

_Filed: 2026-10-21T21:53:15Z_
_Operation: service-com-amazonaws-supplychain-galaxypublicapigateway | Class: stub_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 30 operations in `services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, holding zero state. Nothing works end-to-end.

Implement a FIRST, SMALL, STATEFUL slice: the lifecycle of the service's primary resource, the AWS Supply Chain **Instance** (restJson1, sigv4 signing name `scn`). Five operations, in this order:

1. **CreateInstance** — `POST /api/instance`. Input: `instanceName`, `instanceDescription`, `kmsKeyArn`, `webAppDnsDomain`, `tags`, `clientToken` (@idempotencyToken). Must mint a fresh lowercase-hex UUID `instanceId` matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, persist the record, and return `instance` (shape `Instance`) with ALL required members populated from the stored record: `instanceId`, `awsAccountId` (12-digit, from the caller's account), `state` (`InstanceState`, start at `Initializing`), plus `instanceName`, `instanceDescription`, `kmsKeyArn`, `webAppDnsDomain`, `createdTime`, `lastModifiedTime`, `versionNumber`. `clientToken` must be honoured: a repeat call with the same token returns the SAME `instanceId` rather than creating a second instance (the operation is `@idempotent`).
2. **GetInstance** — `GET /api/instance/{instanceId}`. Returns `instance` read from the store; must reflect exactly what CreateInstance/UpdateInstance persisted (name, description, kmsKeyArn, timestamps, versionNumber). Unknown or deleted id => `ResourceNotFoundException` (HTTP 404); an id that does not match the UUID pattern/length 36 => `ValidationException` (HTTP 400). The instance should become `Active` (either immediately on create or on first read) so a client that polls GetInstance for readiness terminates.
3. **ListInstances** — `GET /api/instance`. Returns `instances` containing every instance created in this process and not yet deleted. Must honour `maxResults` (range 0..20, default 10) by truncating the page and emitting an opaque `nextToken` (len 1..1024) when more remain, resuming exactly where the previous page stopped; must honour `instanceNameFilter` (list, 0..10) and `instanceStateFilter` (list<InstanceState>, 0..6) by filtering the stored records.
4. **UpdateInstance** — `PATCH /api/instance/{instanceId}`. Applies `instanceName` and/or `instanceDescription` to the STORED record (absent member = leave unchanged), bumps `lastModifiedTime` (and `versionNumber`), and returns the updated `instance`. A subsequent GetInstance must show the new values. Unknown id => `ResourceNotFoundException` 404.
5. **DeleteInstance** — `DELETE /api/instance/{instanceId}`. Marks the stored instance `Deleting`/removes it and returns the final `instance` payload. After it, GetInstance must return `ResourceNotFoundException` 404 and the id must no longer appear in ListInstances. Unknown id => 404.

NEVER STUBS: each of the five must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). No canned bodies, no echoing the input back, no minting an id that is never stored. Required output members must be real values, not placeholders.

## State Model

In `src/state.rs` (the service's `ComAmazonawsSupplychainGalaxypublicapigatewayState`, held in the one shared engine):

- `instances: BTreeMap<String /* instanceId */, Instance>` where `Instance { instance_id: String, aws_account_id: String, state: InstanceState, error_message: Option<String>, instance_name: Option<String>, instance_description: Option<String>, kms_key_arn: Option<String>, web_app_dns_domain: Option<String>, created_time: SystemTime, last_modified_time: SystemTime, version_number: f64, tags: BTreeMap<String,String> }`. BTreeMap keeps ListInstances ordering stable across pages.
- `idempotency: HashMap<String /* clientToken */, String /* instanceId */>` so a replayed CreateInstance returns the original id.
- Room for the later slices (left stubbed for now, filed separately): `namespaces: BTreeMap<(instanceId, name), DataLakeNamespace>`, `datasets: BTreeMap<(instanceId, namespace, name), DataLakeDataset>`, `flows: BTreeMap<(instanceId, name), DataIntegrationFlow>`, `events: Vec<DataIntegrationEvent>`, `tags_by_arn: BTreeMap<String, BTreeMap<String,String>>`.

Out of scope for this first slice and expected to keep answering 501: the data-lake namespace/dataset CRUD, data-integration flow CRUD + executions, SendDataIntegrationEvent / Get/ListDataIntegrationEvents, bill-of-materials import jobs, and Tag/Untag/ListTagsForResource. Those will be filed as individual gap issues.

Done = a client script can create an instance, poll it to `Active`, list it, rename it, read the rename back, delete it, and observe a 404 — with every response body computed from the store.

## Evidence

OPERATIONS.md (crate root) lists 30 operations for sdkId `SupplyChain`, protocol restJson1, signing name `scn`; CreateInstance `POST /api/instance` -> output `instance` (Instance, required) with errors ConflictException 409 / ValidationException 400 / ServiceQuotaExceededException 402; GetInstance `GET /api/instance/{instanceId}` -> `instance` (required) with ResourceNotFoundException 404; ListInstances `GET /api/instance` is @paginated(inputToken=nextToken, outputToken=nextToken, items=instances) with `maxResults` range 0..20 default 10 and `instanceNameFilter`/`instanceStateFilter`; UpdateInstance `PATCH /api/instance/{instanceId}`; DeleteInstance `DELETE /api/instance/{instanceId}`. The `Instance` shape (smithy_query) requires `instanceId`, `awsAccountId`, `state` and carries createdTime/lastModifiedTime/versionNumber/kmsKeyArn/webAppDnsDomain. Triage: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/` matches all 30 handler files, including create_instance.rs, get_instance.rs, list_instances.rs, update_instance.rs and delete_instance.rs — no operation is implemented and no state is kept. The crate's only test is the generated `tests/state_roundtrip.rs`; no behavioural coverage exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI supplychain client at it (service `scn`, restJson1).
2. CreateInstance with instanceName='demo-instance', instanceDescription='bootstrap demo' and a clientToken — expect 200 with instance.instanceId (UUID), instance.awsAccountId and instance.state; today it answers HTTP 501 NotImplemented.
3. Repeat the same CreateInstance call with the identical clientToken — expect the SAME instanceId, not a second instance.
4. GetInstance with that instanceId — expect the stored name/description/timestamps back, and state eventually Active.
5. ListInstances with maxResults=1 after creating two instances — expect one instance plus a nextToken, and the second instance on the follow-up page; apply instanceNameFilter=['demo-instance'] and expect only that one.
6. UpdateInstance with instanceName='demo-renamed', then GetInstance — expect the new name and a bumped lastModifiedTime.
7. DeleteInstance, then GetInstance with the same id — expect ResourceNotFoundException (HTTP 404) and the id absent from ListInstances.
8. GetInstance with instanceId='not-a-uuid' — expect ValidationException (HTTP 400).
