Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.alias-create-describe-update-delete-lifecycle stepfunctions.alias-delete-then-describe-not-found stepfunctions.sm-alias-create-describe-delete
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.alias-create-describe-update-delete-lifecycle|run#31|AssertionError|assert mutate_err["Message"] == expected_message, f'expected the message {expected_message!r}, got {mutate_err["Message"]!r}'
Issue-Key: stepfunctions.alias-delete-then-describe-not-found|run#23|AssertionError|assert update_err == expected, (
Issue-Key: stepfunctions.sm-alias-create-describe-delete|run#18|AssertionError|assert update_error.get("Message") == expected_update, (
Issue-Commit: a28b00726e6917844828701198c7c2f9a84a8d63

# UpdateStateMachineAlias on a deleted alias returns the alias-read ResourceNotFound message instead of AWS's "Request references a resource that does not exist."

> _Note: this issue was identified against the code base at commit `a28b00726e6917844828701198c7c2f9a84a8d63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStateMachineAlias

## Description
This issue carries ONE net-new root cause in the Step Functions alias control plane. It must be closed before submitting; a fixer should re-replay every check id in trace_ids (sfn.alias-create-describe-update-delete-lifecycle, sfn.alias-delete-then-describe-not-found, sfn.sm-alias-create-describe-delete). All three checks were left UNMARKED by the replay report (no ALREADY-FILED and no OWNED marker), so no open issue carries their replay keys.

=== Cause 1 of 1: the ResourceNotFound refusal of the alias MUTATION path (UpdateStateMachineAlias) is rendered with the alias READ path's message, which interpolates the requested alias ARN, instead of AWS's fixed sentence ===
Failing check ids: sfn.alias-create-describe-update-delete-lifecycle, sfn.alias-delete-then-describe-not-found, sfn.sm-alias-create-describe-delete.

AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json) models UpdateStateMachineAlias with input {stateMachineAliasArn (required, Arn, @length 1..256), description, routingConfiguration (@length 1..2)}, output {updateDate (required)} and errors ConflictException (409), InvalidArn (400), ResourceNotFound (@httpError 404 in the model, 400 on the wire for this awsJson1_0 service), StateMachineDeleting (400) and ValidationException (400). The model does not state the message text, so this is recorded behavior: AWS's two alias surfaces word the SAME ResourceNotFound error class DIFFERENTLY, and the three checks grade both wordings in the same run.
  * The READ path — DescribeStateMachineAlias on an alias that has been deleted — answers HTTP 400, code ResourceNotFound, message `Resource not found: '<stateMachineAliasArn>'`. Fixed text: `Resource not found: '` and the closing `'`; variable part: the alias ARN exactly as supplied in the request. The emulator already matches this (the Describe assertions pass in all three checks).
  * The MUTATION path — UpdateStateMachineAlias on that same deleted alias — answers HTTP 400, code ResourceNotFound, message `Request references a resource that does not exist.` This message is ENTIRELY fixed text: it has NO variable part, and in particular it must not name the alias ARN, the state machine, the alias name or any other request value.
So the requirement graded here is: UpdateStateMachineAlias(stateMachineAliasArn=<deleted alias>) -> HTTP 400, error code ResourceNotFound, message exactly `Request references a resource that does not exist.`

Emulator: status (400) and code (ResourceNotFound) are correct, but the message is the alias-read rendering — the emulator emits `Resource not found: '<alias arn>'` for the mutation too. Replay evidence, from three independent checks: got `"Resource not found: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-behav-sm-c340253559f5:sim2real-behav-alias-lifecycle-eb6d22b819e5'"`, `"Resource not found: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-behav-sm-187de7c6f6e7:sim2real-behav-alias-gone-c7b36db6a229'"` and `"Resource not found: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-sfn-87d87b88d840:sim2real-alias-908b43f1b1f8'"`. The ARNs, state-machine names and alias names in those strings are each run's generated values and are not part of any requirement; the defect is that the mutation path uses the read path's template at all, and therefore interpolates a request value into a message AWS renders with none.

Why: a client cannot distinguish the two refusals, and anything that pins AWS's documented sentence never matches. Concretely: deploy tooling that re-points an alias (the blue/green and weighted-rollout pattern, e.g. Terraform's aws_sfn_alias update path) logs and surfaces a message that does not exist on AWS; parity/assertion harnesses and support runbooks keyed on `Request references a resource that does not exist.` fail against the emulator even though code and status agree; and operators are shown an ARN-specific "resource not found" where AWS deliberately returns a request-independent sentence, which hides that the whole UPDATE request — not just one looked-up ARN — was rejected.

Fix: the ResourceNotFound construction in the alias MUTATION handlers (UpdateStateMachineAlias; the same alias-mutation refusal family also covers DeleteStateMachineAlias, which shares the ResourceNotFound error per the model) in the sfn alias handler/validation layer. It must emit AWS's fixed sentence with no interpolation, while leaving the alias READ path's `Resource not found: '<arn>'` rendering (DescribeStateMachineAlias) untouched — the two wordings are both required and must not be unified.

Related but NOT covering this: open issue 20261025T131505Z-sfn-step-functions-alias-operations-resourcenotfound-returned-as-http.md reports the READ path's status/wording and an UpdateStateMachineAlias error-CODE misclassification for a foreign-version re-point — it describes neither the mutation path's message template nor these replay keys (the lifecycle check now fails at a different assertion). Fixed issue 20261024T215142Z-sfn-startexecution-does-not-accept-state-machine-alias.md addressed CreateStateMachineAlias's ResourceNotFound message for a routingConfiguration naming a non-existent VERSION — a different operation and a different refusal — so this is not a reopening of it.

Traces: sfn.alias-create-describe-update-delete-lifecycle (assertion at check_sfn_alias_create_describe_update_delete_lifecycle.py line 227), sfn.alias-delete-then-describe-not-found (line 185), sfn.sm-alias-create-describe-delete (line 65).

## Evidence
Cause 1 — three mismatch verdicts, same divergence.

Expected (all three checks): after CreateStateMachineAlias + DeleteStateMachineAlias, UpdateStateMachineAlias(stateMachineAliasArn=<deleted alias arn>, routingConfiguration=[{stateMachineVersionArn: <v1>, weight: 100}]) -> HTTP 400, Error.Code `ResourceNotFound`, Error.Message exactly `Request references a resource that does not exist.` (wholly fixed text, no variable parts).

Actual:
1) sfn.alias-create-describe-update-delete-lifecycle — File "check_sfn_alias_create_describe_update_delete_lifecycle.py", line 227, in run / `assert mutate_err["Message"] == expected_message` / AssertionError: expected the message 'Request references a resource that does not exist.', got "Resource not found: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-behav-sm-c340253559f5:sim2real-behav-alias-lifecycle-eb6d22b819e5'".
2) sfn.alias-delete-then-describe-not-found — File "check_sfn_alias_delete_then_describe_not_found.py", line 185, in run / `assert update_err == expected` / AssertionError: UpdateStateMachineAlias on a deleted alias: expected {'Code': 'ResourceNotFound', 'Message': 'Request references a resource that does not exist.'}, got {'Code': 'ResourceNotFound', 'Message': "Resource not found: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-behav-sm-187de7c6f6e7:sim2real-behav-alias-gone-c7b36db6a229'"}.
3) sfn.sm-alias-create-describe-delete — File "check_sfn_sm_alias_create_describe_delete.py", line 65, in run / `assert update_error.get("Message") == expected_update` / AssertionError: expected ResourceNotFound message 'Request references a resource that does not exist.', got "Resource not found: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-sfn-87d87b88d840:sim2real-alias-908b43f1b1f8'".

Control observation in the SAME runs: the preceding DescribeStateMachineAlias assertion on the same deleted alias PASSES — it expects `Resource not found: '<alias arn>'` with HTTP 400 / ResourceNotFound and the emulator returns exactly that. So status and code are right everywhere; the divergence is scoped to the message the MUTATION path renders.

Model: UpdateStateMachineAlias errors = ConflictException(409), InvalidArn(400), ResourceNotFound(404 modeled / 400 on the wire), StateMachineDeleting(400), ValidationException(400); DeleteStateMachineAlias and DescribeStateMachineAlias bind the same ResourceNotFound shape.

## Steps to Reproduce
1. iam:CreateRole with a trust policy allowing sts:AssumeRole for Service states.amazonaws.com.
2. sfn:CreateStateMachine(name=<sm>, definition=<any valid ASL, e.g. a single Pass state with End:true>, roleArn=<role>, type=STANDARD) -> stateMachineArn.
3. sfn:PublishStateMachineVersion(stateMachineArn=<sm>) -> stateMachineVersionArn v1.
4. sfn:CreateStateMachineAlias(name=<alias>, routingConfiguration=[{stateMachineVersionArn: v1, weight: 100}]) -> stateMachineAliasArn.
5. sfn:DescribeStateMachineAlias(stateMachineAliasArn=<alias arn>) -> 200, confirming the alias exists.
6. sfn:DeleteStateMachineAlias(stateMachineAliasArn=<alias arn>) -> 200.
7. Poll sfn:DescribeStateMachineAlias(stateMachineAliasArn=<alias arn>) until it is refused (the delete is eventually consistent). Confirm the READ refusal is HTTP 400 / ResourceNotFound / `Resource not found: '<alias arn>'` — this is correct today and must stay correct.
8. sfn:UpdateStateMachineAlias(stateMachineAliasArn=<alias arn>, routingConfiguration=[{stateMachineVersionArn: v1, weight: 100}]).
   Expected: HTTP 400, code ResourceNotFound, message exactly `Request references a resource that does not exist.`
   Observed: HTTP 400, code ResourceNotFound, message `Resource not found: '<alias arn>'` — the read path's template, with the requested alias ARN interpolated.
Replay: sfn.alias-create-describe-update-delete-lifecycle, sfn.alias-delete-then-describe-not-found, sfn.sm-alias-create-describe-delete.
