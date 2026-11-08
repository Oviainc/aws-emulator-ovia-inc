Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.concurrent-generated-template-limit cloudformation.generated-template-concurrency-limit-rejected cloudformation.cdk-deployed-l2-trio-serves-http-and-persists cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete cloudformation.e2e-resource-scan-generated-template-to-managed-stack
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.concurrent-generated-template-limit|run#17|AssertionError|assert message.startswith(
Issue-Key: cloudformation.generated-template-concurrency-limit-rejected|run#13|AssertionError|assert message.startswith(
Issue-Key: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists|_http_until#30|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves|_http_until#37|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write|_http_until#38|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard|run#38|AssertionError|assert rc != 0, (
Issue-Key: cloudformation.e2e-resource-scan-generated-template-to-managed-stack|run#10|AssertionError|assert found is not None, "resource scan did not surface the manually-created SNS topic"

# cloudformation: the generated-template limit wording, the CDK L2 deploys that never settle, and the two keys that same step blocks

> _Note: these causes were carried in
> `20261103T090000Z-cloudformation-generated-template-limit-wording-nested-stacks-collision-reason-cdk-deploys.md`
> as its causes 1 and 4. The change that fixed that report's cause 3 (the name-collision reason,
> whose unit now passes) implements NOTHING on these paths, so they are carried here unchanged with
> the trace ids and replay keys that belong to them, and that report now lives under
> `issues/things-that-now-work/`. Its cause 2 (nested child stacks) needed nothing: it is
> implemented (`state/provisioning/nested_stack.rs`, `tests/all/nested_stacks.rs`) and its unit
> fails at cause 2 of this report instead._

## Operation
CreateGeneratedTemplate, CreateChangeSet, ExecuteChangeSet, UpdateStack

## Description

Cause 1 of 4: the generated-template concurrency-limit refusal message does not match AWS.
AWS: recorded behavior only — exceeding the in-flight generated-template limit is refused with AWS's
wording for that quota. Both checks assert `message.startswith(<AWS's sentence>)`, and that sentence
is not disclosed by the replay harness.
Emulator: `No more than 5 generated templates can be in an InProgress or Pending status at one
time`, which is the sentence the model's `ConcurrentResourcesLimitExceededException` documentation
states.
Evidence that neither reading of the model's text is AWS's: the emulator was tried answering that
sentence WITH its period and wrapped in this surface's SDK envelope — the form
`(Service: CfnTemplateGenerationServiceLambda; Status Code: 400; Error Code: <exception>; Request ID:
<id>; Proxy: null)` that this surface's other two recorded refusals carry
(`state::generated_template::with_component_envelope`) — and both checks still reported "unexpected
limit message". Since the emulator's answer then OPENED with the model's sentence, AWS's sentence
does not begin with it at all. The wording was reverted rather than left invented, and
`tests/all/generated_template_limit_wording.rs` is the empty suite that records why.
Why: clients that branch on the limit message mis-handle the refusal.
Fix: `state::generated_template::concurrent_generations_exceeded`, from a recording of the refusal —
the sentence has to come from the corpus, since nothing in the model or the docs states it.
Traces: cloudformation.concurrent-generated-template-limit,
cloudformation.generated-template-concurrency-limit-rejected.

Cause 2 of 4: CDK L2 deploys that must then serve HTTP never settle.
AWS: recorded behavior only — each of these deploys completes and the deployed endpoint answers well
inside 60s.
Emulator: `cdk deploy <stack> --require-approval never` exits 1, and the tail of its output the
replay discloses ends `…, HandlerErrorCode: InvalidRequest)` followed by `DeploymentError: Resource
updates failed:` — so one resource of each of those stacks is refused by the service that owns it,
with the class the provisioner puts a refused REQUEST in. Which resource, and which refusal, is not
in the evidence: the replay discloses only the tail, and there is no `cdk` in the fixer's tool set to
reproduce the deploy with. `cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete` fails
at the same step (`_cdk#13`), which is why nested stacks cannot be observed end to end even though
they are implemented.
Beyond the deploy, these three checks poll the deployed REST endpoint
(`_http_until#30`/`#37`/`#38`), which needs an Amazon API Gateway REST data plane: a stage a client
can reach over HTTP, and the integration behind it. `AWS::ApiGateway::RestApi`, `::Resource`,
`::Method`, `::Deployment`, `::Stage` and `::Account` have NO provisioning arm at all
(`state::provisioning::owner_for` carries only `AWS::ApiGateway::DomainName` for that surface), so
nothing is created in API Gateway for such a stack and nothing serves the endpoint. Those keys
therefore cannot be closed by wording anything differently — the capability has to exist first.
Why: a client gets a hang, or a failed deploy, instead of a deployed, serving stack.
Fix: read the stack's own events for the resource that fails (the first step is reproducing one of
these deploys), then provision the REST resources into Amazon API Gateway and serve the deployed
stage from that service.
Traces: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists,
cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves,
cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write,
cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete.

