Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.access-logs-enable-rejected-for-nonexistent-bucket cloudwatch__elbv2__s3.access-logs-enable-rejected-when-policy-grants-only-another-prefix cloudwatch__elbv2__s3.access-logs-enable-rejected-without-a-bucket-attribute cloudwatch__elbv2__s3.access-logs-enable-rejected-without-bucket-policy-grant cloudwatch__elbv2__s3.access-logs-prefix-starting-with-awslogs-rejected cloudwatch__elbv2__s3.alb-access-log-attributes-default-off-and-round-trip-through-disable cloudwatch__elbv2__s3.target-group-deregistration-delay-above-3600-seconds-rejected cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix cloudwatch__elbv2__s3.lambda-target-request-body-over-1-mb-answered-413-without-invoking-the-function cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm cloudwatch__elbv2__s3.e2e-diagnose-alb-five-xx-errors-lifecycle
Issue-Key: cloudwatch__elbv2__s3.access-logs-enable-rejected-for-nonexistent-bucket|_error#7|AssertionError|raise AssertionError("the call was ACCEPTED; real AWS refuses it")
Issue-Key: cloudwatch__elbv2__s3.access-logs-enable-rejected-when-policy-grants-only-another-prefix|_error#6|AssertionError|raise AssertionError("the call was ACCEPTED; real AWS refuses it")
Issue-Key: cloudwatch__elbv2__s3.access-logs-enable-rejected-without-a-bucket-attribute|_error#6|AssertionError|raise AssertionError("the call was ACCEPTED; real AWS refuses it")
Issue-Key: cloudwatch__elbv2__s3.access-logs-enable-rejected-without-bucket-policy-grant|_error#5|AssertionError|raise AssertionError("the call was ACCEPTED; real AWS refuses it")
Issue-Key: cloudwatch__elbv2__s3.access-logs-prefix-starting-with-awslogs-rejected|_error#7|AssertionError|raise AssertionError("the call was ACCEPTED; real AWS refuses it")
Issue-Key: cloudwatch__elbv2__s3.alb-access-log-attributes-default-off-and-round-trip-through-disable|run#9|AssertionError|assert got == want, f"a fresh ALB's attributes read {got!r}, expected the defaults {want!r}"
Issue-Key: cloudwatch__elbv2__s3.target-group-deregistration-delay-above-3600-seconds-rejected|run#12|AssertionError|assert over == ("ValidationError", f"'{_KEY}' value '3601' must be between '0-3600' inclusive"), (
Issue-Key: cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.lambda-target-request-body-over-1-mb-answered-413-without-invoking-the-function|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3|DescribeLoadBalancerAttributes|LoadBalancerNotFound
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# cloudwatch__elbv2__s3: 5 net-new root causes — ALB access-log config accepted unvalidated, null attribute defaults, wrong attribute-range error, dead ALB data plane, LoadBalancerNotFound

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
Five distinct net-new root causes in the ALB access-logging / target-metric surface. Each cause is numbered with its own failing check ids; all five must be closed.

Cause 1 of 5: ModifyLoadBalancerAttributes accepts access-log configurations that AWS refuses.
AWS: Smithy (elasticloadbalancingv2) models ModifyLoadBalancerAttributes with the refusals InvalidConfigurationRequestException (awsQueryError code InvalidConfigurationRequest, HTTP 400) and LoadBalancerNotFoundException. AWS validates the access-log destination before storing it and answers 400 InvalidConfigurationRequest when: access_logs.s3.enabled=true names a bucket that does not exist; the bucket policy grants the log-delivery principal only a different prefix than access_logs.s3.prefix; access_logs.s3.enabled=true is sent without an access_logs.s3.bucket attribute; the bucket has no log-delivery grant at all; or access_logs.s3.prefix begins with 'AWSLogs'. The message's fixed text is the InvalidConfigurationRequest wording for that class; its variable parts are the bucket name and prefix from the request.
Emulator: all five requests are ACCEPTED (2xx) and the attributes are stored, so the check's refusal assertion fails with 'the call was ACCEPTED; real AWS refuses it'.
Why: a caller gets a load balancer that believes it is logging to a destination that can never receive logs, and the misconfiguration is only discovered as silently missing log objects instead of at the API boundary.
Fix: the ModifyLoadBalancerAttributes handler's validation in the elbv2 service crate (access-log destination existence, bucket-policy grant, prefix constraints, attribute co-requirements).
Traces: cloudwatch__elbv2__s3.access-logs-enable-rejected-for-nonexistent-bucket, cloudwatch__elbv2__s3.access-logs-enable-rejected-when-policy-grants-only-another-prefix, cloudwatch__elbv2__s3.access-logs-enable-rejected-without-a-bucket-attribute, cloudwatch__elbv2__s3.access-logs-enable-rejected-without-bucket-policy-grant, cloudwatch__elbv2__s3.access-logs-prefix-starting-with-awslogs-rejected

Cause 2 of 5: DescribeLoadBalancerAttributes serialises the unset access-log attributes with no value instead of an empty string.
AWS: a fresh ALB's attributes include access_logs.s3.enabled='false', access_logs.s3.bucket='' and access_logs.s3.prefix='' — the two string attributes are present with an empty Value, and the same holds after logging is enabled and then disabled.
Emulator: returns those two attributes with no value at all (the client reads None), while enabled/idle_timeout/deletion_protection are correct.
Why: a client that reads attributes into a map and compares or concatenates the bucket/prefix sees a null where AWS guarantees a string, which breaks round-trip comparisons and any code that treats the value as text.
Fix: the attribute serializer for DescribeLoadBalancerAttributes (and the default attribute state) in the elbv2 service crate.
Traces: cloudwatch__elbv2__s3.alb-access-log-attributes-default-off-and-round-trip-through-disable

