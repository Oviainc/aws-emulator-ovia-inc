Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__location.deploy-location-placeindex-round-trip-and-delete cloudformation__location.deploy-location-routecalculator-round-trip-and-delete cloudformation__location.deploy-location-geofencecollection-nonexistent-kms-key-rejected-and-rolled-back cloudformation__location.deploy-location-routecalculator-unsupported-data-source-rejected-and-rolled-back cloudformation__location.deploy-location-map-and-apikey-restricted-to-map-round-trip-and-delete cloudformation__location.deploy-location-tracker-geofencecollection-and-consumer-association-round-trip-and-delete
Issue-Key: cloudformation__location.deploy-location-placeindex-round-trip-and-delete|run#10|AssertionError|assert outputs["IndexName"] == index_name, (
Issue-Key: cloudformation__location.deploy-location-routecalculator-round-trip-and-delete|run#10|AssertionError|assert outputs["CalculatorName"] == calculator_name, (
Issue-Key: cloudformation__location.deploy-location-geofencecollection-nonexistent-kms-key-rejected-and-rolled-back|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__location.deploy-location-routecalculator-unsupported-data-source-rejected-and-rolled-back|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__location|CreateStack|ValidationError
Issue-Commit: 2061b56961bdd6ee54cec665a2a7c785cb8cb220

# cloudformation__location: 5 net-new causes — AWS::Location::* resources are stub-provisioned (Ref returns a synthesized physical id, Location refusals never roll the stack back) and APIKey/TrackerConsumer are unregistered resource types

> _Note: this issue was identified against the code base at commit `2061b56961bdd6ee54cec665a2a7c785cb8cb220`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five distinct defects in CloudFormation's handling of the AWS::Location::* resource types, all grounded in failing behavioral checks of the cloudformation__location suite. Each cause is numbered and carries its own failing check ids; all five must be closed before this issue is submitted.

=== Cause 1 of 5: Ref on AWS::Location::PlaceIndex / ::RouteCalculator returns a CloudFormation-synthesized physical id instead of the declared resource name ===
AWS: AWS::Location::PlaceIndex and AWS::Location::RouteCalculator have a required, client-supplied name (Smithy: CreatePlaceIndex.IndexName @required, target ResourceName, @length min 1 max 100, pattern ^[-._\w]+$; CreateRouteCalculator.CalculatorName likewise @required/ResourceName; DataSource carries cfnMutability create-and-read, i.e. the CFN resource maps 1:1 onto these API inputs). The resource's physical id IS that name, so `{"Ref": "<logical id>"}` resolves to exactly the IndexName / CalculatorName the template declared, and DescribePlaceIndex / DescribeRouteCalculator find the resource under that name (per aws-resource-location-placeindex / aws-resource-location-routecalculator: Ref returns the index/calculator name). CloudFormation only generates a name when the template omits one.
Emulator: the stack reaches CREATE_COMPLETE but the stack Output carrying `{"Ref": "Index"}` is a generated id of the form `<stack-name>-<logical-id>-<12 hex/uppercase chars>` (recorded: expected Ref 'sim2real-loc-idx-…', stack reported 'sim2real-locplaceidx-…-Index-98F16F44C0B4'); the same for the Calculator (expected 'sim2real-loc-calc-…', stack reported 'sim2real-locroutecalc-…-Calculator-57E34E4DACBD'). The fixed parts of that template are the stack name, the literal logical id and a random suffix — the declared IndexName/CalculatorName is dropped entirely. That the PlaceIndex stack settles CREATE_COMPLETE at all while Location's own CreatePlaceIndex is an unimplemented scaffold stub (see open issue 20261030T001704Z-location-createplaceindex-stub.md) shows the provider never calls the Location API: it mints a physical id and records success.
Why: a client that reads Ref/GetAtt out of a stack and then addresses the resource through Location's own API (DescribePlaceIndex(IndexName=…), DescribeRouteCalculator) cannot find it, and a template that names a resource deliberately (so other systems can reference it) gets a different name than it asked for. The resource also does not exist in Location at all, so every downstream read, tag listing and delete assertion fails.
Fix: the AWS::Location::PlaceIndex and AWS::Location::RouteCalculator resource providers in the CloudFormation crate — the create path must forward the declared properties to the Location control plane (CreatePlaceIndex / CreateRouteCalculator) and use the returned IndexName/CalculatorName as the resource's physical id instead of synthesizing one.
Traces: cloudformation__location.deploy-location-placeindex-round-trip-and-delete, cloudformation__location.deploy-location-routecalculator-round-trip-and-delete.

