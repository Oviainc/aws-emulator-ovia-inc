Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-agent-space-asset-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-agent-space-asset-lifecycle devops-agent-get-asset-not-found-vs-access-denied
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|CreateAsset|2xx->2xx|$.asset.assetId
Issue-Key: devops-agent|CreateAsset|2xx->2xx|$.asset.metadata.status
Issue-Key: devops-agent|GetAsset|2xx->2xx|$.asset.metadata.status
Issue-Key: devops-agent|GetAssetFile|2xx->2xx|$.file.metadata
Issue-Key: devops-agent|CreateAssetFile|4xx->4xx|$.message
Issue-Key: devops-agent|GetAsset|4xx->2xx|$.message
Issue-Key: devops-agent|GetAsset|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: devops-agent|GetAsset|4xx->2xx|status
Issue-Commit: 2510d9c8f60c25028e97eba526c5004c4fdaba96

# devops-agent asset APIs drop the metadata document, keep deleted assets readable, and author their own one-file refusal text

> _Note: this issue was identified against the code base at commit `2510d9c8f60c25028e97eba526c5004c4fdaba96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAsset

## Description
Replaying the devops-agent asset-lifecycle wire fixture exposes FIVE distinct causes in the asset surface, all in the same service. They share one theme: the asset/asset-file representation the emulator keeps and serves is incomplete and not lifecycle-correct, and its refusal text is emulator-authored. All five must be closed before this issue is submitted.

Cause 1 of 5: CreateAsset does not return (and therefore does not persist) the asset's metadata document.
AWS: Smithy (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json) models CreateAsset POST /asset/agent-space/{agentSpaceId}/assets with HTTP 201 and output member `asset` -> shape Asset. On Asset, member `metadata` (target Document) is REQUIRED, alongside required assetId/assetType/version/createdAt/updatedAt. CreateAsset's input carries the same `metadata` Document, so the created asset's metadata is the caller's document, echoed back in the response. The recording shows a CreateAsset whose response asset.metadata contains the submitted document (here status=ACTIVE).
Emulator: answers 201 (status correct) but `$.asset.metadata.status` is MISSING — the metadata document submitted on the request is not carried into the response at all. In the same cluster `$.asset.assetId` also diverges from the recorded shape: the emulator mints sequential ids of the form asset-0000000000000002. Hypothesis: AWS mints an opaque/high-entropy asset id rather than a zero-padded counter; a generated id value is per-environment and is NOT itself a requirement, so the filed defect here is the dropped metadata, and the id is noted only because it travels in the same cluster.
Why: metadata is a required output member, so an SDK client deserializing CreateAssetResponse gets an asset with no metadata and any caller that round-trips its own labels (title, description, status, owner tags) through the asset immediately loses them. Clients that key behavior off asset.metadata.<field> see None where AWS always returns their document.
Fix: CreateAsset's handler and the asset state it writes — the input metadata Document must be stored on the asset record and serialized into the required Asset.metadata member.
Traces: devops-agent-agent-space-asset-lifecycle#task#2.

Cause 2 of 5: GetAsset serves an asset with no metadata document.
AWS: Smithy models GetAsset GET /asset/agent-space/{agentSpaceId}/assets/{assetId} -> 200 with required output `asset` -> Asset, whose `metadata` member is required. A read of a created asset returns the asset's metadata document unchanged.
Emulator: answers 200 with the asset but `$.asset.metadata.status` is MISSING — the same dropped document observed on read. This cluster also covers a second fixture (devops-agent-get-asset-not-found-vs-access-denied#task#3), so the gap is not specific to one lifecycle.
Why: the read path is where clients re-fetch state they wrote; a required member coming back absent breaks round-trip reads and any consumer that treats asset.metadata as authoritative.
Fix: GetAsset's handler/serializer plus the asset state record — once metadata is persisted by CreateAsset it must be emitted on every Asset projection (GetAsset, and any list/update path that returns Asset).
Traces: devops-agent-agent-space-asset-lifecycle#task#3 (also devops-agent-get-asset-not-found-vs-access-denied#task#3).

Cause 3 of 5: GetAssetFile omits the file's metadata document.
AWS: Smithy models GetAssetFile GET /asset/agent-space/{agentSpaceId}/assets/{assetId}/files/{path+} -> 200 with required output `file` -> AssetFile, which has members path/content/version/createdAt/updatedAt plus `metadata` (target Document). CreateAssetFile accepts a `metadata` Document for the file, so a file created with metadata reads back with that document. The recording shows file.metadata = {mimeType: text/markdown}.
Emulator: answers 200 with the file but `$.file.metadata` is MISSING entirely (None) — the per-file metadata document submitted at create time is not retained or not serialized.
Why: file metadata is how callers carry content type and provenance alongside asset bytes; a client that reads mimeType off file.metadata to decide how to render or parse the content gets nothing and must guess.
Fix: the asset-file state record plus CreateAssetFile/UpdateAssetFile ingestion and the GetAssetFile serializer — store the file metadata Document and emit the AssetFile.metadata member.
Traces: devops-agent-agent-space-asset-lifecycle#task#5.

Cause 4 of 5: CreateAssetFile's one-file refusal carries an emulator-authored ValidationException message.
AWS: Smithy models CreateAssetFile with error ValidationException (errorKind client, HTTP 400, "The input fails to satisfy the constraints specified by the service."); the model states no per-assetType file-count rule, so the message is recorded behavior only. The recording refuses the second file of a single-file asset type with HTTP 400 and the message "Assets of type 'artifact' support only one file. Use UpdateAssetFile to replace the existing file." Everything in that sentence is FIXED text except the quoted asset type, which is the asset's assetType taken from state ('artifact' in this recording); the message names no assetId and no file path.
Emulator: answers HTTP 400 (status correct) with the message 'Asset asset-0000000000000002 contains a single file at data.json, so another file cannot be created in it' — a different sentence that leaks the asset id and the existing path, never names the asset type, and omits AWS's "Use UpdateAssetFile to replace the existing file." remediation. The id and path in that string are per-run values filling the emulator's own template, not part of any requirement.
Why: callers that surface, log or match the refusal text — and any that key remediation off AWS's explicit UpdateAssetFile instruction — see text that never occurs against AWS.
Fix: the CreateAssetFile refusal arm and its shared message builder; the message must be parameterised on the asset's assetType, not on the asset id and existing path. Status 400 and code ValidationException are already right and must be preserved. Note: open issue 20261031T084417Z-devops-agent-createassetfile-one-file-refusal-carries-an.md describes the same wording from the BEHAVIORAL corpus, but it carries no wire replay key, so this wire step is not covered by it; fixing one should close both.
Traces: devops-agent-agent-space-asset-lifecycle#task#7.

Cause 5 of 5: after DeleteAsset, GetAsset still serves the deleted asset instead of refusing it as not found.
AWS: Smithy models DeleteAsset DELETE /asset/agent-space/{agentSpaceId}/assets/{assetId} -> 200 (idempotent) and GetAsset with error ResourceNotFoundException (errorKind client, HTTP 404, required member `message`, "The requested resource could not be found."). After a successful delete the asset no longer exists, so GetAsset answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and the message "Asset not found: <assetId>" — the fixed text is "Asset not found: " and the only variable part is the requested assetId from the request path (asset-0000000000000002 in this recording).
Emulator: answers HTTP 200 and returns the asset body; `$.message` and the `x-amzn-errortype` header are absent because no error is raised at all — the delete did not remove the asset from state (or GetAsset ignores the deleted marker).
Why: this is a state-correctness break, not just wording. A client cannot tell a deleted asset from a live one: deletion appears to succeed and then silently does not take effect, so cleanup loops never terminate, re-create flows see phantom conflicts, and anything that treats 404 as "gone" reads stale content indefinitely.
Fix: DeleteAsset's handler and the asset state store (the asset and its files must actually be removed/tombstoned), plus GetAsset's lookup arm so a missing asset raises ResourceNotFoundException with HTTP 404 and AWS's "Asset not found: <assetId>" message.
Traces: devops-agent-agent-space-asset-lifecycle#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 40 steps, 22 passed, 18 mismatched, score 55%). Five net-new mismatch clusters, all service=devops-agent, none carrying stub_path or owned_by:

Cause 1 — fixture devops-agent-agent-space-asset-lifecycle, step task#2, CreateAsset, expected 201 / actual 201:
  $.asset.assetId: value_mismatch (expected '<shaped-not-asserted>', got 'asset-0000000000000002')
  $.asset.metadata.status: missing (expected 'ACTIVE', got None)

Cause 2 — fixture devops-agent-agent-space-asset-lifecycle, step task#3, GetAsset, expected 200 / actual 200 (affected_count 2; also devops-agent-get-asset-not-found-vs-access-denied#task#3):
  $.asset.metadata.status: missing (expected 'ACTIVE', got None)

Cause 3 — fixture devops-agent-agent-space-asset-lifecycle, step task#5, GetAssetFile, expected 200 / actual 200:
  $.file.metadata: missing (expected {'mimeType': 'text/markdown'}, got None)

Cause 4 — fixture devops-agent-agent-space-asset-lifecycle, step task#7, CreateAssetFile, expected 400 / actual 400:
  $.message: value_mismatch
    expected: "Assets of type 'artifact' support only one file. Use UpdateAssetFile to replace the existing file."
    actual:   'Asset asset-0000000000000002 contains a single file at data.json, so another file cannot be created in it'

Cause 5 — fixture devops-agent-agent-space-asset-lifecycle, step task#9, GetAsset after DeleteAsset, expected 404 / actual 200:
  status: expected 404, got 200
  $.message: missing (expected 'Asset not found: asset-0000000000000002', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)

Model grounding (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json): Asset.metadata is a REQUIRED Document member (as are assetId/assetType/version/createdAt/updatedAt); AssetFile carries a metadata Document; CreateAsset and CreateAssetFile both accept a metadata Document on input; GetAsset models ResourceNotFoundException (client, HTTP 404, required message); CreateAssetFile models ValidationException (client, HTTP 400).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout (health endpoint /__admin/health answers 200).
2. Replay wire fixture devops-agent-agent-space-asset-lifecycle from trace set simparity-wire-aws-v1 and compare each step against the recording.
3. Cause 1 — step task#2: CreateAsset(agentSpaceId=<space>, assetType=..., metadata={'status':'ACTIVE'}, content={...}). Observe HTTP 201 but the response asset has no metadata.status (metadata document dropped).
4. Cause 2 — step task#3: GetAsset(agentSpaceId=<space>, assetId=<asset>). Observe HTTP 200 with asset.metadata.status absent; the same gap reproduces in fixture devops-agent-get-asset-not-found-vs-access-denied step task#3.
5. Cause 3 — step task#5: GetAssetFile(agentSpaceId=<space>, assetId=<asset>, path=<file created with metadata {'mimeType':'text/markdown'}>). Observe HTTP 200 with file.metadata absent.
6. Cause 4 — step task#7: CreateAssetFile a SECOND file into an asset whose assetType allows exactly one file. Observe HTTP 400 ValidationException with 'Asset <assetId> contains a single file at <path>, so another file cannot be created in it' instead of "Assets of type '<assetType>' support only one file. Use UpdateAssetFile to replace the existing file."
7. Cause 5 — step task#9: DeleteAsset(agentSpaceId=<space>, assetId=<asset>), then GetAsset for the same assetId. Observe HTTP 200 with the asset body instead of HTTP 404 / x-amzn-errortype ResourceNotFoundException / message 'Asset not found: <assetId>'.
8. Re-replay devops-agent-agent-space-asset-lifecycle steps task#2, task#3, task#5, task#7 and task#9 (plus devops-agent-get-asset-not-found-vs-access-denied#task#3) to confirm all five causes are closed.
