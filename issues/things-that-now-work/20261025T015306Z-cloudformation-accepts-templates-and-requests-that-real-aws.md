Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.circular-dependency-rejected-state-unchanged cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle cloudformation.create-stack-exceeding-parameter-count-limit-rejected cloudformation.create-stack-macro-missing-auto-expand-capability-rejected cloudformation.update-stack-malformed-json-template-rejected cloudformation.fn-find-in-map-nested-lookup-drives-property cloudformation.find-in-map-default-value-without-transform cloudformation.foreach-outside-allowed-sections-rejected cloudformation.language-extensions-length-tojsonstring-and-dynamic-getatt cloudformation.rules-assertions-gate-stack-creation-rejected cloudformation.unresolvable-output-intrinsic-fails-the-stack-rather-than-vanishing cloudformation.condition-boolean-operators-gate-resources-and-outputs
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.circular-dependency-rejected-state-unchanged|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle|run#11|AssertionError|assert refusal["when"] == "create-call", (
Issue-Key: cloudformation.create-stack-exceeding-parameter-count-limit-rejected|_error#4|AssertionError|raise AssertionError(f"expected {description} to fail, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.create-stack-macro-missing-auto-expand-capability-rejected|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: cloudformation.update-stack-malformed-json-template-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: cloudformation.fn-find-in-map-nested-lookup-drives-property|_error#15|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.find-in-map-default-value-without-transform|run#8|AssertionError|assert created["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation.foreach-outside-allowed-sections-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation.language-extensions-length-tojsonstring-and-dynamic-getatt|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation.rules-assertions-gate-stack-creation-rejected|_assert_rolled_back#8|AssertionError|assert final["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.unresolvable-output-intrinsic-fails-the-stack-rather-than-vanishing|_client_error#13|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.condition-boolean-operators-gate-resources-and-outputs|_error#23|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation accepts templates and requests that real AWS refuses: cycles, parameter-count limit, CAPABILITY_AUTO_EXPAND, malformed JSON, Fn::FindInMap, Fn::ForEach, LanguageExtensions, Rules, Conditions, unresolvable Outputs (10 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
This issue bundles 10 net-new root causes in CloudFormation's synchronous template/request validation. The open issue 20261022T043551Z-cloudformation-stack-mutation-guards-not-enforced-capabilities-client.md describes the same family for other operations; it does not carry the replay keys below.

Common shape: real AWS refuses these calls synchronously with a 400 ValidationError (or InsufficientCapabilitiesException), leaving no stack behind. The emulator returns HTTP 200 with a StackId, so the caller believes the template was accepted. Each cause below is a distinct validation rule that is missing.

Cause 1 of 10: a template with a resource dependency cycle is accepted.
AWS: CreateStack refuses the template up front with ValidationError ('Circular dependency between resources'); no stack is created.
Emulator: HTTP 200 + StackId; the cycle only surfaces later as a resource-level CREATE_FAILED ('Value null at value failed to satisfy constraint: Member must not be null'), i.e. a different and misleading failure.
Why: callers that treat a 200 as "template valid" deploy a template AWS would never accept; the refusal is also supposed to leave state unchanged.
Fix: a dependency-graph cycle check in CreateStack/UpdateStack template validation.
Traces: cloudformation.circular-dependency-rejected-state-unchanged, cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle.

Cause 2 of 10: the 200-parameter template limit is not enforced.
AWS: a template declaring more than 200 Parameters is refused with ValidationError; Smithy models LimitExceededException on CreateStack for account/template limits.
Emulator: CreateStack with 201 parameters succeeds with a StackId.
Fix: template-limit validation in CreateStack.
Traces: cloudformation.create-stack-exceeding-parameter-count-limit-rejected.

Cause 3 of 10: CAPABILITY_AUTO_EXPAND is not required for macro/transform templates.
AWS: Smithy: CreateStack.Capabilities (enum Capability = CAPABILITY_IAM | CAPABILITY_NAMED_IAM | CAPABILITY_AUTO_EXPAND) and the modeled error InsufficientCapabilitiesException (client, HTTP 400). A template declaring a macro without CAPABILITY_AUTO_EXPAND must be refused.
Emulator: accepted, 200 + StackId.
Fix: capability gating in CreateStack/UpdateStack/CreateChangeSet.
Traces: cloudformation.create-stack-macro-missing-auto-expand-capability-rejected.

Cause 4 of 10: UpdateStack accepts a malformed JSON template body.
AWS: ValidationError 'Template format error: ...'; the update never starts.
Emulator: 200 + StackId.
Fix: parse/validate TemplateBody in UpdateStack the way CreateStack does.
Traces: cloudformation.update-stack-malformed-json-template-rejected.

