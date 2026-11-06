Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update
Issue-Key: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event|run#21|AssertionError|assert rc == 0, f"the deploy exited {rc}; output tail {out[-800:]!r}"
Issue-Key: cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update|run#15|AssertionError|assert rc == 0, f"the first `cdk deploy` exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 81fe993c0fc493a00f42b69c764d54e4324ad80b

# cloudformation__sns__sqs: `cdk deploy` of a new stack dies on GetTemplate — a CREATE change set's stack is still REVIEW_IN_PROGRESS with no template, so the read is refused 400 ValidationError

> _Note: this issue was identified against the code base at commit `81fe993c0fc493a00f42b69c764d54e4324ad80b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplate

## Description
REOPENS 20261022T043551Z-cloudformation-change-sets-never-materialize-their-stack-review.md: its fix left the call refused. That issue's Cause 2 was exactly "GetTemplate on the change-set-created stack fails" (400 instead of 200 with the change set's template body); the stack record now exists, but the GetTemplate read inside a real `cdk deploy` is still answered with a 400 ValidationError instead of the template.

Cause 1 of 1: GetTemplate on a stack minted by CreateChangeSet(ChangeSetType=CREATE) is refused with ValidationError "Template does not exist for stack [<stack-name>] in REVIEW_IN_PROGRESS state" during the deploy, which aborts `cdk deploy` with exit code 1. Failing checks: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event, cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update.

AWS: a `cdk deploy` of a stack that does not yet exist is a change-set deploy — CreateChangeSet(ChangeSetType=CREATE) mints the stack in REVIEW_IN_PROGRESS, ExecuteChangeSet applies the change set's stored template and drives the stack CREATE_IN_PROGRESS -> CREATE_COMPLETE, and the CLI's template read over that stack is answered, so the deploy exits 0 and the stack is afterwards describable with its NotificationARNs, Outputs and the full event stream. Smithy (GetTemplate, services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): "Returns the template body for a specified stack. You can get the template for running or deleted stacks. ... If the template doesn't exist, a ValidationError is returned"; output StagesAvailable + TemplateBody, modelled error ChangeSetNotFoundException (404) only. The ONLY documented ground for the ValidationError is that the stack carries no template; a stack whose CREATE change set has been executed carries one, so AWS answers 200. Recorded behavior: both checks' `cdk deploy` runs succeed end to end against AWS (the nested-stack check gets past asset publication — "success: Published ... Template" — and then deploys).

Emulator: `cdk deploy` exits 1 with the CLI reporting `ValidationError: Template does not exist for stack [<stack-name>] in REVIEW_IN_PROGRESS state`. Fixed text of that message is "Template does not exist for stack [" ... "] in " ... " state"; the variable parts are the stack name as named in the request and the stack's own current status from emulator state (here REVIEW_IN_PROGRESS). It is produced by GetTemplate when the resolved stack's stored template body is absent (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/get_template.rs, the `stack.template_body.clone().ok_or_else(...)` arm calling crate::state::template_does_not_exist(name_or_id, &stack.status)). So at the moment the CDK CLI reads the template the emulator's stack is BOTH still in REVIEW_IN_PROGRESS and still template-less — the CREATE change set's template was never attached to the stack it created, or the stack was never carried out of the review state by the execution. Hypothesis: the stack record written by CreateChangeSet(ChangeSetType=CREATE) is left with template_body = None and status REVIEW_IN_PROGRESS, and the deploy's template read happens before/independently of ExecuteChangeSet attaching it, so no ordering of the CLI's calls can see a template.

Why: every change-set-based deploy of a NEW stack is unusable. `cdk deploy`, `sam deploy` and any tool that follows the create-change-set/execute flow aborts non-zero before the stack is created, so a client relying on AWS behavior gets no stack at all: no NotificationARNs recorded, no AWS::NotificationARNs resolution, no stack events published to the notification topic, and no nested-stack producer/consumer wiring. It also makes the stack's review state a dead end: a stack reported as existing can never have its template read back.

Fix: the CloudFormation service's change-set path — CreateChangeSet must store the change set's template on the stack record it mints in REVIEW_IN_PROGRESS (or GetTemplate must serve the pending change set's body for such a stack), and ExecuteChangeSet must attach that template and leave the review state before the stack is readable as created. Component: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_change_set.rs, .../ops/execute_change_set.rs and the stack state they write, plus the refusal arm in .../ops/get_template.rs.

Traces: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event (assertion `rc == 0` at check line 64; tail `❌ <stack> failed to deploy / ‣ ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state`), cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update (assertion on the FIRST `cdk deploy`, check line 61, same ValidationError after all three nested templates published successfully).

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

1) cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event [mismatch]
   check line 64: `assert rc == 0, f"the deploy exited {rc}; output tail {out[-800:]!r}"`
   AssertionError: the deploy exited 1; output tail:
     'Synthesis time: 4.48s\n\n<stack>: creating CloudFormation changeset...\n<stack>: deploying... [1/1]\n❌  <stack> failed to deploy\n‣ ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state\n'
   Expected: exit 0, then DescribeStacks reporting NotificationARNs == [topic] and Outputs {"NotificationArns": topic}, and one notification per stack event on the subscribed queue.
   Got: exit 1; the stack is never created, so none of the downstream assertions are even reached.

2) cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update [mismatch]
   check line 61: `assert rc == 0, f"the first `cdk deploy` exited {rc}; output tail {out[-800:]!r}"`
   AssertionError: the first `cdk deploy` exited 1; output tail shows all assets published OK
     ('success: Published Producer Nested Stack Template', 'success: Published Consumer Nested Stack Template', 'success: Published <stack> Template'), then
     '<stack>: creating CloudFormation changeset...\n<stack>: deploying... [1/1]\n❌  <stack> failed to deploy\n‣ ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'

Diverging response, as the refusal reaches the client:
   HTTP 400, Error/Code ValidationError, Error/Message "Template does not exist for stack [<stack-name>] in REVIEW_IN_PROGRESS state".
   Fixed text: "Template does not exist for stack [", "] in ", " state". Values from the request/state: the stack name supplied by the caller, and the stack's current status held by the emulator.
Expected from AWS: HTTP 200 with GetTemplateResult.TemplateBody (the executed change set's template) and StagesAvailable [Original, Processed]; `cdk deploy` exits 0.

Stack names (sim2real-cdk-notify-<hex>, sim2real-cdk-sib-<hex>), asset hashes, synthesis times, account 000000000000 and region us-west-2 are environment-specific and are not part of the requirement; the requirement is the 200/template answer and the zero exit of the deploy.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Stage a minimal asset-free CDK app (one AWS::CloudFormation::WaitConditionHandle plus an Output joining AWS::NotificationARNs) and run:
   cdk deploy <new-stack-name> --require-approval never --notification-arns <sns-topic-arn> --app "python3 app.py" --ci --no-color
3. The CLI prints "creating CloudFormation changeset...", then "❌ <stack> failed to deploy" with "‣ ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state" and exits 1. On AWS the deploy exits 0.
4. Equivalently, without CDK: CreateChangeSet(StackName=<new>, ChangeSetName=<name>, ChangeSetType=CREATE, TemplateBody=...), then GetTemplate(StackName=<new>) — the emulator answers 400 ValidationError "Template does not exist for stack [<new>] in REVIEW_IN_PROGRESS state"; after ExecuteChangeSet the same read must answer 200 with the change set's template body.
5. Re-replay both checks in trace_ids to confirm the fix: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event and cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update.
