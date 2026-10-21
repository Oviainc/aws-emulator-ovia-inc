Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the com-amazonaws-bedrock-amazonbedrockcontrolplaneservice emulator

_Filed: 2026-10-21T21:18:59Z_
_Operation: service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate is a fresh scaffold: all 108 operations under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return HTTP 501 `NotImplemented` while holding zero state. Implement a FIRST, SMALL, stateful slice: the guardrail lifecycle, which is the Bedrock control plane's primary self-contained resource (every other resource — custom models, provisioned throughput, evaluation jobs — depends on model/job machinery, while guardrails are pure control-plane CRUD).

Happy path to implement, in order (wire protocol: `restJson1`, sigv4 signing name `bedrock`):

1. **CreateGuardrail** — `POST /guardrails` -> 202. Input: `name` (required, 1..50, `^[0-9a-zA-Z-_]+$`), `blockedInputMessaging` (required, 1..500), `blockedOutputsMessaging` (required, 1..500), `description`, the optional policy configs (`topicPolicyConfig`, `contentPolicyConfig`, `wordPolicyConfig`, `sensitiveInformationPolicyConfig`, `contextualGroundingPolicyConfig`), `kmsKeyId`, `tags`, `clientRequestToken` (@idempotencyToken). Must mint and PERSIST a new guardrail: generate `guardrailId` matching `^[a-z0-9]+$`, build `guardrailArn` as `arn:aws:bedrock:<region>:<account>:guardrail/<guardrailId>`, set `version` = `DRAFT`, `status` = `READY`, `createdAt`/`updatedAt` = now. Return `guardrailId`, `guardrailArn`, `version`, `createdAt` from the stored record — not echoed input.
2. **GetGuardrail** — `GET /guardrails/{guardrailIdentifier}` -> 200, optional `?guardrailVersion=`. `guardrailIdentifier` accepts BOTH the bare id and the full ARN (per its pattern). Must look the record up in state and return the stored `name`, `description`, `guardrailId`, `guardrailArn`, `version`, `status`, the stored policies (converted from the `*PolicyConfig` input shapes to their `*Policy` output shapes), `blockedInputMessaging`, `blockedOutputsMessaging`, `createdAt`, `updatedAt`, `kmsKeyArn`. Unknown identifier (or unknown version) -> `ResourceNotFoundException` (HTTP 404), never 501 or an empty 200.
3. **ListGuardrails** — `GET /guardrails` -> 200, @paginated(inputToken=nextToken, outputToken=nextToken, items=guardrails). Must enumerate the stored guardrails as `GuardrailSummary{id, arn, status, name, description, version, createdAt, updatedAt}` (all required except `description`). Honor `maxResults` (1..1000) by truncating and emitting a `nextToken` that resumes exactly where the page ended; honor the `guardrailIdentifier` query filter (return only that guardrail and its versions). A created guardrail MUST appear here; a deleted one MUST NOT.
4. **UpdateGuardrail** — `PUT /guardrails/{guardrailIdentifier}` -> 202, @idempotent. Required `name`, `blockedInputMessaging`, `blockedOutputsMessaging` plus the optional policy configs. Must mutate the stored DRAFT in place (replacing the policy set, not merging blindly) and bump `updatedAt`; return `guardrailId`, `guardrailArn`, `version` (`DRAFT`), `updatedAt`. A following GetGuardrail must observe the new values. Unknown identifier -> `ResourceNotFoundException` (404).
5. **CreateGuardrailVersion** — `POST /guardrails/{guardrailIdentifier}` -> 202. Must snapshot the current DRAFT into a new immutable numbered version (`^[1-9][0-9]{0,7}$`, starting at `1` and incrementing per guardrail) with the optional `description`, and return `guardrailId` + `version`. `GetGuardrail?guardrailVersion=1` must then return the snapshot as it was at version time, unaffected by later DRAFT updates.
6. **DeleteGuardrail** — `DELETE /guardrails/{guardrailIdentifier}` -> 202, @idempotent, optional `?guardrailVersion=` (numbered versions only). With no version, remove the guardrail and all its versions; with a version, remove only that version. Afterwards GetGuardrail -> `ResourceNotFoundException` (404) and ListGuardrails omits it. Deleting an already-absent guardrail -> `ResourceNotFoundException` (404).

NEVER STUBS: every one of these six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). Hardcoded ids, echoed inputs, random ids that are never stored, and empty 200s all count as failures. Required-field validation (`name` pattern/length, messaging length 1..500, `maxResults` range) must produce `ValidationException` (HTTP 400) rather than being accepted silently.

## State Model

In `ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState` (src/state.rs):

