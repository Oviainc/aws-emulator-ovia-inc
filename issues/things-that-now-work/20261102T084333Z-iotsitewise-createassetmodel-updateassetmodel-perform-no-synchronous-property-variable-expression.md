Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.sitewise-create-asset-model-rejects-a-duplicate-property-name iotsitewise.sitewise-create-asset-model-rejects-a-cross-model-property-reference-with-no-hierarchyid iotsitewise.sitewise-create-asset-model-rejects-a-hierarchyid-naming-no-declared-hierarchy iotsitewise.sitewise-create-asset-model-rejects-a-metric-variable-unused-by-the-expression iotsitewise.sitewise-create-asset-model-rejects-an-unknown-variable-in-a-metric-expression iotsitewise.sitewise-create-asset-model-rejects-a-tumbling-interval-outside-the-window-limits iotsitewise.sitewise-update-asset-model-rejects-a-property-respecified-by-name-instead-of-id
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.sitewise-create-asset-model-rejects-a-duplicate-property-name|_error_and_message#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error message")
Issue-Key: iotsitewise.sitewise-create-asset-model-rejects-a-cross-model-property-reference-with-no-hierarchyid|_error_and_message#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error message")
Issue-Key: iotsitewise.sitewise-create-asset-model-rejects-a-hierarchyid-naming-no-declared-hierarchy|_error_and_message#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error message")
Issue-Key: iotsitewise.sitewise-create-asset-model-rejects-a-metric-variable-unused-by-the-expression|_error_and_message#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error message")
Issue-Key: iotsitewise.sitewise-create-asset-model-rejects-an-unknown-variable-in-a-metric-expression|_error_and_message#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error message")
Issue-Key: iotsitewise.sitewise-create-asset-model-rejects-a-tumbling-interval-outside-the-window-limits|_error_and_message#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error message")
Issue-Key: iotsitewise.sitewise-update-asset-model-rejects-a-property-respecified-by-name-instead-of-id|_error_and_message#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error message")
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# CreateAssetModel/UpdateAssetModel perform no synchronous property, variable, expression or window validation (7 rules AWS refuses with InvalidRequestException)

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetModel

## Description
The asset-model write path stores assetModelProperties essentially verbatim: it never resolves a metric/transform variable against the model being defined, never parses the metric expression, never range-checks the tumbling window, never checks property-name uniqueness and never compares the submitted property set against the one the model already has. Every request below is answered 2xx with a model id (or 200 for the update) where AWS refuses it synchronously with InvalidRequestException (httpError 400, member message) and creates/changes nothing. This issue bundles 7 distinct validation rules as 7 causes; all seven must be closed. Related: open issue 20261029T190317Z-iotsitewise-refusal-paths-answer-emulator-authored-messages-and-the.md covers refusal paths that DO fire but word their message wrongly - it does not cover these, where no refusal exists at all.

Cause 1 of 7: a duplicate property NAME in one CreateAssetModel is accepted.
AWS: Smithy: CreateAssetModel input assetModelProperties (AssetModelPropertyDefinitions), each AssetModelPropertyDefinition with name @required (Name, @length 1..256); error InvalidRequestException httpError 400. Two definitions sharing one name are refused 400 with the message 'Property <name> name is not unique in the Asset Model.' - fixed text 'Property ' and ' name is not unique in the Asset Model.'; the variable part is the duplicated property's name from the request. No model is created.
Emulator: the create is accepted (the check's _error_and_message reports 'the call succeeded; real AWS rejects it') and a model with two identically named properties is left in ListAssetModels.
Why: property names are the handles a model's own metric/transform variables resolve by (VariableValue.propertyId documents 'You can use the property name if it's from the same asset model'), so a model with two same-named properties makes every same-model reference ambiguous, and a client relying on AWS rejecting the definition silently builds an unusable model.
Fix: CreateAssetModel's request validation, before any state is written - reject a name collision across the submitted property definitions.
Traces: iotsitewise.sitewise-create-asset-model-rejects-a-duplicate-property-name.

Cause 2 of 7: a metric variable naming ANOTHER model's property id with no hierarchyId is accepted.
AWS: Smithy: VariableValue.propertyId (Macro) + VariableValue.hierarchyId (Macro), documented 'The ID of the hierarchy to query for the property ID' - without a hierarchyId the reference is resolved only inside the model being defined. A propertyId that exists on a child model but is not reachable without a scope is refused 400 InvalidRequestException with 'Property <propertyId> does not exist.' - fixed text 'Property ' and ' does not exist.', variable part the propertyId as submitted (AWS names the very id that does exist on the child, because it does not exist in the resolution scope). No model is created.
Emulator: the create is accepted and the model is left behind; the unresolvable variable is simply stored.
Why: the whole point of hierarchyId is scope selection; a client that omits it gets a model that looks valid and a rollup that can never compute, instead of the 400 that tells it to add the hierarchy.
Fix: CreateAssetModel's variable resolution - resolve every VariableValue against the same model, or the named hierarchy's child model, only, and refuse an unresolvable propertyId.
Traces: iotsitewise.sitewise-create-asset-model-rejects-a-cross-model-property-reference-with-no-hierarchyid.

