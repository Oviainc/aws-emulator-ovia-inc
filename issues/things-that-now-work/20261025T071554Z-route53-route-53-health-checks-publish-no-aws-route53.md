Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.health-check-failure-threshold-delays-status-flip
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.health-check-failure-threshold-delays-status-flip|_poll_for_datapoints#9|Timeout|time.sleep(15.0)
Issue-Commit: 79958e9ac1a4e2804b5bffcb8136e3ce17c50800

# Route 53 health checks publish no AWS/Route53 HealthCheckStatus CloudWatch metric, so health-check status hysteresis is unobservable

> _Note: this issue was identified against the code base at commit `79958e9ac1a4e2804b5bffcb8136e3ce17c50800`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHealthCheck

## Description
Cause 1 of 1: a health check created through Route 53 never produces the aggregate AWS/Route53 `HealthCheckStatus` CloudWatch metric that AWS publishes for it, so a client polling `cloudwatch:GetMetricStatistics` for that metric gets an answer set that never becomes non-empty and never flips.

AWS: Route 53 evaluates an endpoint from its checker fleet and publishes the aggregate assessment for each health check as the CloudWatch metric `HealthCheckStatus` in namespace `AWS/Route53`, dimensioned by `HealthCheckId` (1 = healthy, 0 = unhealthy). The debouncing rule is modeled on the operation input: Smithy `HealthCheckConfig.FailureThreshold` is documented as "The number of consecutive health checks that an endpoint must pass or fail for Amazon Route 53 to change the current status of the endpoint from unhealthy to healthy or vice versa" (default 3), and `HealthCheckConfig.RequestInterval` is the probe spacing. So a newly created health check pointed at an unreachable endpoint is reported healthy until FailureThreshold consecutive probes have failed, and only then does the aggregate metric report 0. The metric itself is not described by the Route 53 Smithy model (GetHealthCheckStatus/GetHealthCheckLastFailureReason expose only each checker's raw last single-probe observation, not the debounced aggregate) — for the metric plane this is recorded behavior only: the recording shows `GetMetricStatistics(Namespace=AWS/Route53, MetricName=HealthCheckStatus, Dimensions=[{HealthCheckId}], Period=60, Statistics=[Minimum])` returning datapoints, the earliest with `Minimum == 1.0`, and a later datapoint with `Minimum == 0.0` once the threshold is crossed.

Emulator: CreateHealthCheck succeeds (it is no longer the NotImplemented stub that issue 20261024T114159Z-route53-createhealthcheck-stub.md describes — the check gets past creation), but the health check is inert with respect to metrics: the replayed check loops on GetMetricStatistics for that namespace/metric/dimension and never advances past its polling loop, and the check is killed by the harness timeout (`Check timed out after 60.0s`) instead of observing either the initial healthy datapoint or the later flip. No `AWS/Route53` `HealthCheckStatus` datapoint for the created HealthCheckId is ever observed.

Why: a client that drives DNS failover or alarms off health-check status — the standard pattern of a CloudWatch alarm on `AWS/Route53`/`HealthCheckStatus` dimensioned by HealthCheckId — gets an empty metric forever. Alarms stay in INSUFFICIENT_DATA, failover automation never triggers, and no test of the FailureThreshold hysteresis (new check is healthy first, flips only after FailureThreshold consecutive failures) can run at all, because the series it is asserted over does not exist. The gap is silent: every Route 53 call returns 200, so nothing tells the client the status plane is missing.

Hypothesis: the health-check record is stored as pure configuration with no evaluator attached, so nothing ever emits a metric datapoint for it.

Fix: the Route 53 health-check state/evaluation component (the CreateHealthCheck handler's state, which owns the health-check record and should drive its periodic assessment) must emit the aggregate `HealthCheckStatus` datapoints into the shared CloudWatch metric store under namespace `AWS/Route53` with the `HealthCheckId` dimension, honouring FailureThreshold/RequestInterval for the transition, so GetMetricStatistics can read them back. Note the harness budget: the check is allowed 60s while FailureThreshold*RequestInterval in the recording is 150s of wall clock, so the emitted series must be produced on the emulator's own accelerated clock rather than real elapsed time for the behavior to be observable at all.

Traces: route53.health-check-failure-threshold-delays-status-flip (mismatch, "Check timed out after 60.0s"). No open issue covers the health-check metric plane; the nearest open issues are 20261025T035714Z-route53-route-53-health-check-references-are-unusable-changeresourcerecordsets.md (health-check IDs rejected by ChangeResourceRecordSets/CFN), 20261025T045147Z-route53-route-53-updatehealthcheck-has-no-atomic-optimistic-concurrency.md and 20261025T035714Z-route53-gethealthcheckstatus-stub.md (the per-checker API, not the aggregate metric) — none of them makes a health check publish HealthCheckStatus.

## Evidence
Expected (AWS, from the recording): after CreateHealthCheck with HealthCheckConfig={IPAddress:8.8.8.8, Port:8081, Type:TCP, RequestInterval:30, FailureThreshold:5}, cloudwatch.get_metric_statistics(Namespace='AWS/Route53', MetricName='HealthCheckStatus', Dimensions=[{'Name':'HealthCheckId','Value':<id>}], Period=60, Statistics=['Minimum']) returns a non-empty Datapoints list; the earliest datapoint has Minimum == 1.0 (still healthy, because FailureThreshold=5 consecutive failures at RequestInterval=30s cannot have accumulated yet), and a later datapoint has Minimum == 0.0 once the threshold is crossed.

Actual (emulator): CreateHealthCheck returns 200 with a HealthCheck.Id, but GetMetricStatistics for that namespace/metric/dimension never yields a datapoint — the check stays in its polling loop and is terminated with 'Check timed out after 60.0s'. Neither the initial healthy datapoint nor the flip to 0.0 is ever observable.

## Steps to Reproduce
1. route53.create_health_check(CallerReference=<unique>, HealthCheckConfig={'IPAddress':'8.8.8.8','Port':8081,'Type':'TCP','RequestInterval':30,'FailureThreshold':5}) and take HealthCheck.Id.
2. Poll cloudwatch.get_metric_statistics(Namespace='AWS/Route53', MetricName='HealthCheckStatus', Dimensions=[{'Name':'HealthCheckId','Value':<id>}], StartTime=now-1200s, EndTime=now, Period=60, Statistics=['Minimum']).
3. Observe Datapoints is empty on every poll; it never becomes non-empty, so neither the 'first datapoint is 1.0' nor the 'later flips to 0.0' assertion can be evaluated and the check runs until the harness timeout.
