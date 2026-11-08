Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.concurrent-generated-template-limit cloudformation.generated-template-concurrency-limit-rejected cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete cloudformation.cdk-deploy-replaces-a-rollback-complete-stack-with-a-new-one cloudformation.cdk-deployed-l2-trio-serves-http-and-persists cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard cloudformation.e2e-resource-scan-generated-template-to-managed-stack
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.concurrent-generated-template-limit|run#17|AssertionError|assert message.startswith(
Issue-Key: cloudformation.generated-template-concurrency-limit-rejected|run#13|AssertionError|assert message.startswith(
Issue-Key: cloudformation.cdk-deploy-replaces-a-rollback-complete-stack-with-a-new-one|run#16|AssertionError|assert (bad["ResourceStatus"], reasons) == ("DELETE_COMPLETE", [
Issue-Key: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists|_http_until#30|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves|_http_until#37|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write|_http_until#38|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard|run#38|AssertionError|assert rc != 0, (
Issue-Key: cloudformation.e2e-resource-scan-generated-template-to-managed-stack|run#10|AssertionError|assert found is not None, "resource scan did not surface the manually-created SNS topic"

# cloudformation: four causes left open by the in-use-export bundle — the generated-template limit wording, nested child stacks, the name-collision reason, and CDK deploys that never settle

> _Note: these four causes were filed as 4, 6, 7 and 8 of
> `20261102T101202Z-cloudformation-8-net-new-root-causes-in-use.md`, whose other four causes
> are fixed and whose report now lives under `issues/things-that-now-work/`. They are carried
> here unchanged, each with the trace ids and replay keys that belong to it, because nothing
> in that change implements them. Where a key moved on, this report says what it moved onto._

## Operation
CreateGeneratedTemplate, CreateStack, UpdateStack

## Description

Cause 1 of 4 (was cause 4 of 8): the generated-template concurrency-limit refusal message does not match AWS.
AWS: recorded behavior only — exceeding the in-flight generated-template limit is refused with AWS's wording for that quota. Both checks assert `message.startswith(<AWS's sentence>)`, and that sentence is not disclosed by the replay harness.
Emulator: `No more than 5 generated templates can be in an InProgress or Pending status at one time`, which is the sentence the model's `ConcurrentResourcesLimitExceededException` documentation states.
Evidence that neither reading of the model's text is AWS's: the emulator was tried answering that sentence WITH its period and wrapped in this surface's SDK envelope — the form
`(Service: CfnTemplateGenerationServiceLambda; Status Code: 400; Error Code: <exception>; Request ID: <id>; Proxy: null)` that this surface's other two recorded refusals carry
(`state::generated_template::with_component_envelope`) — and both checks still reported "unexpected limit message". Since the emulator's answer then OPENED with the model's sentence, AWS's sentence does not begin with it at all. The wording was reverted rather than left invented.
Why: clients that branch on the limit message mis-handle the refusal.
Fix: `state::generated_template::concurrent_generations_exceeded`, from a recording of the refusal — the sentence has to come from the corpus, since nothing in the model or the docs states it.
Traces: cloudformation.concurrent-generated-template-limit, cloudformation.generated-template-concurrency-limit-rejected.

Cause 2 of 4 (was cause 6 of 8): a nested child stack is not addressable as a stack of its own.
AWS: recorded behavior only — a nested stack's `PhysicalResourceId` is the child stack's ARN, `DescribeStacks` on that id returns the child, and deleting the parent cascades to it.
Emulator: `DescribeStacks` on the child id raises ValidationError `Stack with id <parent>-<Logical><suffix> does not exist` — only a generated name exists, not a stack. `ops/describe_stacks.rs` and `ops/list_stacks.rs` both state that nested stacks are not implemented (no `ParentId`/`RootId`), and `AWS::CloudFormation::Stack` has no provisioning arm.
Why: every second-hop navigation into nested stacks (CDK nested stacks, outputs, events) fails.
Fix: nested-stack provisioning must register a REAL child stack — created from the resource's `TemplateURL`, with the parent's id as its `ParentId`/`RootId` — and return that stack's ARN as the resource's physical id, with the parent's deletion cascading to it.
Traces: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete.

Cause 3 of 4 (was cause 7 of 8): a name-collision CREATE_FAILED reason is the downstream handler's text instead of CloudFormation's.
AWS: recorded behavior only — when a resource's declared physical name is already owned, the resource event's reason is `<physical-name> already exists in stack <owning-stack-ARN>` and the stack-level reason is `The following resource(s) failed to create: [<LogicalId>]. ` (which the emulator already words, `state::resource_failure_summary`).
Emulator: the resource reason is the owning service's own refusal, quoted — `Resource handler returned message: "Parameter <name> already exists; set Overwrite to update it" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)`.
Why: clients cannot tell which stack owns the colliding name, which is the actionable part of AWS's message.
Fix: collision detection in the resource provisioner — the index of which live stack of the Region owns which physical name, read under the store lock and passed into `state::provisioning::reconcile`, so the resource fails with CloudFormation's OWN reason before the owning service is called.
Replay note: this unit now fails EARLIER than the reason, on `assert failed["StackStatus"] == "ROLLBACK_COMPLETE"` — "the failed create left 'REVIEW_IN_PROGRESS'" — so a stack a CDK deploy creates through a CREATE change set whose execution fails is left in the status it had before the execution instead of being rolled back. That is part of this cause's unit and probably of cause 4 below.
Traces: cloudformation.cdk-deploy-replaces-a-rollback-complete-stack-with-a-new-one.

Cause 4 of 4 (was cause 8 of 8): CDK L2 deploys that must then serve or read back never settle.
AWS: recorded behavior only — each of these deploys completes and the deployed endpoint/resource answers well inside 60s.
Emulator: `cdk deploy <stack> --require-approval never` exits 1 after `creating CloudFormation changeset... deploying... [1/1]`, so the check's own budget is exhausted waiting for an endpoint nothing deployed. Every CDK-driven unit of the original bundle fails at this same step, including the two carried here for other causes
(`deploy-cdk-cross-stack-export-ordering-and-in-use-guard` at `_cdk#19`, `cdk-nested-stack-is-tracked-and-cascades-on-delete` at `_cdk#13`), which is why they are filed here: whatever fails the CDK change-set deploy path fails them all, and the in-use export guard those two also cover is now implemented and passing on its API-driven units.
Why: a client gets a hang instead of a deployed, serving stack.
Fix: whichever stage of the `CreateChangeSet` → `ExecuteChangeSet` pipeline these resource sets fail at. The replay does not disclose enough of the CDK output to name it, so the first step is reproducing one of these deploys against the emulator and reading the stack's own events.
Traces: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists, cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves, cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write, cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard, cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete.

