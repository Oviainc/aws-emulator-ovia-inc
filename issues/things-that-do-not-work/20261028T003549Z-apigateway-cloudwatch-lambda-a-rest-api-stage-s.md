Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage
Issue-Key: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage|_http#7|Timeout|with urllib.request.urlopen(req, timeout=timeout) as resp:
Issue-Commit: 9b4e6b1ff6c9db4163689c6acf2e0383e4d25f93

# apigateway__cloudwatch__lambda: a REST API stage's AWS/ApiGateway request metrics are never obtainable — the Count/4XXError/5XXError/Latency/IntegrationLatency series keyed by ApiName+Stage never converge and the check never completes

> _Note: this issue was identified against the code base at commit `9b4e6b1ff6c9db4163689c6acf2e0383e4d25f93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause, grounded in one failing behavioral check whose id leads with `apigateway__cloudwatch__lambda.`. The report marks it neither ALREADY FILED nor OWNED. Cause count: 1.

=== Cause 1 of 1: a deployed REST API stage's request telemetry never reaches CloudWatch under the AWS/ApiGateway ApiName+Stage dimensions, and the stage/metric pipeline never settles ===
AWS: this is cross-service behavior that the Smithy models describe only at the edges — recorded behavior otherwise. Smithy (com.amazonaws.cloudwatch) GetMetricStatistics takes Namespace, MetricName, Dimensions, StartTime, EndTime, Period and Statistics and returns Datapoints; the AWS/ApiGateway namespace and its dimension keys are not modeled anywhere, so the requirement is the recorded one. Recorded behavior: a REGIONAL REST API with an `ANY /{proxy+}` AWS_PROXY integration to a Lambda, deployed to stage `live`, serves `GET <stage>/ok` as 200 with the function's own body and `GET <stage>/boom` as 500 with the function's own 500 body passed through; API Gateway then publishes, in namespace `AWS/ApiGateway` at 60 s resolution keyed by the dimension pair {ApiName=<the api's name>, Stage=live}: `Count` Sum == the exact number of requests made, `5XXError` Sum == exactly the number of integration 500s, `4XXError` published and Sum == 0 over at least one datapoint for a batch with no 4xx, and `Latency` and `IntegrationLatency` each with SampleCount == one sample per request (every request reached the integration). The counts are exact, not lower bounds. A REST API's series are keyed by ApiName ONLY: the same `Count` queried with dimensions {ApiId=<rest api id>, Stage=live} — the HTTP-API dimension pair — must return ZERO datapoints.
Emulator: the check never completes. The replay verdict is `Check timed out after 60.0s` with no assertion reached. Its first blocking wait is `_ready_then_align(base, [('/ok', 200), ('/boom', 500)])`, which re-probes both stage routes until 40 continuous seconds pass with BOTH answering their expected status before it even starts measuring; everything after it (14 HTTP requests, then `_await` polling GetMetricStatistics on a 10 s interval with 240 s budgets per metric) is likewise unbounded by the 60 s harness budget. So the timeout places the stall at the stage not serving the AWS_PROXY integration correctly and/or the metric series never being readable. Hypothesis, stated as such because neither a model nor the replay output names the component: the AWS/ApiGateway metric publication for REST stages is absent, so even a correctly serving stage would leave `_await` spinning until the budget expires. Note also that this check's own timing design (a 40 s quiet streak, then alignment to the next UTC minute boundary, then multi-minute metric polling) needs well over 60 s of wall clock even against a perfect target, so the timeout alone does not isolate which of the two stages stalled — it does establish that the end-to-end path is not obtainable.
Why: a client cannot observe any REST API traffic through CloudWatch against this emulator. Dashboards, CloudWatch alarms on 5XXError/Latency, autoscaling and SLO tooling all read exactly these series; with nothing published they read as "no traffic" rather than "no telemetry", so an alarm on 5XXError never fires for an API that is in fact serving 500s and an alarm with `treat missing data as breaching` fires permanently. The effect-absent half matters too: if the emulator ever publishes a REST API's Count under {ApiId, Stage} (the HTTP-API keying) instead of {ApiName, Stage}, a client's dashboards silently query the wrong series.
Fix: the API Gateway data plane's metric-emission component for a deployed REST stage, plus the CloudWatch state that must make those datapoints readable through GetMetricStatistics — each request served by a stage must emit, into namespace `AWS/ApiGateway` keyed by {ApiName, Stage}, one `Count` unit, one `Latency` and one `IntegrationLatency` sample, and a `4XXError`/`5XXError` unit classified from the response status actually returned (including a published zero for a class with no occurrences), at 60 s resolution; and a REST API must never publish these series under an `ApiId` dimension. The stage data plane must also pass an AWS_PROXY integration's own statusCode and body through unchanged, including a 500, which is the precondition this check waits on first.
Traces: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage.

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1: 173/344 passed (50.3%). Service group `apigateway__cloudwatch__lambda` 0/1 (0%), 1 failed. That one check is marked neither ALREADY FILED nor OWNED.

[mismatch] apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage
  Error: Check timed out after 60.0s — no assertion was reached, so no response value was captured.

What the check had already done, and where it must have been waiting (from the check body in the report):
  - iam CreateRole (lambda trust policy) and lambda CreateFunction(python3.12, handler returning {'statusCode':200,'body':'LAMBDA-200'} for any path and {'statusCode':500,'body':'LAMBDA-500'} for a path ending '/boom'), retried for up to 90 s past IAM propagation, then waiter function_active_v2.
  - apigateway CreateRestApi(endpointConfiguration={'types':['REGIONAL']}), CreateResource(pathPart='{proxy+}'), PutMethod(ANY, authorizationType='NONE'), PutIntegration(type='AWS_PROXY', integrationHttpMethod='POST', uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn arn>/invocations), lambda AddPermission(Principal='apigateway.amazonaws.com', SourceArn='arn:aws:execute-api:<region>:<acct>:<api id>/*'), CreateDeployment(stageName='live').
  - _ready_then_align(base, [('/ok', 200), ('/boom', 500)]) — loops probing BOTH routes every 0.5 s and requires 40 continuous seconds in which '/ok' answers 200 AND '/boom' answers 500, then aligns to a UTC minute boundary; raises 'no 40s window of correct answers from ... within 200s' if the streak never forms. The 60 s harness budget expired inside this wait or the metric waits after it.

The assertions that were therefore never evaluated (lines 221-250 of the check):
  stage = [{'Name':'ApiName','Value':<api name>}, {'Name':'Stage','Value':'live'}]
  Count      Sum        over Namespace 'AWS/ApiGateway', Period 60 -> must equal 14 (10 '/ok' + 4 '/boom')
  5XXError   Sum                                                   -> must equal exactly 4
  4XXError   Sum                                                   -> must be 0 over > 0 datapoints (published, reading zero)
  Latency    SampleCount                                           -> must equal 14
  IntegrationLatency SampleCount                                   -> must equal 14
  Count Sum with Dimensions [{'Name':'ApiId','Value':<api id>},{'Name':'Stage','Value':'live'}] -> must return 0 datapoints
(the 10/4/14 request counts are the check's constants _N_OK/_N_BOOM and their total; the api name, api id and function name are per-run values filling the dimension slots, not requirements.)

## Steps to Reproduce
Boot the emulator and point an AWS SDK at it (region = the Region the client signs for).
  1. iam CreateRole(AssumeRolePolicyDocument allowing sts:AssumeRole to Service lambda.amazonaws.com) -> note Role Arn.
  2. lambda CreateFunction(Runtime='python3.12', Role=<that arn>, Handler='index.handler', Timeout=10, ZipFile containing index.py whose handler returns {'statusCode':500,'body':'LAMBDA-500'} when (event['path'] or event['rawPath']) ends with '/boom' and {'statusCode':200,'body':'LAMBDA-200'} otherwise) -> wait until the function is Active. Note FunctionName FN and FunctionArn FA.
  3. apigateway CreateRestApi(name=NAME, endpointConfiguration={'types':['REGIONAL']}) -> note restApiId API; GetResources -> note the '/' resource id ROOT.
  4. apigateway CreateResource(restApiId=API, parentId=ROOT, pathPart='{proxy+}') -> note id PROXY; PutMethod(restApiId=API, resourceId=PROXY, httpMethod='ANY', authorizationType='NONE'); PutIntegration(restApiId=API, resourceId=PROXY, httpMethod='ANY', type='AWS_PROXY', integrationHttpMethod='POST', uri='arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/'+FA+'/invocations').
  5. lambda AddPermission(FunctionName=FN, StatementId='apigw', Action='lambda:InvokeFunction', Principal='apigateway.amazonaws.com', SourceArn='arn:aws:execute-api:<region>:<account>:'+API+'/*').
  6. apigateway CreateDeployment(restApiId=API, stageName='live') -> note the stage invoke base URL BASE.
  7. Probe the data plane: GET BASE+'/ok' and GET BASE+'/boom'.
     -> EXPECT 200 with body b'LAMBDA-200' and 500 with body b'LAMBDA-500' respectively (the integration's own statusCode and body passed through), and EXPECT both to keep answering that way continuously — this is the precondition the failing check waits 40 s on and never gets past within its budget.
     -> OBSERVE, per the replay, that this wait never completes inside the budget: the stage/metric path is not obtainable end to end.
  8. Issue 10 further GET BASE+'/ok' and 4 further GET BASE+'/boom' (14 requests total), noting the UTC minute T0 in which they were issued.
  9. cloudwatch GetMetricStatistics(Namespace='AWS/ApiGateway', MetricName='Count', Dimensions=[{'Name':'ApiName','Value':NAME},{'Name':'Stage','Value':'live'}], StartTime=T0, EndTime=now+3min, Period=60, Statistics=['Sum']).
     -> EXPECT the Sum over the returned datapoints to equal 14.
     -> OBSERVE no such series (nothing was published).
 10. Repeat step 9 for MetricName='5XXError' Statistics=['Sum'] -> EXPECT Sum == 4; for MetricName='4XXError' Statistics=['Sum'] -> EXPECT at least one datapoint and Sum == 0; for MetricName='Latency' and MetricName='IntegrationLatency' Statistics=['SampleCount'] -> EXPECT SampleCount == 14 each.
 11. Effect-absent: GetMetricStatistics(Namespace='AWS/ApiGateway', MetricName='Count', Dimensions=[{'Name':'ApiId','Value':API},{'Name':'Stage','Value':'live'}], same window/Period) -> EXPECT ZERO datapoints: a REST API's series are keyed by ApiName, never by the HTTP-API ApiId dimension.

Re-replay after the fix exactly the check id in trace_ids.