Cause 3 of 5: ModifyTargetGroupAttributes refuses an out-of-range deregistration delay with the wrong error code and message.
AWS: recorded behaviour — a deregistration_delay.timeout_seconds above the 0-3600 range is refused with error code 'ValidationError' and the message "'deregistration_delay.timeout_seconds' value '<value>' must be between '0-3600' inclusive"; the quoted attribute key and bounds are fixed text and the value is taken from the request. (Smithy declares InvalidConfigurationRequestException for this operation, but the recording shows the range check answering ValidationError.)
Emulator: answers error code 'InvalidConfigurationRequest' with the message "The value '<value>' is not valid for the attribute 'deregistration_delay.timeout_seconds': it must be an integer from 0 to 3600".
Why: both the code a client branches on and the message it surfaces differ from AWS, so error handling keyed on ValidationError does not fire.
Fix: the ModifyTargetGroupAttributes handler's attribute-range validation and its error construction in the elbv2 service crate.
Traces: cloudwatch__elbv2__s3.target-group-deregistration-delay-above-3600-seconds-rejected

Cause 4 of 5: the ALB data plane answers no request, so no access-log object and no per-target CloudWatch metric is ever produced.
AWS: recorded behaviour — a request to the load balancer's DNS name is routed to the registered target (instance or Lambda), the ALB writes a gzipped access-log record under <prefix>/AWSLogs/<account>/elasticloadbalancing/<region>/..., writes the ELB test file when logging is first enabled, and publishes HTTPCode_Target_5XX_Count / HTTPCode_ELB_5XX_Count / RequestCount / LambdaUserError to CloudWatch under the AWS/ApplicationELB namespace with LoadBalancer and TargetGroup dimensions; an over-1-MiB body to a Lambda target is answered 413 without invoking the function and recorded with its own error reason.
Emulator: nothing is served, written or published; every one of these checks polls and then fails with 'Check timed out after 60.0s'.
Why: no observability or routing behaviour of an ALB can be exercised — access-log fields, the elb/target 5xx split, metric dimensioning and alarm behaviour are all unobservable, and a client waits out its whole budget.
Fix: the elbv2 data plane (listener/target routing, access-log writer, and the CloudWatch metric publisher it feeds) in the elbv2 service crate.
Traces: cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class, cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address, cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix, cloudwatch__elbv2__s3.lambda-target-request-body-over-1-mb-answered-413-without-invoking-the-function, cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s, cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx, cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered, cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm

Cause 5 of 5: DescribeLoadBalancerAttributes answers LoadBalancerNotFound for a load balancer created in the same run.
AWS: DescribeLoadBalancerAttributes on the ARN returned by CreateLoadBalancer succeeds; LoadBalancerNotFoundException (awsQueryError LoadBalancerNotFound, 400) is reserved for an ARN that names no load balancer.
Emulator: refuses with LoadBalancerNotFound and the message "Load balancer '<arn>' not found" for the ARN it has just minted, aborting the deploy phase.
Why: a client cannot read back the attributes of the load balancer it just created, and a 'not found' refusal misreports an existing resource.
Fix: the load-balancer lookup shared by DescribeLoadBalancerAttributes and the create path's state write, in the elbv2 service crate.
Traces: cloudwatch__elbv2__s3.e2e-diagnose-alb-five-xx-errors-lifecycle

## Evidence
Cause 1 — replay detail: 'AssertionError: the call was ACCEPTED; real AWS refuses it' for all five refusal checks. Expected: HTTP 400, error code InvalidConfigurationRequest. Actual: 2xx, attributes stored.
Cause 2 — 'AssertionError: a fresh ALB''s attributes read {''access_logs.s3.enabled'': ''false'', ''access_logs.s3.bucket'': None, ''access_logs.s3.prefix'': None, ''idle_timeout.timeout_seconds'': ''60'', ''deletion_protection.enabled'': ''false''}, expected the defaults {''access_logs.s3.enabled'': ''false'', ''access_logs.s3.bucket'': '''', ''access_logs.s3.prefix'': '''', ...}'. Expected empty-string values; actual: absent.
Cause 3 — expected ('ValidationError', "'deregistration_delay.timeout_seconds' value '3601' must be between '0-3600' inclusive"); actual ('InvalidConfigurationRequest', "The value '3601' is not valid for the attribute 'deregistration_delay.timeout_seconds': it must be an integer from 0 to 3600").
Cause 4 — 'Error: Check timed out after 60.0s' on all eight checks; expected an access-log object / test file / CloudWatch datapoint within the polling budget.
Cause 5 — '[phase 1/5 deploy] LoadBalancerNotFoundException: An error occurred (LoadBalancerNotFound) when calling the DescribeLoadBalancerAttributes operation: Load balancer ''<alb arn>'' not found'; expected the attributes of the just-created ALB.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite. 2. Replay the cloudwatch__elbv2__s3 checks in trace_ids. 3. Cause 1: create an ALB, then ModifyLoadBalancerAttributes with access_logs.s3.enabled=true and (a) a bucket that does not exist, (b) a bucket whose policy grants only another prefix, (c) no bucket attribute, (d) a bucket with no delivery grant, (e) prefix 'AWSLogs/...'; each is accepted. 4. Cause 2: DescribeLoadBalancerAttributes on a fresh ALB and read access_logs.s3.bucket/prefix. 5. Cause 3: ModifyTargetGroupAttributes with deregistration_delay.timeout_seconds=3601. 6. Cause 4: send a request to the ALB DNS name and poll the log bucket and AWS/ApplicationELB metrics. 7. Cause 5: CreateLoadBalancer then DescribeLoadBalancerAttributes on the returned ARN.
