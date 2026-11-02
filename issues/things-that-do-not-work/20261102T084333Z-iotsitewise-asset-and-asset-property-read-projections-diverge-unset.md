Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.sitewise-unset-asset-model-description-stays-absent iotsitewise.sitewise-list-asset-models-summary-agrees-with-describe iotsitewise.sitewise-an-asset-inherits-its-property-ids-from-its-asset-model iotsitewise.sitewise-measurement-property-gains-a-server-assigned-id-and-a-path iotsitewise.sitewise-asset-property-value-round-trips-through-the-data-plane
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.sitewise-unset-asset-model-description-stays-absent|run#8|AssertionError|assert "assetModelDescription" not in described, (
Issue-Key: iotsitewise.sitewise-list-asset-models-summary-agrees-with-describe|run#11|AssertionError|assert sorted(summary) == ["arn", "assetModelType", "creationDate", "id", "lastUpdateDate",
Issue-Key: iotsitewise.sitewise-an-asset-inherits-its-property-ids-from-its-asset-model|run#22|KeyError|topic = prop["notification"]["topic"]
Issue-Key: iotsitewise.sitewise-measurement-property-gains-a-server-assigned-id-and-a-path|run#12|KeyError|assert [step["name"] for step in prop["path"]] == [name, prop["name"]], (
Issue-Key: iotsitewise.sitewise-asset-property-value-round-trips-through-the-data-plane|run#28|KeyError|assert latest["quality"] == "GOOD", (
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# Asset and asset-property read projections diverge: unset description serialized as an empty string, property notification/path absent, GetAssetPropertyValue omits quality

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssetModel

## Description
Three independent projection defects on the read paths: an unset description is materialised as an empty string instead of being omitted, the per-property derived members (notification, path) are never emitted, and the data-plane read drops the derived quality. This issue bundles 3 causes; all three must be closed.

Cause 1 of 3: an asset model created with no assetModelDescription is reported with an EMPTY-STRING description by both DescribeAssetModel and the ListAssetModels summary, instead of the key being absent.
AWS: Smithy: DescribeAssetModel output assetModelDescription targets Description, which carries @length min 1 max 2048 and pattern ^[^\u0000-\u001F\u007F]+$ - the empty string is not a legal Description value, so it can never be a correct answer. AssetModelSummary.description targets the same Description shape. Recorded behavior for a model created without a description: DescribeAssetModel omits assetModelDescription entirely, and the ListAssetModels summary carries exactly the eight scalar keys arn, assetModelType, creationDate, id, lastUpdateDate, name, status, version - no description key, and (as the same check asserts) neither properties nor hierarchies.
Emulator: DescribeAssetModel answers 200 with assetModelDescription = '' for a model created with no description, and the ListAssetModels summary carries a ninth key, description, with the same empty string.
Why: '' round-trips back into UpdateAssetModel/CreateAssetModel as an illegal Description (@length min 1), so a read-modify-write of an undescribed model is refused by real AWS; and a client cannot distinguish 'no description' from 'the description is the empty string'. The extra summary key also breaks the strict-subset relationship between the summary and the describe projection.
Fix: the DescribeAssetModel and ListAssetModels serializers - omit the description member when the stored description is unset rather than defaulting it to an empty string (the state should hold none, not an empty string).
Traces: iotsitewise.sitewise-unset-asset-model-description-stays-absent; iotsitewise.sitewise-list-asset-models-summary-agrees-with-describe.

Cause 2 of 3: DescribeAsset omits each asset property's notification block and its structured path.
AWS: Smithy: DescribeAsset output assetProperties @required (AssetProperties of AssetProperty); AssetProperty.notification = PropertyNotification{topic @required, state @required: PropertyNotificationState}, AssetProperty.path = AssetPropertyPath, documented 'The structured path to the property from the root of the asset'. For an asset created without any notification configuration AWS reports every property's notification with state DISABLED and the MQTT topic IoT SiteWise would publish to, of the form $aws/sitewise/asset-models/<assetModelId>/assets/<assetId>/properties/<propertyId> (so the topic names the model, names the asset, and ends at the property id), and reports path as the hops from the ASSET root down to the property - the first hop being the asset's own name, not the model's.
Emulator: DescribeAsset answers 200 for an ACTIVE asset but each entry of assetProperties carries neither notification nor path (both checks fail with a raw KeyError: 'notification' / KeyError: 'path').
Why: a client cannot discover the MQTT topic or notification state for a property without a separate UpdateAssetProperty round trip, and cannot render or address a property by its structured path - the only way to disambiguate same-named properties under different composite models.
Fix: the DescribeAsset handler/serializer - derive each property's notification (state plus the topic built from the model/asset/property ids) and compute each property's path rooted at the asset. Related open issue 20261029T190317Z-iotsitewise-asset-and-asset-model-reads-omit-property-path.md records the same missing members from the WIRE corpus (its cause 1), bundled there with the composite-model empty-collection projection; these two behavioral checks are additional grounding for the notification/path half and do not carry that issue's replay keys.
Traces: iotsitewise.sitewise-an-asset-inherits-its-property-ids-from-its-asset-model; iotsitewise.sitewise-measurement-property-gains-a-server-assigned-id-and-a-path.

