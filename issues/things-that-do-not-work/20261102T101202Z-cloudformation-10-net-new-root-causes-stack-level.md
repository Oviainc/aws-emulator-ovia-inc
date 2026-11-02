Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cfn-topstacks-duplicate-stack-name-rejected-leaving-the-original cloudformation.parameter-constraint-description-on-number-and-allowed-values cloudformation.parameter-constraint-description-replaces-pattern-message cloudformation.stack-create-resource-types-excluding-template-type-rejected cloudformation.stack-create-resource-types-with-capabilities cloudformation.stack-update-resource-types-excluding-added-type-rejected cloudformation.stack-update-disable-validation-defers-property-error-to-provisioning cloudformation.template-body-size-boundary-create-and-update-stack cloudformation.create-stack-instances-duplicate-operation-id cloudformation.cfn-topstacks-list-stack-resources-forged-next-token-rejected cloudformation.cfn-topstacks-list-stacks-unknown-status-filter-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cfn-topstacks-duplicate-stack-name-rejected-leaving-the-original|run#6|AssertionError|assert code == "AlreadyExistsException", f"a duplicate name must raise AlreadyExistsException; got {code!r}"
Issue-Key: cloudformation.parameter-constraint-description-on-number-and-allowed-values|_refused#8|AssertionError|raise AssertionError(f"CreateStack with {params!r} was accepted as {stack_id}")
Issue-Key: cloudformation.parameter-constraint-description-replaces-pattern-message|_refused#7|AssertionError|raise AssertionError(f"CreateStack with {params!r} was accepted as {stack_id}")
Issue-Key: cloudformation.stack-create-resource-types-excluding-template-type-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation.stack-create-resource-types-with-capabilities|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation.stack-update-resource-types-excluding-added-type-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation.stack-update-disable-validation-defers-property-error-to-provisioning|run#10|AssertionError|assert _of(validated, "Param") == [], f"validation must stop the update before Param is provisioned: {validated!r}"
Issue-Key: cloudformation.template-body-size-boundary-create-and-update-stack|run#13|AssertionError|assert (code, msg) == ("ValidationError", _length_message(over_new)), (
Issue-Key: cloudformation.create-stack-instances-duplicate-operation-id|run#12|AssertionError|assert code == "OperationIdAlreadyExistsException", f"expected OperationIdAlreadyExistsException, got {code!r}"
Issue-Key: cloudformation.cfn-topstacks-list-stack-resources-forged-next-token-rejected|run#17|AssertionError|assert cross.get("Message") == "Hash mismatch.", (
Issue-Key: cloudformation.cfn-topstacks-list-stacks-unknown-status-filter-rejected|run#5|AssertionError|assert got == ("ValidationError", _ACTIVE_FILTER_MESSAGE), (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation: 10 net-new root causes — stack-level validation and refusal taxonomy (duplicate names, parameter constraints, ResourceTypes, NextToken, enum filters)

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Ten distinct net-new root causes in CloudFormation request validation and refusal classification. Open issue 20261029T214321Z-cloudformation-template-and-parameter-validation-is-not-enforced.md names a neighbouring symptom but carries none of the replay keys below.

Cause 1 of 10: a duplicate stack name is refused with the wrong error.
AWS: Smithy: CreateStack models AlreadyExistsException (httpError 400, awsQueryError AlreadyExistsException) — creating a stack whose name is already in use raises AlreadyExistsException and leaves the original stack untouched.
Emulator: the refusal is InsufficientCapabilitiesException.
Why: a client's 'already exists -> adopt/update' branch never fires; a capabilities error suggests a template problem instead.
Fix: duplicate-name detection in the CreateStack handler's error classification.
Traces: cloudformation.cfn-topstacks-duplicate-stack-name-rejected-leaving-the-original.

