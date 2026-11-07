Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.obs-sfn-3-express-billed-metrics-equal-summed-billing-details-standard-twin-has-none
Issue-Key: cloudwatch__stepfunctions.obs-sfn-3-express-billed-metrics-equal-summed-billing-details-standard-twin-has-none|_poll_until_stable#16|Timeout|time.sleep(interval_s)
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# cloudwatch__stepfunctions: the AWS/States Express billed-duration/billed-memory metric series are never published for EXPRESS executions

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
Cause 1 of 1 — EXPRESS billing metrics are not emitted to AWS/States.
AWS: recorded behavior (Step Functions dev guide, express-limits/metrics): every EXPRESS execution publishes ExpressExecutionMemory (one sample per execution), ExpressExecutionBilledDuration and ExpressExecutionBilledMemory to the AWS/States namespace under [StateMachineArn]; the Sums of the billed series equal the sum of the per-execution StartSyncExecution billingDetails (billedDurationInMilliseconds, and billedMemoryUsedInMB converted at 10^6 bytes per billed MB), and a STANDARD state machine publishes no Express series at all.
Emulator: StartSyncExecution returns its billingDetails, but polling CloudWatch for the three AWS/States Express series never yields the readings, so the agreement between metrics and billingDetails can never be checked; the replay ends on its budget with the series empty.
Why: EXPRESS cost and memory monitoring is built entirely on these series — an empty series reads as "no billable usage", and the STANDARD-vs-EXPRESS distinction (STANDARD must have none) is unobservable.
Fix: the EXPRESS execution path's metric emission (publish ExpressExecutionMemory / ExpressExecutionBilledDuration / ExpressExecutionBilledMemory under [StateMachineArn]) and the CloudWatch store serving them.
Traces: cloudwatch__stepfunctions.obs-sfn-3-express-billed-metrics-equal-summed-billing-details-standard-twin-has-none

## Evidence
From this replay of simparity-behavioral-aws-v1: the check 'obs-sfn-3-express-billed-metrics-equal-summed-billing-details-standard-twin-has-none' ended with "Check timed out after 60.0s" (its own metric budget is 420 s) while polling AWS/States for ExpressExecutionMemory, ExpressExecutionBilledDuration and ExpressExecutionBilledMemory under the EXPRESS state machine's StateMachineArn after three StartSyncExecution calls with distinct wait values. No datapoint had been served when the budget ended.

## Steps to Reproduce
1. Create an IAM role trusted by states.amazonaws.com, a /aws/vendedlogs/states/ log group, an EXPRESS state machine logging to it, and a STANDARD twin.
2. Call StartSyncExecution three times with inputs {"wait": 0}, {"wait": 1}, {"wait": 2}; record each response's billingDetails.
3. Poll cloudwatch:GetMetricStatistics in namespace AWS/States for ExpressExecutionMemory, ExpressExecutionBilledDuration and ExpressExecutionBilledMemory with Dimension StateMachineArn=<express arn>; the billed Sums must equal the summed billingDetails (memory at 10^6 bytes per billed MB) and ExpressExecutionMemory must have one sample per execution.
4. Query the same series for the STANDARD twin: they must be empty.
