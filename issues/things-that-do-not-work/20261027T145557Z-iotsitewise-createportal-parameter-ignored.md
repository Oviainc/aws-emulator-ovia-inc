Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# clientToken is still dropped by the creates, updates, deletes and batch associations the idempotency work skipped — their comments now cite siblings that DO honour it

_Filed: 2026-10-27T14:55:57Z_
_Operation: CreatePortal | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has a shared idempotency engine in `crate::state` (`request_fingerprint` / `idempotent_verdict` / `record_idempotent_call`, with the `TokenVerdict::{Replay,Spent,Serve}` verdicts), and about a dozen handlers use it: CreateAsset, CreateAssetModel, UpdateAsset, UpdateAssetModel, UpdateAssetProperty, UpdatePortal, UpdateComputationModel, AssociateAssets, DisassociateAssets, AssociateTimeSeriesToAssetProperty, DisassociateTimeSeriesFromAssetProperty.

The rest of the mutating surface never reads `input.client_token` at all, even though the member is modelled `@idempotencyToken` on every one of them. Each of those handlers carries a comment saying the token is deliberately not honoured "exactly as <sibling> leaves it" — and the siblings they name have since been converted, so the stated justification is now false inside this same crate:

* `create_portal.rs` says it leaves the token unhonoured "exactly as `CreateAssetModel` leaves it", but `create_asset_model.rs:259` calls `store.idempotent_verdict(...)`.
* `update_dataset.rs` says "exactly as `UpdateAsset` leaves it", but `update_asset.rs:201` calls `store.idempotent_verdict(...)`.
* `batch_associate_project_assets.rs` says "as `CreateProject` and `AssociateAssets` leave it", but `associate_assets.rs:219` calls `store.idempotent_verdict(...)`.

The effect is that a retried call (including an SDK-internal retry after a timeout) mints a second resource or applies the mutation twice on exactly the operations whose siblings replay. Operations still ignoring the member (handler never mentions `client_token`): CreatePortal, CreateProject, CreateAccessPolicy, CreateDataset, CreateAssetModelCompositeModel, UpdateProject, UpdateDataset, UpdateDashboard, UpdateAccessPolicy, DeleteAsset, DeleteAssetModel, DeleteAssetModelCompositeModel, DeleteAssetModelInterfaceRelationship, DeleteAccessPolicy, DeletePortal, DeleteProject, DeleteWorkspace, DeleteTimeSeries, PutAssetModelInterfaceRelationship, BatchAssociateProjectAssets, BatchDisassociateProjectAssets, BatchAssociateDataSegmentsToDataset, BatchDeleteDatasetDataSegments, ExecuteQuery.

(For the record: CreateWorkspace, CreateTask, CreatePipeline, CreateDashboard, CreateComputationModel, CreateApplication, CreateEnrichmentJob, CreateDatasetExportJob, StartSearch, StartQuery, StartPipelineExecution and ExecuteAction DO replay, but through a per-record `client_token` field match rather than the shared helper, so there are two mechanisms as well as a set of handlers with none.)

## Evidence

Comment vs. the code it points at, both in this crate:

- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_portal.rs:236-240 — "// `clientToken` is NOT honoured for idempotent replay: this change stores no / token -> created-id mapping, so a retry with the same token mints a second / portal rather than replaying the first answer. Deliberately left to the / idempotency issue rather than half-done here, exactly as `CreateAssetModel` / leaves it; nothing is fabricated for it."
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_asset_model.rs:259 — "match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {"

- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/update_dataset.rs:189-192 — "// `clientToken` is NOT honoured for idempotent replay: this change stores no / token -> answer mapping, so a retry with the same token updates the dataset / a second time rather than replaying the first answer. Left to the / idempotency issue, exactly as `UpdateAsset` leaves it."
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/update_asset.rs:201 — "match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {"

- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_associate_project_assets.rs:163-169 — "// `clientToken` is NOT honoured for idempotent replay: ... Deliberately left to the / idempotency issue rather than half-done here, as `CreateProject` and / `AssociateAssets` leave it."
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/associate_assets.rs:219 — "match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {"

Model: `PutStorageConfiguration`-style aside, every one of the listed operations declares the member, e.g. smithy_query(query="operation", operation="DeleteAssetModel") shows `"clientToken": {"target": "ClientToken", "httpQuery": "clientToken", "idempotencyToken": true, "length": {"min": 36, "max": 64}}`.

Test coverage: tests/all/client_token_idempotency.rs exercises the converted handlers only; no test sends a clientToken to CreatePortal, UpdateDataset, DeleteAsset, DeleteTimeSeries or the Batch*ProjectAssets calls, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreatePortal with portalName='P1', portalContactEmail='a@b.c' and clientToken='c0ffee-aaaa-bbbb-cccc-dddddddddddd-0123456789' (36+ chars).
2. Repeat the identical CreatePortal with the SAME clientToken.
3. Observe two portals with two different portalIds in ListPortals, where the sibling CreateAssetModel the comment at create_portal.rs:239 points at replays the first answer for the same input.
4. Same shape for UpdateDataset (applied twice), DeleteAsset/DeleteTimeSeries (second call 404s instead of replaying the first 202/200) and BatchAssociateProjectAssets.
