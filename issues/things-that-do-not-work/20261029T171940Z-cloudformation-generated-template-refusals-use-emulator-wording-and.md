Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-create-generated-template-output-member
Issue-Step: teardown#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-create-generated-template-output-member cloudformation-generated-template-and-resource-scan cloudformation-generated-template-workflow cloudformation-generated-template-lifecycle cloudformation-template-generator-and-hook-result-errors
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DeleteGeneratedTemplate|2xx->4xx|status
Issue-Key: cloudformation|DescribeGeneratedTemplate|4xx->2xx|$
Issue-Key: cloudformation|DescribeGeneratedTemplate|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|DescribeGeneratedTemplate|4xx->2xx|status
Issue-Key: cloudformation|CreateGeneratedTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeGeneratedTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|GetGeneratedTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DeleteGeneratedTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|UpdateGeneratedTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation generated-template refusals use emulator wording and DeleteGeneratedTemplate never takes effect

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGeneratedTemplate

## Description
Every refusal on the generated-template surface is written in the emulator's own words rather than AWS's, and the delete path is broken in two ways: a valid delete is refused with 429 and, where a delete is accepted, the template stays readable forever. 7 net-new root causes, one service, all in the generated-template handlers/state.

The replay reported owned_by=[] and no stub for all seven. The open issue "CloudFormation refusal taxonomy diverges: wrong not-found/validation message templates, duplicate ClientRequestToken not TokenAlreadyExistsException, ..." is the nearest neighbour but carries none of the fixture#step bindings below.

Cause 1 of 7: DeleteGeneratedTemplate refuses a valid delete with 429.
AWS: Smithy: DeleteGeneratedTemplate output Unit (an empty 200 result); its only modelled errors are GeneratedTemplateNotFoundException (404) and ConcurrentResourcesLimitExceededException (429, awsQueryError ConcurrentResourcesLimitExceeded). A delete of an existing, COMPLETE generated template returns 200.
Emulator: status 429 for the delete of a template it had just created and reported COMPLETE.
Why: teardown of any generator workflow fails — a client retries forever on a throttling-class error that will never clear, and the template cannot be removed.
Fix: the DeleteGeneratedTemplate handler (the concurrent-resources gate must not fire for a single delete of an existing template).
Traces: cloudformation-create-generated-template-output-member#teardown#2, cloudformation-generated-template-and-resource-scan#teardown#7, cloudformation-generated-template-workflow#teardown#11.

Cause 2 of 7: a deleted generated template is still readable.
AWS: Smithy: DescribeGeneratedTemplate error GeneratedTemplateNotFoundException, httpError 404, awsQuery code GeneratedTemplateNotFound. After DeleteGeneratedTemplate succeeds, DescribeGeneratedTemplate on that name answers 404 GeneratedTemplateNotFound.
Emulator: keeps answering 200 with the full DescribeGeneratedTemplateResult (Status COMPLETE) for the whole 60 s poll window — the poll never observes the error code.
Why: delete is a no-op as far as any reader can tell; a create-delete-recreate cycle sees a stale template and an AlreadyExists on the recreate.
Fix: the DeleteGeneratedTemplate handler must remove the template from generated-template state (and the read handlers must honour the removal).
Traces: cloudformation-generated-template-and-resource-scan#teardown#8, cloudformation-generated-template-workflow#teardown#12.

