Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns.deploy-sns-topic-inline-policy-lifecycle-then-not-found cloudformation__sns.deploy-sns-topic-inline-policy-missing-policydocument-rejected cloudformation__sns.deploy-sns-topic-policy-missing-policydocument-rejected cloudformation__sns.deploy-sns-topic-maximum-message-size-boundary-rejected cloudformation__sns.sns-topic-display-name-too-long-rejected cloudformation__sns.deploy-stackset-self-managed-provisions-a-real-stack-instance-then-removes-it cloudformation__sns.stackset-invalid-name-pattern-rejected-then-succeeds-with-a-valid-name cloudformation__sns.stack-tags-plus-system-tags-exceed-topic-tag-limit-fails
Issue-Key: cloudformation__sns.deploy-sns-topic-inline-policy-lifecycle-then-not-found|run#18|AssertionError|assert outputs["InlinePolicyRef"] == topic_arn, (
Issue-Key: cloudformation__sns.deploy-sns-topic-inline-policy-missing-policydocument-rejected|failed_reason#18|AssertionError|assert len(rows) == 1, f"expected one {status} row for {logical_id}, got {rows!r}"
Issue-Key: cloudformation__sns.deploy-sns-topic-policy-missing-policydocument-rejected|run#20|AssertionError|assert stack_failed_reasons == [_STACK_REASON], (
Issue-Key: cloudformation__sns.deploy-sns-topic-maximum-message-size-boundary-rejected|_assert_rejected#3|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__sns.sns-topic-display-name-too-long-rejected|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: cloudformation__sns.deploy-stackset-self-managed-provisions-a-real-stack-instance-then-removes-it|run#12|AssertionError|assert listed == [outputs["StackSetId"]], (
Issue-Key: cloudformation__sns.stackset-invalid-name-pattern-rejected-then-succeeds-with-a-valid-name|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__sns.stack-tags-plus-system-tags-exceed-topic-tag-limit-fails|run#25|AssertionError|assert masked == expected, masked
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__sns: 6 net-new causes — TopicInlinePolicy Ref is not the topic ARN, no required-property validation, no SNS property-constraint enforcement, StackSets not listed, invalid StackSet name accepted, tag-quota reason contains a literal {request_token} placeholder

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Six distinct root causes. Each is numbered with its own failing check ids; all six must be closed before this issue is done.

Cause 1 of 6: Ref on AWS::SNS::TopicInlinePolicy is a generated name instead of the topic ARN.
AWS: Ref on AWS::SNS::TopicInlinePolicy returns the ARN of the topic the policy is attached to.
Emulator: Ref returns `<stack-name>-<logical-id>-<suffix>`.
Why: a template that passes that Ref onward hands downstream code a value SNS does not accept as a TopicArn.
Fix: the AWS::SNS::TopicInlinePolicy handler's physical-resource-id/Ref resolution.
Traces: cloudformation__sns.deploy-sns-topic-inline-policy-lifecycle-then-not-found.

Cause 2 of 6: a missing required PolicyDocument on the SNS policy resources produces no CREATE_FAILED event at all.
AWS: CloudFormation's resource-schema validation catches the missing required property; the resource gets a CREATE_FAILED event and the stack's reason is `Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` — the count comes from the request, the rest is fixed text.
Emulator: there is no CREATE_FAILED row for the stack at all, and the stack reason list is empty, for both AWS::SNS::TopicInlinePolicy and AWS::SNS::TopicPolicy.
Why: a client cannot tell that its template is schema-invalid, nor which property is missing.
Fix: CloudFormation's resource-schema required-property validation and the CREATE_FAILED event/reason it must emit.
Traces: cloudformation__sns.deploy-sns-topic-inline-policy-missing-policydocument-rejected, .deploy-sns-topic-policy-missing-policydocument-rejected.

Cause 3 of 6: AWS::SNS::Topic property value constraints are not enforced, so the stack reaches CREATE_COMPLETE.
AWS: SNS refuses a MaximumMessageSize below its allowed minimum and a DisplayName longer than the allowed maximum; CloudFormation then marks the resource CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: both stacks end `CREATE_COMPLETE`.
Why: a client believes it configured a topic AWS would reject, so the topic's real limits differ from its template.
Fix: the AWS::SNS::Topic handler's property validation / propagation of the SNS refusal.
Traces: cloudformation__sns.deploy-sns-topic-maximum-message-size-boundary-rejected, .sns-topic-display-name-too-long-rejected.

