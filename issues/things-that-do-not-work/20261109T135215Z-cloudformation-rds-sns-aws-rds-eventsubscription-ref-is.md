Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds__sns.deploy-cfn-rds-event-subscription-provisions-and-rejects-bad-category cloudformation__rds__sns.deploy-rds-event-subscription-undefined-category-rejected-and-rolled-back
Issue-Key: cloudformation__rds__sns.deploy-cfn-rds-event-subscription-provisions-and-rejects-bad-category|run#11|AssertionError|assert outputs["SubscriptionNameOut"] == sub_name, (
Issue-Key: cloudformation__rds__sns.deploy-rds-event-subscription-undefined-category-rejected-and-rolled-back|run#8|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudformation__rds__sns: AWS::RDS::EventSubscription Ref is a synthesized physical id, and an undefined event category is never refused

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSubscription

## Description
2 net-new root causes in the AWS::RDS::EventSubscription provider.

Cause 1 of 2: Ref of an AWS::RDS::EventSubscription returns a CloudFormation-generated name instead of the subscription's own name.
AWS: the physical id of an AWS::RDS::EventSubscription is its SubscriptionName, so !Ref returns the name the template supplied (and, when the template omits it, the generated name RDS stored and DescribeEventSubscriptions reads back). Recorded behavior only for the identity rule; the check supplies the SubscriptionName explicitly.
Emulator: Ref returns a synthesized '<stack-name>-EventSub-<hex>' string while the subscription that exists in RDS carries the template-supplied name.
Why: every consumer of !Ref - a stack output a script feeds to DescribeEventSubscriptions or DeleteEventSubscription, a cross-resource reference - is handed an identifier RDS does not know, so the next call 404s.
Fix: the AWS::RDS::EventSubscription provider's physical-id assignment in the CloudFormation service.
Traces: cloudformation__rds__sns.deploy-cfn-rds-event-subscription-provisions-and-rejects-bad-category.

Cause 2 of 2: an EventSubscription declaring an undefined EventCategory is created instead of refused, so the stack reaches CREATE_COMPLETE.
AWS: CreateEventSubscription validates each requested category against the categories defined for the given SourceType (as DescribeEventCategories lists them) and refuses an undefined one, so the resource goes CREATE_FAILED and the stack settles ROLLBACK_COMPLETE.
Emulator: the stack settles CREATE_COMPLETE - the bogus category is accepted and stored.
Why: a typo'd category deploys 'successfully' and the subscription silently never matches any event; a client's negative test for the rejection can never pass.
Fix: EventCategories validation in the RDS CreateEventSubscription handler (the CFN provider propagates whatever RDS answers).
Traces: cloudformation__rds__sns.deploy-rds-event-subscription-undefined-category-rejected-and-rolled-back.

## Evidence
Cause 1: the EventSubscription Ref was 'sim2real-cfnrds-stk-<hex>-EventSub-<HEX>', expected the declared SubscriptionName 'sim2real-cfnrds-sub-<hex>'.
Cause 2: invalid category stack settled at 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE.

## Steps to Reproduce
Cause 1: deploy a stack with an AWS::RDS::EventSubscription that sets SubscriptionName and outputs !Ref of it; compare the output with DescribeEventSubscriptions.
Cause 2: deploy a stack whose AWS::RDS::EventSubscription lists an EventCategory that DescribeEventCategories does not define for its SourceType; wait for the stack's terminal status.