Cause 5 of 10: Fn::FindInMap is mis-evaluated in both directions.
AWS: a Fn::FindInMap whose top-level/second-level key does not exist is refused with ValidationError; a Fn::FindInMap carrying the DefaultValue option resolves to that default and the stack creates (DefaultValue needs no transform).
Emulator: the invalid lookup is accepted (200 + StackId), and the valid DefaultValue form fails the resource with CREATE_FAILED ('Value null at value failed to satisfy constraint: Member must not be null').
Why: mappings silently resolve to null, which then corrupts the created resource's properties.
Fix: the Fn::FindInMap intrinsic evaluator and its validation.
Traces: cloudformation.fn-find-in-map-nested-lookup-drives-property, cloudformation.find-in-map-default-value-without-transform.

Cause 6 of 10: Fn::ForEach used outside the sections AWS allows is accepted.
AWS: ValidationError refusing the Fn::ForEach in a disallowed section (e.g. Parameters).
Emulator: 200 + StackId.
Fix: AWS::LanguageExtensions transform validation.
Traces: cloudformation.foreach-outside-allowed-sections-rejected.

Cause 7 of 10: AWS::LanguageExtensions intrinsics are accepted in a template that does not declare the transform.
AWS: Fn::Length / Fn::ToJsonString / dynamic Fn::GetAtt without 'Transform: AWS::LanguageExtensions' are refused with ValidationError.
Emulator: accepted, 200 + StackId.
Fix: intrinsic allow-list keyed on the declared transforms.
Traces: cloudformation.language-extensions-length-tojsonstring-and-dynamic-getatt.

Cause 8 of 10: the Rules section is never evaluated.
AWS: a failing Fn::Assert in Rules refuses the create (the stack never leaves ROLLBACK/refusal); the assertion description is reported.
Emulator: the stack reaches CREATE_COMPLETE with no reason recorded.
Fix: Rules evaluation during template processing.
Traces: cloudformation.rules-assertions-gate-stack-creation-rejected.

Cause 9 of 10: an Output whose Value is an unresolvable intrinsic is accepted.
AWS: the call is refused with ValidationError rather than the Output silently vanishing.
Emulator: 200 + StackId and the Output disappears.
Fix: Outputs intrinsic resolution must fail the request, not drop the entry.
Traces: cloudformation.unresolvable-output-intrinsic-fails-the-stack-rather-than-vanishing.

Cause 10 of 10: invalid Conditions (boolean operator) usage is accepted.
AWS: a malformed Fn::And/Fn::Or/Fn::Not condition, or a condition referenced where AWS forbids it, is refused with ValidationError; the gated resources and Outputs are then evaluated from the condition result.
Emulator: the call succeeds with a StackId.
Fix: Conditions parsing/validation and condition-gated resource/Output evaluation.
Traces: cloudformation.condition-boolean-operators-gate-resources-and-outputs.

## Evidence
Replayed suite, service cloudformation. Every cause fails the same way — the harness expected a ClientError and got an HTTP 200 success:
- circular-dependency-rejected-state-unchanged: "expected the call to be rejected, but it succeeded and returned {'StackId': 'arn:aws:cloudformation:...', 'HTTPStatusCode': 200 ...}".
- cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle: "CreateStack accepted the cyclic template instead of refusing it synchronously; full refusal {'when': 'rollback', 'code': 'CREATE_FAILED', ...}".
- create-stack-exceeding-parameter-count-limit-rejected: "expected CreateStack with 201 parameters to fail, but it SUCCEEDED".
- create-stack-macro-missing-auto-expand-capability-rejected: "expected the call to be rejected, but it succeeded".
- update-stack-malformed-json-template-rejected: "expected the call to be refused, but it succeeded".
- fn-find-in-map-nested-lookup-drives-property: "expected a ClientError, but the call SUCCEEDED"; find-in-map-default-value-without-transform: "DefaultValue without the transform must create; got 'CREATE_FAILED'".
- foreach-outside-allowed-sections-rejected / language-extensions-length-tojsonstring-and-dynamic-getatt: "expected the call to be refused, but it succeeded".
- rules-assertions-gate-stack-creation-rejected: "a failing rule assertion must roll the create back; got 'CREATE_COMPLETE'".
- unresolvable-output-intrinsic-fails-the-stack-rather-than-vanishing: "real AWS rejects this call, but it SUCCEEDED".
- condition-boolean-operators-gate-resources-and-outputs: "expected the call to be rejected, but it succeeded".

## Steps to Reproduce
1. Boot the emulator and replay the suite; re-run the check ids in trace_ids.
2. Manual repro (cause 1): CreateStack with two resources that Ref each other — AWS answers 400 ValidationError, the emulator answers 200 with a StackId.
3. Manual repro (cause 3): CreateStack with a template declaring 'Transform: MyMacro' and no Capabilities — AWS answers InsufficientCapabilitiesException (400); the emulator creates the stack.
4. Manual repro (cause 2): CreateStack with 201 Parameters — AWS refuses, the emulator returns a StackId.