Cause 3 of 4: a CDK-driven update of a publishing stack is not held to the in-use export guard.
AWS: recorded behavior only — `cdk deploy <producer>` exits non-zero when it would change the value
of an export another live stack still imports.
Emulator: the producer's deploy exits 0 while the exported value changes from one run value to
another, with the consumer stack still importing that export.
The guard itself is implemented and in force on `UpdateStack` and on `ExecuteChangeSet`
(`state::exports::in_use_export_withdrawn`, read against
`ComAmazonawsCloudformationCloudformationState::importers_by_export`), and its API-driven units pass
— so what is missing is whatever the CDK deploy does differently: which call it makes, or which half
of the relation (the producer's published outputs, or the consumer's recorded imports) is not what
that call reads. The replay does not disclose the calls.
Why: a client is told a deployment succeeded that silently withdrew a value another stack resolved.
Fix: reproduce the two-stack CDK deploy, read which calls the producer's update makes and which of
the two halves the guard reads is empty for them.
Traces: cloudformation.deploy-cdk-cross-stack-export-ordering-and-in-use-guard.

Cause 4 of 4: a generated template cannot be imported into a managed stack.
AWS: recorded behavior only — the template the IaC generator produced is used to bring the scanned
resource under a stack's management, and `CreateChangeSet` accepts it.
Emulator: `CreateChangeSet` refuses with ValidationError `The following resources to import
[ImportedTopic] must have DeletionPolicy attribute specified in the template.` — the documented
import rule (`state::resource_import`), applied to a template whose resources carry no
`DeletionPolicy` attribute. The record's `TemplateConfiguration` is what assigns one
(`state::generated_template::last_complete_template` renders `DeletionPolicy` and
`UpdateReplacePolicy` onto every resource), and the emulator assigns neither unless the create
supplied them, so a generated template created without a `TemplateConfiguration` can never be
imported.
Why: the scan-to-managed-stack workflow the IaC generator exists for cannot be completed.
Fix: establish, from a recording of `DescribeGeneratedTemplate` for a create that supplied no
`TemplateConfiguration`, what that configuration is in force as — and, if it is `RETAIN`, default
both settings to it in `state::generated_template::StoredGeneratedTemplate::starting`, which is
what then renders the attribute the import requires. Four suites assert the current reading
(`generated_templates.rs`, `generated_template_updates.rs`, `generated_template_body.rs`,
`list_generated_templates.rs` all expect no configuration where the request supplied none), so the
default has to come from the recording rather than from a guess.
Traces: cloudformation.e2e-resource-scan-generated-template-to-managed-stack.

## Evidence
C1: "unexpected limit message 'No more than 5 generated templates can be in an InProgress or Pending
status at one time'", and the same check against that sentence with its period plus this surface's
envelope.
C2: '`cdk deploy <stack> --require-approval never` exited 1, expected 0' for every CDK-driven unit,
with the output tail ending `…, HandlerErrorCode: InvalidRequest)`.
C3: '`cdk deploy <stack>-producer` exited 0 while changing the exported parameter Value from
<value> to <value>; <stack>-consumer still imports the export, so CloudFormation must refuse this
update'.
C4: ClientError (ValidationError) on CreateChangeSet: 'The following resources to import
[<LogicalId>] must have DeletionPolicy attribute specified in the template.'
Ids, names, uuids, values and timestamps are per-run fillers.

## Steps to Reproduce
1) Exceed the in-flight generated-template limit and read the refusal's message. 2) Run the CDK
deploys listed above and read the stack's own events for the resource that fails, then call the REST
endpoint the deploy published. 3) Deploy the two-stack CDK app, change the producer's exported value
and read the producer's calls and its stack status. 4) Scan a resource no stack manages, create a
generated template for it with no `TemplateConfiguration`, and use the template it answers in an
IMPORT change set.
