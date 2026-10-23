Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|ListAssets|2xx->2xx|$.assets[*]
Issue-Commit: 134aa7970ed0c23d5be465e9c36f71c1d4a91758

# MediaPackage VOD ListAssets returns no assets when filtered by packagingGroupId

> _Note: this issue was identified against the code base at commit `134aa7970ed0c23d5be465e9c36f71c1d4a91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAssets

## Description
ListAssets answers HTTP 200 with an EMPTY assets list when the request carries a packagingGroupId filter that an existing asset belongs to, so a created asset is undiscoverable through the filtered list path.

Cause 1 of 1: packagingGroupId-filtered ListAssets drops every matching asset.

AWS: Smithy (com-amazonaws-mediapackagevod-mediapackagevod) models ListAssets as GET /assets, code 200, with input member PackagingGroupId bound as httpQuery "packagingGroupId" (plus MaxResults @range 1..1000 and NextToken), and output member Assets (jsonName "assets", list of AssetShallow) with NextToken. The operation is @paginated with items=Assets. CreateAsset (POST /assets, code 200) requires Id, PackagingGroupId, SourceArn and SourceRoleArn and returns the created asset's id/arn/packagingGroupId. AWS therefore returns, in assets[], every asset whose packagingGroupId equals the query-string filter — an AssetShallow element carrying id, arn, sourceArn, sourceRoleArn, packagingGroupId, createdAt and tags. ListAssets models no error for "filter matched nothing"; an empty assets list means no asset is in that group. The recorded response for this step shows a 200 whose assets[0] is the asset created earlier in the fixture, in the packaging group named by the filter.

Emulator: the replayed step answers 200 (status matches) but the body's assets list is empty — the replay reports "$.assets[0]: missing (expected an asset object with id/arn/sourceArn/sourceRoleArn/packagingGroupId/createdAt/tags, got None)". The asset was created successfully earlier in the same fixture, so the resource exists in state; only the filtered listing fails to surface it.

Why: a client cannot enumerate the assets of a packaging group. The documented discovery pattern — CreateAsset, then ListAssets?packagingGroupId=<group> to find the asset and read its egressEndpoints/arn — returns nothing, which is indistinguishable from "the group is empty". Callers that poll the filtered list after creating an asset loop forever or conclude creation failed; paginating clients (items=Assets) terminate on the first empty page; infrastructure tools that reconcile desired against actual state see zero assets and will re-create or garbage-collect resources that do exist. Because the status is a success 200, no SDK retry or error handling is triggered — the wrong answer is silently accepted.

Fix: the mediapackage-vod ListAssets handler and the asset lookup it performs over service state — specifically the packagingGroupId filter predicate / group-to-asset index applied when the httpQuery parameter is present, and the AssetShallow projection that fills assets[]. Either the filter never matches stored assets (group id compared against the wrong field or not normalized against the value stored by CreateAsset), or the created asset is not registered in whatever collection the filtered path reads, while the unfiltered path or the create response reads a different store. The handler must return the stored assets whose packagingGroupId equals the filter, each serialized with id, arn, sourceArn, sourceRoleArn, packagingGroupId, createdAt and tags.

Traces: fixture mediapackage-vod-list-assets-filtered-by-packaging-group, step task#9 (verdict mismatch, expected 200 / actual 200, diverging key $.assets[0]). Not covered by the open issues "MediaPackage VOD: egress manifest URL serves nothing, CreatePackagingGroup is not authorization-gated, and DeleteAsset answers the wrong success status" or "Implement mediapackage-vod TagResource" — neither concerns ListAssets or its packagingGroupId filter.

## Evidence
Expected (recorded AWS): HTTP 200, body assets[0] = {id: <asset id>, arn: arn:aws:mediapackage-vod:<region>:<account>:assets/<asset id>, sourceArn: arn:aws:s3:::<bucket>/source/asset.m3u8, sourceRoleArn: <iam role arn>, packagingGroupId: <the group named in the packagingGroupId query parameter>, createdAt: <timestamp, shape-only>, tags: {}}.
Actual (emulator): HTTP 200 with an empty assets list — replay detail: "$.assets[0]: missing (expected {'id': ..., 'arn': ..., 'sourceArn': ..., 'sourceRoleArn': ..., 'packagingGroupId': ..., 'createdAt': '<shaped-not-asserted>', 'tags': {}}, got None)". divergence_key: service=mediapackage-vod|ListAssets|status=200->200|keys=$.assets[0].
The generated asset id, bucket name, role arn, group id, account, region and createdAt differ per environment and are not requirements; the requirement is that an asset belonging to the filtered packaging group appears in assets[] with those members populated.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Replay fixture mediapackage-vod-list-assets-filtered-by-packaging-group; it creates an S3 bucket and IAM role, creates a packaging group, then CreateAsset (POST /assets) with that packagingGroupId, all of which succeed.
3. Observe step task#9: GET /assets?packagingGroupId=<the group just used>.
4. Emulator answers 200 with assets == [] (assets[0] missing); the recorded AWS response has assets[0] describing the asset created in step 2.