Cause 3 of 3: GetAssetPropertyValue omits quality for a value written without one, instead of deriving GOOD.
AWS: Smithy: GetAssetPropertyValue output propertyValue (AssetPropertyValue{value @required, timestamp @required, quality: Quality enum [GOOD, BAD, UNCERTAIN]}); BatchPutAssetPropertyValue entries carry AssetPropertyValue too, where quality is likewise optional on input. Recorded behavior: a write that supplied no quality at all reads back with quality = GOOD - the service derives the default on ingest, so the read always reports one of the three enum values.
Emulator: the value and timestamp round-trip correctly but propertyValue has no quality key at all (the check fails with KeyError: 'quality').
Why: quality is how every consumer decides whether to use a reading; an absent quality forces clients to invent a default, and a client that keys on propertyValue['quality'] (as the modeled enum invites) raises instead of reading GOOD.
Fix: the ingest path (BatchPutAssetPropertyValue) should default a missing quality to GOOD when it stores the point, and/or the GetAssetPropertyValue serializer must emit the derived quality; the same derivation applies to the history and aggregate reads of the same stored point.
Traces: iotsitewise.sitewise-asset-property-value-round-trips-through-the-data-plane.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 54/87 passed, all 33 failures in iotsitewise.

1) iotsitewise.sitewise-unset-asset-model-description-stays-absent - CreateAssetModel with no assetModelDescription, then DescribeAssetModel. Expected the key to be absent. AssertionError: an unset description came back as ''; real AWS omits the key entirely.
2) iotsitewise.sitewise-list-asset-models-summary-agrees-with-describe - the summary for that model carries ['arn', 'assetModelType', 'creationDate', 'description', 'id', 'lastUpdateDate', 'name', 'status', 'version']; real AWS reports exactly the eight scalar fields (no 'description') and neither properties nor hierarchies. The shared-field comparisons against DescribeAssetModel that follow were never reached.
3) iotsitewise.sitewise-an-asset-inherits-its-property-ids-from-its-asset-model - after the asset reaches ACTIVE, DescribeAsset's property ids do match the model's, but reading prop['notification']['topic'] raises KeyError: 'notification' (expected a topic containing /asset-models/<assetModelId>/ and /assets/<assetId>/ and ending in /properties/<propertyId>, with state DISABLED).
4) iotsitewise.sitewise-measurement-property-gains-a-server-assigned-id-and-a-path - reading [step['name'] for step in prop['path']] raises KeyError: 'path' (expected [<assetName>, <propertyName>]).
5) iotsitewise.sitewise-asset-property-value-round-trips-through-the-data-plane - BatchPutAssetPropertyValue writes {doubleValue: 42.5} at a timestamp with no quality; GetAssetPropertyValue returns the value and timestamp correctly, then latest['quality'] raises KeyError: 'quality' (expected 'GOOD' - real AWS derives GOOD for a write that supplied no quality at all).

The names/ids/values quoted are these runs' values filling the templates; the requirement is the shape: description omitted when unset, notification{state, derived topic} and asset-rooted path present on every asset property, quality present and derived.

Model grounding: Description @length 1..2048 (so the empty string is not a legal value), targeted by both DescribeAssetModel.assetModelDescription and AssetModelSummary.description; AssetProperty.notification = PropertyNotification{topic @required, state @required} and AssetProperty.path = AssetPropertyPath; AssetPropertyValue.quality = Quality enum [GOOD, BAD, UNCERTAIN].

## Steps to Reproduce
1. POST /asset-models with no assetModelDescription; GET /asset-models/{id} - observe assetModelDescription: ''. GET /asset-models (ListAssetModels) - observe the summary carries a ninth key, description, also ''.
2. Create an asset from a model with two measurement properties, wait for ACTIVE, GET /assets/{assetId} - observe each assetProperties entry has no notification and no path.
3. POST /properties (BatchPutAssetPropertyValue) with one double value and no quality for that asset's property, then GET /properties/latest (GetAssetPropertyValue) - observe propertyValue has value and timestamp but no quality.
4. Re-replay the five checks listed in trace_ids to confirm.
