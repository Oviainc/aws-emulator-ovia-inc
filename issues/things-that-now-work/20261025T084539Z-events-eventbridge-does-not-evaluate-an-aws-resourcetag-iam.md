Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.resource-tag-condition-scopes-disable-rule-to-rules-carrying-the-tag
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.resource-tag-condition-scopes-disable-rule-to-rules-carrying-the-tag|_error#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 8b92040b06acdac381093d8b397331a442bd94ad

# EventBridge does not evaluate an aws:ResourceTag IAM condition: DisableRule succeeds for a role scoped to a tag the rule does not carry (1 cause)

> _Note: this issue was identified against the code base at commit `8b92040b06acdac381093d8b397331a442bd94ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableRule

## Description
Cause 1 of 1: a resource-tag-scoped identity policy is not enforced on DisableRule.
AWS: recorded behavior only for the authorization decision (the Smithy model does not express IAM). A role whose only grant is events:DisableRule with Condition StringEquals aws:ResourceTag/<key> = "alpha" can disable a rule TAGGED alpha, but the identical call against a rule tagged "beta" is refused with AccessDeniedException and that rule stays ENABLED; after the beta rule is retagged alpha, the byte-identical call by the same role succeeds and the rule reads back DISABLED. The decision follows the target rule's tags.
Emulator: the DisableRule call made by the tag-scoped role against the beta rule answers HTTP 200 with an empty body — the condition key is never evaluated and the rule is disabled.
Why: tag-based access control is a primary blast-radius control for EventBridge rules. An emulator that ignores aws:ResourceTag lets a test suite "prove" a least-privilege policy works when in production the same call is denied (or, worse, validates a policy that in the emulator permits mutating every rule in the account).
Fix: the eventbridge authorization path for the rule mutators (DisableRule/EnableRule/PutRule/DeleteRule/PutTargets) must resolve the target rule's tags and evaluate aws:ResourceTag condition keys against them before acting.
Note: the open issue 20261025T002357Z-events-eventbridge-performs-no-authorization-event-bus-resource-policies.md covers event-bus resource policies, caller identity policies, iam:PassRole and KMS grants; it does not name resource-tag condition keys or DisableRule, and the replay report does not mark this check OWNED by it.
Traces: events.resource-tag-condition-scopes-disable-rule-to-rules-carrying-the-tag.

## Evidence
events.resource-tag-condition-scopes-disable-rule-to-rules-carrying-the-tag [mismatch]:
  File "check_events_resource_tag_condition_scopes_disable_rule_to_rules_carrying_the_tag.py", line 59, in run
    code, message = _error(lambda: scoped.disable_rule(Name=beta, EventBusName=bus))
  AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Expected: HTTP 400 with error code AccessDeniedException, and the beta rule still ENABLED on a subsequent DescribeRule.
Actual: HTTP 200, empty body; the rule is disabled.

## Steps to Reproduce
1. Boot the emulator; create events, iam and sts clients.
2. create_event_bus(Name=B); put_rule(Name='alpha', EventBusName=B, EventPattern=...) tagged {sim2real-team: alpha}; put_rule(Name='beta', EventBusName=B, EventPattern=...) tagged {sim2real-team: beta}.
3. Create a role trusted by the caller with an inline policy allowing Action events:DisableRule on the rules' ARNs with Condition {"StringEquals": {"aws:ResourceTag/sim2real-team": "alpha"}}; assume it.
4. With the assumed role: disable_rule(Name='alpha', EventBusName=B) -> expect 200.
5. With the same assumed role: disable_rule(Name='beta', EventBusName=B) -> expect AccessDeniedException; observe HTTP 200 instead, and describe_rule(Name='beta') reporting DISABLED instead of ENABLED.
6. tag_resource on the beta rule with sim2real-team=alpha and repeat step 5 -> expect 200 and DISABLED.
Replay events.resource-tag-condition-scopes-disable-rule-to-rules-carrying-the-tag to confirm.