Cause 4 of 6: a stack set a stack creates is not returned by ListStackSets.
AWS: after the stack completes, ListStackSets returns the stack set, findable by the name the stack's Ref reports, and its self-managed instance can be provisioned and removed.
Emulator: ListStackSets returns an empty list for the stack's Ref.
Why: a client that declares a stack set in a template cannot find or operate it afterwards.
Fix: the AWS::CloudFormation::StackSet resource handler must register the stack set in CloudFormation state.
Traces: cloudformation__sns.deploy-stackset-self-managed-provisions-a-real-stack-instance-then-removes-it.

Cause 5 of 6: a StackSet name that breaks the name pattern is accepted and the stack reaches CREATE_COMPLETE.
AWS: CreateStackSet refuses a StackSetName that does not match the allowed pattern, so the resource is CREATE_FAILED and the stack ROLLBACK_COMPLETE; only the retry with a valid name succeeds.
Emulator: the stack with the invalid name ends `CREATE_COMPLETE`.
Why: invalid stack-set names are silently accepted, so a client's later operations against that name fail with no deploy-time signal.
Fix: the AWS::CloudFormation::StackSet handler's name validation.
Traces: cloudformation__sns.stackset-invalid-name-pattern-rejected-then-succeeds-with-a-valid-name.

Cause 6 of 6: the tag-quota failure reason carries an unsubstituted `{request_token}` placeholder.
AWS: a handler failure reason is `Resource handler returned message: "<the service's message>" (RequestToken: <the operation's request token>, HandlerErrorCode: <code>)`; the RequestToken value is the real token for that operation. The fixed text is the wrapper; the message, token and code are values.
Emulator: the reason is emitted literally as `Resource handler returned message: "Could not complete request: tag quota of per resource exceeded" (RequestToken: {request_token}, HandlerErrorCode: ServiceLimitExceeded)` — the format string was never interpolated, so `{request_token}` appears verbatim.
Why: any client or tool that parses the request token out of a failure reason reads the literal placeholder, and the reason is not a value AWS ever emits.
Fix: the handler-failure-reason formatting in the CloudFormation provisioner (the RequestToken substitution).
Traces: cloudformation__sns.stack-tags-plus-system-tags-exceed-topic-tag-limit-fails.

## Evidence
Cause 1: AssertionError: Ref InlinePolicy was '<stack>-InlinePolicy-<suffix>', expected the topic arn 'arn:aws:sns:us-west-2:000000000000:<topic>'.
Cause 2: AssertionError: expected one CREATE_FAILED row for <stack>, got []; and expected the stack's CREATE_FAILED reason to be 'Validation failed with 1 error(s). Call DescribeEvents ...', got [].
Cause 3: AssertionError: expected MaximumMessageSize=100 to roll back, ended in 'CREATE_COMPLETE'; AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'.
Cause 4: AssertionError: ListStackSets [] for Ref 'sim2real-behav-cfn-ss-<id>'.
Cause 5: AssertionError: rejected stack ended 'CREATE_COMPLETE'.
Cause 6: AssertionError: Resource handler returned message: "Could not complete request: tag quota of per resource exceeded" (RequestToken: {request_token}, HandlerErrorCode: ServiceLimitExceeded) — the literal '{request_token}' is in the emitted reason.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: deploy AWS::SNS::TopicInlinePolicy and read its Ref.
3. Cause 2: CreateStack with AWS::SNS::TopicInlinePolicy and with AWS::SNS::TopicPolicy, each lacking PolicyDocument; read DescribeStackEvents.
4. Cause 3: CreateStack with AWS::SNS::Topic MaximumMessageSize=100, and with an over-long DisplayName.
5. Cause 4: deploy AWS::CloudFormation::StackSet and call ListStackSets.
6. Cause 5: deploy AWS::CloudFormation::StackSet with a StackSetName breaking the name pattern.
7. Cause 6: deploy a topic with enough stack tags plus system tags to exceed the topic tag quota and read the resource's failure reason.
