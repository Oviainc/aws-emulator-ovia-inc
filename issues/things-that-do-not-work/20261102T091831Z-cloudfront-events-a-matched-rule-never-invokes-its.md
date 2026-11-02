Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.eventbridge-put-events-custom-detail-triggers-cloudfront-cache-invalidation cloudfront__events.iam-role-trusts-events-service-for-rule-target-invocation cloudfront__events.iam-role-without-events-trust-rejected-as-rule-target-role
Issue-Key: cloudfront__events.eventbridge-put-events-custom-detail-triggers-cloudfront-cache-invalidation|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__events.iam-role-trusts-events-service-for-rule-target-invocation|_drain_queue#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudfront__events.iam-role-without-events-trust-rejected-as-rule-target-role|_drain_queue#17|Timeout|resp = sqs.receive_message(**kwargs)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# cloudfront__events: a matched rule never invokes its Lambda target and an untrusted target role never dead-letters as FAILED_TO_ASSUME_ROLE (2 causes)

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two net-new root causes in this service group.

Cause 1 of 2: a rule whose event pattern matches a PutEvents entry never invokes its Lambda target, so neither the CloudFront invalidation the function would create nor the detail the function would receive is ever produced.
Failing checks: cloudfront__events.eventbridge-put-events-custom-detail-triggers-cloudfront-cache-invalidation, cloudfront__events.iam-role-trusts-events-service-for-rule-target-invocation.
AWS: a PutEvents entry whose detail-type matches the rule's pattern causes EventBridge to assume the target RoleArn (which trusts events.amazonaws.com) and invoke the Lambda target once, handing it the full envelope with `detail` unchanged; the function's permitted CreateInvalidation against a fabricated distribution id is refused by CloudFront as NoSuchDistribution (not AccessDenied), and an event with a mismatched detail-type creates no invalidation. Recorded behavior only for the delivery; Smithy (cloudfront) models CreateInvalidation's NoSuchDistribution error, which is the refusal the function must observe.
Emulator: both checks end `Check timed out after 60.0s`. Nothing is invoked on the positive path, so no invalidation is created, the function's signal queue stays empty and the NoSuchDistribution-vs-AccessDenied distinction is never reached.
Why: a client whose cache-invalidation automation is `PutEvents -> rule -> Lambda -> CreateInvalidation` sees no invalidation at all, and the mismatched-detail-type negative passes vacuously.
Fix: the EventBridge rule-target delivery path for Lambda targets in the events crate (pattern match -> assume target RoleArn -> Lambda Invoke), and the Lambda invoke path it drives. Open issues 20261023T140640Z-cloudfront-events-3-of-the-4-causes-remain-open.md, 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md, 20261101T214154Z-cloudfront-events-two-more-root-causes-a-matched.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md own neighbouring checks; none carries these two replay keys.

Cause 2 of 2: a target whose RoleArn omits the events.amazonaws.com trust statement produces no dead-letter record, so the FAILED_TO_ASSUME_ROLE failure classification is never observable.
Failing check: cloudfront__events.iam-role-without-events-trust-rejected-as-rule-target-role.
AWS: when EventBridge cannot assume a target's RoleArn, the event for THAT target is written to the target's DeadLetterConfig queue with the dead-letter message attributes recording the failure, including `ERROR_CODE` = `FAILED_TO_ASSUME_ROLE`, while a sibling target on the same rule using a correctly-trusted role IS invoked with the event detail. Recorded behavior only (the attribute names are the AWS dead-letter attributes, not modeled shapes).
Emulator: `Check timed out after 60.0s` — neither the DLQ receives a dead-letter with ERROR_CODE=FAILED_TO_ASSUME_ROLE nor the correctly-trusted sibling target is invoked.
Why: the diagnostic an operator relies on to tell "my target role is mistrusted" from "my pattern did not match" does not exist; failures are silent instead of dead-lettered with a cause.
Fix: the target-delivery failure path in the events crate — role-assumption failure must be classified and written to the target's DeadLetterConfig queue with the AWS dead-letter attributes (ERROR_CODE=FAILED_TO_ASSUME_ROLE, plus RULE_ARN/TARGET_ARN/ERROR_MESSAGE) — and it must be per-target, leaving the sibling target's delivery intact.
Traces: the three failing check ids listed above.

## Evidence
Cause 1 — Expected: the matching PutEvents entry invokes the Lambda target exactly once with the envelope's `detail` unchanged, a CloudFront invalidation is created for the configured paths, and the function's permitted CreateInvalidation on a fabricated distribution is refused as NoSuchDistribution rather than AccessDenied; the mismatched detail-type creates no invalidation. Actual: `Check timed out after 60.0s` on both checks — no invocation, no invalidation, no signal message.
Cause 2 — Expected: the mistrusted target's event lands on its dead-letter queue with message attribute ERROR_CODE = FAILED_TO_ASSUME_ROLE while the sibling target with a correctly-trusted role is invoked with the detail. Actual: `Check timed out after 60.0s` — the DLQ stays empty and the sibling is not invoked.

## Steps to Reproduce
Cause 1: create a minimal CloudFront distribution and a Lambda function whose handler calls cloudfront:CreateInvalidation and reports to an SQS signal queue; create a rule with a detail-type pattern, PutTargets the function with a RoleArn trusting events.amazonaws.com, and grant the function's resource policy events.amazonaws.com; PutEvents a matching entry and then a mismatched one. On AWS one invalidation appears and the signal queue gets one message; on the emulator nothing happens.
Cause 2: on one rule, put two targets — target A with a RoleArn whose trust policy omits events.amazonaws.com and a DeadLetterConfig queue, target B with a correctly-trusted role. PutEvents a matching event. On AWS the DLQ receives a message with ERROR_CODE=FAILED_TO_ASSUME_ROLE and target B is invoked; on the emulator both stay silent.
