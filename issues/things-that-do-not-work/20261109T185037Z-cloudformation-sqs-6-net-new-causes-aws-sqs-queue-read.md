Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cfn-sqs-queue-getatt-unknown-attribute-rejected-rolls-back cloudformation__sqs.cfn-sqs-queue-readonly-arn-ignored-and-ref-getatt-match-sqs cloudformation__sqs.cfn-sqs-queue-readonly-queueurl-set-in-template-rejected-rolls-back cloudformation__sqs.cfn-sqs-queueinlinepolicy-ref-is-queue-url-and-removed-by-stack-update cloudformation__sqs.stack-policy-implicit-deny-for-unnamed-resource cloudformation__sqs.stack-tag-removal-propagates-to-queue-without-replacement cloudformation__sqs.stack-tags-over-fifty-rejected-before-queue-creation
Issue-Key: cloudformation__sqs.cfn-sqs-queue-getatt-unknown-attribute-rejected-rolls-back|run#15|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__sqs.cfn-sqs-queue-readonly-arn-ignored-and-ref-getatt-match-sqs|run#9|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__sqs.cfn-sqs-queue-readonly-queueurl-set-in-template-rejected-rolls-back|run#19|AssertionError|assert masked == [_EXPECTED_REASON], (
Issue-Key: cloudformation__sqs.cfn-sqs-queueinlinepolicy-ref-is-queue-url-and-removed-by-stack-update|run#12|AssertionError|assert inline_policy_ref == queue_url, (
Issue-Key: cloudformation__sqs.stack-policy-implicit-deny-for-unnamed-resource|run#24|AssertionError|assert protected_failures == ["Action not allowed by stack policy"], (
Issue-Key: cloudformation__sqs.stack-tag-removal-propagates-to-queue-without-replacement|run#18|ValueError|queue_done = decisive.index(("Queue", "UPDATE_COMPLETE"))
Issue-Key: cloudformation__sqs.stack-tags-over-fifty-rejected-before-queue-creation|run#22|AssertionError|assert msg == "Tags must have length less than or equal to 50", msg
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 6 net-new causes: AWS::SQS::Queue read-only/GetAtt handling, QueueInlinePolicy Ref, stack-policy denial wording, tag propagation and the CreateStack Tags cap message

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Six further distinct root causes in the cloudformation__sqs suite (separate from the import/refactor/handler-reason issue filed alongside it). Each is listed with its own failing check ids; all six must be closed.

Cause 1 of 6 — an unknown Fn::GetAtt attribute on AWS::SQS::Queue is accepted. AWS: a template referencing an attribute the resource type does not expose fails the stack (the GetAtt cannot be resolved) and it rolls back. Emulator: the stack reaches CREATE_COMPLETE, i.e. the unknown attribute resolved to something. Why: a typo'd GetAtt silently yields a wrong value instead of a refusal, and the template the client validated against the emulator is rejected by real AWS. Fix: Fn::GetAtt resolution against the AWS::SQS::Queue resource schema (reject unknown attribute names). Traces: cloudformation__sqs.cfn-sqs-queue-getatt-unknown-attribute-rejected-rolls-back.

Cause 2 of 6 — a read-only property present in the template blocks the create instead of being ignored. AWS: a readOnlyProperty such as Arn supplied in AWS::SQS::Queue Properties is ignored; the queue is created and Ref/GetAtt match SQS's own values. Emulator: the stack ends ROLLBACK_COMPLETE. Why: templates real AWS deploys are refused, and the client is pushed to hand-edit generated templates. Fix: property handling in the AWS::SQS::Queue handler (ignore readOnlyProperties on create). Traces: cloudformation__sqs.cfn-sqs-queue-readonly-arn-ignored-and-ref-getatt-match-sqs.

Cause 3 of 6 — the refusal message for setting a read-only property is wrong. AWS: setting QueueUrl in the template fails the resource with ResourceStatusReason 'Resource handler returned message: "QueueUrl is a read-only property." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — the fixed text is '<Property> is a read-only property.' with the property name taken from the template. Emulator: 'Resource handler returned message: "Unknown Attribute QueueUrl." (RequestToken: ..., HandlerErrorCode: InvalidRequest)' — it reports an unknown attribute rather than a read-only one. Why: the client is told the property does not exist (so they remove it or doubt the schema) instead of that it is read-only. Fix: the read-only-property check and its message in the AWS::SQS::Queue handler. Traces: cloudformation__sqs.cfn-sqs-queue-readonly-queueurl-set-in-template-rejected-rolls-back.

