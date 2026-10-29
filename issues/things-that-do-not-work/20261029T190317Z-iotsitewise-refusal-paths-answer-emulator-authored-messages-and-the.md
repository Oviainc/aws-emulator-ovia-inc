Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-create-asset-model-composite-model-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-create-asset-model-composite-model-lifecycle iotsitewise-create-bulk-import-job-invalid-csv-schema iotsitewise-get-asset-property-aggregates-asset-not-found iotsitewise-get-asset-property-aggregates-invalid-resolution
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|DescribeAssetModelCompositeModel|4xx->4xx|$.message
Issue-Key: iotsitewise|CreateBulkImportJob|4xx->4xx|$.message
Issue-Key: iotsitewise|GetAssetPropertyAggregates|4xx->4xx|$.message
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# Refusal paths answer emulator-authored messages and the wrong validation reason

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssetPropertyAggregates

## Description
Four refusals land on the right HTTP status but carry a message the emulator authored itself — and in one case refuse for an entirely different reason than AWS. Prior message-rule work on this crate (20261024T100803Z-iotsitewise-updateasset-error-response, now fixed) covered the create/update not-found and already-exists templates; none of the four handlers below were covered.

Cause 1 of 4: DescribeAssetModelCompositeModel's not-found message is the emulator's own template.
AWS: Smithy: DescribeAssetModelCompositeModel error ResourceNotFoundException, httpError 404, member message. For a composite-model id the named asset model does not hold, AWS's message is the fixed text "CompositeModel: " + the requested assetModelCompositeModelId + " not found for AssetModel: " + the requested assetModelId. Fixed text: "CompositeModel: ", " not found for AssetModel: "; variable parts: the two ids, both taken verbatim from the request path.
Emulator: answers 404 with "Asset Model Composite Model with ID <compositeModelId> not found" — the emulator's generic "<Resource> with ID <id> not found" template, which neither uses AWS's wording nor names the asset model the lookup was scoped to.
Why: a client or test that matches on the message (and any operator reading the log) cannot tell which asset model the composite model was missing from, and message-equality assertions written against AWS fail.
Fix: the DescribeAssetModelCompositeModel handler's not-found refusal must emit the service's composite-model-scoped message instead of the crate's generic not-found template.
Traces: iotsitewise-create-asset-model-composite-model-lifecycle#task#8.

Cause 2 of 4: CreateBulkImportJob refuses an invalid CSV schema with an unrelated storage-tier precondition.
AWS: Smithy: CreateBulkImportJob error InvalidRequestException, httpError 400, member message. When the job's jobConfiguration/fileFormat CSV column list does not describe a usable row, AWS refuses the create with 400 and the fixed text "Provided CSV file format contains must contain an identifier, data type, timestamp, and value" (AWS's wording verbatim, including its grammatical defect; it carries no variable parts). The CSV-schema check is evaluated on this request, so it is the refusal the caller gets.
Emulator: answers 400 but with "Before you create a bulk import job that ingests data into time series outside of a workspace, you must enable IoT SiteWise warm tier or IoT SiteWise cold tier." — a different precondition entirely, reached because the emulator evaluates the tier gate first (and over storage state it does not even report; see the companion storage-configuration issue) and never evaluates the CSV column list at all.
Why: a caller is told to go enable a storage tier when the actual defect is the CSV column list in its own request, so it fixes the wrong thing and the create keeps failing; and a request with a VALID schema but no tier gets the same message, making the two failures indistinguishable.
Fix: the CreateBulkImportJob handler's validation — add the CSV file-format column validation (identifier, data type, timestamp, value) and order it ahead of the warm/cold-tier precondition, each with its own AWS message.
Traces: iotsitewise-create-bulk-import-job-invalid-csv-schema#task#1.

Cause 3 of 4: GetAssetPropertyAggregates reports an unknown (assetId, propertyId) pair as a missing ASSET, in the emulator's own wording.
AWS: Smithy: GetAssetPropertyAggregates error ResourceNotFoundException, httpError 404, member message; the operation addresses a property by assetId+propertyId query parameters. AWS refuses an unresolvable pair as a PROPERTY lookup failure and names both ids: fixed text "Property not found (Property Id = ", ", Asset Id = ", ")"; variable parts: the propertyId and assetId from the request query string.
Emulator: answers 404 with "Asset with ID <assetId> not found" — the crate's generic asset not-found template, which reports the wrong resource kind and drops the propertyId.
Why: a client cannot distinguish "this asset does not exist" from "this asset has no such property" — the single most common cause of this 404 — and loses the propertyId it needs to report the failure.
Fix: the GetAssetPropertyAggregates handler's property-resolution refusal must raise the property-scoped message naming both ids, rather than delegating to the generic asset not-found template.
Traces: iotsitewise-get-asset-property-aggregates-asset-not-found#task#1.