Cause 2 of 10: numeric and AllowedValues parameter constraints are not enforced.
AWS: recorded behavior only — CreateStack rejects a parameter value outside a Number parameter's MinValue/MaxValue or outside AllowedValues with ValidationError before any resource is provisioned.
Emulator: CreateStack accepts Size=1/Count=8/Env=dev/Tier=gold against those constraints and returns a StackId.
Why: a template's own input contract is unenforceable, and bad values reach resources.
Fix: parameter validation in the CreateStack/UpdateStack handler.
Traces: cloudformation.parameter-constraint-description-on-number-and-allowed-values.

Cause 3 of 10: AllowedPattern/ConstraintDescription is not enforced and never surfaces.
AWS: recorded behavior only — a value violating AllowedPattern is refused with ValidationError whose message is the parameter's ConstraintDescription in place of the default pattern text.
Emulator: CreateStack accepts Described='Bad1' and returns a StackId; no refusal and no ConstraintDescription anywhere.
Why: template authors lose both the guard and its documented message.
Fix: parameter validation + message selection in the CreateStack handler.
Traces: cloudformation.parameter-constraint-description-replaces-pattern-message.

Cause 4 of 10: the ResourceTypes allow-list is not enforced on CreateStack.
AWS: Smithy: CreateStack.ResourceTypes is the list of types the stack may create; a template type outside the list is refused (InsufficientCapabilitiesException/ValidationError) before provisioning, including when Capabilities are also supplied.
Emulator: both calls return 200 with a StackId.
Why: the allow-list a caller uses to bound a deploy is inert.
Fix: ResourceTypes enforcement in the CreateStack handler's validation.
Traces: cloudformation.stack-create-resource-types-excluding-template-type-rejected, cloudformation.stack-create-resource-types-with-capabilities.

Cause 5 of 10: the ResourceTypes allow-list is not enforced on UpdateStack.
AWS: as above for UpdateStack: adding a resource of a type absent from ResourceTypes is refused.
Emulator: the update returns 200 with a StackId/OperationId.
Why: same inert guard on the update path.
Fix: ResourceTypes enforcement in the UpdateStack handler.
Traces: cloudformation.stack-update-resource-types-excluding-added-type-rejected.

Cause 6 of 10: pre-provision property validation does not happen (DisableValidation default).
AWS: recorded behavior only — with validation enabled, an invalid resource property stops the update before the resource is provisioned, so no CREATE_IN_PROGRESS event for it is emitted.
Emulator: the resource is provisioned and then fails, producing ('Param','CREATE_IN_PROGRESS') followed by ('Param','CREATE_FAILED','Resource handler returned message: "1 validation error detected: Value \'NotAType\' at \'type\' failed to satisfy constraint: ..."') and a stack UPDATE_FAILED/UPDATE_ROLLBACK cycle.
Why: a client that relies on up-front validation pays for a partially applied, rolled-back update instead of a cheap refusal.
Fix: the template/property validation stage ahead of the provisioner.
Traces: cloudformation.stack-update-disable-validation-defers-property-error-to-provisioning.

Cause 7 of 10: the inline template-size refusal message is wrong at the boundary.
AWS: recorded behavior only — a TemplateBody over the 51200-byte inline limit is refused with ValidationError whose message quotes the offending value: "1 validation error detected: Value '<first chars of the template>...<N chars total>' at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200".
Emulator: the message omits the quoted value: "1 validation error detected: Value at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200". The fixed text is the constraint sentence; the variable parts are the template prefix and its total length.
Why: a client that parses the offending value out of the message gets nothing.
Fix: the constraint-violation message formatter used by CreateStack/UpdateStack.
Traces: cloudformation.template-body-size-boundary-create-and-update-stack.

