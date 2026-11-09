Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__polly.cfn-polly-lexicon-attributes-and-getatt-read-back-then-not-found-after-delete cloudformation__polly.cfn-polly-lexicon-invalid-pls-content-rejected
Issue-Key: cloudformation__polly|CreateStack|ValidationError
Issue-Commit: 560e7694040bdb3912f85c22ba90afbac643f510

# cloudformation__polly: AWS::Polly::Lexicon is not a registered resource type — CreateStack refuses every Polly lexicon template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `560e7694040bdb3912f85c22ba90afbac643f510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation template processor does not know the AWS::Polly::Lexicon resource type, so any stack that declares a Polly lexicon is rejected at template-validation time and no Polly-backed stack can ever be deployed.

AWS: AWS::Polly::Lexicon is a resource type CloudFormation supports. CreateStack with a template whose Resources block declares a single AWS::Polly::Lexicon (Properties: Name, Content) is accepted — it returns 200 with a StackId, the stack proceeds to CREATE_COMPLETE, Ref on the resource resolves to the lexicon name and GetAtt/DescribeStackResource expose the created lexicon; DeleteStack removes it so a subsequent Polly GetLexicon answers LexiconNotFoundException. When the lexicon's PLS Content is invalid, the resource — not the template parser — fails: the stack reaches CREATE_FAILED/ROLLBACK_COMPLETE and the resource's ResourceStatusReason follows CloudFormation's handler-failure template `Resource handler returned message: "<the Polly refusal message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)`, where the quoted message and the handler error code come from the Polly provisioning call and the request token is per-call. The message `Template format error: Unrecognized resource types: [<type>]` with code ValidationError is AWS's refusal for a type CloudFormation genuinely does not have; its fixed text is `Template format error: Unrecognized resource types: [` + the unknown type names + `]`, and it must not be produced for a type that is supported. Smithy: cloudformation CreateStack declares no modeled error for this — it is the common ValidationError envelope at HTTP 400.

Emulator: CreateStack returns HTTP 400 with error code `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::Polly::Lexicon]` for every template that declares the type. Both deploy checks die at the very first CreateStack call, so nothing downstream (stack status, Ref/GetAtt read-back, delete-then-not-found, or the invalid-PLS CREATE_FAILED reason) is ever reached.

Why: a client that provisions Polly lexicons through CloudFormation (CDK, SAM, raw templates) cannot deploy at all — the whole stack is refused rather than the one resource, so even unrelated resources in the same template never get created. There is also no way to observe the handler-failure path, so IaC error handling cannot be exercised against the emulator.

Fix: register AWS::Polly::Lexicon in the CloudFormation template processor's resource-type registry and give it a provisioning handler that calls Polly PutLexicon on create (mapping an invalid PLS document to a resource-level CREATE_FAILED whose ResourceStatusReason uses the handler-failure template above), DeleteLexicon on delete, and resolves Ref to the lexicon Name plus the type's documented GetAtt attributes.

Traces: the two deploy checks below, both of which fail on the first CreateStack with the identical ValidationError.

## Evidence
Expected: CreateStack(TemplateBody with Resources.<Logical>.Type = "AWS::Polly::Lexicon") -> HTTP 200 with a StackId; stack reaches CREATE_COMPLETE (valid PLS) or CREATE_FAILED with a `Resource handler returned message: "..." (RequestToken: ..., HandlerErrorCode: ...)` reason (invalid PLS).

Actual (both checks): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Polly::Lexicon]

Message anatomy: `Template format error: Unrecognized resource types: [` ... `]` is fixed text; `AWS::Polly::Lexicon` is the type name taken from the request template. The divergence is that this refusal is emitted at all for a supported type — the correct response is a 200 StackId.

## Steps to Reproduce
1. Create a bucketless template with one resource: Resources: { Lex: { Type: "AWS::Polly::Lexicon", Properties: { Name: "<unique name>", Content: "<valid PLS 1.0 document>" } } }.
2. cloudformation.create_stack(StackName=<unique>, TemplateBody=<that template>, Tags=[{Key: sim2real, Value: <name>}]).
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Polly::Lexicon]" instead of a StackId.
4. Repeat with an intentionally invalid PLS Content: the same template-format refusal appears instead of the stack reaching CREATE_FAILED with a resource-handler reason.
