Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes iotsitewise-windfarm-trajwire-property-definition-mistakes iotsitewise-windfarm-trajwire-raw-measurement-rollup-fails-then-repaired iotsitewise-windfarm-trajwire-teardown-order-mistakes
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|CreateAssetModel|4xx->2xx|$.message
Issue-Key: iotsitewise|CreateAssetModel|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotsitewise|CreateAssetModel|4xx->2xx|status
Issue-Key: iotsitewise|CreateAssetModel|2xx->4xx|$.assetModelArn
Issue-Key: iotsitewise|CreateAssetModel|2xx->4xx|$.assetModelId
Issue-Key: iotsitewise|CreateAssetModel|2xx->4xx|$.assetModelStatus
Issue-Key: iotsitewise|CreateAssetModel|2xx->4xx|status
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelProperties[*].type.metric.variables[*].value.hierarchyId
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelStatus.error
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelStatus.state
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelProperties[*].type.metric.variables[*].value.propertyId
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelVersion
Issue-Key: iotsitewise|DescribeAssetModel|4xx->2xx|$
Issue-Key: iotsitewise|DescribeAssetModel|4xx->2xx|$.message
Issue-Key: iotsitewise|DescribeAssetModel|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iotsitewise|DescribeAssetModel|4xx->2xx|status
Issue-Related: 20261029T190317Z-iotsitewise-asset-and-asset-model-reads-omit-property-path.md
Issue-Related: 20261103T025010Z-iotsitewise-asset-hierarchy-and-delete-refusals-answer-the-wrong.md
Issue-Commit: bd9fe9d226ef24de2d25d46b8c18046d76d56a4d

# Invalid asset-model definitions settle ACTIVE instead of being refused or settling FAILED