## Evidence
C1: "unexpected limit message 'No more than 5 generated templates can be in an InProgress or Pending status at one time'", and the same check against that sentence with its period plus this surface's envelope.
C2: ClientError (ValidationError) on DescribeStacks: 'Stack with id <parent>-ChildNestedStackChildNestedStackResource<suffix> does not exist'.
C3: CREATE_FAILED event ResourceStatusReason 'Resource handler returned message: "Parameter /<name> already exists; set Overwrite to update it" (RequestToken: ..., HandlerErrorCode: AlreadyExists)'; and, ahead of it, "the failed create left 'REVIEW_IN_PROGRESS'".
C4: '`cdk deploy <stack> --require-approval never` exited 1, expected 0' for every CDK-driven unit.
Ids, names, uuids and timestamps are per-run fillers.

## Steps to Reproduce
1) Exceed the in-flight generated-template limit and read the refusal's message. 2) Deploy a CDK app with a nested stack and `DescribeStacks` the child's `PhysicalResourceId`. 3) Create a stack whose resource name is already owned by another stack and read the CREATE_FAILED event and the stack's status. 4) Run the CDK deploys listed above and read the stack's own events for the stage that does not settle.

## What the change resolving this report did, and what it left open

FIXED — cause 3: a name-collision `CREATE_FAILED` reason is now CloudFormation's own. A create the
owning service refuses because the name is already in use is reported as
`<physical-name> already exists in stack <owning-stack-ARN>` whenever CloudFormation's own index of
the Region's live stacks is what holds that name (`state::physical_name::managed_physical_names` /
`taken_by_another_stack`, read under the store lock by every operation that deploys a template and
carried in `provisioning::StackProvenance::managed_names`), and the rollback now disposes of the
resource whose create failed as it disposes of every record it held, so that resource's last events
are `DELETE_IN_PROGRESS`/`DELETE_COMPLETE` rather than the failure. Its replay unit
(`cloudformation.cdk-deploy-replaces-a-rollback-complete-stack-with-a-new-one`) passes.
Covered by `tests/all/cross_stack_name_collision.rs`.

ALREADY IMPLEMENTED — cause 2: nested child stacks are real stacks
(`state/provisioning/nested_stack.rs`, `tests/all/nested_stacks.rs`): the provider creates the child
through CloudFormation's own `CreateStack`, the resource's physical id is the child's stack ARN, and
the parent's deletion cascades. Its unit still fails earlier, at the CDK deploy of cause 4.

STILL OPEN — carried, unchanged, with their trace ids and replay keys, to
`issues/things-that-do-not-work/20261103T120000Z-cloudformation-generated-template-limit-wording-and-the-cdk-l2-deploys.md`,
because nothing in that change implements them:

STILL OPEN — cause 1: the generated-template concurrency refusal's wording. Both checks reject the
model's documented sentence, and the attempt recorded above (that sentence with its period inside
this surface's SDK envelope) was rejected too, so AWS's sentence does not begin with the model's
text and nothing in the model, the docs or the replay output discloses it. It has to come from a
recording of the refusal; inventing a sentence would be a value no rule produces.

STILL OPEN — cause 4: the CDK L2 deploys. Each fails on a resource whose owning service answers
`HandlerErrorCode: InvalidRequest`, and the replay discloses only the tail of the CDK output, so
neither the resource nor the refusal is in evidence; the checks that follow those deploys also need
an Amazon API Gateway REST data plane that serves HTTP, which no listener of this emulator provides.
The two units carried here for other causes fail at that same step.

STILL OPEN — the in-use export guard on a CDK-driven producer update
(`deploy-cdk-cross-stack-export-ordering-and-in-use-guard`): the guard is in force on `UpdateStack`
and on `ExecuteChangeSet` (`state::exports`), and its API-driven units pass, but the CDK deploy that
changes the exported value still exits 0.

STILL OPEN — `e2e-resource-scan-generated-template-to-managed-stack`: it now reaches
`CreateChangeSet` and is refused by the documented import rule (every resource to import must carry
a `DeletionPolicy`), which the generated template's `TemplateConfiguration` is what would assign.

## Also fixed in the change that split this report out
The four causes of the original bundle that ARE fixed — the in-use export guard on every deployment of a publishing stack, the cancelled-operation reason's wording, the resource scan's inventory of the resources no stack manages, and the SDK exception a refused Amazon S3 call is reported as (plus `DeleteStack` with `RetainResources`, which that last one needed) — are described in
`issues/things-that-now-work/20261102T101202Z-cloudformation-8-net-new-root-causes-in-use.md`.
