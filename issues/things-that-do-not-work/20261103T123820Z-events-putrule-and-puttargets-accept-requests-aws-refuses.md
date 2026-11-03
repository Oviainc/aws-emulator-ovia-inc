Issue-Source: trace-sweep-divergence
Issue-Trace: events-ebecs-launch-type-and-network-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-ebecs-launch-type-and-network-mistakes events-ebecs-task-count-and-batch-atomicity events-ebecs-task-definition-reference-forms events-poller-schedule-rule-target-introspection events-poller-cron-expression-mistakes events-put-rule-checks-the-event-pattern-before-the-bus-and-never-lands-on-default
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutTargets|4xx->2xx|$.__type
Issue-Key: events|PutTargets|4xx->2xx|$.message
Issue-Key: events|PutTargets|4xx->2xx|status
Issue-Key: events|PutTargets|4xx->2xx|$.Message
Issue-Key: events|PutRule|4xx->2xx|$.__type
Issue-Key: events|PutRule|4xx->2xx|$.message
Issue-Key: events|PutRule|4xx->2xx|status
Issue-Key: events|DeleteEventBus|2xx->4xx|status
Issue-Key: events|PutRule|4xx->4xx|$.__type
Issue-Key: events|PutRule|4xx->4xx|$.message
Issue-Key: events|ListTargetsByRule|2xx->2xx|$.Targets[*]
Issue-Key: events|ListTargetsByRule|2xx->2xx|$.Targets[Id="Target2"]
Issue-Commit: e0cc4f83c3608c610536f43d6e35ff223081a6f4

# events: PutRule and PutTargets accept requests AWS refuses, and the rejected state persists (7 causes)

