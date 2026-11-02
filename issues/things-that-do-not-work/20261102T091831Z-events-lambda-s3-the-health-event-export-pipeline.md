Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.health-ev-exists-filter-on-event-arn-withholds-an-incomplete-detail-at-the-rule events__lambda__s3.health-ev-permission-scoped-to-the-default-bus-rule-arn-does-not-admit-the-custom-bus-rule events__lambda__s3.health-ev-service-events-reach-a-default-bus-rule-and-never-a-custom-bus-rule events__lambda__s3.health-ev-target-input-path-hands-the-function-the-detail-alone events__lambda__s3.health-ev-target-upsert-replaces-input-path-with-constant-input-for-later-deliveries events__lambda__s3.health-ev-the-export-bucket-wipe-is-idempotent-and-converges-to-empty events__lambda__s3.health-ev-affected-entity-filter-matches-any-element-of-the-entity-array events__lambda__s3.health-ev-or-combinations-over-one-thousand-refused-while-one-thousand-match events__lambda__s3.health-ev-e2e-health-eventbridge-csv-export-lifecycle
Issue-Key: events__lambda__s3.health-ev-exists-filter-on-event-arn-withholds-an-incomplete-detail-at-the-rule|_poll_bodies#12|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-permission-scoped-to-the-default-bus-rule-arn-does-not-admit-the-custom-bus-rule|_poll_bodies#18|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-service-events-reach-a-default-bus-rule-and-never-a-custom-bus-rule|_poll#16|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-target-input-path-hands-the-function-the-detail-alone|_poll_bodies#9|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-target-upsert-replaces-input-path-with-constant-input-for-later-deliveries|_poll_bodies#8|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-the-export-bucket-wipe-is-idempotent-and-converges-to-empty|_poll_keys#9|Timeout|time.sleep(3.0)
Issue-Key: events__lambda__s3.health-ev-affected-entity-filter-matches-any-element-of-the-entity-array|run#5|AssertionError|assert verdicts == {"only": True, "last-of-three": True, "none-of-two": False,
Issue-Key: events__lambda__s3.health-ev-or-combinations-over-one-thousand-refused-while-one-thousand-match|_error#8|AssertionError|raise AssertionError(f"the call succeeded where AWS refuses it: {result!r}")
Issue-Key: events__lambda__s3|DescribeEventBus|ResourceNotFoundException
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# events__lambda__s3: the health-event export pipeline never delivers, array-of-objects patterns never match, the $or combination limit is unenforced, and a CFN-created bus is invisible (4 causes)

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Four net-new root causes in this service group.