> _Note: this issue was identified against the code base at commit `bd9fe9d226ef24de2d25d46b8c18046d76d56a4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetModel

## Description
CreateAssetModel performs none of the semantic validation AWS applies to an asset-model definition, and the asset-model lifecycle never reaches the FAILED state AWS uses to report a definition that cannot be realised. The result is a cascade: illegal definitions are accepted 202 and settle ACTIVE, which consumes the model name and makes every following step in the fixture answer the wrong thing. The open issue 20261102T084333Z-iotsitewise-createassetmodel-updateassetmodel-perform-no-synchronous-property-variable-expression.md and 20261102T084333Z-iotsitewise-illegal-asset-model-definitions-settle-active-instead-of.md describe adjacent faults but carry none of these replay keys.

Cause 1 of 7: a CreateAssetModel whose assetModelHierarchies contains two hierarchies with the same name is accepted.
  AWS: Smithy: AssetModelHierarchyDefinition.name is required, and its doc states the name identifies the hierarchy within the model; CreateAssetModel models InvalidRequestException (httpError 400). Recorded behavior: AWS refuses with 400 and the message 'Hierarchy <name> is not unique.' — fixed text 'Hierarchy ' / ' name is not unique.' as recorded ('Hierarchy Turbines name is not unique.'), the variable part being the duplicated hierarchy name taken from the request.
  Emulator: accepts the definition, answering 202 with no $.message and no x-amzn-errortype.
  Why: a model with two identically named hierarchies is unusable — a metric variable's hierarchyId by name becomes ambiguous — and the client gets no signal. Worse, the accepted model consumes the name the client intended for the corrected model (see cause 2/3).
  Fix: CreateAssetModel validation — reject duplicate hierarchy names synchronously with InvalidRequestException.
  Traces: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#task#5 (cluster also covers iotsitewise-windfarm-trajwire-property-definition-mistakes#task#5).

Cause 2 of 7: a CreateAssetModel whose metric property variable references a property that cannot be a metric variable is not validated; the request is instead refused for a name collision created by cause 1.
  AWS: Smithy: VariableValue.propertyId doc — 'The ID of the property to use as the variable. You can use the property name if it's from the same asset model'; CreateAssetModel models InvalidRequestException (400). Recorded behavior: AWS refuses with 400 and 'Property Id <propertyId> invalid for variables in Metric property.' — fixed text 'Property Id ' / ' invalid for variables in Metric property.', variable part the offending propertyId from the request.
  Emulator: answers 409 with x-amzn-errortype ResourceAlreadyExistsException and 'Another asset model is already using the same name'.
  Why: the client is told its model NAME is taken when the actual defect is in its property definition. It will rename and re-submit a model that is still invalid, and loop. Note the name is only taken because cause 1 accepted the previous invalid definition — a fixer must confirm both the validation and the name-release behaviour.
  Fix: CreateAssetModel validation — validate metric variable references before the name-uniqueness check, raising InvalidRequestException with AWS's sentence.
  Traces: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#task#6 (cluster also covers task#7, task#8, iotsitewise-windfarm-trajwire-property-definition-mistakes#task#6, #task#7).

Cause 3 of 7: a LEGAL CreateAssetModel is refused 409 because a name was consumed by an invalid definition the emulator should never have accepted.
  AWS: Smithy: CreateAssetModel http code 202, required output assetModelId, assetModelArn, assetModelStatus. Recorded behavior: AWS answers 202 with assetModelId, assetModelArn and assetModelStatus {'state': 'CREATING'}.
  Emulator: answers 409 and emits none of assetModelId / assetModelArn / assetModelStatus.
  Why: the corrected definition — the one the client is supposed to succeed with — cannot be created at all, so the whole workflow dead-ends. The response is also missing three members the model marks required.
  Fix: CreateAssetModel handler and asset-model name registry; a definition refused at validation must not reserve its name.
  Traces: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#task#9 (cluster also covers iotsitewise-windfarm-trajwire-property-definition-mistakes#task#8).

Cause 4 of 7: a model whose metric references a non-metric property of a child model settles ACTIVE with no error, where AWS settles it FAILED with a VALIDATION_ERROR.
  AWS: Smithy: AssetModelStatus = {state (required, AssetModelState enum), error (ErrorDetails)}; ErrorDetails = {code (required, ErrorCode enum: VALIDATION_ERROR | INTERNAL_FAILURE), message (required)}. Recorded behavior: AWS accepts the create, then DescribeAssetModel reports assetModelStatus.state 'FAILED' with assetModelStatus.error {code: 'VALIDATION_ERROR', message: 'A metric property references a non-metric property from a child model. Metrics can reference properties from the current model and metric properties from child models.'} — that message is fixed text for this rule, with no variable part.
  Emulator: DescribeAssetModel reports assetModelStatus.state 'ACTIVE' and omits assetModelStatus.error entirely; the fixture's poll for FAILED gives up because ACTIVE is a terminal fail_on value.
  Why: FAILED is how AWS reports asynchronous model validation. A client polling for a terminal state sees ACTIVE and proceeds to create assets against a model that AWS would never have activated, so downstream property reads are silently wrong instead of loudly refused.
  Fix: asset-model state machine — evaluate definition rules when the model settles and record FAILED + ErrorDetails{VALIDATION_ERROR, message} in assetModelStatus.
  Traces: iotsitewise-windfarm-trajwire-raw-measurement-rollup-fails-then-repaired#task#4.

Cause 5 of 7: DeleteAssetModel on a model still referenced by a parent model's hierarchy does not drive the model to FAILED, and does not bump assetModelVersion.
  AWS: Smithy: DeleteAssetModel is 202-accepted with required assetModelStatus; DescribeAssetModel output carries assetModelStatus and assetModelVersion (@pattern ^(0|([1-9]{1}\d*))$). Recorded behavior: after the delete is accepted, DescribeAssetModel reports assetModelStatus {state: 'FAILED', error: {code: 'VALIDATION_ERROR', message: 'Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model.'}} and assetModelVersion incremented (recorded as '2'). The message is fixed text for this rule.
  Emulator: DescribeAssetModel keeps reporting the model as it was — no assetModelStatus.error, state not FAILED, version not incremented — so the fixture's poll runs to its 60s cap.
  Why: the failure of an asynchronous delete is invisible. A client polling the model's status to learn why its delete did not take never sees the reason, and hangs or concludes the delete succeeded.
  Fix: asset-model state machine on the delete path — record the FAILED status with ErrorDetails and advance assetModelVersion.
  Traces: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#19.

Cause 6 of 7: DescribeAssetModel after an accepted DeleteAssetModel keeps returning 200 for the deleted model.
  AWS: Smithy: DescribeAssetModel errors include ResourceNotFoundException (httpError 404). Recorded behavior: once the model is deleted, DescribeAssetModel answers 404 with message 'Asset Model with ID <assetModelId> not found' — fixed text 'Asset Model with ID ' / ' not found', variable part the assetModelId from the request.
  Emulator: answers 200 with the full model body (including state ACTIVE) for the whole 60s poll window; $.message and x-amzn-errortype absent.
  Why: deletion never becomes observable. Teardown and idempotent 'delete then recreate' flows never converge, and a client cannot distinguish a live model from a deleted one.
  Fix: asset-model state/DescribeAssetModel handler — a model that completed deletion must be removed from the read path and 404 with AWS's message.
  Traces: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#teardown#14.

Cause 7 of 7: the same post-delete read divergence on the rollup fixture — DescribeAssetModel answers 200 where AWS answers 404 'Asset Model with ID <assetModelId> not found'.
  AWS: as cause 6 (DescribeAssetModel -> ResourceNotFoundException, httpError 404).
  Emulator: 200, with $.message and x-amzn-errortype missing.
  Why: as cause 6; recorded separately because it is a different fixture and a different delete path (a model whose definition had failed validation), so fixing only the teardown path can leave this one returning 200.
  Fix: same component as cause 6.
  Traces: iotsitewise-windfarm-trajwire-raw-measurement-rollup-fails-then-repaired#task#5 (expected 404, got 200).

## Evidence
Cause 1: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#task#5 — expected status 400 with $.message 'Hierarchy Turbines name is not unique.' and an x-amzn-errortype header; actual status 202, $.message and x-amzn-errortype absent.
Cause 2: ...#task#6 — expected 400 / 'Property Id ActivePowerAvg invalid for variables in Metric property.'; actual 409 / x-amzn-errortype ResourceAlreadyExistsException / 'Another asset model is already using the same name'.
Cause 3: ...#task#9 — expected 202 with assetModelId, assetModelArn, assetModelStatus {'state':'CREATING'}; actual 409 with all three members missing.
Cause 4: iotsitewise-windfarm-trajwire-raw-measurement-rollup-fails-then-repaired#task#4 — expected assetModelStatus.state 'FAILED' and assetModelStatus.error {'code':'VALIDATION_ERROR','message':'A metric property references a non-metric property from a child model. Metrics can reference properties from the current model and metric properties from child models.'}; actual state 'ACTIVE', error absent (poll aborted: 'ACTIVE' is in fail_on after 2 attempts).
Cause 5: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#19 — expected assetModelStatus {'state':'FAILED','error':{'code':'VALIDATION_ERROR','message':'Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model.'}} and assetModelVersion '2'; actual the pre-delete body with no error and the old version (poll capped at 60s).
Cause 6: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#teardown#14 — expected 404 / 'Asset Model with ID <id> not found' / ResourceNotFoundException; actual 200 with the full ACTIVE model body; poll: error code None not in ['ResourceNotFoundException'] after 13 attempts.
Cause 7: iotsitewise-windfarm-trajwire-raw-measurement-rollup-fails-then-repaired#task#5 — expected 404 / 'Asset Model with ID <id> not found'; actual 200, $.message and x-amzn-errortype missing.
Smithy grounding: CreateAssetModel http code 202 with required output assetModelId/assetModelArn/assetModelStatus and InvalidRequestException(400)/ResourceAlreadyExistsException(409); AssetModelHierarchyDefinition.name required; VariableValue.propertyId doc (name allowed only from the same asset model); AssetModelStatus{state required, error ErrorDetails}; ErrorDetails{code ErrorCode required, message required}; ErrorCode values [VALIDATION_ERROR, INTERNAL_FAILURE]; DescribeAssetModel errors include ResourceNotFoundException(404); DescribeAssetModel.assetModelVersion @pattern ^(0|([1-9]{1}\d*))$.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Cause 1: CreateAssetModel with two assetModelHierarchies entries sharing one name — fixture iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes task#5; expect 400, observe 202.
3. Cause 2/3: continue that fixture to task#6 (metric variable referencing an ineligible property) and task#9 (the corrected, legal definition); observe 409 ResourceAlreadyExistsException in both places.
4. Cause 4: replay iotsitewise-windfarm-trajwire-raw-measurement-rollup-fails-then-repaired task#4 and poll DescribeAssetModel for assetModelStatus.state; observe ACTIVE where FAILED + VALIDATION_ERROR is expected.
5. Cause 5: replay iotsitewise-windfarm-trajwire-teardown-order-mistakes to task#19 — DeleteAssetModel on a model referenced by a parent hierarchy, then poll DescribeAssetModel for the FAILED status and the bumped assetModelVersion.
6. Cause 6/7: replay iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes teardown#14 and iotsitewise-windfarm-trajwire-raw-measurement-rollup-fails-then-repaired task#5 — DescribeAssetModel after the delete; expect 404 ResourceNotFoundException, observe 200.
