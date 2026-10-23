Issue-Source: trace-sweep-divergence
Issue-Trace: nova-act-error-paths-validation-and-conflict
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: nova-act-error-paths-validation-and-conflict nova-act-workflow-definition-export-config
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|CreateWorkflowDefinition|4xx->2xx|$.message
Issue-Key: nova-act|CreateWorkflowDefinition|4xx->2xx|$.resourceId
Issue-Key: nova-act|CreateWorkflowDefinition|4xx->2xx|$.resourceType
Issue-Key: nova-act|CreateWorkflowDefinition|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: nova-act|CreateWorkflowDefinition|4xx->2xx|status
Issue-Commit: 0f903c65ef17614b948ed7843da2bc7f944d5d1a

# nova-act CreateWorkflowDefinition skips precondition checks and returns 201

> _Note: this issue was identified against the code base at commit `0f903c65ef17614b948ed7843da2bc7f944d5d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflowDefinition

## Description
CreateWorkflowDefinition creates and returns 201 for requests AWS rejects before any resource is created: it enforces neither name uniqueness nor export-bucket validity. Both causes are the same missing stage — the handler goes straight from input shape validation to state insertion with no precondition checks. TWO causes must be closed.

Cause 1 of 2: duplicate definition name is accepted instead of raising ConflictException.
AWS: Smithy: CreateWorkflowDefinition (PUT /workflow-definitions, success code 201) models ConflictException (errorKind client, httpError 409, required members message, resourceId, resourceType), and CreateWorkflowDefinitionRequest.name is documented 'Must be unique within your account and region.' The recording shows the second create of the same name answering 409 with x-amzn-errortype 'ConflictException:http://internal.amazon.com/coral/com.amazon.novaact/', resourceType 'workflow-definition' and resourceId the definition's ARN.
Emulator: answers 201 with a success body — no ConflictException at all, so $.message, $.resourceId, $.resourceType and the x-amzn-errortype header are all absent.
Why: a client cannot discover that a name is taken. Create-if-absent logic that keys off 409 silently treats the duplicate as a fresh resource, and whichever record the emulator keeps (overwrite or shadow) diverges from the first definition's state, so subsequent reads of that name return attributes the client never wrote.
Fix: CreateWorkflowDefinition's handler/state layer must check the per-account/region definition name index before insert and raise ConflictException (409) with resourceType 'workflow-definition' and the definition ARN as resourceId. The recorded message 'Workflow definition already exists' is an example; the status and error code are the requirement.
Traces: nova-act-error-paths-validation-and-conflict#task#3.

Cause 2 of 2: an unusable exportConfig bucket is accepted instead of raising ValidationException.
AWS: Smithy: CreateWorkflowDefinition models ValidationException (errorKind client, httpError 400) and takes exportConfig (WorkflowExportConfig) 'Configuration for exporting workflow execution data to Amazon Simple Storage Service'. Recorded behavior: when the named export bucket does not exist / is not owned by the calling account, creation fails with 400 and x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.novaact/' — i.e. the export target is validated at create time, before the definition exists.
Emulator: answers 201 and creates the definition, emitting no error body or x-amzn-errortype.
Why: the misconfiguration is not surfaced at the only point the client checks it. A client that relies on create-time rejection to catch a wrong bucket name or a cross-account bucket gets a definition that looks healthy, and the failure only appears later (or never, in the emulator), so negative-path tests and setup wizards built on this error never fire.
Fix: CreateWorkflowDefinition's validation stage must resolve the exportConfig bucket against the emulator's S3 state and the calling account and raise ValidationException (400) when it is absent or not owned, before the definition is inserted. The recorded message ("Failed to validate export bucket '<bucket>'. Ensure the bucket exists, is accessible, and is owned by the calling account (<account>).") is an example built from the request; the status and error code are the requirement.
Traces: nova-act-workflow-definition-export-config#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1, two mismatch clusters on CreateWorkflowDefinition:

1) nova-act-error-paths-validation-and-conflict#task#3 — status: expected 409, got 201; $.message missing (expected 'Workflow definition already exists'); $.resourceId missing (expected 'arn:aws:nova-act:us-east-1:000000000000:workflow-definition/sim2real-probe-nova-act-wfvalid-09aa3f6a'); $.resourceType missing (expected 'workflow-definition'); $headers.x-amzn-errortype missing (expected 'ConflictException:http://internal.amazon.com/coral/com.amazon.novaact/').
2) nova-act-workflow-definition-export-config#task#1 — status: expected 400, got 201; $.message missing (expected "Failed to validate export bucket 'sim2real-probe-nova-act-wfexport-bkt-e7744edf'. Ensure the bucket exists, is accessible, and is owned by the calling account (000000000000)."); $headers.x-amzn-errortype missing (expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.novaact/').

In both cases the emulator produced the success response modeled for CreateWorkflowDefinition (HTTP 201) where AWS produced a modeled client error, so no error code was emitted at all.

## Steps to Reproduce
Cause 1: replay nova-act-error-paths-validation-and-conflict. Step task#2 creates workflow definition 'sim2real-probe-nova-act-wfvalid-<suffix>'; step task#3 re-issues PUT /workflow-definitions with the same name. Expect 409 ConflictException (x-amzn-errortype ConflictException:...) with resourceType 'workflow-definition'; the emulator returns 201.

Cause 2: replay nova-act-workflow-definition-export-config. Step task#1 issues PUT /workflow-definitions with an exportConfig naming an S3 bucket that does not exist in the account. Expect 400 ValidationException (x-amzn-errortype ValidationException:...); the emulator returns 201 and creates the definition.

Re-replay both fixture/steps to confirm the fix.