Cause 3 of 7: a variable's hierarchyId naming no declared hierarchy is accepted.
AWS: Smithy: CreateAssetModel input assetModelHierarchies (AssetModelHierarchyDefinitions, each with name + childAssetModelId); VariableValue.hierarchyId may be the hierarchy's name. A hierarchyId that matches no hierarchy declared in the same request is refused 400 InvalidRequestException with 'AssetModelHierarchy <hierarchyId> does not exist.' - fixed text 'AssetModelHierarchy ' and ' does not exist.', variable part the hierarchyId exactly as submitted. No model is created.
Emulator: accepted; the model is created with a variable pointing at a hierarchy that does not exist.
Why: a mistyped hierarchy name is the most common authoring error here and AWS reports it by name; the emulator turns it into a silently dead metric.
Fix: same validation point as cause 2 - resolve hierarchyId against the request's own assetModelHierarchies (by id or name) and refuse when it names none.
Traces: iotsitewise.sitewise-create-asset-model-rejects-a-hierarchyid-naming-no-declared-hierarchy.

Cause 4 of 7: a declared metric variable that the expression never uses is accepted.
AWS: Smithy: Metric.expression (Expression, @length 0..1024) + Metric.variables (ExpressionVariables); the expression is parsed and every declared variable must appear in it. A metric declaring variable power with expression '1 + 1' is refused 400 InvalidRequestException with the message '`<expression>`: Variable [<variableName>] is unused in formula. Formulas must use all defined variables' - fixed text is the backtick-quoted expression prefix, then 'Variable [', then '] is unused in formula. Formulas must use all defined variables' (no trailing period); variable parts: the expression and the unused variable's name, both from the request. No model is created.
Emulator: accepted.
Why: the message proves AWS parsed the formula; a client that relies on that rejection ships a metric whose variables are not wired to the formula it thinks it deployed.
Fix: CreateAssetModel's metric validation - parse Metric.expression and cross-check the declared variable set both ways.
Traces: iotsitewise.sitewise-create-asset-model-rejects-a-metric-variable-unused-by-the-expression.

Cause 5 of 7: an unknown function/variable token in a metric expression is accepted.
AWS: Smithy: Metric.expression as above. An expression referencing an undeclared token (e.g. avg(nope) where only power is declared) is refused 400 InvalidRequestException with the message '`<expression>`: Unknown function or variable ''<token>'' at pos <n> in expression ''<expression>''' - fixed text: the backtick-quoted expression prefix, 'Unknown function or variable ', the single-quoted token, ' at pos ', the position, ' in expression ', the single-quoted expression; variable parts: the expression, the offending token and its 0-based character position in the expression, all derived from the request.
Emulator: accepted.
Why: the position in the message is what proves the expression was parsed rather than stored; without the refusal the model goes ACTIVE and the metric never produces a data point, with nothing anywhere naming the bad token.
Fix: the same expression parser as cause 4 must report unknown tokens with their position.
Traces: iotsitewise.sitewise-create-asset-model-rejects-an-unknown-variable-in-a-metric-expression.

Cause 6 of 7: a tumbling-window interval outside the service's limits is accepted.
AWS: Smithy: TumblingWindow.interval @required (Interval, @length 2..23), documented 'The interval time must be between 1 minute and 1 week'. An interval below that floor (e.g. 30s) is refused 400 InvalidRequestException with the message '`<expression>`: Window time interval ''<interval>'' is not within limits' - fixed text: the backtick-quoted expression prefix, 'Window time interval ', the single-quoted interval, ' is not within limits' (no trailing period); variable parts: the metric's expression and the submitted interval string. No model is created.
Emulator: accepted; a 30-second tumbling window is stored.
Why: the client gets a model AWS would never have created, so the same template is accepted by the emulator and rejected in production - the exact class of divergence that makes a local run worthless as a pre-flight.
Fix: CreateAssetModel's window validation - bound-check TumblingWindow.interval (and its ISO-8601 spellings) against the 1 minute..1 week range.
Traces: iotsitewise.sitewise-create-asset-model-rejects-a-tumbling-interval-outside-the-window-limits.