=== Cause 2 of 5: an AWS::Location::GeofenceCollection whose KmsKeyId names a nonexistent key reaches CREATE_COMPLETE instead of CREATE_FAILED + ROLLBACK_COMPLETE ===
AWS: CreateGeofenceCollection.KmsKeyId (Smithy: target KmsKeyId, @length min 1 max 2048, cfnMutability create-and-read) is resolved against KMS at create time; a key id/alias that does not exist is refused with ValidationException (modeled, httpError 400) whose message forwards the KMS failure. The CloudFormation resource handler therefore fails the resource and the stack rolls back: the Collection gets a CREATE_FAILED event whose ResourceStatusReason is the registry-handler form `Resource handler returned message: "<message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)`, with HandlerErrorCode `InvalidRequest` and message `Invalid request provided: operation error KMS: DescribeKey, https response error StatusCode: 400, RequestID: <id>, NotFoundException: Alias arn:<partition>:kms:<region>:<account>:alias/<name> is not found. (Service: Location, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)`; the stack ends ROLLBACK_COMPLETE and no geofence collection is left behind (DescribeGeofenceCollection then answers ResourceNotFoundException `Geofence collection not found: <CollectionName>.`). The fixed text here is the wording; the partition, region, account, alias name, request ids and attempt count are taken from the request and the environment.
Emulator: the stack ends CREATE_COMPLETE — the bad KmsKeyId is accepted, no CREATE_FAILED event or handler reason is produced, and nothing rolls back (recorded: "stack 'sim2real-locgeofcbad-…' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE").
Why: a client validating its template against a missing/mis-typed KMS key is told the deployment succeeded; its rollback and failure-reason handling never runs, and it believes it owns an encrypted collection that either does not exist or is unencrypted.
Fix: the AWS::Location::GeofenceCollection provider's create path (forward KmsKeyId resolution to the Location control plane and surface its refusal) plus the stack engine's resource-failure/rollback path that must record the CREATE_FAILED handler reason and drive ROLLBACK_COMPLETE.
Traces: cloudformation__location.deploy-location-geofencecollection-nonexistent-kms-key-rejected-and-rolled-back.

=== Cause 3 of 5: an AWS::Location::RouteCalculator with an unsupported DataSource reaches CREATE_COMPLETE instead of rolling back with Location's refusal ===
AWS: CreateRouteCalculator.DataSource is @required and accepts only the supported providers; an unsupported value (e.g. "Bing") is refused with ValidationException (modeled, httpError 400) and the message `'DataSource' must be one of Esri, Grab, Here.` (fixed text; the member name quoted and the allowed-value list are the requirement — see also open issue 20261031T020253Z-location-createroutecalculator-s-unsupported-datasource-validationexception-message-renders-the.md for the Location-side wording). The CloudFormation handler forwards that refusal, so the Calculator gets one CREATE_FAILED event with ResourceStatusReason `Resource handler returned message: "Invalid request provided: 'DataSource' must be one of Esri, Grab, Here. (Service: Location, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)` and the stack ends ROLLBACK_COMPLETE with no calculator created.
Emulator: the stack ends CREATE_COMPLETE (recorded: "stack 'sim2real-locroutecalcbad-…' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE") — the unsupported DataSource is accepted by the provider even though Location's own CreateRouteCalculator does refuse it, confirming the provider does not call the service.
Why: template-level validation of an enum-like property silently passes, so a client deploys a route calculator that cannot exist and never sees the provider's failure reason.
Fix: the AWS::Location::RouteCalculator provider's create path (forward to CreateRouteCalculator and propagate ValidationException as a CREATE_FAILED handler reason) and the engine's rollback path.
Traces: cloudformation__location.deploy-location-routecalculator-unsupported-data-source-rejected-and-rolled-back.

=== Cause 4 of 5: AWS::Location::APIKey is not a registered resource type ===
AWS: AWS::Location::APIKey is a supported CloudFormation resource type backed by Location's CreateKey (Smithy: CreateKey with @required KeyName and Restrictions), so CreateStack accepts a template declaring it and provisions the API key.
Emulator: CreateStack is refused 400 ValidationError with message `Template format error: Unrecognized resource types: [AWS::Location::APIKey]`. The error class and wording are AWS's for a genuinely unknown type; the defect is that this type IS known to AWS, so the refusal must not happen.
Why: no stack that declares a Location API key (the normal way to hand a map to a browser client, with Restrictions scoping it to one resource) can be deployed at all; the whole template is rejected before any resource is created.
Fix: register AWS::Location::APIKey in the CloudFormation resource-type registry and add its resource provider (create/read/update/delete against Location CreateKey/DescribeKey/UpdateKey/DeleteKey). Related, already open: the Location-side CreateMap stub (20261030T001704Z-location-createmap-stub.md) blocks this template's AWS::Location::Map too, but it does not cover this registry gap.
Traces: cloudformation__location.deploy-location-map-and-apikey-restricted-to-map-round-trip-and-delete.

