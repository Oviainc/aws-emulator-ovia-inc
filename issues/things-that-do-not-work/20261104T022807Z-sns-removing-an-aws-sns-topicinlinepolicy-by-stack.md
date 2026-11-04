Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.deploy-cfn-removing-a-topic-inline-policy-restores-the-default-policy
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.deploy-cfn-removing-a-topic-inline-policy-restores-the-default-policy|_until_refused#15|Timeout|raise
Issue-Commit: 490c1cbe5b0cc74feaea217ae97fc7bca73b7940

# sns: removing an AWS::SNS::TopicInlinePolicy by stack update never restores the topic's default access policy, and the role it granted keeps publishing — the check cannot reach a verdict inside the 60 s cap

> _Note: this issue was identified against the code base at commit `490c1cbe5b0cc74feaea217ae97fc7bca73b7940`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTopicAttributes

## Description
Cause 1 of 1: after an AWS::SNS::TopicInlinePolicy resource is removed from a stack by UpdateStack, the topic's access policy is never put back to the AWS default statement, and SNS Publish on that topic is still never authorized against the topic policy — so the role the removed inline policy had named goes on publishing. The check therefore never reaches either of its two observable outcomes and is killed by the harness at the 60 s cap.

AWS: Smithy (com.amazonaws.sns) models GetTopicAttributes -> Attributes (TopicAttributesMap), which carries the topic's "Policy" as a JSON access-policy document; a topic that has no customer-supplied policy carries exactly one statement whose Sid is the default statement id (recorded behavior: the check requires the statement list to become ["__default_statement_ID"] after the inline policy is deleted). Publish models AuthorizationErrorException with awsQueryError code "AuthorizationError", HTTP 403. AWS's refusal message for a principal that no policy authorizes has the fixed text "User: <principal-arn> is not authorized to perform: SNS:Publish on resource: <topic-arn> because no identity-based policy allows the SNS:Publish action" — the fixed text is everything outside the two substitutions; the variable parts are the caller's assumed-role ARN (arn:<partition>:sts::<account>:assumed-role/<role-name>/<session-name>, all taken from the request's credentials) and the TopicArn from the request. AWS's AWS::SNS::TopicInlinePolicy provider, on resource delete, drops the inline policy so the topic reverts to that default policy; deployment/undeployment of a topic policy is therefore observable through GetTopicAttributes within normal stack-update time.

Emulator: the replay shows the check never completing — it is terminated with "Check timed out after 60.0s". The only unbounded waits in the check after UpdateStack are (a) the 60 s poll that waits for GetTopicAttributes(TopicArn).Attributes["Policy"] statement Sids to become ["__default_statement_ID"], and (b) the loop that waits for the assumed role's Publish to start being refused with AuthorizationError. Both are open-ended only while the emulator keeps answering the old value: the topic policy keeps reporting the deleted inline statement (Sid from the template, here "StackRolePublishes") instead of the single default statement, and Publish by the role keeps answering 200 with a MessageId instead of 403 AuthorizationError. The sibling check sns.deploy-cfn-topic-policy-authorizes-only-the-named-role-and-follows-updates reaches the same wall from the authorization side (open issue 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md), which confirms the topic policy is not consulted at all on Publish.

Why: a client that manages a topic's access policy through CloudFormation cannot revoke a grant. Deleting the TopicInlinePolicy resource reads back as still in force (GetTopicAttributes shows the stale statement), and the principal it granted keeps publishing after the grant was removed, so an infrastructure change that is supposed to close access silently leaves it open. Nothing the client can observe distinguishes a successful revocation from none.

Note on already-filed neighbours: open issue 20261025T025317Z-sns-cloudformation-provisioned-sns-resources-topicinlinepolicy-unknown-a-subscription.md lists this check id, but its cause 1 is CreateStack refusing the template with 400 ValidationError "Unrecognized resource types: [AWS::SNS::TopicInlinePolicy]" — that refusal no longer happens in this replay (the check now runs past stack creation and hangs), so the deletion/revocation behavior described here is not covered by it. Open issue 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md covers topic-policy authorization on Publish generally but does not carry this check.

Fix: the AWS::SNS::TopicInlinePolicy resource provider's delete/update path in the CloudFormation provider layer (it must clear the inline policy and leave the topic with its default policy), and the SNS topic-attribute state that serves GetTopicAttributes "Policy" plus the Publish handler's topic-policy authorization so a removed grant is actually enforced with 403 AuthorizationError.

Traces: sns.deploy-cfn-removing-a-topic-inline-policy-restores-the-default-policy.

## Evidence
Expected (AWS): after UpdateStack removes the AWS::SNS::TopicInlinePolicy resource, GetTopicAttributes(TopicArn) returns a Policy document whose only statement Sid is the default statement id ("__default_statement_ID"), and Publish by the assumed role the removed policy had named is refused 403 AuthorizationError with message "User: arn:<partition>:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: SNS:Publish on resource: <topic-arn> because no identity-based policy allows the SNS:Publish action" (fixed text as quoted; principal ARN and topic ARN substituted from the request).

Actual (emulator): the check is terminated by the harness — "Error: Check timed out after 60.0s" — with no verdict. It is blocked in its post-update waits: the topic policy never changes to the single default statement (it keeps the template's inline statement Sid, e.g. "StackRolePublishes"), and Publish by that role keeps returning 200 with a MessageId instead of 403 AuthorizationError, so neither the policy poll nor the until-refused loop can ever terminate.

Failing check: sns.deploy-cfn-removing-a-topic-inline-policy-restores-the-default-policy (verdict: mismatch).

## Steps to Reproduce
1. CreateStack with Capabilities=[CAPABILITY_IAM] and a template declaring: AWS::SNS::Topic (Topic), AWS::IAM::Role (Role) trusting arn:<partition>:iam::<account>:root for sts:AssumeRole, and AWS::SNS::TopicInlinePolicy (InlinePolicy) with TopicArn={Ref: Topic} and a PolicyDocument containing one statement Sid="StackRolePublishes", Effect=Allow, Principal={AWS: GetAtt Role.Arn}, Action="sns:Publish", Resource={Ref: Topic}. Outputs: TopicArn, RoleName, RoleArn. Wait for CREATE_COMPLETE.
2. GetTopicAttributes(TopicArn) and parse Attributes["Policy"] -> the statement Sid list must be ["StackRolePublishes"].
3. sts:AssumeRole into RoleArn and Publish(TopicArn, Message="sim2real-allowed") with those credentials -> must succeed.
4. UpdateStack with the same template minus the InlinePolicy resource; wait for UPDATE_COMPLETE.
5. Poll GetTopicAttributes(TopicArn) -> within seconds the Policy statement Sid list must become ["__default_statement_ID"]. Observed: it keeps returning the removed inline statement, so the poll never satisfies.
6. Publish(TopicArn, Message=...) with the same assumed-role credentials -> must be refused 403 AuthorizationError with the message quoted in Evidence. Observed: it keeps succeeding with a MessageId, and the check is killed at the 60 s harness cap.
