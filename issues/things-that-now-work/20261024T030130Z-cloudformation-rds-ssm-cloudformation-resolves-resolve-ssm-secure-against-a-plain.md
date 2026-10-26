Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected
Issue-Key: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected|run#14|AssertionError|assert stack["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CloudFormation resolves {{resolve:ssm-secure:...}} against a plain String SSM parameter and completes the stack instead of failing it

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause. AWS: the `{{resolve:ssm-secure:<name>:<version>}}` dynamic reference resolves ONLY an SSM parameter of type SecureString. Pointing it at a parameter of type String is a misuse CloudFormation refuses: the resource whose property carries the reference fails during create, the stack rolls back, and the terminal stack status is ROLLBACK_COMPLETE (with the resource recorded CREATE_FAILED and a ResourceStatusReason naming the ssm-secure resolution failure — the reason text is an example, the terminal status and the CREATE_FAILED resource are the requirement). Recorded behavior only, since the dynamic-reference rule is a CloudFormation template feature rather than a modeled API member.

Emulator: the stack reaches CREATE_COMPLETE. The ssm-secure reference against a plain String parameter is resolved (or silently passed through) and the resource is created, so a misuse AWS rejects produces a successful stack.

Why: two bad outcomes for a client. First, a template that would never deploy on AWS deploys here, so the emulator cannot be used to validate a template before a real deployment, and the negative-path handling (catch the rollback, report the misconfiguration) is never exercised. Second, the reference's whole purpose is to keep a secret out of the template and out of the stack: resolving a non-SecureString parameter under ssm-secure means the value was taken from a store that offers no encryption guarantee while the template claims it did, which is precisely the confusion AWS refuses to allow.

Fix: the CloudFormation dynamic-reference resolver — `ssm-secure` must look the parameter up through the SSM service state, require Type == SecureString, and fail the resource (driving the stack to ROLLBACK_COMPLETE with the resource CREATE_FAILED) otherwise; `ssm` must remain the only form that accepts a String parameter.

Traces: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected.

## Evidence
cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected — AssertionError: expected ROLLBACK_COMPLETE for a plain param used via ssm-secure, got 'CREATE_COMPLETE'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ssm: PutParameter with Type=String (not SecureString), e.g. name /sim2real/plain-param.
3. cloudformation: CreateStack with a template whose resource property is '{{resolve:ssm-secure:/sim2real/plain-param:1}}'.
4. Poll DescribeStacks for the terminal status: it is CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and DescribeStackEvents records no CREATE_FAILED for the resource carrying the reference.
