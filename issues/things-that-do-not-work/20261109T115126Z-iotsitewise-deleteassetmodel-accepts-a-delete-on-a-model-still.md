Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.sitewise-delete-asset-model-is-refused-synchronously-while-still-creating
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.sitewise-delete-asset-model-is-refused-synchronously-while-still-creating|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with {result!r}")
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# DeleteAssetModel accepts a delete on a model still CREATING (202 DELETING) instead of refusing it with InvalidRequestException naming the model's unsettled state

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAssetModel

## Description
Note: open issue 20261102T084333Z-iotsitewise-illegal-asset-model-definitions-settle-active-instead-of.md already describes this same defect as its 'Cause 5 of 5' and lists this check among its trace ids, but this replay did not mark the check as OWNED/ALREADY FILED, so it is filed here rather than dropped. A fixer closing that issue's cause 5 closes this one too.

Cause 1 of 1: DeleteAssetModel has no busy-state precondition, so a delete issued while the model is still CREATING is accepted and moves the model to DELETING.
AWS: Smithy: DeleteAssetModel DELETE /asset-models/{assetModelId}, success code 202, output assetModelStatus @required; modeled errors InvalidRequestException (httpError 400), ConflictingOperationException (409), PreconditionFailedException (412), ResourceNotFoundException (404), ThrottlingException (429), InternalFailureException (500). CreateAssetModel (POST /asset-models, code 202) answers assetModelStatus.state=CREATING, and an asset model may only be modified once it has left CREATING: the delete is refused HTTP 400 InvalidRequestException with the message 'This asset model must be ACTIVE or FAILED before you can modify it. AssetModelID: <assetModelId> AssetModelName: <assetModelName> AssetModelState: <state>'. Fixed text: 'This asset model must be ACTIVE or FAILED before you can modify it. AssetModelID: ', ' AssetModelName: ', ' AssetModelState: '; variable parts: the model's id and name from the request/state and the model's current state from the emulator's own state (CREATING here). The refusal leaves the in-flight transition untouched — a DescribeAssetModel immediately afterwards must NOT report DELETING — the model goes on to settle ACTIVE, and the identical delete then succeeds with assetModelStatus.state=DELETING and the model eventually 404s on DescribeAssetModel with ResourceNotFoundException.
Emulator: the delete issued immediately after the create, inside the CREATING window, answers HTTP 202 with {assetModelId: <id>, assetModelStatus: {state: 'DELETING'}} — no error at all. The model is torn down mid-create instead of being refused.
Why: this is the busy-state guard for every asset-model write. A client that races a delete against a create gets a half-created model destroyed rather than a retryable 400, and the AWS-shaped code path (catch InvalidRequestException, wait for the model to settle, retry the delete) is never exercised against the emulator — so clients are written without it and break on real AWS.
Fix: the DeleteAssetModel handler's precondition — gate on the model's current state (only ACTIVE or FAILED may be modified) and refuse with InvalidRequestException and the service's state message, writing no transition, before any state change is applied.
Traces: iotsitewise.sitewise-delete-asset-model-is-refused-synchronously-while-still-creating.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 55/91 passed; iotsitewise 55/87 (63%).

iotsitewise.sitewise-delete-asset-model-is-refused-synchronously-while-still-creating — CreateAssetModel(assetModelName=<unique>, assetModelProperties=[{name: ActivePower, dataType: DOUBLE, type: {measurement: {}}}]) answers assetModelStatus.state='CREATING' (asserted), then DeleteAssetModel(assetModelId=<id>) is issued with no read in between.
Expected: ClientError with Code 'InvalidRequestException' and Message 'This asset model must be ACTIVE or FAILED before you can modify it. AssetModelID: <assetModelId> AssetModelName: <assetModelName> AssetModelState: CREATING', then DescribeAssetModel NOT reporting DELETING, the model settling ACTIVE, the identical delete then returning state DELETING, and DescribeAssetModel finally answering ResourceNotFoundException.
Actual: AssertionError: expected a ClientError, call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'assetModelId': '<id>', 'assetModelStatus': {'state': 'DELETING'}}.

The asset-model id and name quoted are this run's values filling the message template; the requirement is the 400 InvalidRequestException, the message template above, and that the refused delete leaves the model's CREATING transition untouched.

## Steps to Reproduce
1. POST /asset-models (CreateAssetModel) with one DOUBLE measurement property; confirm the response reports assetModelStatus.state='CREATING'.
2. Immediately DELETE /asset-models/{assetModelId} (DeleteAssetModel) while the model is still CREATING.
3. Observe HTTP 202 with assetModelStatus.state='DELETING' instead of HTTP 400 InvalidRequestException 'This asset model must be ACTIVE or FAILED before you can modify it. AssetModelID: <id> AssetModelName: <name> AssetModelState: CREATING'.
4. Observe that DescribeAssetModel now reports DELETING, i.e. the refused delete mutated the model's in-flight transition.
5. Re-replay iotsitewise.sitewise-delete-asset-model-is-refused-synchronously-while-still-creating.