Cause 4 of 6 — AWS::SQS::QueueInlinePolicy's Ref/PhysicalResourceId is not the queue URL. AWS: the physical id (and therefore Ref) of AWS::SQS::QueueInlinePolicy is the URL of the queue it applies to, and removing the resource in a stack update drops the policy. Emulator: the PhysicalResourceId is a generated name 'sim2real-behav-cfn-qip-<stack suffix>-QueueInlinePolicy-<hash>' instead of 'https://sqs.<region>.amazonaws.com/<account>/<queue>'. Why: any template doing Ref on that resource (or any client reading its physical id to find the queue) gets an identifier that resolves to nothing. Fix: the physical-id/Ref computation of the AWS::SQS::QueueInlinePolicy handler. Traces: cloudformation__sqs.cfn-sqs-queueinlinepolicy-ref-is-queue-url-and-removed-by-stack-update.

Cause 5 of 6 — the stack-policy denial reason is not AWS's wording. AWS: a resource an implicit deny protects fails with exactly one UPDATE_FAILED event whose ResourceStatusReason is the fixed text 'Action not allowed by stack policy'. Emulator: 'Action denied by stack policy: No statement allows [Update:Modify] for resource [LogicalResourceId/ProtectedQueue];' — different fixed text plus emulator-authored detail. Why: clients and tooling match on the documented sentence to recognise a stack-policy denial; the emulator's sentence does not match it. Fix: the stack-policy evaluation's event reason text in the stack-update path. (Related but filed separately for a different check: open issue 20261102T094631Z-cloudformation-sqs-4-net-new-causes-an-import.md.) Traces: cloudformation__sqs.stack-policy-implicit-deny-for-unnamed-resource.

Cause 6 of 6 — removing a stack tag does not produce the queue's UPDATE_COMPLETE event (tag propagation is not applied without replacement). AWS: removing a Tag from the stack updates the queue's tags in place, and the queue's event sequence contains ('Queue', 'UPDATE_COMPLETE'). Emulator: that pair is absent from the event list — ValueError: ('Queue', 'UPDATE_COMPLETE') is not in list — so the resource was never updated for the tag change. Why: stack tags are documented to propagate to supporting resources; a client removing a tag sees it persist on the queue, with no event trail. Fix: stack-tag propagation in the AWS::SQS::Queue update path and its stack events. Traces: cloudformation__sqs.stack-tag-removal-propagates-to-queue-without-replacement.

Cause 7 of 6 is not claimed; one more message defect rides with Cause 6's component: CreateStack with 51 Tags is refused with the wrong message. AWS: ValidationError (HTTP 400) with the fixed text 'Tags must have length less than or equal to 50'. Emulator: '1 validation error detected: Value '51 items' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50'. Fix: the CreateStack Tags cap validation message. Traces: cloudformation__sqs.stack-tags-over-fifty-rejected-before-queue-creation. (Count it with Cause 6 when closing: 7 checks, 6 causes plus this message fix.)

## Evidence
Cause 1: AssertionError: an unknown GetAtt attribute must fail the stack and roll it back; got 'CREATE_COMPLETE'
Cause 2: AssertionError: a readOnlyProperty value in the template must not block the create; got 'ROLLBACK_COMPLETE'
Cause 3: AssertionError: the queue's CREATE_FAILED reason must be the handler's exact InvalidRequest message; got ['Resource handler returned message: "Unknown Attribute QueueUrl." (RequestToken: ..., HandlerErrorCode: InvalidRequest)'] — expected 'Resource handler returned message: "QueueUrl is a read-only property." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'
Cause 4: AssertionError: QueueInlinePolicy's PhysicalResourceId (Ref) 'sim2real-behav-cfn-qip-...-QueueInlinePolicy-86AF60B89DCB' must be the queue URL 'https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-behav-cfn-qip-...'
Cause 5: AssertionError: expected exactly one UPDATE_FAILED event for ProtectedQueue carrying the stack-policy denial reason, got: ['Action denied by stack policy: No statement allows [Update:Modify] for resource [LogicalResourceId/ProtectedQueue];'] (expected ['Action not allowed by stack policy'])
Cause 6: ValueError: ('Queue', 'UPDATE_COMPLETE') is not in list
Tags message: AssertionError: 1 validation error detected: Value '51 items' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50 (expected ValidationError 'Tags must have length less than or equal to 50')

## Steps to Reproduce
Cause 1: CreateStack with Fn::GetAtt ['Queue','NotAnAttribute'].
Cause 2: CreateStack with AWS::SQS::Queue Properties including Arn.
Cause 3: CreateStack with AWS::SQS::Queue Properties including QueueUrl; read the CREATE_FAILED reason.
Cause 4: CreateStack with AWS::SQS::QueueInlinePolicy; DescribeStackResource and read its PhysicalResourceId.
Cause 5: SetStackPolicy allowing Update:* on one resource only, then UpdateStack touching another resource; read that resource's UPDATE_FAILED reason.
Cause 6: CreateStack with Tags, then UpdateStack with one tag removed; read the queue's stack events and ListQueueTags.
Tags message: CreateStack with 51 Tags and read Error.Code/Message.
