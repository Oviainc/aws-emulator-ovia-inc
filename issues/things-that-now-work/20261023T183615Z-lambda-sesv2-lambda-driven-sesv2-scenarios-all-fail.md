Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-role-positive lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected lambda__sesv2.lambda-sesv2-account-suppression-list-permission-negative lambda__sesv2.lambda-sesv2-create-contact-missing-permission-negative lambda__sesv2.lambda-sesv2-create-contact-positive lambda__sesv2.lambda-sesv2-delete-email-identity-positive lambda__sesv2.lambda-sesv2-delete-suppressed-destination-positive lambda__sesv2.lambda-sesv2-get-email-identity-missing-permission-negative lambda__sesv2.lambda-sesv2-get-email-identity-positive lambda__sesv2.lambda-sesv2-get-identity-verification-missing-permission-negative lambda__sesv2.lambda-sesv2-list-email-identities-missing-permission-negative lambda__sesv2.lambda-sesv2-list-email-identities-positive lambda__sesv2.lambda-sesv2-list-suppressed-destinations-missing-permission-negative lambda__sesv2.lambda-sesv2-list-suppressed-destinations-positive lambda__sesv2.lambda-sesv2-put-mail-from-domain-missing-permission-negative lambda__sesv2.lambda-sesv2-put-suppressed-destination-missing-permission-negative lambda__sesv2.lambda-sesv2-put-suppressed-destination-positive lambda__sesv2.lambda-sesv2-tag-resource-missing-permission-negative lambda__sesv2.lambda-sesv2-update-configuration-set-event-destination-missing-permission-negative lambda__sesv2.cli-lambda-sesv2-describe-email-identity lambda__sesv2.cli-lambda-sesv2-describe-function-config lambda__sesv2.lambda-execution-role-wrong-trust-negative lambda__sesv2.lambda-wrong-trust-policy-sesv2-negative lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-trust-principal-negative lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-role-arn-negative lambda__sesv2.cfn-lambda-sesv2-wrong-role-arn-negative lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive
Issue-Key: lambda__sesv2|ListEmailIdentities|AmbiguousService
Issue-Key: lambda__sesv2|GetAccount|AmbiguousService
Issue-Key: sesv2|ListSuppressedDestinations|AmbiguousService
Issue-Key: lambda__sesv2|CreateEmailIdentity|AmbiguousService
Issue-Key: lambda__sesv2.lambda-sesv2-delete-suppressed-destination-positive|run#5|AssertionError|assert pre_code == "NotFoundException", f"expected NotFoundException before any put, got {pre_code!r}"
Issue-Key: sesv2|PutSuppressedDestination|AmbiguousService
Issue-Key: lambda__sesv2|CreateConfigurationSet|AmbiguousService
Issue-Key: lambda__sesv2.cli-lambda-sesv2-describe-email-identity|_run_shell#11|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cli-lambda-sesv2-describe-function-config|_run_shell#8|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.lambda-execution-role-wrong-trust-negative|_attempt_create_function_expect_rejection#2|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.lambda-wrong-trust-policy-sesv2-negative|run#9|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-trust-principal-negative|_expect_create_function_rejected#5|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-role-arn-negative|run#5|AssertionError|assert status in {"ROLLBACK_COMPLETE", "CREATE_FAILED"}, f"unexpected stack status: {status}"
Issue-Key: lambda__sesv2.cfn-lambda-sesv2-wrong-role-arn-negative|run#9|AssertionError|assert status in {"ROLLBACK_COMPLETE", "CREATE_FAILED"}, f"unexpected terminal status: {status}"
Issue-Key: lambda__sesv2|GetFunction|ResourceNotFoundException
Issue-Related: 20261023T183615Z-sesv2-is-unreachable-every-request-is-refused-ambiguousservice.md
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# lambda__sesv2: Lambda-driven SESv2 scenarios all fail — 'ses' router ambiguity, Invoke never runs the package, CreateFunction accepts an untrustable role, and CFN reports CREATE_COMPLETE for Lambda functions it never created (5 causes)

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Resolution note (filed with the fix for causes 4 and 5): causes 1 and 3 were fixed by the
> changes that own those surfaces (the sigv4-'ses' router disambiguation and
> `CreateFunction`'s execution-role resolution), and causes 4 and 5 by CloudFormation's
> `AWS::Lambda::Function` / `AWS::Lambda::Permission` provisioners
> (`services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning/`).
> **Cause 2 — `Invoke` runs no deployment package — remains OPEN**, and the units of this
> group that invoke a function still fail on it; it is tracked under
> `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`,
> which owns the in-process execution substrate and the guest-side AWS SDK bridge those
> scenarios need._

## Operation
CreateFunction

## Description
Attribution note: these checks' id token is the composite 'lambda__sesv2'. Its constituent services (lambda, sesv2, plus the cloudformation and iam surfaces the checks drive) are all stood up in this checkout, so the per-service bootstrap gate is FILE. Five root causes behind the 27 lambda__sesv2 failures that are NOT already owned (24 further failures in this group are OWNED by 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md or 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md and are skipped here). Causes 1, 2, 4 and 5 are the same defects those issues describe, under replay keys none of them carries.

=== Cause 1 of 5: SESv2 setup calls are refused 'AmbiguousService' by the sigv4-name router ===
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2): SimpleEmailService_v2, restJson1, sigv4Name 'ses', endpointPrefix 'email'; CreateEmailIdentity POST /v2/email/identities code 200, CreateContactList/CreateConfigurationSet/GetAccount/ListEmailIdentities/ListContactLists/ListSuppressedDestinations likewise 200 with modeled output, refusals being NotFoundException 404 / BadRequestException 400 / TooManyRequestsException 429. Three other on-disk models share sigv4Name 'ses', so the signing name is not a service identity. AmbiguousService is in no AWS model.
Emulator: every SESv2 call made from the test harness (not from inside the function) is answered with code AmbiguousService, message "4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one" (message is an example; the code and 5xx class are the requirement), retried to exhaustion by botocore. Observed on CreateEmailIdentity, CreateContactList, CreateConfigurationSet, GetAccount, ListEmailIdentities, ListContactLists, ListSuppressedDestinations. lambda__sesv2.lambda-sesv2-delete-suppressed-destination-positive asserts the shape directly: 'expected NotFoundException before any put, got AmbiguousService'.
Why: no Lambda-to-SESv2 scenario can even be set up; the whole SESv2 surface is unreachable for every client.
Fix: the request router / service-registry resolution layer (service selection before operation dispatch) must disambiguate the four sigv4-'ses' registrations by protocol + endpoint prefix + request shape. Same defect as 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md cause 1, under new keys.
Traces: lambda__sesv2.cfn-deploy-lambda-sesv2-role-positive, cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected, lambda-sesv2-account-suppression-list-permission-negative, lambda-sesv2-create-contact-missing-permission-negative, lambda-sesv2-create-contact-positive, lambda-sesv2-delete-email-identity-positive, lambda-sesv2-delete-suppressed-destination-positive, lambda-sesv2-get-email-identity-missing-permission-negative, lambda-sesv2-get-email-identity-positive, lambda-sesv2-get-identity-verification-missing-permission-negative, lambda-sesv2-list-email-identities-missing-permission-negative, lambda-sesv2-list-email-identities-positive, lambda-sesv2-list-suppressed-destinations-missing-permission-negative, lambda-sesv2-list-suppressed-destinations-positive, lambda-sesv2-put-mail-from-domain-missing-permission-negative, lambda-sesv2-put-suppressed-destination-missing-permission-negative, lambda-sesv2-put-suppressed-destination-positive, lambda-sesv2-tag-resource-missing-permission-negative, lambda-sesv2-update-configuration-set-event-destination-missing-permission-negative (19 checks).

