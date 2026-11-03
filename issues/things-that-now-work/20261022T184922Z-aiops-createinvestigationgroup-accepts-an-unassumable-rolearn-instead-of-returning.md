Issue-Source: trace-sweep-divergence
Issue-Trace: aiops-error-create-investigation-group-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: aiops-error-create-investigation-group-validation
Issue-Shape-Id: com.amazonaws.aiops#AIOps
Issue-Key: aiops|CreateInvestigationGroup|4xx->2xx|$.message
Issue-Key: aiops|CreateInvestigationGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: aiops|CreateInvestigationGroup|4xx->2xx|status
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# CreateInvestigationGroup accepts an unassumable roleArn instead of returning ValidationException

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvestigationGroup

## Description
Cause 1 of 1: CreateInvestigationGroup performs no check that the supplied roleArn is usable by the service, so a request that AWS rejects is instead accepted and a group is created.

AWS: CreateInvestigationGroup is POST /investigationGroups with a success code of 201 and models ValidationException (httpError 400) among its errors; roleArn is a required member targeting RoleArn (@length 20..2048, @pattern ^arn:.*$) (Smithy: CreateInvestigationGroup.roleArn -> RoleArn; error ValidationException -> 400). Beyond the syntactic pattern, the recorded response shows the service also rejects a role it cannot assume: HTTP 400 with x-amzn-errortype: ValidationException and a message of the form "The InvestigationGroup could not assume the role associated with the specified account. Please validate that the role exists and is configured correctly." (recorded behavior only for the role-assumability rule; the status and error code come from the recorded response, the message text is quoted as an example). Hypothesis: the service resolves/assumes roleArn during creation and surfaces the failure as ValidationException.

Emulator: returns 201 with a created-group body. The response carries no $.message and no x-amzn-errortype header at all, i.e. the request is treated as fully valid and a group is persisted.

Why: a client that provisions an investigation group with a role that does not exist, or whose trust policy does not allow aiops to assume it, is told the group was created successfully. Any code that exercises the failure path (surfacing the misconfiguration to an operator, retrying with a corrected role, rolling back the surrounding stack) never runs against the emulator, and the emulator's state then contains a group that could not exist on AWS, which skews every later Get/List/Update step too.

Fix: the CreateInvestigationGroup handler's input validation in the aiops service crate — add the role-usability check that rejects a roleArn the emulator cannot resolve, and return ValidationException (400, x-amzn-errortype: ValidationException) instead of creating the group and answering 201.

Traces: aiops-error-create-investigation-group-validation#task#1 (expected 400/ValidationException, got 201 with no error payload).

## Evidence
Fixture aiops-error-create-investigation-group-validation, step task#1, operation CreateInvestigationGroup.

Expected (recorded AWS): status 400; $headers.x-amzn-errortype = 'ValidationException'; $.message = 'The InvestigationGroup could not assume the role associated with the specified account. Please validate that the role exists and is configured correctly.' (message quoted as an example of the rule).

Actual (emulator): status 201; $.message missing (got None); $headers.x-amzn-errortype missing (got None).

Replay detail verbatim: "status: expected 400, got 201 | $.message: missing (expected 'The InvestigationGroup could not assume the role associated with the specified account. Please validate that the role exists and is configured correctly.', got None) | $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)". Divergence key: service=aiops|CreateInvestigationGroup|status=400->201|keys=$.message,$headers.x-amzn-errortype,status. Affected count 1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture aiops-error-create-investigation-group-validation, step task#1: POST /investigationGroups with a required name plus a roleArn that is syntactically an ARN (matches ^arn:.*$) but names a role the service cannot assume.
3. Observe the emulator answers 201 and persists an investigation group; AWS answers 400 with x-amzn-errortype: ValidationException and no group created.
