Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-agent-space-asset-lifecycle
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-agent-space-asset-lifecycle
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|CreateAsset|2xx->2xx|$.asset.assetId
Issue-Commit: 34dbf414468f61ae46fbffffd5c442600be3c02e

# devops-agent CreateAsset mints asset.assetId from a store-wide hex counter, diverging from the service's asset-identifier shape

> _Note: this issue was identified against the code base at commit `34dbf414468f61ae46fbffffd5c442600be3c02e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAsset

## Description
Cause 1 of 1: the identifier CreateAsset returns in $.asset.assetId does not have the shape the DevOps Agent asset plane mints, because the emulator derives it from one store-wide monotonic sequence shared by every resource type in the service.

AWS: recorded behavior only. The Smithy model types Asset$assetId as ResourceId (@length min 1 max 128, @pattern ^[a-zA-Z0-9_.-]+$, documented "Generic resource identifier (allows alphanumeric characters, hyphens, and underscores; 1-128 characters)") and documents the member only as "The unique identifier for this asset" — it states no concrete identifier format, so the requirement here is the recorded one. The recording's assetId is a per-asset opaque identifier whose literal value is environment-specific, so the replay harness does not assert it literally ('<shaped-not-asserted>'); it asserts the identifier's SHAPE, and that shape is the requirement. Status for the step is 201 (CreateAsset's modeled success status) in both the recording and the emulator, and no other member of the response diverged.

Emulator: the handler mints the id with `store.mint_id(ASSET_ID_PREFIX)`, i.e. `format!("{prefix}-{:016x}", self.next_sequence())` over the single `next_id` field of the service's state — the SAME sequence that mints agentSpaceIds and trigger ids. So the id is the literal prefix "asset-" followed by exactly 16 lowercase hex digits of a global creation counter, and the first asset created in a fresh store comes back as asset-0000000000000002 rather than ...0001, because the agent space created earlier in the same fixture consumed sequence 1. Replay reports `$.asset.assetId: value_mismatch (expected '<shaped-not-asserted>', got 'asset-0000000000000002')` on the 201 response.

Here "asset-" plus a fixed-width hex counter is the emulator's own fixed text and the digits are the store's cross-resource sequence number; neither is a value the request or AWS's identifier form supplies. Only the shape is at issue — the per-run digits of any single recording are not a requirement and are not what is being compared.

Why: a client that persists, validates or pattern-matches asset identifiers (an SDK consumer asserting the service's id form, a test fixture, or anything that round-trips an id through its own storage with a width or charset expectation) accepts ids from the emulator that the real service would never issue, and rejects none that it would. The shared counter also makes an asset's id a function of how many unrelated agent spaces, tasks and triggers happened to be created first, so the identifier is neither independent of other resources nor unpredictable: creation order and resource counts of the whole store are inferable from any one asset id, and a client that assumes ids of one resource type form their own sequence sees gaps.

Fix: the asset-identifier minting the CreateAsset handler uses — `crate::state::ComAmazonawsDevopsagentDevopsagentState::mint_id` / `next_sequence` in the service's state, as called from the CreateAsset handler — must produce an identifier of the shape the asset plane returns, instead of a prefix plus a 16-hex-digit global counter shared with every other resource type. Note the existing open devops-agent issues all concern refusal wording (GetAgentSpace, GetBacklogTask, GetTrigger, GetOperatorApp, CreateAssetFile) and the unimplemented asset-file stubs; none of them covers the shape of a minted assetId on the CreateAsset success path.

Traces: devops-agent-agent-space-asset-lifecycle#task#2 (CreateAsset, 201 -> 201, keys=$.asset.assetId).

## Evidence
Replay cluster divergence_key=service=devops-agent|CreateAsset|status=201->201|keys=$.asset.assetId, verdict=mismatch.
Expected: 201 with $.asset.assetId holding an asset identifier of the recorded shape (asserted by shape, printed as '<shaped-not-asserted>' because the literal value is environment-specific).
Actual: 201 with $.asset.assetId = 'asset-0000000000000002' — the fixed text "asset-" plus 16 lowercase hex digits of the service state's single `next_id` counter (state.rs: `format!("{prefix}-{:016x}", self.next_sequence())`, reached from create_asset.rs via `store.mint_id(ASSET_ID_PREFIX)`); the counter is shared with agentSpaceId/trigger minting, which is why the first asset of the fixture is numbered 2.
Model: Asset$assetId -> ResourceId, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$, doc "The unique identifier for this asset" — no identifier format stated, so the recorded shape is the requirement.
Affected steps in cluster: devops-agent-agent-space-asset-lifecycle#task#2 (1).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture devops-agent-agent-space-asset-lifecycle.
2. At step task#2 the fixture issues POST /asset/agent-space/{agentSpaceId}/assets (CreateAsset) against the agent space created earlier in the same fixture.
3. The response is 201, but $.asset.assetId comes back as 'asset-' + 16 hex digits of the store-wide counter (e.g. asset-0000000000000002 for the first asset in a fresh store), which fails the recorded identifier-shape assertion for that key.
4. Equivalently, against a fresh emulator: CreateAgentSpace, then CreateAsset in that space, and observe that the returned assetId's numeric suffix continues the agent space's sequence instead of being an identifier of the asset plane's own form.