Cause 3 of 7: CreateGeneratedTemplate's AlreadyExists message is the emulator's.
AWS: Smithy: CreateGeneratedTemplate error AlreadyExistsException, httpError 400. The message is 'Template with name <GeneratedTemplateName> already exists. (Service: CfnTemplateGenerationServiceLambda; Status Code: 400; Error Code: AlreadyExistsException; Request ID: <request id>; Proxy: null)' — fixed text apart from the requested template name and the per-request id.
Emulator: 400 with 'Generated template [<name>] already exists'.
Why: callers that match on the AWS wording (including the CLI's own error rendering and retry heuristics) do not recognise the refusal.
Fix: the CreateGeneratedTemplate validation's error message template.
Traces: cloudformation-generated-template-and-resource-scan#task#2.

Cause 4 of 7: DescribeGeneratedTemplate's not-found message is the emulator's.
AWS: Smithy: GeneratedTemplateNotFoundException, httpError 404, code GeneratedTemplateNotFound. Message: 'Template with arn <name-or-arn supplied by the caller> does not exist (Service: CfnTemplateGenerationServiceLambda; Status Code: 400; Error Code: GeneratedTemplateNotFoundException; Request ID: <request id>; Proxy: null)' — only the caller-supplied identifier and the request id vary.
Emulator: 404 with 'Generated template <name> cannot be found'.
Why: the status and code are right but the message a client surfaces/matches is not AWS's.
Fix: the GeneratedTemplateNotFound error constructor shared by the generated-template read handlers.
Traces: cloudformation-generated-template-and-resource-scan#task#4, cloudformation-generated-template-lifecycle#task#6, cloudformation-generated-template-lifecycle#teardown#8, cloudformation-generated-template-workflow#task#8.

Cause 5 of 7: GetGeneratedTemplate's not-found message is the emulator's (same template, different operation, so it is not fixed by cause 4 alone).
AWS: as cause 4 — 'Template with arn <identifier> does not exist (Service: CfnTemplateGenerationServiceLambda; ...)'.
Emulator: 404 'Generated template <name> cannot be found'.
Why/Fix: as cause 4, in the GetGeneratedTemplate handler.
Traces: cloudformation-generated-template-workflow#task#9, cloudformation-template-generator-and-hook-result-errors#task#1.

Cause 6 of 7: DeleteGeneratedTemplate's not-found message is the emulator's.
AWS: as cause 4.
Emulator: 404 'Generated template <name> cannot be found'.
Why/Fix: as cause 4, in the DeleteGeneratedTemplate handler.
Traces: cloudformation-generated-template-workflow#task#10.

Cause 7 of 7: UpdateGeneratedTemplate's not-found message is the emulator's.
AWS: as cause 4.
Emulator: 404 'Generated template <name> cannot be found'.
Why/Fix: as cause 4, in the UpdateGeneratedTemplate handler.
Traces: cloudformation-template-generator-and-hook-result-errors#task#2.

## Evidence
Replay of simparity-wire-aws-v1, 7 mismatch clusters:
- cloudformation-create-generated-template-output-member#teardown#2 DeleteGeneratedTemplate: status expected 200, got 429 (3 affected steps, also cloudformation-generated-template-and-resource-scan#teardown#7 and cloudformation-generated-template-workflow#teardown#11).
- cloudformation-generated-template-and-resource-scan#teardown#8 DescribeGeneratedTemplate: status expected 404, got 200; poll capped at 60 s with error code None not in ['GeneratedTemplateNotFound'] after 61 attempts; the emulator kept returning Status COMPLETE / StatusReason 'Template complete'.
- cloudformation-generated-template-and-resource-scan#task#2 CreateGeneratedTemplate 400: Message expected 'Template with name <name> already exists. (Service: CfnTemplateGenerationServiceLambda; Status Code: 400; Error Code: AlreadyExistsException; Request ID: <id>; Proxy: null)', got 'Generated template [<name>] already exists'.
- cloudformation-generated-template-and-resource-scan#task#4 DescribeGeneratedTemplate 404: Message expected 'Template with arn <name> does not exist (Service: CfnTemplateGenerationServiceLambda; Status Code: 400; Error Code: GeneratedTemplateNotFoundException; Request ID: <id>; Proxy: null)', got 'Generated template <name> cannot be found' (4 affected steps).
- cloudformation-generated-template-workflow#task#9 GetGeneratedTemplate 404: same expected/actual message pair (2 affected steps).
- cloudformation-generated-template-workflow#task#10 DeleteGeneratedTemplate 404: same expected/actual message pair.
- cloudformation-template-generator-and-hook-result-errors#task#2 UpdateGeneratedTemplate 404: same expected/actual message pair.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. CreateGeneratedTemplate for a new name, wait for Status COMPLETE, then DeleteGeneratedTemplate(<name>) — the emulator answers 429 instead of 200.
3. On a path where the delete is accepted, call DescribeGeneratedTemplate(<name>) afterwards — the emulator answers 200 with Status COMPLETE instead of 404 GeneratedTemplateNotFound.
4. CreateGeneratedTemplate with an existing name — compare the 400 message with AWS's 'Template with name ... already exists. (Service: CfnTemplateGenerationServiceLambda; ...)'.
5. Call DescribeGeneratedTemplate / GetGeneratedTemplate / DeleteGeneratedTemplate / UpdateGeneratedTemplate with an unknown name — all four return 404 with 'Generated template ... cannot be found' instead of AWS's 'Template with arn ... does not exist (Service: CfnTemplateGenerationServiceLambda; ...)'.
