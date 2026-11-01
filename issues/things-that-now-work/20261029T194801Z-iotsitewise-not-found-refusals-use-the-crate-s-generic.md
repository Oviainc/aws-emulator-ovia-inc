Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.asset-model-composite-model-lifecycle iotsitewise.create-asset-duplicate-external-id-rejected iotsitewise.list-time-series-and-delete-time-series-lifecycle iotsitewise.gateway-lifecycle-create-update-delete
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.asset-model-composite-model-lifecycle|run#19|AssertionError|assert final_message == expected_message, f"expected message {expected_message!r}, got {final_message!r}"
Issue-Key: iotsitewise.create-asset-duplicate-external-id-rejected|run#21|AssertionError|assert freed_message == expected_freed, f"expected message {expected_freed!r}, got {freed_message!r}"
Issue-Key: iotsitewise.list-time-series-and-delete-time-series-lifecycle|run#13|AssertionError|assert final_message == expected_message, f"expected message {expected_message!r}, got {final_message!r}"
Issue-Key: iotsitewise.gateway-lifecycle-create-update-delete|run#38|AssertionError|assert (got_describe["Code"], got_describe["Message"]) == ("ResourceNotFoundException", expected_describe_gone), got_describe
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# Not-found refusals use the crate's generic "<Resource> with ID <id> not found" template instead of the service's own messages (composite model, asset-by-externalId, time series, gateway)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssetModelCompositeModel

## Description
Four resource families answer ResourceNotFoundException on the correct HTTP status (404) but render the message from one shared emulator-authored template, "<Resource> with ID <id> not found". AWS uses a DIFFERENT, resource-specific sentence for each of them, and in three of the four the identifier AWS interpolates is not the one the emulator prints. This issue bundles 4 causes; all four must be closed.

Note on an existing issue: open issue 20261029T190317Z-iotsitewise-refusal-paths-answer-emulator-authored-messages-and-the covers the DescribeAssetModelCompositeModel message from the WIRE corpus (its replay keys are wire fixture steps, not these behavioral checks). Cause 1 below is grounded in a behavioral check it does not carry, and additionally requires the UpdateAssetModelCompositeModel path that issue never names; Causes 2-4 are not covered by it or by any other open issue.

Cause 1 of 4: the composite-model not-found message does not name the asset model, and the same defect exists on the Update path.
AWS: Smithy: DescribeAssetModelCompositeModel and UpdateAssetModelCompositeModel both declare ResourceNotFoundException (httpError 404) with a `message` member, and both address the resource by the path labels assetModelId + assetModelCompositeModelId. For a composite-model id the named asset model does not hold, AWS's message is "CompositeModel: <assetModelCompositeModelId> not found for AssetModel: <assetModelId>" — fixed text "CompositeModel: " and " not found for AssetModel: "; variable parts: the two ids, taken verbatim from the request path.
Emulator: answers 404 with "Asset Model Composite Model with ID <assetModelCompositeModelId> not found", dropping the asset model the lookup was scoped to.
Why: a client cannot tell which asset model the composite model was missing from (the same composite-model id can be absent from many models), and message assertions written against AWS fail on both the read and the mutate path.
Fix: the DescribeAssetModelCompositeModel handler AND the UpdateAssetModelCompositeModel handler must emit the service's composite-model-scoped message naming both ids, instead of delegating to the crate's generic not-found template.
Traces: iotsitewise.asset-model-composite-model-lifecycle (the check asserts the identical message on UpdateAssetModelCompositeModel immediately after the Describe assertion that failed, so fixing only Describe leaves the check failing).

Cause 2 of 4: an asset addressed by externalId is reported with the literal "externalId:" prefix in an ID-shaped message.
AWS: Smithy: DescribeAsset error ResourceNotFoundException (httpError 404, member message); input assetId is a CustomID httpLabel whose @pattern admits either a UUID or the form "externalId:<ExternalId>". When the request addresses the asset by external id and nothing resolves, AWS refuses with "asset with externalId: <externalId> doesn't exist." — fixed text "asset with externalId: " (lower-case 'asset') and " doesn't exist." (with the trailing period); variable part: the external id ALONE, i.e. with the "externalId:" selector stripped off the path value.
Emulator: answers "Asset with ID externalId:<externalId> not found" — the generic ID template, capitalised differently, echoing the selector prefix and calling an external id an "ID".
Why: a client that keys retry/creation logic on the documented external-id text fails to match, and the message misreports the kind of identifier that failed to resolve.
Fix: the DescribeAsset handler's not-found refusal must branch on how assetId was addressed (UUID vs. externalId: selector) and render the external-id message with the bare external id.
Traces: iotsitewise.create-asset-duplicate-external-id-rejected.

