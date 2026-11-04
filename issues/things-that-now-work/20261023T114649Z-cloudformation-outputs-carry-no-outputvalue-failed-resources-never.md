Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-create-positive apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-update-positive apigateway__cloudformation__lambda.cfn-custom-resource-signals-success-updates-stack-status-positive apigateway__cloudformation__lambda.cfn-nested-stack-lambda-provider-reuse-positive apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created cloudformation__lambda.cfnuf-failed-alias-update-leaves-live-alias-serving-previous-version cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue apigateway__cloudformation__lambda.cfn-lambda-role-missing-trust-principal-rejected apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure apigateway__cloudformation__lambda.cfn-custom-resource-lambda-missing-servicetoken-permission-rejected apigateway__cognito-idp__lambda.cfn-lambda-role-missing-trust-policy-negative apigateway__cognito-idp__lambda.cfn-lambda-role-wrong-service-principal-negative cloudformation__iam__lambda__s3.cfn-custom-resource-lambda-missing-s3-permission-negative cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative cloudformation__iam__lambda__s3.cfn-passrole-denied-for-service-role-negative cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back apigateway__cloudformation__lambda.sam-invalid-document-error-taxonomy-rejected cloudformation__iam__lambda__s3.cfn-stack-named-iam-role-requires-capability-named-iam-negative cloudformation__iam__lambda__s3.cfn-stack-with-iam-role-requires-capability-iam-negative cloudformation__iam__lambda__s3.cfn-template-url-s3-missing-object-negative cloudformation__iam__lambda__s3.cfn-template-url-s3-versioned-object-positive cloudformation__lambda__s3__sqs.cfn-template-from-s3-positive cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals cloudformation__lambda__ssm.cfn-ssm-parameter-type-resolution-and-rule-assertion-negative cloudformation__iam__lambda__s3.cfn-iam-role-created-before-lambda-function-positive cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive apigateway__cognito-idp__lambda.cfn-apigw-authorizer-wrong-userpool-arn-negative apigateway__cognito-idp__lambda.cfn-apigw-cognito-authorizer-stack-positive events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-target-role-missing-trust-negative events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-create-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-update-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-signals-success-updates-stack-status-positive|run#9|KeyError|marker_matches = [o["OutputValue"] for o in outputs if o["OutputKey"] == "MarkerOutput"]
Issue-Key: apigateway__cloudformation__lambda.cfn-nested-stack-lambda-provider-reuse-positive|_deploy_cfn#8|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly|_deploy_cfn#5|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created|_deploy_cfn#5|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__lambda.cfnuf-failed-alias-update-leaves-live-alias-serving-previous-version|outputs#5|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in self.describe().get("Outputs") or []}
Issue-Key: cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue|_outputs#15|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-lambda-role-missing-trust-principal-rejected|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure|run#8|AssertionError|raise AssertionError(f"expected ROLLBACK_COMPLETE, got {status}. {detail}")
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-lambda-missing-servicetoken-permission-rejected|run#11|AssertionError|assert status == "ROLLBACK_COMPLETE"
Issue-Key: apigateway__cognito-idp__lambda.cfn-lambda-role-missing-trust-policy-negative|run#3|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected the stack to roll back, got {status}"
Issue-Key: apigateway__cognito-idp__lambda.cfn-lambda-role-wrong-service-principal-negative|run#4|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected the stack to roll back, got {status}"
Issue-Key: cloudformation__iam__lambda__s3.cfn-custom-resource-lambda-missing-s3-permission-negative|run#11|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"expected rollback, got {bad_status}"
Issue-Key: cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative|run#10|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"expected rollback, got {bad_status}"
Issue-Key: cloudformation__iam__lambda__s3.cfn-passrole-denied-for-service-role-negative|run#22|AssertionError|assert final_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back|run#11|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back|run#9|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: apigateway__cloudformation__lambda.sam-invalid-document-error-taxonomy-rejected|run#4|AssertionError|assert described["Status"] == "FAILED", (
Issue-Key: cloudformation__iam__lambda__s3.cfn-stack-named-iam-role-requires-capability-named-iam-negative|_error_code#6|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation__iam__lambda__s3.cfn-stack-with-iam-role-requires-capability-iam-negative|_error_code#5|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation__iam__lambda__s3.cfn-template-url-s3-missing-object-negative|run#10|AssertionError|assert err == want, f"expected {want!r}, got {err!r}"
Issue-Key: cloudformation__iam__lambda__s3|CreateStack|NotImplemented
Issue-Key: cloudformation__lambda__s3__sqs|CreateStack|NotImplemented
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals|run#14|AssertionError|assert physical_id.startswith("sim2real-xsvc-cr-"), (
Issue-Key: cloudformation__lambda__ssm.cfn-ssm-parameter-type-resolution-and-rule-assertion-negative|run#11|AssertionError|assert outputs["ResolvedBootstrapVersion"] == "21", (
Issue-Key: cloudformation__iam__lambda__s3.cfn-iam-role-created-before-lambda-function-positive|_timestamp#7|AssertionError|raise AssertionError(f"no {status} event found for {logical_id}")
Issue-Key: cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive|_timestamp#12|AssertionError|raise AssertionError(f"no {status} event found for {logical_id}")
Issue-Key: cognito-identity-provider|AdminCreateUser|InvalidParameterException
Issue-Key: events__lambda__s3__stepfunctions|ListExecutions|InvalidArn
Issue-Key: sfn|StartExecution|InvalidArn
> _Disposition of the eight numbered causes, so this record does not claim more than was
> done:_
>
> * _Cause 1 (Outputs carry no `OutputValue`) — DECLINED, platform blocker: every unit of it
>   reads a custom resource's response `Data`, which needs the provider to be INVOKED, and
>   `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs` answers
>   `RequestResponse` with HTTP 501 because this emulator has no in-process runtime for a
>   deployment package._
> * _Cause 2 (a failed resource never rolls the stack back) — DECLINED, the same blocker: the
>   rollback machinery is real and exercised (`tests/all/malformed_policy_document_rollback.rs`);
>   what is missing is the provider run, or the `AWS::Lambda::Function` provisioning the
>   related issue below owns._
> * _Cause 3 (the IAM capability gate) — FIXED (`src/state/template.rs`, `missing_capabilities`)._
> * _Cause 4 (`TemplateURL`) — FIXED (`src/state/template_url.rs`)._
> * _Cause 5 (a custom resource's response body is ignored) — DECLINED, the same blocker as
>   cause 1._
> * _Cause 6 (SSM-typed parameters and dynamic references) — FIXED
>   (`src/state/ssm_parameters.rs`, `src/state/dynamic_references.rs`)._
> * _Cause 7 (per-resource lifecycle events) — FIXED (`src/state/timeline.rs`)._
> * _Cause 8 (physical ids are the logical id) — DECLINED, platform blocker: it needs a
>   provisioner arm per type in `src/state/provisioner.rs`, which is 28,667 lines, and the
>   merge gate's file-length reviewer refuses any fix that touches a file over 5,000 lines
>   until it is split._

Issue-Commit: 048778663a26e363156c6db724999c4fb229af36


# CloudFormation: Outputs carry no OutputValue, failed resources never roll the stack back, capabilities and dynamic references are ignored, and physical ids are logical ids

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Eight distinct root causes in the CloudFormation emulation. Each is listed with the failing checks it is grounded in.

Cause 1 of 8 - DescribeStacks Output entries omit OutputValue.
AWS: Smithy (cloudformation#Output): the Output structure has members OutputKey, OutputValue ("The value associated with the output"), Description and ExportName; a stack in CREATE_COMPLETE reports every declared Output with its resolved OutputValue.
Emulator: the Output entry exists but has no OutputValue member, so every consumer raises KeyError: 'OutputValue'.
Why: Outputs are the documented way to hand resource identifiers (table names, stream ARNs, function names, queue URLs) from a stack to its callers; with no value, nothing downstream of a stack can be wired up.
Fix: the Outputs resolution/serialization in the stack-describe path (resolve each Output's Value expression and emit OutputValue).
Checks: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-create-positive, apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive, apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-update-positive, apigateway__cloudformation__lambda.cfn-custom-resource-signals-success-updates-stack-status-positive, apigateway__cloudformation__lambda.cfn-nested-stack-lambda-provider-reuse-positive, apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly, apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created, cloudformation__lambda.cfnuf-failed-alias-update-leaves-live-alias-serving-previous-version, cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive, cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue.

Cause 2 of 8 - a stack whose resource creation fails still reaches CREATE_COMPLETE instead of rolling back.
AWS: when a resource in a CreateStack fails, the stack goes CREATE_FAILED -> ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE (with OnFailure/DisableRollback defaults), and DescribeStacks reports that terminal status. This applies to a role whose trust policy makes it unusable, a template referencing a nonexistent nested template or region, a denied iam:PassRole, a custom resource that answers Status=FAILED, and a dynamic reference that cannot be resolved.
Emulator: the stack reports CREATE_COMPLETE in all those cases. Replay: "expected ROLLBACK_COMPLETE, got CREATE_COMPLETE", "expected the stack to roll back, got CREATE_COMPLETE", "a custom resource that answered FAILED left the stack in CREATE_COMPLETE, expected ROLLBACK_COMPLETE (CREATE_COMPLETE here means the response Status was ignored)".
Why: a deployment that did not work is reported as successful, so callers proceed against resources that do not exist and no negative deployment test can fail.
Fix: the stack provisioning state machine - propagate per-resource create failures into CREATE_FAILED and run the rollback transition.
Checks: apigateway__cloudformation__lambda.cfn-lambda-role-missing-trust-principal-rejected, apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure, apigateway__cloudformation__lambda.cfn-custom-resource-lambda-missing-servicetoken-permission-rejected, apigateway__cognito-idp__lambda.cfn-lambda-role-missing-trust-policy-negative, apigateway__cognito-idp__lambda.cfn-lambda-role-wrong-service-principal-negative, cloudformation__iam__lambda__s3.cfn-custom-resource-lambda-missing-s3-permission-negative, cloudformation__iam__lambda__s3.cfn-nested-stack-template-wrong-region-negative, cloudformation__iam__lambda__s3.cfn-passrole-denied-for-service-role-negative, cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back, cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative, cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back, apigateway__cloudformation__lambda.sam-invalid-document-error-taxonomy-rejected.

Cause 3 of 8 - the IAM capability acknowledgement is not enforced.
AWS: Smithy (cloudformation#Capability enum: CAPABILITY_IAM, CAPABILITY_NAMED_IAM, CAPABILITY_AUTO_EXPAND). A CreateStack whose template declares IAM resources without CAPABILITY_IAM - or named IAM resources without CAPABILITY_NAMED_IAM - is refused with InsufficientCapabilitiesException.
Emulator: the call succeeds and returns a StackId (HTTP 200).
Why: the guard rail that stops a template from silently minting IAM principals is absent, so a template that real AWS refuses deploys here.
Fix: capability validation in the CreateStack/UpdateStack handler, keyed off the resource types the template declares.
Checks: cloudformation__iam__lambda__s3.cfn-stack-named-iam-role-requires-capability-named-iam-negative, cloudformation__iam__lambda__s3.cfn-stack-with-iam-role-requires-capability-iam-negative.

Cause 4 of 8 - CreateStack with TemplateURL is unimplemented, so an S3-hosted template cannot be deployed or validated.
AWS: CreateStack accepts TemplateURL pointing at an S3 object; a missing object is refused with ValidationError and the message "S3 object does not exist. Bucket: <b>, Key: <k>. Error: NoSuchKey"; an existing object is fetched and deployed.
Emulator: ('NotImplemented', "operation CreateStack with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator") - a 501 where AWS answers a 400 ValidationError, and no S3-hosted template deploys at all.
Why: the standard packaging flow (aws cloudformation package / CDK / SAM upload-then-deploy) cannot be exercised, and the negative case is misclassified as a server error.
Fix: TemplateURL fetching in the CreateStack handler, plus the ValidationError mapping for a missing/unreadable object.
Checks: cloudformation__iam__lambda__s3.cfn-template-url-s3-missing-object-negative, cloudformation__iam__lambda__s3.cfn-template-url-s3-versioned-object-positive, cloudformation__lambda__s3__sqs.cfn-template-from-s3-positive.

Cause 5 of 8 - a custom resource's response body is ignored.
AWS: the provider PUTs a response document to the pre-signed URL; CloudFormation adopts PhysicalResourceId from that body (so Ref of the custom resource returns the responder-minted id) and honours Status (FAILED fails the resource).
Emulator: Ref returns an emulator-minted id derived from the stack and logical id (e.g. '<stack>-Recorded-<suffix>') instead of the id in the response body, and a Status=FAILED response leaves the resource successful.
Why: custom-resource providers return the real id of the thing they created; discarding it breaks every Ref/GetAtt that depends on it, and discarding Status makes a failing provider invisible.
Fix: the custom-resource response handling in the stack provisioner (parse the response document; adopt PhysicalResourceId; map Status to resource success/failure).
Checks: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals, cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative.

Cause 6 of 8 - SSM dynamic references and SSM-typed parameters are not resolved.
AWS: Ref of a parameter of type AWS::SSM::Parameter::Value<String> substitutes the parameter's VALUE at deploy time, and {{resolve:ssm:<name>}} resolves to the parameter value (an unresolvable one fails the stack, see Cause 2).
Emulator: Ref returns the parameter NAME, e.g. '/sim2real-xsvc-bootstrap-ok-...' where the value '21' was expected.
Why: templates parameterised through Parameter Store deploy with the name of the parameter substituted into env vars and properties, so the deployed resources are misconfigured while the stack reports success.
Fix: parameter/dynamic-reference resolution in the template processor.
Checks: cloudformation__lambda__ssm.cfn-ssm-parameter-type-resolution-and-rule-assertion-negative, cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back.

Cause 7 of 8 - DescribeStackEvents does not emit per-resource lifecycle events.
AWS: every resource produces CREATE_IN_PROGRESS/CREATE_COMPLETE events (and DELETE_IN_PROGRESS/DELETE_COMPLETE on delete) with LogicalResourceId, so ordering and completion can be read from the event stream.
Emulator: the events are absent. Replay: "no CREATE_COMPLETE event found for ExecRole", "no DELETE_COMPLETE event found for Function" (only stack-level events are present, e.g. '<stack>=CREATE_IN_PROGRESS:User Initiated').
Why: callers and the console determine resource ordering, failure attribution and completion from stack events; with only stack-level events nothing can be attributed to a resource.
Fix: event emission in the stack provisioner, one event pair per resource transition.
Checks: cloudformation__iam__lambda__s3.cfn-iam-role-created-before-lambda-function-positive, cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive.

Cause 8 of 8 - physical resource ids are the logical/stack-derived name rather than the id the owning service mints.
AWS: the physical id of a resource is the identifier its service issues - AWS::Cognito::UserPool -> a pool id matching ^[\w-]+_[0-9a-zA-Z]+$, AWS::StepFunctions::StateMachine -> the state-machine ARN - and Ref/GetAtt and every cross-service call use it.
Emulator: the physical id is '<stack-name>-<LogicalId>-<suffix>', so the owning service rejects it: cognito-idp AdminCreateUser answers InvalidParameterException "Value '<stack>-UserPool-<suffix>' at 'userPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w-]+_[0-9a-zA-Z]+$", and sfn ListExecutions/StartExecution answer InvalidArn "Invalid ARN prefix: '<stack>-NegStateMachine-<suffix>'".
Why: nothing a stack creates can be addressed through its own service API, so every cross-service flow that starts from a stack is dead.
Fix: the resource providers for these types must mint (or adopt from the service) the service-shaped physical id and store it as the resource's PhysicalResourceId.
Checks: apigateway__cognito-idp__lambda.cfn-apigw-authorizer-wrong-userpool-arn-negative, apigateway__cognito-idp__lambda.cfn-apigw-cognito-authorizer-stack-positive, events__lambda__s3__sfn.cfn-deploy-eventbridge-target-role-missing-trust-negative, events__lambda__s3__sfn.cfn-deploy-sfn-task-token-lambda-callback-stack.

Related already-filed issues that this one does NOT cover: 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md (AWS::Lambda::Function resources never materialise) and 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never.md (which owns the AWS::IAM::Role GetRole NoSuchEntity failures). The causes above are distinct from both.

## Evidence
Cause 1: expected Outputs[i] == {OutputKey, OutputValue, ...}; actual the entry has no OutputValue -> KeyError: 'OutputValue'.
Cause 2: expected DescribeStacks StackStatus ROLLBACK_COMPLETE; actual CREATE_COMPLETE ("expected ROLLBACK_COMPLETE, got CREATE_COMPLETE. <stack>=CREATE_COMPLETE: | <stack>=CREATE_IN_PROGRESS:User Initiated").
Cause 3: expected HTTP 400 InsufficientCapabilitiesException; actual HTTP 200 with {'StackId': 'arn:aws:cloudformation:us-west-2:...:stack/<name>/<uuid>', 'OperationId': ...}.
Cause 4: expected ('ValidationError', 'S3 object does not exist. Bucket: <b>, Key: templates/does-not-exist.json. Error: NoSuchKey'); actual ('NotImplemented', 'operation CreateStack with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator').
Cause 5: expected Ref -> the PhysicalResourceId in the provider's response body, and Status=FAILED -> ROLLBACK_COMPLETE; actual Ref -> '<stack>-Recorded-<suffix>' and status CREATE_COMPLETE.
Cause 6: expected Ref of an AWS::SSM::Parameter::Value<String> parameter -> the value '21'; actual the parameter name '/sim2real-xsvc-bootstrap-ok-<suffix>'.
Cause 7: expected a CREATE_COMPLETE event for logical id ExecRole and a DELETE_COMPLETE event for Function; actual no such events.
Cause 8: expected a physical id matching the owning service's id shape; actual '<stack>-<LogicalId>-<suffix>', rejected downstream with cognito-idp InvalidParameterException (400) and sfn InvalidArn (400).

## Steps to Reproduce
Cause 1: CreateStack with an Outputs section, wait CREATE_COMPLETE, DescribeStacks -> Outputs entries lack OutputValue.
Cause 2: CreateStack with a resource that cannot be created (e.g. a Lambda execution role whose trust policy names the wrong service principal, or a custom resource whose provider answers Status=FAILED), poll DescribeStacks -> CREATE_COMPLETE.
Cause 3: CreateStack with a template declaring AWS::IAM::Role and no Capabilities (and again with RoleName set and only CAPABILITY_IAM) -> 200.
Cause 4: PutObject a template to S3, CreateStack with TemplateURL -> NotImplemented; repeat with a missing key -> NotImplemented instead of ValidationError.
Cause 5: CreateStack with an AWS::CloudFormation::CustomResource whose provider returns a PhysicalResourceId of its own and, separately, Status=FAILED; Ref the resource / read the stack status.
Cause 6: PutParameter /p = 21; CreateStack with a parameter of type AWS::SSM::Parameter::Value<String> defaulting to /p and an Output of Ref of it -> the name is returned.
Cause 7: CreateStack then DeleteStack, DescribeStackEvents -> no per-resource CREATE_COMPLETE/DELETE_COMPLETE events.
Cause 8: CreateStack declaring AWS::Cognito::UserPool (or AWS::StepFunctions::StateMachine), take Ref of it and call cognito-idp AdminCreateUser (or sfn ListExecutions) with it -> rejected on the id shape.