- `guardrails: HashMap<String /* guardrailId */, Guardrail>`
- `guardrail_names: HashMap<String /* name */, String /* guardrailId */>` — to reject a duplicate name with `ConflictException` and to resolve identifiers
- `struct Guardrail { id: String, arn: String, name: String, description: Option<String>, kms_key_arn: Option<String>, tags: Vec<Tag>, created_at: SystemTime, draft: GuardrailRevision, versions: BTreeMap<u32 /* version number */, GuardrailRevision>, next_version: u32 }`
- `struct GuardrailRevision { version: String /* "DRAFT" or "1","2",... */, status: GuardrailStatus /* READY */, description: Option<String>, blocked_input_messaging: String, blocked_outputs_messaging: String, topic_policy: Option<GuardrailTopicPolicy>, content_policy: Option<GuardrailContentPolicy>, word_policy: Option<GuardrailWordPolicy>, sensitive_information_policy: Option<GuardrailSensitiveInformationPolicy>, contextual_grounding_policy: Option<GuardrailContextualGroundingPolicy>, cross_region_details: Option<GuardrailCrossRegionDetails>, created_at: SystemTime, updated_at: SystemTime }`
- `idempotency: HashMap<String /* clientRequestToken */, String /* guardrailId */>` — so a replayed CreateGuardrail returns the first result instead of creating a second guardrail

Identifier resolution helper: given `guardrailIdentifier`, strip the `...:guardrail/` ARN prefix if present, then index `guardrails`.

All other 102 operations (custom models, model customization/import/copy/invocation jobs, evaluation jobs, inference profiles, provisioned throughput, automated reasoning policies, marketplace endpoints, logging configuration, tagging, foundation-model listing) stay 501 for now and will be filed as individual gap issues later. Keep this first slice small.

## Evidence

OPERATIONS.md (crate root) lines 469-513 define the guardrail operations used above, including routes (`POST /guardrails` -> 202, `GET /guardrails/{guardrailIdentifier}` -> 200, `GET /guardrails` -> 200 with @paginated(items=guardrails), `PUT /guardrails/{guardrailIdentifier}` -> 202, `POST /guardrails/{guardrailIdentifier}` -> 202, `DELETE /guardrails/{guardrailIdentifier}` -> 202), required members, output members and the `ResourceNotFoundException (HTTP 404)` / `ValidationException (HTTP 400)` / `ConflictException (HTTP 400)` error sets. OPERATIONS.md header declares protocol `restJson1`, sigv4 name `bedrock`, 108 operations. `smithy_query shape=GuardrailSummary` gives the ListGuardrails item fields: required `id`, `arn`, `status`, `name`, `version`, `createdAt`, `updatedAt` plus optional `description`/`crossRegionDetails`. Every file under `src/ops/` (including create_guardrail.rs, get_guardrail.rs, list_guardrails.rs, update_guardrail.rs, create_guardrail_version.rs, delete_guardrail.rs) is an unmodified scaffold returning `Scaffolded::not_implemented(..)` -> HTTP 501, per README.md lines 36-53. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Point an AWS SDK/CLI `bedrock` client at the emulator endpoint.
2. `CreateGuardrail` with name=`my-guard`, blockedInputMessaging=`blocked in`, blockedOutputsMessaging=`blocked out`, a simple `wordPolicyConfig` with one word, and a clientRequestToken — expect 202 with a non-empty `guardrailId`, an ARN ending `guardrail/<guardrailId>`, `version`=`DRAFT`, and a `createdAt` timestamp (today: HTTP 501).
3. Replay the same `CreateGuardrail` with the same clientRequestToken — expect the SAME guardrailId, not a second guardrail.
4. `GetGuardrail` with the returned guardrailId — expect 200 with name=`my-guard`, status=`READY`, the stored word policy, and both messaging strings exactly as created. Repeat passing the full ARN as the identifier — same result.
5. `ListGuardrails` — expect the new guardrail present as a GuardrailSummary with matching id/arn/name/version/status/createdAt/updatedAt.
6. `CreateGuardrailVersion` on that guardrail — expect version `1`; then `GetGuardrail?guardrailVersion=1` returns the snapshot.
7. `UpdateGuardrail` with name=`my-guard`, a different `description` and a changed word policy — expect 202; a following `GetGuardrail` (DRAFT) shows the new values with a later `updatedAt`, while `GetGuardrail?guardrailVersion=1` still shows the original.
8. `ListGuardrails` with `maxResults=1` across several created guardrails — expect exactly 1 item plus a `nextToken` that pages through the rest without duplicates or omissions.
9. `DeleteGuardrail` on the guardrail — expect 202; a following `GetGuardrail` returns `ResourceNotFoundException` (HTTP 404) and `ListGuardrails` no longer lists it.
10. `GetGuardrail` with an identifier that was never created — expect `ResourceNotFoundException` (HTTP 404), not 501 or an empty 200.