Cause 3 of 4: a time series addressed by alias is reported as a missing "Time Series with ID".
AWS: Smithy: DescribeTimeSeries and DeleteTimeSeries both declare ResourceNotFoundException (httpError 404, member message) and both accept alias as an httpQuery parameter. For an alias no time series holds, AWS refuses 404 with "dataAlias with accountId AwsAccountId{value=<accountId>}, alias PropertyAlias(super=PropertyAlias{value=<alias>}) not found" — fixed text "dataAlias with accountId AwsAccountId{value=", "}, alias PropertyAlias(super=PropertyAlias{value=", "}) not found"; variable parts: the caller's 12-digit account id and the alias from the request query string.
Emulator: answers "Time Series with ID <alias> not found" — the generic template, which neither names the account scope nor reports the lookup as an alias lookup.
Why: the alias and the (assetId, propertyId) pair are two different ways to address a time series and AWS's message distinguishes them; the emulator's does not, so a client cannot tell which identifier failed, and equality assertions fail. The same message is required from DeleteTimeSeries, which the check asserts on the re-delete of an already-deleted alias.
Fix: the shared time-series resolution used by the DescribeTimeSeries and DeleteTimeSeries handlers must raise the alias-scoped not-found message (account id + alias) rather than the crate's generic not-found template.
Traces: iotsitewise.list-time-series-and-delete-time-series-lifecycle.

Cause 4 of 4: gateway not-found messages are the generic template, and AWS words Describe and Update differently.
AWS: Smithy: DescribeGateway and UpdateGateway both declare ResourceNotFoundException (httpError 404, member message) and address the gateway by the gatewayId httpLabel. DescribeGateway's message is "No gateway with ID <gatewayId> was found. Please verify that you supplied the correct gateway ID and try again." and UpdateGateway's is "A gateway with ID <gatewayId> could not be found. Please verify you supplied the correct gateway ID and try again." — two distinct fixed texts (note "verify that you supplied" vs "verify you supplied"), each with the gatewayId from the request path as its only variable part.
Emulator: both answer the generic "Gateway with ID <gatewayId> not found".
Why: clients and tests asserting the documented guidance text fail, and the emulator collapses two separately worded refusals into one, hiding which operation refused.
Fix: the DescribeGateway and UpdateGateway handlers' not-found refusals must each render their own service message instead of the shared not-found template.
Traces: iotsitewise.gateway-lifecycle-create-update-delete (fails on the DescribeGateway message; the check asserts UpdateGateway's distinct message immediately after, so both wordings are needed) and iotsitewise.gateway-duplicate-core-device-rejected (asserts the same DescribeGateway text after DeleteGateway).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the emulator built from mainline: 46/60 passed, all 14 failures in iotsitewise. All four failures below match on HTTP status (404) and error code (ResourceNotFoundException) and diverge only on the message. The ids/aliases quoted are that run's values filling the templates; the fixed text and variable parts are stated per cause above.

1) iotsitewise.asset-model-composite-model-lifecycle — DescribeAssetModelCompositeModel after DeleteAssetModelCompositeModel: expected 'CompositeModel: <compositeModelId> not found for AssetModel: <assetModelId>', got 'Asset Model Composite Model with ID <compositeModelId> not found'.

2) iotsitewise.create-asset-duplicate-external-id-rejected — DescribeAsset(assetId="externalId:<externalId>") after the holding asset was deleted: expected "asset with externalId: <externalId> doesn't exist.", got 'Asset with ID externalId:<externalId> not found'.

3) iotsitewise.list-time-series-and-delete-time-series-lifecycle — DescribeTimeSeries(alias=...) after DeleteTimeSeries: expected 'dataAlias with accountId AwsAccountId{value=<accountId>}, alias PropertyAlias(super=PropertyAlias{value=<alias>}) not found', got 'Time Series with ID <alias> not found'.

4) iotsitewise.gateway-lifecycle-create-update-delete — DescribeGateway after DeleteGateway: expected 'No gateway with ID <gatewayId> was found. Please verify that you supplied the correct gateway ID and try again.', got 'Gateway with ID <gatewayId> not found'.

Model grounding: DescribeAssetModelCompositeModel / UpdateAssetModelCompositeModel, DescribeAsset, DescribeTimeSeries / DeleteTimeSeries and DescribeGateway / UpdateGateway all declare ResourceNotFoundException with httpError 404 and a `message` member; DescribeAsset.assetId and DeleteTimeSeries/DescribeTimeSeries alias+assetId+propertyId are the addressing members named above.

## Steps to Reproduce
1. Re-replay iotsitewise.asset-model-composite-model-lifecycle: CreateAssetModelCompositeModel, wait ACTIVE, DeleteAssetModelCompositeModel, then GET /asset-models/{assetModelId}/composite-models/{compositeModelId} and PUT the same path — observe both 404 messages use the generic template and never name the asset model.
2. Re-replay iotsitewise.create-asset-duplicate-external-id-rejected: CreateAsset with assetExternalId, DeleteAsset, then GET /assets/externalId:<externalId> — observe the 404 message echoes the 'externalId:' selector inside an 'with ID' sentence.
3. Re-replay iotsitewise.list-time-series-and-delete-time-series-lifecycle: ingest by propertyAlias only, DeleteTimeSeries(alias=...), then DescribeTimeSeries(alias=...) and DeleteTimeSeries(alias=...) again — observe both 404 messages are 'Time Series with ID <alias> not found'.
4. Re-replay iotsitewise.gateway-lifecycle-create-update-delete: CreateGateway, DeleteGateway, then DescribeGateway and UpdateGateway on the deleted id — observe both 404s carry the generic gateway template instead of the two distinct service sentences.
