Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative|run#15|AssertionError|assert good_status == 200, f"expected 200, got {good_status}: {good_body!r}"
Issue-Key: apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive|_update_stack_and_wait#12|AssertionError|raise AssertionError(f"Stack update {stack_name} ended in {stack['StackStatus']}")
Issue-Key: apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https|run#35|AssertionError|assert sorted(stages) == ["Prod", "Stage"], (
Issue-Commit: 63b1f768cca526e685e8991a6e4ba2a43400c274

# apigateway/cloudformation/lambda: non-proxy AWS integration with a correctly-trusted credentials role answers 500, a Deployment-logical-id swap update rolls back, and the SAM implicit API synthesizes only one stage (3 causes)

> _Note: this issue was identified against the code base at commit `63b1f768cca526e685e8991a6e4ba2a43400c274`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Three net-new root causes in the apigateway__cloudformation__lambda scenario, all grounded in failing checks the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 3 — a REST non-proxy AWS integration whose `credentials` role DOES trust apigateway.amazonaws.com answers HTTP 500 instead of the integration's 200.
AWS: Smithy (apigateway) PutIntegration carries `credentials`, the IAM role ARN API Gateway assumes to call the integration target; when that role's AssumeRolePolicyDocument trusts apigateway.amazonaws.com and its policy permits lambda:InvokeFunction, the method invokes the function and the stage serves the integration response (200 with the handler body). The 5xx belongs only to the sibling method whose role trusts the wrong principal (ec2.amazonaws.com) — that one must fail and must never invoke the function. Recorded behavior: the control (good-role) request is 200.
Emulator: the good-role method answers 500 with body {"message":"Internal server error"} — exactly the status/body the bad-role method is supposed to produce, so the positive and negative arms of the behavior are indistinguishable.
Why: a client cannot tell a genuinely mis-trusted credentials role from a working one; every REST non-proxy AWS integration looks permanently broken, and permission-boundary diagnosis built on the 200/5xx split is impossible.
Fix: the execute-api data-plane handler for REST non-proxy AWS integrations — resolution/assumption of the integration `credentials` role and the downstream integration call.
Failing check: apigateway__cloudformation__lambda.cfn-apigw-role-missing-trust-principal-negative.

Cause 2 of 3 — a stack UPDATE that changes an AWS::ApiGateway::Deployment logical id and repoints the method integration ends UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
AWS: Smithy (cloudformation) UpdateStack followed by DescribeStacks reports StackStatus UPDATE_COMPLETE when every resource operation succeeds; UPDATE_ROLLBACK_COMPLETE is reserved for an update in which a resource operation failed and CloudFormation reverted the stack. Deleting the old Deployment logical id and creating a new one (the standard 'versioned logical id' pattern, because no AWS::ApiGateway::Deployment property forces replacement) is a supported update and must succeed, after which the stage serves the newly captured integration.
Emulator: DescribeStacks reports UPDATE_ROLLBACK_COMPLETE, so the swapped Lambda target never takes effect and the stack is left on the pre-update template.
Why: any redeploy-by-logical-id workflow — the only template-level way to force a fresh API Gateway deployment snapshot — silently reverts, so clients keep serving stale integrations while CloudFormation reports a terminal rollback.
Fix: the CloudFormation update path for the AWS::ApiGateway::Deployment / AWS::ApiGateway::Method resource providers (delete-then-create of a replaced Deployment and the integration uri update).
Failing check: apigateway__cloudformation__lambda.cfn-apigw-stack-update-swap-lambda-target-positive.

Cause 3 of 3 — the SAM transform of an implicit API synthesizes only the `Prod` stage; the stage its explicit Deployment resource declares is missing.
AWS: a SAM template whose function carries an Api event and that also declares the ServerlessRestApi Deployment produces exactly two stages on the implicit REST API — the servable `Prod` stage and the `Stage` stage the deployment resource declares. Smithy (apigateway) GetStages (GET /restapis/{restApiId}/stages, output Stages.item: list of Stage) lists both.
Emulator: GetStages returns ['Prod'] only.
Why: templates and clients that address the second stage (or enumerate stages to pick one) see an API that is missing half its deployment surface, and the SAM-synthesized resource set does not match what the transform is specified to emit.
Fix: the AWS::Serverless transform in the CloudFormation template processor — stage synthesis for an implicit ServerlessRestApi.
Failing check: apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https.

## Evidence
Cause 1: AssertionError: expected 200, got 500: b'{"message":"Internal server error"}' (check line 116, asserting the good-role method's status). Expected 200 with the echo handler's body; the fixed text of the emulator's body is the generic API Gateway {"message":"Internal server error"} envelope, which belongs to the bad-role arm only.
Cause 2: AssertionError: Stack update <stack-name> ended in UPDATE_ROLLBACK_COMPLETE (raised from _update_stack_and_wait). Expected DescribeStacks StackStatus UPDATE_COMPLETE. The stack name is a per-run generated value, not part of the requirement.
Cause 3: AssertionError: SAM's implicit API carries exactly two stages: the servable `Prod` and the `Stage` stage its deployment resource declares. Got ['Prod'].
None of the three checks carried an 'ALREADY FILED' or 'OWNED by' marker in the replay report.

## Steps to Reproduce
Cause 1: CreateStack a template with two echo Lambda functions and two REST methods using non-proxy AWS integrations; give one method's integration a credentials role whose AssumeRolePolicyDocument trusts apigateway.amazonaws.com (plus lambda:InvokeFunction) and the other a role trusting ec2.amazonaws.com; CreateDeployment, then GET the good method's stage URL -> expect 200 with the handler body, observe 500 {"message":"Internal server error"}.
Cause 2: CreateStack a template with FuncA/FuncB, a Method integrating FuncA and a Deployment under logical id D1; wait CREATE_COMPLETE; UpdateStack with the integration repointed to FuncB and the Deployment under a new logical id D2 -> expect UPDATE_COMPLETE and the stage serving FuncB, observe UPDATE_ROLLBACK_COMPLETE.
Cause 3: CreateStack a SAM template (Transform: AWS::Serverless-2016-10-31) with a function carrying an Api event plus the ServerlessRestApi Deployment resource; after CREATE_COMPLETE call apigateway GetStages on the synthesized rest api id -> expect stage names ['Prod','Stage'], observe ['Prod'].
