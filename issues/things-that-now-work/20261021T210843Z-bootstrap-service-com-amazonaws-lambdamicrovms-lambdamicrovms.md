Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the lambdamicrovms emulator

_Filed: 2026-10-21T21:08:43Z_
_Operation: service-com-amazonaws-lambdamicrovms-lambdamicrovms | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 24 operations in `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/` is an unmodified scaffold that carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented`, holding zero state (CreateMicrovmAuthToken has no handler at all — see `src/ops/SKIPPED_STUBS.md`). Nothing works end to end.

Implement a FIRST minimal, stateful slice: the image -> microVM lifecycle, in this order (restJson1 wire protocol, sigv4 name `lambda`):

1. **CreateMicrovmImage** — `POST /2025-09-09/microvm-images` -> 201. Input: `name` (required, 1..64, `^[a-zA-Z0-9-_]+$`), `baseImageArn` (required), `buildRoleArn` (required, IAM role ARN pattern), `codeArtifact` (required), plus optional `baseImageVersion`, `description`, `logging`, `egressNetworkConnectors`, `cpuConfigurations`, `resources`, `additionalOsCapabilities`, `hooks`, `environmentVariables`, `tags`, `clientToken`. Must MINT and persist a new image: generate `imageArn` (`arn:aws:lambda:<region>:<account>:microvm-image:<name>`), a first `imageVersion` (e.g. `"1"`), `state=CREATED` (or `CREATING`), `createdAt`. Response must echo the stored record, not the request struct. Reject a duplicate `name` with `ConflictException` (409); honour `clientToken` so a replay returns the same image instead of a second one.
2. **GetMicrovmImage** — `GET /2025-09-09/microvm-images/{imageIdentifier}` -> 200. Resolve `imageIdentifier` as EITHER the image name/ID or the full `imageArn`. Return the persisted `imageArn`, `name`, `state`, `createdAt`, `tags`, `latestActiveImageVersion` (the version created in step 1), `updatedAt`. Unknown identifier -> `ResourceNotFoundException` (404), never 200.
3. **RunMicrovm** — `POST /2025-09-09/microvms` -> 200. Input: `imageIdentifier` (required), optional `imageVersion`, `executionRoleArn`, `idlePolicy`, `logging`, `ingressNetworkConnectors`, `egressNetworkConnectors`, `runHookPayload`, `maximumDurationInSeconds`, `clientToken`. Must look the image up in state (404 `ResourceNotFoundException` if absent), then mint and persist a microVM: `microvmId`, `state=PENDING` (then `RUNNING`), a synthesized `endpoint` URL, the resolved `imageArn` + `imageVersion`, `maximumDurationInSeconds` (default when omitted; the doc gives valid range 1..28800 — reject out-of-range with `ValidationException` 400), `startedAt`. `clientToken` replay must return the SAME `microvmId`.
4. **GetMicrovm** — `GET /2025-09-09/microvms/{microvmIdentifier}` -> 200. Return the stored microVM exactly as created: `microvmId`, `state`, `endpoint`, `imageArn`, `imageVersion`, `executionRoleArn`, `idlePolicy`, `maximumDurationInSeconds`, `startedAt`, and `terminatedAt`/`stateReason` once terminated. Unknown id -> 404 `ResourceNotFoundException`.
5. **ListMicrovms** — `GET /2025-09-09/microvms` -> 200. `items` (required) built from the store, honouring the `imageIdentifier` / `imageVersion` query filters and the `@paginated` contract: `maxResults` caps the page and a `nextToken` is returned (and accepted) when more remain. A microVM created in step 3 must appear; a terminated one must reflect `TERMINATED`.
6. **TerminateMicrovm** — `DELETE /2025-09-09/microvms/{microvmIdentifier}` -> 200, empty body. Must transition the persisted microVM to `TERMINATING`/`TERMINATED` and set `terminatedAt`, so a following GetMicrovm observes the new state and ListMicrovms no longer reports it as RUNNING. `@idempotent`: a second terminate of an already-terminated microVM succeeds; unknown id -> 404.

Every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no random ID that is never stored. A create followed by a read must return what was created.

## State Model

In `src/state.rs` (`ComAmazonawsLambdamicrovmsLambdamicrovmsState`):

- `images: BTreeMap<String /*name*/, MicrovmImage>` where `MicrovmImage { image_arn: String, name: String, state: MicrovmImageState, created_at: DateTime, updated_at: Option<DateTime>, latest_active_image_version: Option<String>, latest_failed_image_version: Option<String>, tags: BTreeMap<String,String>, next_version_seq: u64, versions: BTreeMap<String /*imageVersion*/, MicrovmImageVersion> }`
- `MicrovmImageVersion { image_version: String, state: MicrovmImageVersionState /*SUCCESSFUL*/, status: MicrovmImageVersionStatus /*ACTIVE*/, base_image_arn: String, base_image_version: Option<String>, build_role_arn: String, description: Option<String>, code_artifact: CodeArtifact, logging: Option<Logging>, egress_network_connectors: Vec<String>, cpu_configurations: Vec<CpuConfiguration>, resources: Vec<Resources>, additional_os_capabilities: Vec<Capability>, hooks: Option<Hooks>, environment_variables: BTreeMap<String,String>, created_at: DateTime, updated_at: Option<DateTime>, tags: BTreeMap<String,String> }`
- `arn_index: BTreeMap<String /*imageArn*/, String /*name*/>` so `imageIdentifier` resolves by ARN or by name.
- `microvms: BTreeMap<String /*microvmId*/, Microvm>` where `Microvm { microvm_id: String, state: MicrovmState, endpoint: String, image_arn: String, image_version: String, execution_role_arn: Option<String>, idle_policy: Option<IdlePolicy>, maximum_duration_in_seconds: i32, started_at: DateTime, terminated_at: Option<DateTime>, state_reason: Option<String>, ingress_network_connectors: Vec<String>, egress_network_connectors: Vec<String> }`
- `idempotency: BTreeMap<String /*clientToken*/, String /*minted id: imageArn or microvmId*/>` for the `@idempotencyToken` members on CreateMicrovmImage / RunMicrovm.
- `pagination: BTreeMap<String /*nextToken*/, usize /*offset*/>` (or an opaque encoded offset) shared by the paginated list operations.

The remaining 18 operations (UpdateMicrovmImage, DeleteMicrovmImage, DeleteMicrovmImageVersion, Get/ListMicrovmImageVersion(s), Get/ListMicrovmImageBuild(s), UpdateMicrovmImageVersion, ListMicrovmImages, ListManagedMicrovmImage(Version)s, Suspend/ResumeMicrovm, CreateMicrovmShellAuthToken, CreateMicrovmAuthToken, Tag/Untag/ListTags) stay at 501 for this slice and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/OPERATIONS.md` documents all 24 operations with routes, required input/output members and modeled errors: CreateMicrovmImage (POST /2025-09-09/microvm-images -> 201, required name/baseImageArn/buildRoleArn/codeArtifact, outputs imageArn/state/createdAt/imageVersion), GetMicrovmImage (GET .../microvm-images/{imageIdentifier}, @readonly, 404 ResourceNotFoundException), RunMicrovm (POST /2025-09-09/microvms, required imageIdentifier, outputs microvmId/state/endpoint/imageArn/imageVersion/maximumDurationInSeconds/startedAt), GetMicrovm (GET .../microvms/{microvmIdentifier}), ListMicrovms (GET .../microvms, @paginated(inputToken=nextToken, outputToken=nextToken, items=items) with imageIdentifier/imageVersion filters), TerminateMicrovm (DELETE .../microvms/{microvmIdentifier}, @idempotent). The RunMicrovm doc block in `src/ops/run_microvm.rs` states the documented lifecycle: "The MicroVM starts in PENDING state and transitions to RUNNING once provisioning completes." That same file (lines 4-6, 219) shows the scaffold marker and `Err(crate::scaffolded::Scaffolded::not_implemented("RunMicrovm"))`; every sibling in `src/ops/` is the identical scaffold, and `src/ops/SKIPPED_STUBS.md` records CreateMicrovmAuthToken as modeled-but-unwired. No handler touches `crate::state::service_state`, so no state is persisted anywhere and no test asserts any behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for the `lambda` signing name at it.
2. CreateMicrovmImage with name="demo-image", baseImageArn=<a base image ARN>, buildRoleArn="arn:aws:iam::123456789012:role/build", codeArtifact=<artifact> — expect 201 with a minted imageArn, state and createdAt; today it returns HTTP 501 NotImplemented.
3. GetMicrovmImage with imageIdentifier="demo-image" (and again with the returned imageArn) — expect 200 echoing the persisted image including latestActiveImageVersion.
4. RunMicrovm with imageIdentifier="demo-image" — expect 200 with a minted microvmId, state PENDING/RUNNING, a non-empty endpoint and the resolved imageArn/imageVersion.
5. GetMicrovm with the returned microvmId — expect 200 with exactly the fields stored by RunMicrovm (endpoint, imageArn, maximumDurationInSeconds, startedAt).
6. ListMicrovms with imageIdentifier="demo-image" — expect the new microVM in items; repeat with maxResults=1 against two microVMs and expect a nextToken that pages to the second.
7. TerminateMicrovm with the microvmId — expect 200, then GetMicrovm shows TERMINATING/TERMINATED with terminatedAt set, and a second TerminateMicrovm still succeeds (idempotent).
8. GetMicrovm / GetMicrovmImage with an identifier that was never created — expect 404 ResourceNotFoundException, not 501 and not a hollow 200.