Cause 8 of 10: a duplicate StackSet OperationId is refused with the wrong error.
AWS: Smithy: CreateStackInstances models both OperationIdAlreadyExistsException (httpError 409) and OperationInProgressException (409); reusing an OperationId that already exists raises OperationIdAlreadyExistsException.
Emulator: the refusal is OperationInProgressException.
Why: a retry loop that treats OperationInProgress as 'wait and retry' spins forever instead of recognising a duplicate submission.
Fix: operation-id bookkeeping/error classification in the CreateStackInstances handler.
Traces: cloudformation.create-stack-instances-duplicate-operation-id.

Cause 9 of 10: ListStackResources NextToken is not stack-scoped, so a foreign token is indistinguishable from a corrupt one.
AWS: recorded behavior only — a genuine pagination token carries a stack-scoped checksum, so presenting another stack's token fails distinctly from presenting an unparsable one.
Emulator: both answer the same ValidationError '<hex token> is not a valid NextToken'.
Why: a client that pages across stacks silently crosses streams instead of being told the token belongs elsewhere.
Fix: NextToken minting/validation in the ListStackResources handler.
Traces: cloudformation.cfn-topstacks-list-stack-resources-forged-next-token-rejected.

Cause 10 of 10: the ListStacks StackStatusFilter enum refusal message does not match AWS.
AWS: recorded behavior only — an unknown status value is refused with ValidationError naming the offending value, its member path and the enum value set AWS publishes for StackStatus.
Emulator: ValidationError "1 validation error detected: Value 'ACTIVE' at 'stackStatusFilter.1.member' failed to satisfy constraint: Member must satisfy enum value set: [CREATE_IN_PROGRESS, ... IMPORT_ROLLBACK_COMPLETE]" — the set and/or member path differ from the recorded refusal.
Why: clients matching the published enum text do not recognise the refusal.
Fix: the enum-constraint message for ListStacks.StackStatusFilter.
Traces: cloudformation.cfn-topstacks-list-stacks-unknown-status-filter-rejected.

## Evidence
C1: "a duplicate name must raise AlreadyExistsException; got 'InsufficientCapabilitiesException'".
C2: 'CreateStack with [Size=1, Count=8, Env=dev, Tier=gold] was accepted as arn:aws:cloudformation:...:stack/<name>/<uuid>'.
C3: 'CreateStack with [Described=Bad1, Plain=fine] was accepted as arn:...:stack/<name>/<uuid>'.
C4/C5: 'expected the call to be refused, but it succeeded: {StackId: ..., OperationId: ..., HTTPStatusCode: 200}'.
C6: event list shows ('Param','CREATE_IN_PROGRESS',None) before ('Param','CREATE_FAILED', 'Resource handler returned message: "1 validation error detected: Value \'NotAType\' at \'type\' ..."').
C7: got ValidationError "1 validation error detected: Value at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200" for a 51201-byte body; expected the same sentence with the value quoted.
C8: "expected OperationIdAlreadyExistsException, got 'OperationInProgressException'".
C9: got {'Type':'Sender','Code':'ValidationError','Message':'7b2263...3130307d is not a valid NextToken'} for a foreign stack's token.
C10: got ('ValidationError', "1 validation error detected: Value 'ACTIVE' at 'stackStatusFilter.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]") where the recorded refusal differs.
Per-run names/ids/uuids above are template fillers, not requirements.

## Steps to Reproduce
Replay the checks in trace_ids: 1) CreateStack twice with the same StackName; 2) CreateStack with a Number parameter outside MinValue/MaxValue and a value outside AllowedValues; 3) CreateStack with a value violating AllowedPattern on a parameter declaring ConstraintDescription; 4) CreateStack with ResourceTypes excluding a template type, with and without Capabilities; 5) UpdateStack adding a resource whose type is not in ResourceTypes; 6) UpdateStack introducing an invalid resource property and read DescribeStackEvents; 7) CreateStack with a 51201-byte TemplateBody; 8) CreateStackInstances twice with the same OperationId; 9) ListStackResources with another stack's NextToken and with an unparsable one; 10) ListStacks with StackStatusFilter=['ACTIVE'].
