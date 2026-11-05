Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__lambda.sam-schedule-events-create-rules-in-declared-state
Issue-Key: cloudformation__events__lambda.sam-schedule-events-create-rules-in-declared-state|run#10|AssertionError|assert got == (schedule, state), f"{logical}: (schedule, state) {got!r}, expected {(schedule, state)!r}"
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# cloudformation__events__lambda: a SAM Schedule event declared disabled creates its EventBridge rule ENABLED, so the declared rule state is ignored

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRule

## Description
One root cause. The failing check is not marked OWNED or ALREADY FILED in the replay report.

AWS: recorded behavior only for the SAM transform; the EventBridge surface is modeled — PutRule/DescribeRule carry a State member whose values are ENABLED and DISABLED. A SAM AWS::Serverless::Function with a Schedule event source that declares the event disabled (Enabled: false, or State: DISABLED) is expanded into an AWS::Events::Rule whose State is DISABLED, so DescribeRule on the synthesized rule reports ScheduleExpression unchanged and State "DISABLED". Only the state differs between the enabled and disabled twin; the schedule expression is carried through verbatim in both.
Emulator: the synthesized rule's ScheduleExpression is correct but its State is always ENABLED — the declared disabled state is dropped during the transform (or defaulted at PutRule), so the enabled and disabled twins are indistinguishable.
Why: a client that ships a scheduled function in a paused state gets a rule that fires. Deployments that rely on declaring a schedule disabled and enabling it later (blue/green cutovers, maintenance windows, cost control) silently run the function.
Fix: the SAM transform's expansion of a Schedule event source into AWS::Events::Rule (mapping Enabled/State onto the rule's State), and the AWS::Events::Rule resource provider that applies it via PutRule.
Traces: cloudformation__events__lambda.sam-schedule-events-create-rules-in-declared-state.

## Evidence
Expected for the disabled twin: DescribeRule returns (ScheduleExpression, State) == ('rate(7 days)', 'DISABLED').
Actual (replay): `AssertionError: FnPaused: (schedule, state) ('rate(7 days)', 'ENABLED'), expected ('rate(7 days)', 'DISABLED')` — the schedule expression is the one declared in the template; the state is not.

## Steps to Reproduce
1. CreateStack (Capabilities CAPABILITY_AUTO_EXPAND) with a SAM template declaring two AWS::Serverless::Function resources that each have a Schedule event source with the same ScheduleExpression, one declared enabled and one declared disabled.
2. Wait for CREATE_COMPLETE.
3. Call events DescribeRule on each synthesized rule — both report State "ENABLED"; the disabled one should report "DISABLED".
