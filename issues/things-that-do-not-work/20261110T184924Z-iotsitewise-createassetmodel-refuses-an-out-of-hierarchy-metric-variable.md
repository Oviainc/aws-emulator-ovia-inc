Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.sitewise-create-asset-model-settles-failed-when-a-metric-reaches-outside-its-hierarchy
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|CreateAssetModel|InvalidRequestException
Issue-Commit: 2588a9c9d3ea0b6cf31cfe9f1b8e1e2847ea6037

# CreateAssetModel refuses an out-of-hierarchy metric variable synchronously with 'Property <id> does not exist.' instead of accepting it as CREATING and settling the model FAILED/VALIDATION_ERROR

> _Note: this issue was identified against the code base at commit `2588a9c9d3ea0b6cf31cfe9f1b8e1e2847ea6037`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetModel

## Description
REOPENS 20261102T084333Z-iotsitewise-createassetmodel-updateassetmodel-perform-no-synchronous-property-variable-expression.md: its fix left the call refused.

This issue bundles 1 cause; it must be closed.

Cause 1 of 1: a metric variable that DOES carry a hierarchyId naming a hierarchy declared in the same request, but whose propertyId belongs to an asset model outside that hierarchy, is refused synchronously by CreateAssetModel with InvalidRequestException 'Property <propertyId> does not exist.' AWS accepts that request and defers the diagnosis to the asynchronous settle plane.

AWS: Smithy (com.amazonaws.iotsitewise#AWSIoTSiteWise): VariableValue.propertyId (Macro, @length 1..256) documented 'The ID of the property to use as the variable. You can use the property name if it's from the same asset model'; VariableValue.hierarchyId (Macro, @length 1..256) documented 'The ID of the hierarchy to query for the property ID ... you can have several hierarchies using the same model and therefore the same propertyId'; CreateAssetModel output assetModelStatus; AssetModelStatus{state @required (AssetModelState), error (ErrorDetails)}; ErrorDetails{code @required (ErrorCode enum [VALIDATION_ERROR, INTERNAL_FAILURE]), message @required}. IoT SiteWise splits asset-model validation across two planes: a synchronous request-validation plane (400 InvalidRequestException) and an asynchronous plane that accepts the request, answers 2xx with assetModelStatus.state = CREATING, and then settles the model at state = FAILED with assetModelStatus.error = {code: VALIDATION_ERROR, message: <the rule that fired>}. A variable whose hierarchyId resolves to a declared hierarchy, but whose propertyId is not a property the hierarchy's child model exposes, belongs to the ASYNCHRONOUS plane: the create is accepted as CREATING (nothing in the synchronous response distinguishes it from a legal create) and the model settles FAILED with error.code = VALIDATION_ERROR and error.message = "A formula property references a child asset model property that doesn't exist." — entirely FIXED text, including the trailing period, with no variable parts. The same variable aimed at the real child's metric reaches ACTIVE. The synchronous 'Property <propertyId> does not exist.' message (fixed text 'Property ' and ' does not exist.', variable part the propertyId exactly as submitted) belongs to the DIFFERENT case where no hierarchyId scopes the reference at all (cause 2 of the reopened issue) — it must not fire when a declared hierarchy is named.

Emulator: POST /asset-models answers 400 InvalidRequestException with message 'Property <propertyId> does not exist.' (replay: 'An error occurred (InvalidRequestException) when calling the CreateAssetModel operation: Property 1093c07a-93df-4905-bc3c-aff166df633a does not exist.', that uuid being this run's stranger-model metric id filling the template). No model is created, so state CREATING is never reported and the FAILED/VALIDATION_ERROR settle is unreachable. The synchronous variable-resolution rule introduced for the no-hierarchyId case is now applied to hierarchy-scoped references as well: it resolves the propertyId only against the model being defined (and/or fails to resolve it against the named hierarchy's child model) and refuses instead of accepting.

Why: a client that mistargets a rollup gets an exception at call time rather than the accept-then-poll flow AWS requires, so the correct AWS-shaped code path (accept, poll DescribeAssetModel, read assetModelStatus.error) is never exercised and catch-and-abort code is written instead; conversely a template that AWS accepts (and that a client may legitimately create and then repair with UpdateAssetModel) cannot be created against the emulator at all. The FAILED state and its VALIDATION_ERROR diagnosis remain unreachable for this rule.

Fix: CreateAssetModel's synchronous variable validation (the request-validation stage of the asset-model write path) must scope its refusal to references with NO hierarchyId, and route a hierarchy-scoped propertyId that the child model does not expose to the asset-model settle/transition machinery, which writes state = FAILED with ErrorDetails{VALIDATION_ERROR, "A formula property references a child asset model property that doesn't exist."} while leaving the model listed and describable. Related open issue 20261102T084333Z-iotsitewise-illegal-asset-model-definitions-settle-active-instead-of.md covers the missing asynchronous FAILED path for this and other rules (its recorded signature for this check was the ACTIVE settle); it does not cover the synchronous refusal now blocking the request before any model exists.

Traces: iotsitewise.sitewise-create-asset-model-settles-failed-when-a-metric-reaches-outside-its-hierarchy.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 62/91 passed; every other failure is already owned by an open issue.

Failing check: iotsitewise.sitewise-create-asset-model-settles-failed-when-a-metric-reaches-outside-its-hierarchy [error].

What the check does: creates two sibling child models, each with a measurement ActivePower and a metric ActivePowerAvg (expression 'avg(power)', variable power -> propertyId 'ActivePower', tumbling window 5m), waits for both to be ACTIVE, reads the SECOND model's ActivePowerAvg server-assigned id, then creates a third model that declares hierarchy {name: 'Turbines', childAssetModelId: <first child>} and one metric FleetAverageActivePower whose single variable is {propertyId: <second child's ActivePowerAvg id>, hierarchyId: 'Turbines'} — i.e. exactly one thing is wrong: the propertyId belongs to a model outside the declared hierarchy.

