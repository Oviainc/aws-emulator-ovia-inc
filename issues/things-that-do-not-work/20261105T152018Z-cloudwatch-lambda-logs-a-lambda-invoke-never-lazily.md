Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow cloudwatch__lambda__logs.metrics-math-and-emf-lambda-stdout-emf-line-materializes-metric-without-put-metric-data
Issue-Key: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow|_run_shell#14|Timeout|result = subprocess.run(
Issue-Key: cloudwatch__lambda__logs.metrics-math-and-emf-lambda-stdout-emf-line-materializes-metric-without-put-metric-data|_poll#18|Timeout|last = fn()
Issue-Commit: 8d52f52fda0ec71b00c9bf8f5ac2d8cb4a7a8ef6

# cloudwatch__lambda__logs: a Lambda invoke never lazily creates /aws/lambda/<fn>, and an EMF line printed to stdout never materialises a CloudWatch metric (2 causes)

> _Note: this issue was identified against the code base at commit `8d52f52fda0ec71b00c9bf8f5ac2d8cb4a7a8ef6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes in the Lambda -> CloudWatch Logs -> CloudWatch metrics chain. Both checks are killed by the 60s run budget on their first observable step.

Cause 1 of 2: a successful `lambda invoke` does not lazily create the function's own log group, so `logs describe-log-groups --log-group-name-prefix /aws/lambda/<fn>` never returns it.
AWS: recorded behavior only (the lazy-creation rule is AWS Lambda documented behaviour, not modelled in Smithy). Before the first invocation the group does not exist, so DescribeLogGroups with that prefix returns an empty logGroups list; the first invocation by a function whose execution role carries AWSLambdaBasicExecutionRole creates the log group named exactly `/aws/lambda/<function-name>`, and DescribeLogGroups then returns one entry whose logGroupName is that string. Invoke itself answers 200 with StatusCode 200 in both cases.
Emulator: the pre-invoke assertion passes (no group), the invoke reports StatusCode 200, and the subsequent DescribeLogGroups poll never returns any logGroups entry for the prefix — the check ends with `Error: Check timed out after 60.0s`.
Why: the single most common way a client finds a function's logs (derive `/aws/lambda/<name>`, describe it, then read it) never resolves; log-group-dependent wiring such as retention policies, subscription filters and metric filters on the function's own group cannot be attached because the group never appears.
Fix: the Lambda invocation path's log-group/log-stream provisioning into CloudWatch Logs (the component that must create `/aws/lambda/<function-name>` on first invocation and register it in the logs state).
Trace: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow.

Cause 2 of 2: an Embedded Metric Format (EMF) line printed to a Lambda's stdout is never extracted into a CloudWatch metric, and the raw line never lands in the function's log group.
AWS: recorded behavior only (EMF extraction is documented CloudWatch behaviour, not modelled in Smithy). A log event whose message is an EMF document — `_aws.CloudWatchMetrics[].Namespace`, `Dimensions`, `Metrics[].Name`/`Unit` plus the member values — materialises the declared metric WITHOUT any PutMetricData call, so GetMetricStatistics over that namespace/metric name and the EXACT declared dimension set returns one datapoint per period carrying the aggregate of the declared values (here Sum, SampleCount, Minimum, Maximum and the declared Unit), a query naming a different Unit or a dimension subset/superset returns no datapoints, and the EMF line itself is retrievable verbatim from the function's log group. The execution role in this check explicitly Denies cloudwatch:PutMetricData, so a datapoint can only come from extraction.
Emulator: the GetMetricStatistics poll never sees any datapoint for the declared namespace/metric/dimensions and the check ends with `Error: Check timed out after 60.0s`; the subsequent verbatim-log-line assertion is never reached.
Why: any workload that publishes metrics the EMF way — the documented zero-API-call path used by the Lambda Powertools libraries and the CloudWatch agent — produces no metrics at all here, so alarms and dashboards built on those metrics never have data, and the unit/dimension-identity rules (which make a wrong-unit or wrong-dimension query return nothing) cannot be validated.
Fix: the CloudWatch Logs ingest path's EMF parsing/extraction component that must emit the declared metrics into CloudWatch metric state, plus the Lambda stdout-to-log-group delivery it depends on (see Cause 1).
Trace: cloudwatch__lambda__logs.metrics-math-and-emf-lambda-stdout-emf-line-materializes-metric-without-put-metric-data.

Related but NOT covering these: open issue 20261105T111951Z-cloudwatch-lambda-logs-5-net-new-root-causes.md owns other checks in this group; the suite does not attribute these two to it.

## Evidence
Cause 1 — Expected: DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<fn>") returns [] before the first invoke and exactly one entry with logGroupName == "/aws/lambda/<fn>" after it. Actual: still [] after a StatusCode-200 invoke; `Error: Check timed out after 60.0s`.

Cause 2 — Expected: after a handler prints one EMF document declaring namespace N, metric CheckoutLatency (Unit Milliseconds), dimension set {Service, Stage} and three values, GetMetricStatistics(Namespace=N, MetricName=CheckoutLatency, Dimensions={Service,Stage}, Period=60, Statistics=[Sum,SampleCount,Minimum,Maximum]) returns ONE datapoint whose (Sum, SampleCount, Minimum, Maximum, Unit) equals the aggregate of the declared values with the declared unit; the same query with a different Unit, with a dimension subset, or with an extra dimension returns []; and FilterLogEvents on /aws/lambda/<fn> contains the EMF line byte-for-byte. Actual: no datapoint ever appears; `Error: Check timed out after 60.0s`.

## Steps to Reproduce
Cause 1:
1. CreateRole trusting lambda.amazonaws.com, attach arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole.
2. CreateFunction (python3.12) and wait for State==Active.
3. DescribeLogGroups --log-group-name-prefix /aws/lambda/<fn> -> [] (correct).
4. Invoke the function (StatusCode 200).
5. Poll DescribeLogGroups with the same prefix -> stays empty.

Cause 2:
1. CreateLogGroup /aws/lambda/<fn> and PutRetentionPolicy.
2. CreateRole trusting lambda.amazonaws.com with an inline policy that DENIES cloudwatch:PutMetricData; CreateFunction whose handler only prints the lines it is given.
3. Invoke it with one EMF document: {"_aws":{"Timestamp":<ms>,"CloudWatchMetrics":[{"Namespace":"<ns>","Dimensions":[["Service","Stage"]],"Metrics":[{"Name":"CheckoutLatency","Unit":"Milliseconds"}]}]},"Service":"checkout","Stage":"prod","CheckoutLatency":[12,30,48],"requestId":"..."}.
4. Poll GetMetricStatistics for that namespace/metric/dimension set -> no datapoints.
5. (blocked) FilterLogEvents on the group for the verbatim EMF line -> never reached.