Cause 7 of 7: UpdateAssetModel re-specifying an existing property by NAME without its id is accepted instead of refused as a destructive replacement.
AWS: Smithy: UpdateAssetModel is a whole-model replace; AssetModelProperty.id is optional on the wire and documented 'If you are calling UpdateAssetModel to modify an existing property: This can be either the actual ID in UUID format, or else externalId: followed by the external ID'; error InvalidRequestException 400. Submitting a property whose name matches an existing property but carrying no id is refused 400 with 'Cannot create property <name>. Creation will result in replacement of property <existingPropertyId>' - fixed text 'Cannot create property ' and '. Creation will result in replacement of property ' (no trailing period); variable parts: the property name from the request and the server-assigned id of the property that would have been destroyed, from the model's stored state. The property set is unchanged.
Emulator: the update succeeds, so the property is silently re-created (its id - and therefore every time series, variable reference and ingest path addressed by that id - is replaced) with no refusal at all.
Why: this is the data-loss guard of the whole update path; losing it means a client that round-trips a model description it mutated by name destroys its own property ids and history without any error.
Fix: UpdateAssetModel's property-diff validation - match submitted properties to stored ones by id (or externalId) and refuse an id-less definition whose name collides with an existing property.
Traces: iotsitewise.sitewise-update-asset-model-rejects-a-property-respecified-by-name-instead-of-id.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 54/87 passed, all 33 failures in iotsitewise. The seven checks grounding this issue all fail in their harness helper _error_and_message, i.e. the AWS call that must raise returned 2xx:

1) iotsitewise.sitewise-create-asset-model-rejects-a-duplicate-property-name - CreateAssetModel with two properties both named ActivePower. Expected ('InvalidRequestException', 'Property ActivePower name is not unique in the Asset Model.'); got a successful create. AssertionError: the call succeeded; real AWS rejects it, so there is no error message.
2) iotsitewise.sitewise-create-asset-model-rejects-a-cross-model-property-reference-with-no-hierarchyid - CreateAssetModel whose metric variable carries the CHILD model's metric propertyId and NO hierarchyId (the hierarchy itself is declared). Expected InvalidRequestException 'Property <childMetricId> does not exist.'; got a successful create.
3) iotsitewise.sitewise-create-asset-model-rejects-a-hierarchyid-naming-no-declared-hierarchy - same shape with hierarchyId='Nope' while the declared hierarchy is 'Turbines'. Expected InvalidRequestException 'AssetModelHierarchy Nope does not exist.'; got a successful create.
4) iotsitewise.sitewise-create-asset-model-rejects-a-metric-variable-unused-by-the-expression - metric variables=[power], expression='1 + 1'. Expected InvalidRequestException '`1 + 1`: Variable [power] is unused in formula. Formulas must use all defined variables'; got a successful create.
5) iotsitewise.sitewise-create-asset-model-rejects-an-unknown-variable-in-a-metric-expression - expression='avg(nope)' with only power declared. Expected InvalidRequestException "`avg(nope)`: Unknown function or variable 'nope' at pos 4 in expression 'avg(nope)'"; got a successful create.
6) iotsitewise.sitewise-create-asset-model-rejects-a-tumbling-interval-outside-the-window-limits - expression='avg(power)', tumbling interval='30s'. Expected InvalidRequestException "`avg(power)`: Window time interval '30s' is not within limits"; got a successful create.
7) iotsitewise.sitewise-update-asset-model-rejects-a-property-respecified-by-name-instead-of-id - model created with one measurement ActivePower, then UpdateAssetModel resubmitting the same definition with no id. Expected InvalidRequestException 'Cannot create property ActivePower. Creation will result in replacement of property <originalPropertyId>'; got a 2xx and the update applied.

The names, ids, expressions and intervals above are these runs' values filling the templates; the requirement is the rule and the message template, as stated per cause.

Model grounding: CreateAssetModel / UpdateAssetModel both declare InvalidRequestException (httpError 400, member message); AssetModelPropertyDefinition.name @required; Metric.expression @length 0..1024 with Metric.variables; TumblingWindow.interval @required @length 2..23, documented 'must be between 1 minute and 1 week'; VariableValue.propertyId/hierarchyId are Macro and documented as resolvable by name within the same model or via a hierarchy.

## Steps to Reproduce
1. Re-replay iotsitewise.sitewise-create-asset-model-rejects-a-duplicate-property-name: POST /asset-models with two property definitions named ActivePower - observe a successful create instead of 400 InvalidRequestException.
2. Re-replay iotsitewise.sitewise-create-asset-model-rejects-a-cross-model-property-reference-with-no-hierarchyid and ...-a-hierarchyid-naming-no-declared-hierarchy: create a child model, read its metric property id, then POST /asset-models with a metric variable carrying that id with no hierarchyId, and again with hierarchyId='Nope' - both are accepted.
3. Re-replay ...-a-metric-variable-unused-by-the-expression, ...-an-unknown-variable-in-a-metric-expression and ...-a-tumbling-interval-outside-the-window-limits: POST /asset-models with expression '1 + 1' (variable power declared), with expression 'avg(nope)', and with tumbling interval '30s' - all three are accepted.
4. Re-replay iotsitewise.sitewise-update-asset-model-rejects-a-property-respecified-by-name-instead-of-id: create a model with one measurement, GET it, then PUT /asset-models/{id} resubmitting that property WITHOUT its id - observe the update succeeds and the property id changes, instead of 400 InvalidRequestException.
