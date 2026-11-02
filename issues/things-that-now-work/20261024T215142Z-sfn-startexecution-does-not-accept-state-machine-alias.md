Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.alias-repoint-to-foreign-state-machine-version-rejected stepfunctions.e2e-alias-weighted-routing-traffic-split-observed stepfunctions.alias-routing-config-invalid-version-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: sfn|StartExecution|StateMachineDoesNotExist
Issue-Key: stepfunctions.alias-routing-config-invalid-version-rejected|run#14|AssertionError|assert err == expected, f"expected {expected}, got {err}"
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# sfn: StartExecution does not accept state machine ALIAS ARNs (StateMachineDoesNotExist), and CreateStateMachineAlias' ResourceNotFound carries a generic lookup message (2 causes)

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two distinct root causes in the Step Functions alias surface. No open issue covers either: the open sfn issues cover the interpreter (20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md), the control plane's IAM/PassRole gap (20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md), the ASL validator (20261024T114502Z-sfn-step-functions-asl-definition-validator-accepts-invalid-definitions.md) and activities/TestState (20261024T190039Z-sfn-step-functions-activities-and-teststate-diverge-task-states.md); none of them mentions alias resolution on the data plane. The unimplemented-stub issues for ListStateMachineAliases/UpdateStateMachineAlias are about those operations answering NotImplemented, not about StartExecution refusing an alias ARN that CreateStateMachineAlias itself just returned.

Cause 1 of 2: StartExecution refuses a qualified alias ARN.
AWS: Smithy (sfn model, StartExecutionInput.stateMachineArn documentation) states the member 'accepts one of the following inputs: an unqualified state machine ARN; a state machine version ARN (<stateMachineArn>:<version>); a state machine ALIAS ARN (<stateMachineArn>:<aliasName>)', and that 'Step Functions associates executions that you start with an alias ARN with that alias and the state machine version used for that execution'. So StartExecution on an alias ARN must succeed, route to one of the versions in the alias' routingConfiguration in proportion to its weights, and the resulting execution must report stateMachineAliasArn and the resolved stateMachineVersionArn on DescribeExecution. StateMachineDoesNotExist (modeled on StartExecution, HTTP 400) is for a state machine that does not exist.
Emulator: CreateStateMachineAlias succeeds and returns the alias ARN '<stateMachineArn>:<aliasName>', but StartExecution with that same ARN is refused with HTTP 400 StateMachineDoesNotExist, message e.g. "State Machine Does Not Exist: 'arn:aws:states:<region>:<account>:stateMachine:<name>:live'" (recorded example). Nothing is started, so no execution exists to describe and no alias/version attribution can be read back.
Why: every alias-based deployment pattern breaks on the emulator. A client that publishes versions, creates a PROD/live alias and then starts executions through the alias (the standard blue/green and weighted-rollout pattern, and what Terraform's aws_sfn_alias produces) gets a hard 400 for a resource the emulator itself just created, instead of a routed execution. Weighted routing, alias attribution (stateMachineAliasArn/stateMachineVersionArn on DescribeExecution) and any downstream effect-absent check built on an alias execution are all unobservable.
Fix: the StartExecution handler's ARN resolution, plus the alias/version lookup in the sfn state layer: a qualified ARN whose suffix names an alias must resolve through that alias' routingConfiguration (weighted pick) and the execution record must carry the alias and the resolved version ARN.
Traces: sfn.alias-repoint-to-foreign-state-machine-version-rejected (StartExecution through the alias after create, before the rejection under test is even reached), sfn.e2e-alias-weighted-routing-traffic-split-observed (25 StartExecution calls on a 20/80 alias). The same cause is reported separately for the cloudwatch__sfn service token (cloudwatch__sfn.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version).

Cause 2 of 2: CreateStateMachineAlias' ResourceNotFound for a routingConfiguration version that does not exist carries a generic resource-lookup message instead of the alias-validation message.
AWS: Smithy models ResourceNotFound (HTTP 404, doc 'Could not find the referenced resource.') on CreateStateMachineAlias; the recorded behavior for a routingConfiguration entry naming a never-published version ARN is ResourceNotFound with the fixed, request-independent validation sentence 'Routing configuration must contain state machine version ARNs that already exist.' That sentence is the service's message for this specific rule (routingConfiguration members must reference existing versions), not a per-request rendering.
Emulator: same code (ResourceNotFound) but the message is the generic lookup text, recorded example 'could not find the resource arn:aws:states:us-west-2:000000000000:stateMachine:<name>:2' — i.e. the input is validated by a generic ARN lookup rather than by the routingConfiguration rule.
Why: the code is right, so this is the lesser half of the divergence; a client (or a human reading a deploy failure) is told a resource is missing without being told that the alias' routing configuration is what is being rejected, and parity harnesses that pin AWS' validation sentence diverge. Filed at LOW on its own merits.
Fix: the CreateStateMachineAlias handler's routingConfiguration validation — emit the alias-specific ResourceNotFound for a non-existent version ARN rather than falling through to the generic resource-lookup error.
Traces: sfn.alias-routing-config-invalid-version-rejected (the positive control alias on the real version succeeds in the same run, so this is specific to the bogus version).

## Evidence
Cause 1 — sfn.e2e-alias-weighted-routing-traffic-split-observed:
  expected: start_execution(stateMachineArn=<alias arn>) succeeds for 25 calls, each DescribeExecution reporting stateMachineVersionArn in {v1,v2} with a ~20/80 split.
  actual:   botocore.errorfactory.StateMachineDoesNotExist: An error occurred (StateMachineDoesNotExist) when calling the StartExecution operation: State Machine Does Not Exist: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-sfn-sm-72c5772a7bd5:sim2real-sfn-alias-167901efd0a9' — on the first call, from the ARN CreateStateMachineAlias returned.
sfn.alias-repoint-to-foreign-state-machine-version-rejected:
  actual:   StateMachineDoesNotExist on StartExecution for 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-sfn-alias-neg-a-3a2b403b46b2:live'.
Model: StartExecutionInput.stateMachineArn documentation enumerates unqualified / version / ALIAS ARN as accepted inputs.

Cause 2 — sfn.alias-routing-config-invalid-version-rejected:
  expected: {'Code': 'ResourceNotFound', 'Message': 'Routing configuration must contain state machine version ARNs that already exist.'}
  actual:   {'Code': 'ResourceNotFound', 'Message': 'could not find the resource arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-sfn-f92423a7918d:2'}
  (status/code agree; the message is the divergence.)

## Steps to Reproduce
Cause 1: create a STANDARD state machine, publish version 1, update+publish version 2, then CreateStateMachineAlias(name='live', routingConfiguration=[{v1,50},{v2,50}]). Take the returned stateMachineAliasArn and call StartExecution(stateMachineArn=<that alias arn>, input='{}'). Expected: an executionArn, and DescribeExecution reporting stateMachineAliasArn plus the routed stateMachineVersionArn. Observed: HTTP 400 StateMachineDoesNotExist.
Cause 2: create a state machine, PublishStateMachineVersion once (version 1), then CreateStateMachineAlias with routingConfiguration=[{stateMachineVersionArn:'<smArn>:2', weight:100}] (never published). Expected ResourceNotFound with 'Routing configuration must contain state machine version ARNs that already exist.'; observed ResourceNotFound with 'could not find the resource <arn>'.