> _Note: this issue was identified against the code base at commit `e0cc4f83c3608c610536f43d6e35ff223081a6f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Seven net-new wire root causes in the events service, all one family: the rule/target write path admits requests AWS refuses, so the emulator answers 200 where AWS answers 400 and then carries state AWS never created. Counts: 7 causes.

Cause 1 of 7: PutTargets accepts an ECS-cluster target that carries no EcsParameters.
AWS: when a target's Arn names an Amazon ECS cluster, EcsParameters (with its @required TaskDefinitionArn) must be supplied; PutTargets refuses the request with HTTP 400, body member $.__type = 'ValidationException' and $.message = 'Parameter(s) EcsParameters must be specified for target: <TargetId>.' The fixed text is 'Parameter(s) EcsParameters must be specified for target: ' and the only variable part is the target's own Id taken from the request (the recording shows 'Target0'); no target is stored. Smithy (com.amazonaws.eventbridge): PutTargets input Targets -> Target{Id @required, Arn @required, EcsParameters optional}, EcsParameters.TaskDefinitionArn @required; PutTargets's modeled errors are ConcurrentModificationException / InternalException / LimitExceededException / ManagedRuleException / ResourceNotFoundException, so the ValidationException code/message pairing for this rule is recorded behavior only.
Emulator: HTTP 200 with no FailedEntries and no __type/message at all; the target is created.
Why: a client that forgot the ECS parameters is told its target was accepted. The rule then holds a target that can never launch a task, and the error the SDK should have raised at configuration time never surfaces — the failure moves from a deterministic 400 to silent non-delivery.
Fix: PutTargets' input validation in the events crate — validate per-entry target-type requirements (ECS cluster Arn => EcsParameters present) before any state change and refuse with 400 ValidationException.
Traces: events-ebecs-launch-type-and-network-mistakes#task#2 (representative; the same cluster covers task#3, task#4, task#6, task#7 of that fixture, task#4, task#5, task#9 of events-ebecs-task-count-and-batch-atomicity, task#2, task#3, task#5 of events-ebecs-task-definition-reference-forms and task#3 of events-poller-schedule-rule-target-introspection — 12 steps).

Cause 2 of 7: PutTargets accepts a cross-account target instead of denying it.
AWS: adding a target whose Arn belongs to an account other than the caller's is refused with HTTP 400, $.__type = 'AccessDeniedException' and $.Message (capital M on this error shape, unlike ValidationException's lowercase $.message) = 'Access to the resource <target ARN> is denied. Reason: Adding cross-account target is not permitted.' The fixed text is 'Access to the resource ' + ' is denied. Reason: Adding cross-account target is not permitted.' and the variable part is the target ARN copied from the request (the recording's is an ECS cluster ARN in a foreign account). Smithy: PutTargets does not model AccessDeniedException, so this code/message pairing is recorded behavior only; the Target doc states cross-account delivery requires an explicitly permissioned RoleArn.
Emulator: HTTP 200, no error body, target stored.
Why: a client that typo'd an account id in a target ARN silently gets a target the real service would never accept, so an authorization boundary that AWS enforces at configuration time is invisible here and the misconfiguration ships.
Fix: PutTargets' validation/authorization step — compare each target ARN's account against the caller's and refuse with 400 AccessDeniedException carrying the $.Message member.
Traces: events-ebecs-task-count-and-batch-atomicity#task#10.

Cause 3 of 7: PutRule accepts a malformed ScheduleExpression.
AWS: PutRule parses ScheduleExpression and refuses an invalid rate()/cron() form with HTTP 400, $.__type = 'ValidationException' and $.message = 'Parameter ScheduleExpression is not valid.' — the whole message is fixed text with no variable part — and creates no rule. Smithy: PutRule.ScheduleExpression is @length(min 0, max 256) with no pattern and PutRule's modeled errors do not include ValidationException, so the grammar and this refusal are recorded behavior only; the AWS schedule-expression reference documents the rule.
Emulator: HTTP 200 returning a RuleArn; the rule is created with the invalid schedule.
Why: a typo'd schedule is accepted and the rule then never fires (or fires on an unintended cadence) with no error anywhere; on an update it also replaces a working schedule. Open issue 20261102T091831Z-events-api-destinations-never-deliver-or-dead-letter.md describes this defect from the behavioral suite (its cause 2) but carries no replay key for these wire steps.
Fix: PutRule's validation layer — parse ScheduleExpression (rate value/unit agreement and allowed units; six-field cron with exactly one of day-of-month/day-of-week concrete) before any state change.
Traces: events-poller-cron-expression-mistakes#task#2 (cluster also covers task#3, task#4, task#6 of that fixture).

Cause 4 of 7: teardown's DeleteEventBus is refused 400 where AWS answers 200 — the downstream effect of cause 3.
AWS: DeleteEventBus on a custom bus answers HTTP 200 with an empty body once no rules remain associated with it. In the recording the preceding PutRule calls were all refused (cause 3), so the bus holds no rules and the delete succeeds.
Emulator: HTTP 400. Hypothesis: the emulator holds the rules it should never have created in cause 3 and refuses to delete a bus that still has rules, so the status divergence is a consequence of the accepted invalid schedules rather than an independent DeleteEventBus defect; the replay detail reports only the status, no body.
Why: a teardown sequence that AWS completes leaves the emulator with an undeletable bus, so a client's cleanup fails and the next run collides on an existing name.
Fix: resolve cause 3 first, then confirm DeleteEventBus in the events crate's event-bus state only refuses a bus that genuinely still has rules.
Traces: events-poller-cron-expression-mistakes#teardown#13.

Cause 5 of 7: PutRule resolves the event bus before validating the event pattern, so the wrong error class is returned.
AWS: PutRule validates EventPattern first: a pattern whose top-level value is a bare string is refused with HTTP 400, $.__type = 'InvalidEventPatternException' and $.message = 'Event pattern is not valid. Reason: "source" must be an object or an array\n at [Source: (String)"<the submitted pattern>"; line: <n>, column: <c>]' — even when the request names an event bus that does not exist. The fixed text is 'Event pattern is not valid. Reason: ' + the per-form reason, and the variable parts are the offending member name, the caller's own pattern string and its line/column offset. Smithy: PutRule models InvalidEventPatternException (client, 400) and ResourceNotFoundException (client, 400); the precedence between them is recorded behavior only.
Emulator: HTTP 400 (status agrees) but $.__type = 'ResourceNotFoundException' and $.message = 'Event bus <name> does not exist.' — the pattern is never parsed.
Why: a client that catches InvalidEventPatternException to report a bad pattern to its user sees ResourceNotFoundException instead and reports a missing bus, masking the real defect in the request; retry logic keyed on the error class takes the wrong branch.
Fix: the PutRule handler's validation ordering — parse and validate EventPattern before resolving EventBusName.
Traces: events-put-rule-checks-the-event-pattern-before-the-bus-and-never-lands-on-default#task#2.

Cause 6 of 7: ListTargetsByRule returns targets left behind by PutTargets batches AWS refused (batch atomicity).
AWS: a PutTargets batch whose entries fail validation stores nothing for those entries, so ListTargetsByRule afterwards returns only the targets of the accepted calls. In the recording that is a single target; AWS lists no Targets[1] or Targets[2].
Emulator: ListTargetsByRule answers 200 with two extra entries (Ids 'Target1' and 'Target2') that came from the refused batches — one carrying an ECS LaunchType=FARGATE target with no NetworkConfiguration and one a TaskCount the batch had rejected. Smithy: ListTargetsByRule output Targets is @length(min 1, max 100).
Why: the rule's target list read back by a client does not match what the service accepted, so a reconciliation loop (CDK/CloudFormation drift, or any list-then-diff) sees targets it never created and either deletes live configuration or re-puts a rejected one forever.
Fix: make PutTargets' entry handling atomic per refused entry — reject before mutating rule-target state (events crate rule/target state), rather than storing then reporting.
Traces: events-ebecs-task-count-and-batch-atomicity#task#6 (cluster also covers task#11 of the same fixture).

Cause 7 of 7: the same leftover target is still listed after the rule's targets are re-read later in the fixture.
AWS: after the refused batch and the subsequent RemoveTargets/PutTargets steps, the rule's target set contains no target with Id 'Target2'.
Emulator: ListTargetsByRule still returns Targets[Id="Target2"] with the ECS parameters of the refused entry.
Why: the leftover is durable, not transient — every later read of the rule disagrees with AWS, so target counts and delete-with-targets refusals downstream are computed from state AWS does not have.
Fix: same component as cause 6; verify the target map is not mutated by entries that fail validation and that RemoveTargets clears them.
Traces: events-ebecs-task-count-and-batch-atomicity#task#13.

Neighbouring open issues 20261029T182228Z-events-sqs-eventbridge-puttargets-accepts-conflicting-invalid-input-forms-the.md and 20261101T182649Z-events-s3-sqs-puttargets-accepts-an-sqs-fifo.md cover other PutTargets validation gaps; neither carries any of the seven replay keys above.

## Evidence
Cause 1 — PutTargets, expected HTTP 400 with {"__type": "ValidationException", "message": "Parameter(s) EcsParameters must be specified for target: Target0."}; actual HTTP 200 with neither member present (12 steps across 4 fixtures).
Cause 2 — PutTargets, expected HTTP 400 with {"__type": "AccessDeniedException", "Message": "Access to the resource <ecs cluster ARN in a foreign account> is denied. Reason: Adding cross-account target is not permitted."}; actual HTTP 200, no error body.
Cause 3 — PutRule, expected HTTP 400 with {"__type": "ValidationException", "message": "Parameter ScheduleExpression is not valid."}; actual HTTP 200 (rule created).
Cause 4 — DeleteEventBus, expected HTTP 200; actual HTTP 400.
Cause 5 — PutRule, expected {"__type": "InvalidEventPatternException", "message": "Event pattern is not valid. Reason: \"source\" must be an object or an array\n at [Source: (String)\"<pattern>\"; line: 1, column: 13]"}; actual {"__type": "ResourceNotFoundException", "message": "Event bus <name> does not exist."} — same 400 status, wrong error class and message.
Cause 6 — ListTargetsByRule, expected Targets with no index [1] or [2]; actual two extra entries (Ids Target1, Target2) carrying the EcsParameters of PutTargets batches AWS refused.
Cause 7 — ListTargetsByRule, expected no Targets[Id="Target2"]; actual that entry still present.

## Steps to Reproduce
Replay the corpus: replay_wire_corpus() against the emulator built from this checkout.
Cause 1: PutRule a rule, then PutTargets one target whose Arn is an ECS cluster ARN and which omits EcsParameters; read the status and body (events-ebecs-launch-type-and-network-mistakes#task#2).
Cause 2: PutTargets a target whose ECS cluster ARN names a different account than the caller's; read $.__type and $.Message (events-ebecs-task-count-and-batch-atomicity#task#10).
Cause 3: PutRule with a malformed ScheduleExpression (e.g. a rate with a mismatched unit or a five-field cron) and then ListRules for the name (events-poller-cron-expression-mistakes#task#2).
Cause 4: run that fixture's teardown — DeleteEventBus on the custom bus (events-poller-cron-expression-mistakes#teardown#13).
Cause 5: PutRule naming a nonexistent EventBusName and an EventPattern whose "source" is a bare string; read $.__type (events-put-rule-checks-the-event-pattern-before-the-bus-and-never-lands-on-default#task#2).
Causes 6-7: PutTargets a multi-entry batch containing entries AWS refuses, then ListTargetsByRule for that rule (events-ebecs-task-count-and-batch-atomicity#task#6 and #task#13).