Expected (real AWS): 2xx with assetModelStatus.state = 'CREATING', then the model settles state = 'FAILED' with assetModelStatus.error.code = 'VALIDATION_ERROR' and assetModelStatus.error.message = "A formula property references a child asset model property that doesn't exist."; the control create, with the same shape aimed at the hierarchy's own child, reaches ACTIVE.

Actual (emulator): the third CreateAssetModel raises
  botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the CreateAssetModel operation: Property 1093c07a-93df-4905-bc3c-aff166df633a does not exist.
so the check dies at the create, before any assertion on CREATING/FAILED. The 400 status and the 'Property ... does not exist.' message are AWS's wording for the UNSCOPED reference case, not for this one; the uuid is this run's property id filling that template and is not part of the requirement.

Model grounding: VariableValue{propertyId (Macro @length 1..256), hierarchyId (Macro @length 1..256, 'The ID of the hierarchy to query for the property ID'), propertyPath}; AssetModelStatus{state @required, error: ErrorDetails}; ErrorDetails{code @required: ErrorCode [VALIDATION_ERROR, INTERNAL_FAILURE], message @required, details}; CreateAssetModel declares InvalidRequestException (400) — modelled, but for the synchronous plane only.

## Steps to Reproduce
1. POST /asset-models creating a model with properties [{name: ActivePower, DOUBLE, measurement}, {name: ActivePowerAvg, DOUBLE, metric expression 'avg(power)', variables [{name: power, value: {propertyId: 'ActivePower'}}], tumbling window 5m}]. Poll GET /asset-models/{id} until ACTIVE. Do this twice, giving models A (the intended child) and B (the stranger).
2. GET /asset-models/{B} and take the server-assigned id of its ActivePowerAvg property.
3. POST /asset-models with assetModelHierarchies [{name: 'Turbines', childAssetModelId: A}] and one metric property FleetAverageActivePower whose variable is {name: power, value: {propertyId: <B's ActivePowerAvg id>, hierarchyId: 'Turbines'}}.
4. Observe 400 InvalidRequestException 'Property <that id> does not exist.' instead of a 2xx carrying assetModelStatus.state = CREATING.
5. Real AWS: the create is accepted; polling GET /asset-models/{id} settles state = FAILED with error {code: VALIDATION_ERROR, message: "A formula property references a child asset model property that doesn't exist."}, and the same request aimed at A's own ActivePowerAvg id reaches ACTIVE.
6. Re-replay iotsitewise.sitewise-create-asset-model-settles-failed-when-a-metric-reaches-outside-its-hierarchy to confirm the fix.