Cause 1 of 4: a matched event never invokes the exporter Lambda, so none of the rule/target shaping contracts around the health-event export pipeline can be observed.
Failing checks: events__lambda__s3.health-ev-exists-filter-on-event-arn-withholds-an-incomplete-detail-at-the-rule, events__lambda__s3.health-ev-permission-scoped-to-the-default-bus-rule-arn-does-not-admit-the-custom-bus-rule, events__lambda__s3.health-ev-service-events-reach-a-default-bus-rule-and-never-a-custom-bus-rule, events__lambda__s3.health-ev-target-input-path-hands-the-function-the-detail-alone, events__lambda__s3.health-ev-target-upsert-replaces-input-path-with-constant-input-for-later-deliveries, events__lambda__s3.health-ev-the-export-bucket-wipe-is-idempotent-and-converges-to-empty.
AWS: a rule whose pattern matches invokes its Lambda target once per event, subject to the function's resource policy. Concretely: an `exists: true` requirement on `detail.eventArn` withholds a detail lacking it while a complete twin published alongside IS delivered; a lambda permission whose SourceArn is the default-bus form `arn:aws:events:<region>:<account>:rule/<name>` does NOT admit the same-named rule on a custom bus (whose ARN is `rule/<bus>/<name>`) until the bus-qualified ARN is granted; an S3 `Object Created` service event reaches a default-bus rule and never an identical rule on a custom bus; a target with `InputPath: $.detail` hands the function only the detail while a sibling plain target receives the full envelope carrying the PutEvents EventId; and PutTargets on an existing target Id replaces InputPath with a constant Input so later events deliver the constant. Recorded behavior only for delivery; Smithy (eventbridge) models Target.InputPath and Target.Input (mutually exclusive input forms) and PutTargets' upsert-by-Id semantics, so each of these is a modeled configuration the emulator accepts and then never exercises.
Emulator: all six checks end `Check timed out after 60.0s`. The exporter is never invoked, so nothing is written to the export bucket — which also makes the bucket-wipe/idempotence check unobservable (it needs the pipeline to deliver again after the reset).
Why: every rule-shaping and permission-scoping guarantee a client depends on is untestable and, in practice, broken: matched events produce nothing, and a negative-delivery assertion ("the incomplete detail must NOT be delivered", "the custom-bus rule must NOT be admitted") passes vacuously here and fails on AWS.
Fix: the EventBridge rule-target delivery path for Lambda targets in the events crate (pattern evaluation -> resource-policy authorization against the rule's bus-qualified ARN -> InputPath/Input rendering -> Lambda Invoke), and the Lambda invoke path it drives. Open issue 20261025T002357Z-events-lambda-s3-an-eventbridge-rule-s-lambda-target-is-never.md owns two sibling checks in this group; it does not carry these six replay keys.

Cause 2 of 4: an event pattern matching inside an array of objects never matches — a value pattern under a named key is not applied to each element of the array.
Failing check: events__lambda__s3.health-ev-affected-entity-filter-matches-any-element-of-the-entity-array.
AWS: when the event's field is an array of objects, a pattern on a nested key matches if ANY element of the array satisfies it, and only under the named key. For the five published shapes the verdicts are: the sole element matching -> matched; the last of three matching -> matched; none of two matching -> not matched; an empty array -> not matched; the value present only under a DIFFERENT key -> not matched. Recorded behavior only (the EventBridge pattern language is documented prose, not modeled).
Emulator: every verdict is False — `AssertionError: array-of-objects verdicts were {'only': False, 'last-of-three': False, 'none-of-two': False, 'empty': False, 'other-key': False}`. The two matching shapes are wrongly withheld; the three non-matching ones agree only by accident.
Why: rules that filter on list-valued details (AWS Health affectedEntities, ECS containers, resource lists) never fire, so clients silently lose every event they depend on, and the "only under the named key" scoping cannot be distinguished from the blanket failure.
Fix: the event-pattern matcher in the events crate — array traversal must try each element of a list-valued field against the element pattern and succeed on any match, while still requiring the match to occur under the named key.

Cause 3 of 4: TestEventPattern accepts a pattern whose `$or` arrays multiply past the EventBridge combination limit, where AWS refuses it.
Failing check: events__lambda__s3.health-ev-or-combinations-over-one-thousand-refused-while-one-thousand-match.
AWS: a pattern expanding to exactly 1000 rule combinations is accepted and matches; one more branch (1025 combinations) is refused with HTTP 400 and Error.Code `InvalidEventPatternException`, whose message states the combination limit was exceeded; the refusal is independent of the 2048-character pattern-size limit (the over-limit pattern here is well inside it). Smithy (eventbridge): TestEventPattern models exactly one client error, `InvalidEventPatternException` (400), and EventPattern is `@length(min:0,max:4096)` — so the combination limit is a validation rule beyond the modeled length bound, enforced through that modeled error.
Emulator: the over-limit pattern is accepted — `AssertionError: the call succeeded where AWS refuses it: {'Result': True, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` — i.e. 200 with Result true instead of 400 InvalidEventPatternException.
Why: a pattern that AWS will reject at PutRule/TestEventPattern time passes validation here, so the failure surfaces only against the real service, after the pattern has been promoted.
Fix: pattern validation shared by TestEventPattern and PutRule — compute the `$or` combination count while parsing and refuse with 400 InvalidEventPatternException once it exceeds the limit, before evaluating the pattern.

Cause 4 of 4: an event bus created by a CloudFormation stack is not visible to the events control plane, so the stack's own deploy phase fails.
Failing check: events__lambda__s3.health-ev-e2e-health-eventbridge-csv-export-lifecycle.
AWS: a stack declaring AWS::Events::EventBus reaches CREATE_COMPLETE and the named bus is immediately readable through DescribeEventBus (and usable as a rule's EventBusName).
Emulator: the deploy phase of the lifecycle aborts with `ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeEventBus operation: Event bus <stack-created bus name> does not exist.` — the bus the stack created is not registered in the events store, so the rest of the six-phase export lifecycle never runs.
Why: any estate deployed by CloudFormation that owns its event bus cannot be driven at all; the resource reports as created while every events call against it answers 404.
Fix: the CloudFormation resource provider for AWS::Events::EventBus — creating the resource must register the bus in the events service state (and resolve its physical id/ARN) so DescribeEventBus and rule creation see it. Open issue 20261025T002357Z-events-a-cloudformation-created-aws-events-eventbus-is-invisible.md records the same shape under service token `events`; this check belongs to events__lambda__s3 and its replay key is not carried there.
Traces: the nine failing check ids listed above.

## Evidence
Cause 1 — Expected: the exists-filter rule withholds the incomplete detail and delivers the complete twin; a default-bus-scoped lambda permission does not admit the custom-bus rule until the bus-qualified ARN is granted; an S3 Object Created event reaches the default-bus rule and never the custom-bus rule; InputPath `$.detail` yields only the detail while a sibling target gets the full envelope with the EventId; a PutTargets upsert replacing InputPath with Input delivers the constant thereafter; the export-bucket wipe converges to empty, is idempotent and the pipeline delivers again. Actual: `Check timed out after 60.0s` on all six — the exporter is never invoked and the export bucket stays empty.
Cause 2 — Expected verdicts {'only': True, 'last-of-three': True, 'none-of-two': False, 'empty': False, 'other-key': False}; Actual `{'only': False, 'last-of-three': False, 'none-of-two': False, 'empty': False, 'other-key': False}`.
Cause 3 — Expected: 1025 `$or` combinations refused 400 InvalidEventPatternException (1000 accepted and matching); Actual 200 with `{'Result': True}`.
Cause 4 — Expected: the stack reaches CREATE_COMPLETE and DescribeEventBus returns the stack-created bus; Actual `ResourceNotFoundException ... Event bus <name> does not exist` in phase 1/6 (deploy).

## Steps to Reproduce
Cause 1: stand up a bus, a rule and a Lambda exporter writing to S3, grant the function's resource policy events.amazonaws.com with the rule ARN, then PutEvents matching events and poll the export bucket — on AWS objects appear, on the emulator nothing does. Vary the rule pattern (exists filter on detail.eventArn), the permission SourceArn form (`rule/<name>` vs `rule/<bus>/<name>`), the bus (default vs custom) and the target input form (InputPath `$.detail`, then a PutTargets upsert to a constant Input).
Cause 2: TestEventPattern (or a live rule) with a pattern requiring a value under a named key inside an array of objects, published against five details: sole match, last-of-three match, none-of-two, empty array, value under another key. On AWS the first two match.
Cause 3: build two `$or` arrays multiplying to 1000 combinations (accepted, matches) and then 1025 combinations (<2048 chars) and call TestEventPattern — AWS answers 400 InvalidEventPatternException.
Cause 4: deploy a stack declaring AWS::Events::EventBus and call DescribeEventBus with the declared bus name.