=== Cause 5 of 5: AWS::Location::TrackerConsumer is not a registered resource type ===
AWS: AWS::Location::TrackerConsumer is a supported CloudFormation resource type backed by Location's AssociateTrackerConsumer (Smithy: TrackerName @httpLabel @required, ConsumerArn @required, errors include ValidationException 400 and ResourceNotFoundException 404), so CreateStack accepts a template that associates a geofence collection with a tracker.
Emulator: CreateStack is refused 400 ValidationError with message `Template format error: Unrecognized resource types: [AWS::Location::TrackerConsumer]` — again AWS's wording for an unknown type applied to a type AWS does recognize.
Why: the tracker/geofence-collection association — the standard way a template wires geofence evaluation to a tracker — cannot be expressed in a stack at all, and the entire template is rejected.
Fix: register AWS::Location::TrackerConsumer in the CloudFormation resource-type registry and add its provider (create = AssociateTrackerConsumer, delete = DisassociateTrackerConsumer, read via ListTrackerConsumers). Note the Location-side ListTrackerConsumers stub is already filed (20261104T222303Z-location-listtrackerconsumers-stub.md) and does not cover this registry gap.
Traces: cloudformation__location.deploy-location-tracker-geofencecollection-and-consumer-association-round-trip-and-delete.

## Evidence
Suite: simparity-behavioral-aws-v1, service heading `### cloudformation__location` — 0/6 passed.

Cause 1:
- [mismatch] cloudformation__location.deploy-location-placeindex-round-trip-and-delete
  AssertionError: expected Ref on the Index to equal 'sim2real-loc-idx-f7ba20688436', stack reports 'sim2real-locplaceidx-a6e6e1617149-Index-98F16F44C0B4'
  (template: Resources.Index = AWS::Location::PlaceIndex with Properties {IndexName: <name>, DataSource: "Esri", Tags: [...]}, Outputs.IndexName = {"Ref": "Index"}). Expected Ref == the declared IndexName; got a <stack>-<logical>-<hex> physical id.
- [mismatch] cloudformation__location.deploy-location-routecalculator-round-trip-and-delete
  AssertionError: expected Ref on the Calculator to equal 'sim2real-loc-calc-9ec3ce8bf54f', stack reports 'sim2real-locroutecalc-38d62c8e5a87-Calculator-57E34E4DACBD'

Cause 2:
- [mismatch] cloudformation__location.deploy-location-geofencecollection-nonexistent-kms-key-rejected-and-rolled-back
  AssertionError: stack 'sim2real-locgeofcbad-a3502dcf0c07' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE
  Expected after rollback: Collection CREATE_FAILED with handler reason HandlerErrorCode=InvalidRequest and message "Invalid request provided: operation error KMS: DescribeKey, https response error StatusCode: 400, RequestID: <id>, NotFoundException: Alias arn:<partition>:kms:<region>:<account>:alias/<name> is not found. (Service: Location, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", then DescribeGeofenceCollection -> ResourceNotFoundException "Geofence collection not found: <CollectionName>."
  Actual: CREATE_COMPLETE, no CREATE_FAILED row, no rollback.

Cause 3:
- [mismatch] cloudformation__location.deploy-location-routecalculator-unsupported-data-source-rejected-and-rolled-back
  AssertionError: stack 'sim2real-locroutecalcbad-5d6780ec34aa' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE
  Expected handler reason: ("Invalid request provided: 'DataSource' must be one of Esri, Grab, Here. (Service: Location, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", "InvalidRequest") for DataSource="Bing"; the control value "Esri" must still succeed.

Cause 4:
- [error] cloudformation__location.deploy-location-map-and-apikey-restricted-to-map-round-trip-and-delete
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Location::APIKey]

Cause 5:
- [error] cloudformation__location.deploy-location-tracker-geofencecollection-and-consumer-association-round-trip-and-delete
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Location::TrackerConsumer]

## Steps to Reproduce
Causes 1-3 (provider does not call Location; Ref is synthesized; no rollback):
1. CreateStack with TemplateBody declaring `{"Index": {"Type": "AWS::Location::PlaceIndex", "Properties": {"IndexName": "<name>", "DataSource": "Esri"}}}` and `Outputs.IndexName = {"Ref": "Index"}`.
2. Poll DescribeStacks(StackName=<StackId>) to a terminal status, then read Outputs: the Output value is `<stack>-Index-<hex>`, not `<name>`; DescribePlaceIndex(IndexName="<name>") does not find the index. Repeat with `AWS::Location::RouteCalculator` + `CalculatorName`/`{"Ref": "Calculator"}`.
3. CreateStack with an `AWS::Location::GeofenceCollection` whose `KmsKeyId` is `alias/<name-that-does-not-exist>`: the stack reaches CREATE_COMPLETE instead of CREATE_FAILED on the collection and ROLLBACK_COMPLETE on the stack; DescribeStackEvents has no CREATE_FAILED row for the collection.
4. CreateStack with an `AWS::Location::RouteCalculator` whose `DataSource` is `"Bing"`: the stack reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE, even though calling Location CreateRouteCalculator directly with DataSource="Bing" is refused 400 ValidationException.

Causes 4-5 (unregistered resource types):
5. CreateStack with a template declaring `AWS::Location::APIKey` (Properties KeyName + Restrictions): the call is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Location::APIKey]".
6. CreateStack with a template declaring `AWS::Location::TrackerConsumer` (Properties TrackerName + ConsumerArn): refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Location::TrackerConsumer]".