Cause 4 of 4: the resolution constraint-violation message quotes the regex with its anchors, which AWS's message does not carry.
AWS: Smithy: GetAssetPropertyAggregates input member resolution (Resolution, @required, @length 2..3, @pattern "^1m|15m|1h|1d$"). A value that fails the pattern is refused 400 with AWS's standard constraint-violation text: "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: 1m|15m|1h|1d". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the rejected value and the member name from the request, and the pattern as the SERVICE states it — unanchored, i.e. the Smithy @pattern's leading ^ and trailing $ are not part of the rendered text.
Emulator: answers 400 with the same sentence but renders the pattern as "^1m|15m|1h|1d$", echoing the Smithy trait's anchors verbatim.
Why: the validation message is a documented part of the contract; a client or test asserting on it fails, and the echoed anchors misreport the pattern the service claims to enforce.
Fix: the shared constraint-violation message renderer used by request validation must strip the anchor metacharacters when interpolating a @pattern into the "Member must satisfy regular expression pattern:" text, for this member and every other pattern-constrained member.
Traces: iotsitewise-get-asset-property-aggregates-invalid-resolution#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (score 72.5%, 11/40 steps mismatched). All four steps match on HTTP status and diverge only on $.message:

1) iotsitewise-create-asset-model-composite-model-lifecycle#task#8 DescribeAssetModelCompositeModel — 404 vs 404; expected 'CompositeModel: <compositeModelId> not found for AssetModel: <assetModelId>', got 'Asset Model Composite Model with ID <compositeModelId> not found'.
2) iotsitewise-create-bulk-import-job-invalid-csv-schema#task#1 CreateBulkImportJob — 400 vs 400; expected 'Provided CSV file format contains must contain an identifier, data type, timestamp, and value', got 'Before you create a bulk import job that ingests data into time series outside of a workspace, you must enable IoT SiteWise warm tier or IoT SiteWise cold tier.'
3) iotsitewise-get-asset-property-aggregates-asset-not-found#task#1 GetAssetPropertyAggregates — 404 vs 404; expected 'Property not found (Property Id = <propertyId>, Asset Id = <assetId>)', got 'Asset with ID <assetId> not found'.
4) iotsitewise-get-asset-property-aggregates-invalid-resolution#task#1 GetAssetPropertyAggregates — 400 vs 400; expected "1 validation error detected: Value '2d' at 'resolution' failed to satisfy constraint: Member must satisfy regular expression pattern: 1m|15m|1h|1d", got the same sentence with the pattern rendered as '^1m|15m|1h|1d$'.

The ids and the value '2d' are this recording's values filling the templates; the fixed text and which parts come from the request are stated per cause in the description.

Model grounding: DescribeAssetModelCompositeModel and GetAssetPropertyAggregates both declare ResourceNotFoundException (httpError 404) and InvalidRequestException (httpError 400); CreateBulkImportJob declares InvalidRequestException (httpError 400); GetAssetPropertyAggregates.resolution carries @pattern '^1m|15m|1h|1d$' with @length 2..3.

## Steps to Reproduce
1. Replay fixture iotsitewise-create-asset-model-composite-model-lifecycle, step task#8: GET /asset-models/{assetModelId}/composite-models/{compositeModelId} for a composite model that was just deleted — observe the 404 message uses the emulator's '<Resource> with ID <id> not found' template and never names the asset model.
2. Replay fixture iotsitewise-create-bulk-import-job-invalid-csv-schema, step task#1: POST /jobs (CreateBulkImportJob) whose CSV jobConfiguration column list lacks the identifier/data-type/timestamp/value columns — observe the 400 complains about warm/cold tier instead of the CSV file format.
3. Replay fixture iotsitewise-get-asset-property-aggregates-asset-not-found, step task#1: GET /properties/aggregates with an assetId and propertyId that resolve to nothing — observe the 404 reports a missing asset and omits the propertyId.
4. Replay fixture iotsitewise-get-asset-property-aggregates-invalid-resolution, step task#1: GET /properties/aggregates with resolution=2d — observe the 400 constraint message quotes the pattern with its ^ and $ anchors.