=== Cause 2 of 5: Invoke refuses a resolved function with NotImplemented instead of running it (reached here through the AWS CLI) ===
AWS: Smithy Invoke — POST /2015-03-31/functions/{FunctionName}/invocations, success 200, output carries StatusCode, Payload (the function's return value), ExecutedVersion and FunctionError only when the function itself failed. The modeled error set (InvalidRequestContentException 400, ResourceNotFoundException 404, RequestTooLargeException 413, TooManyRequestsException 429, ServiceException 500, the 502 runtime family) contains no NotImplemented.
Emulator: `aws lambda invoke` exits rc=254 with 'An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator'. The control plane resolved the function; only execution is absent.
Why: any client reading the function's result gets nothing — no 200, no Payload, and a service refusal indistinguishable from FunctionError. Reached identically through the SDK and the CLI.
Fix: the Invoke handler's execution path plus its error mapping (never NotImplemented for a function that resolved); preserve the existing InvocationType=DryRun validation path. Same defect as 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, under two keys it does not carry.
Traces: lambda__sesv2.cli-lambda-sesv2-describe-email-identity, lambda__sesv2.cli-lambda-sesv2-describe-function-config.

=== Cause 3 of 5: CreateFunction accepts an execution role whose trust policy does not permit lambda.amazonaws.com ===
AWS: Smithy CreateFunction — POST /2015-03-31/functions, success code 201, Role required (pattern ^arn:(aws[a-zA-Z-]*)?:iam::\\d{12}:role/?...$), modeled error InvalidParameterValueException (client, httpError 400, documented 'One of the parameters in the request is not valid'). AWS validates at creation time that Lambda can assume the supplied role and refuses with that 400 when the role's trust policy names another principal (recorded behavior for the refusal condition; the status and error code are the requirement, the message text is not).
Emulator: CreateFunction succeeds (201 + FunctionConfiguration) for a role whose AssumeRolePolicyDocument trusts ec2.amazonaws.com instead of lambda.amazonaws.com — no refusal at all.
Why: an over-permissive negative class. A deployment real AWS rejects looks clean, so a function with an unassumable role ships and only breaks at invocation time, and any suite proving AWS rejects it passes falsely.
Fix: CreateFunction's validation in the Lambda handler — resolve the Role ARN in IAM and reject with InvalidParameterValueException (400) unless its trust policy allows lambda.amazonaws.com to sts:AssumeRole.
Traces: lambda__sesv2.lambda-execution-role-wrong-trust-negative, lambda__sesv2.lambda-wrong-trust-policy-sesv2-negative, lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-trust-principal-negative (the last reaches it through a CFN-provisioned role).

=== Cause 4 of 5: a CloudFormation stack whose Lambda function names a bad role ARN still reaches CREATE_COMPLETE ===
AWS: recorded behavior only — when a template's AWS::Lambda::Function names a role ARN that does not resolve (or that Lambda cannot assume), the resource fails to create, the stack ends in CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED, and the function is never created.
Emulator: describe_stacks reports StackStatus CREATE_COMPLETE with no resource failure and no rollback.
Why: CloudFormation's success signal becomes meaningless for Lambda workloads — a broken template is reported as a clean deploy and the failure surfaces only at runtime.
Fix: the CloudFormation AWS::Lambda::Function provisioner must perform CreateFunction's role validation and fail the resource, and the stack must transition to CREATE_FAILED/ROLLBACK rather than CREATE_COMPLETE when a declared resource fails.
Traces: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-role-arn-negative, lambda__sesv2.cfn-lambda-sesv2-wrong-role-arn-negative.

=== Cause 5 of 5: a CFN-provisioned AWS::Lambda::Function does not exist in Lambda after the stack (or stack update) completes ===
AWS: recorded behavior only — CreateStack/UpdateStack reach CREATE_COMPLETE/UPDATE_COMPLETE only once the function exists, and the resource's PhysicalResourceId is a real function that lambda:GetFunction then resolves (Smithy GetFunction's ResourceNotFoundException, httpError 404, is AWS's answer only for a function that was never created).
Emulator: the stack completes and hands back the physical id, but lambda:GetFunction on it answers 404 ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<physical id>'.
Why: every CloudFormation-deployed Lambda workload is broken end to end — a client gets a 404 for a resource CloudFormation told it exists.
Fix: the CloudFormation stack-provisioning path must create AWS::Lambda::Function resources in Lambda's state (and keep them in sync on UpdateStack) and use the created function's real name/ARN as the PhysicalResourceId. Same defect as cause 1 of 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md, under a key it does not carry.
Traces: lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive.

## Evidence
Suite simparity-behavioral-aws-v1: lambda__sesv2 0/51 passed; 27 failures are net-new (24 owned elsewhere).

Cause 1 — expected: 200 + modeled output from the SESv2 setup call. actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateEmailIdentity operation (reached max retries: 3): 4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one. Same for CreateContactList, CreateConfigurationSet, GetAccount, ListEmailIdentities, ListContactLists, ListSuppressedDestinations. Assertion form: 'expected NotFoundException before any put, got AmbiguousService'; 'unexpected error seeding suppression entry: {Code: AmbiguousService}'.

Cause 2 — expected: rc=0 and the handler's JSON payload. actual: AssertionError: Command failed (rc=254): aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:sim2real-sesv2-fn-6ecb984fa424 at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator.

Cause 3 — expected: CreateFunction refused with InvalidParameterValueException (HTTP 400). actual: AssertionError: create_function unexpectedly succeeded with a role whose trust policy does not include lambda.amazonaws.com; and 'create_function(sim2real-badtrust-fn-7f677bd096d0) unexpectedly SUCCEEDED with a role whose trust policy trusts ec2.amazonaws.com instead of lambda.amazonaws.com'; and 'create_function unexpectedly succeeded for sim2real-xsvc-fn-bad-... with role arn:aws:iam::000000000000:role/sim2real-xsvc-role-stk-...-ExecRole-...'.

Cause 4 — expected: stack status in {CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED}. actual: AssertionError: unexpected stack status: CREATE_COMPLETE / unexpected terminal status: CREATE_COMPLETE.

Cause 5 — expected: GetFunction on the stack resource's PhysicalResourceId succeeds after the stack completes. actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetFunction operation: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-updfn-f134b94ca34f.

## Steps to Reproduce
Cause 1: sesv2.create_email_identity / create_contact_list / create_configuration_set / get_account / list_email_identities / list_suppressed_destinations against the emulator -> 5xx AmbiguousService instead of 200 or the modeled 404/400. Replay the 19 cause-1 ids in trace_ids.
Cause 2: create a python3.12 function, wait Active, then `aws lambda invoke --function-name <name> --payload <json> out.json` -> rc=254 NotImplemented instead of rc=0 with the payload. Replay lambda__sesv2.cli-lambda-sesv2-describe-email-identity and cli-lambda-sesv2-describe-function-config.
Cause 3: iam.create_role with AssumeRolePolicyDocument Principal {'Service': 'ec2.amazonaws.com'}, then lambda.create_function(Role=<that role arn>, Runtime=python3.12, ...) -> 201 instead of 400 InvalidParameterValueException. Replay lambda-execution-role-wrong-trust-negative, lambda-wrong-trust-policy-sesv2-negative, cfn-deploy-lambda-sesv2-wrong-trust-principal-negative.
Cause 4: cloudformation.create_stack with an AWS::Lambda::Function whose Role is a non-resolvable/incorrect ARN; poll describe_stacks -> CREATE_COMPLETE instead of CREATE_FAILED/ROLLBACK_*. Replay cfn-deploy-lambda-sesv2-wrong-role-arn-negative, cfn-lambda-sesv2-wrong-role-arn-negative.
Cause 5: cloudformation.create_stack with {ExecRole: AWS::IAM::Role trusting lambda.amazonaws.com, Fn: AWS::Lambda::Function}; wait CREATE_COMPLETE, read the Fn PhysicalResourceId from describe_stack_resources, then lambda.get_function(FunctionName=<that id>) -> 404 'Function not found'. Repeat after update_stack adding a permission. Replay cfn-stack-update-lambda-sesv2-permission-add-positive.
