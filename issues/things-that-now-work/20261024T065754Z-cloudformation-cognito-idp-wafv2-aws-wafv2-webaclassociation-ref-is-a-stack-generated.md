Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-idp__wafv2.webacl-association-follows-template-across-replacement-and-stack-delete cloudformation__cognito-idp__wafv2.webacl-association-unsupported-resource-type-rejected
Issue-Key: cloudformation__cognito-idp__wafv2.webacl-association-follows-template-across-replacement-and-stack-delete|run#13|AssertionError|assert out1["AssocRef"] == expected_ref_a, (
Issue-Key: cloudformation__cognito-idp__wafv2.webacl-association-unsupported-resource-type-rejected|run#16|AssertionError|assert bad_result["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# AWS::WAFv2::WebACLAssociation: Ref is a stack-generated id instead of 'ResourceArn|WebACLArn', and an unsupported protected-resource type reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes in CloudFormation's handling of AWS::WAFv2::WebACLAssociation (exercised against a Cognito user pool).

Cause 1 of 2: the association's physical resource id is not the composite identifier.
AWS: the physical resource id of AWS::WAFv2::WebACLAssociation — what Ref returns — is the composite '<ResourceArn>|<WebACLArn>'. That is what makes the association addressable: a template change that re-points either side replaces the resource and the new Ref names the new pair, and a stack delete can disassociate exactly that pair. Recorded behavior only for the separator, which the recording shows as '|'.
Emulator: Ref returns a stack-generated name — recorded example 'sim2real-xsvc-cfn-wafv2-assoc-dfa07424046d-Assoc-9AFE332AFE17' — where the composite 'arn:aws:cognito:…:user-pool/…|arn:aws:wafv2:…:web-acl/…' is expected.
Why: a consumer cannot learn which resource is protected by which web ACL from the stack, and replacement semantics across a re-pointed association are untestable.
Fix: the CloudFormation AWS::WAFv2::WebACLAssociation resource provider's physical-resource-id construction (and the Ref resolution that reads it).
Traces: cloudformation__cognito-idp__wafv2.webacl-association-follows-template-across-replacement-and-stack-delete.

Cause 2 of 2: an association naming an unsupported resource type is accepted.
AWS: AWS::WAFv2::WebACLAssociation only supports the protected-resource types WAF can front (API Gateway stage, ALB, AppSync GraphQL API, Cognito user pool, App Runner service, Verified Access instance, Amplify app). Naming anything else — an SQS queue ARN here — fails the resource and the stack ends in ROLLBACK_COMPLETE; the underlying AssociateWebACL refuses an unsupported ARN (Smithy com.amazonaws.wafv2 AssociateWebACL models WAFInvalidParameterException, client, httpError 400).
Emulator: the stack reaches 'CREATE_COMPLETE' with the SQS queue "associated", so an impossible protection relationship is materialized.
Why: a template that cannot work in AWS deploys cleanly here, and the rollback path a client depends on to detect a mis-typed ResourceArn never runs.
Fix: resource-type validation in the AWS::WAFv2::WebACLAssociation provider (and in the wafv2 AssociateWebACL handler it calls), plus the failure/rollback propagation into stack status.
Traces: cloudformation__cognito-idp__wafv2.webacl-association-unsupported-resource-type-rejected.

## Evidence
Cause 1 — AssertionError: expected Ref(Assoc) to be the composite 'ResourceArn|WebACLArn' 'arn:aws:cognito:us-west-2:000000000000:user-pool/sim2real-xsvc-cfn-wafv2-assoc-dfa07424046d-Pool-283BA2021EA0|arn:aws:wafv2:us-west-2:000000000000:web-acl/sim2real-xsvc-cfn-wafv2-…-a', got 'sim2real-xsvc-cfn-wafv2-assoc-dfa07424046d-Assoc-9AFE332AFE17'.
Cause 2 — AssertionError: expected a WebACLAssociation naming an SQS queue to fail and roll back the stack as ROLLBACK_COMPLETE, got 'CREATE_COMPLETE': None.

## Steps to Reproduce
1. Create a stack with a Cognito user pool, an AWS::WAFv2::WebACL and an AWS::WAFv2::WebACLAssociation binding them, with an output of !Ref on the association; read the output and compare it to '<ResourceArn>|<WebACLArn>' (cause 1).
2. Create a stack whose AWS::WAFv2::WebACLAssociation ResourceArn names an SQS queue and observe the final stack status — CREATE_COMPLETE instead of ROLLBACK_COMPLETE (cause 2).
3. Or replay the check ids in trace_ids.
