Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix
Issue-Key: cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix|_poll#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last {last!r}")
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# cloudwatch__elbv2__s3: enabling ALB access logs writes no ELBAccessLogTestFile into the bucket, so the delivery-permission probe AWS performs at enable time never happens

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
One net-new root cause (1 of 1) behind the one surviving unmarked cloudwatch__elbv2__s3 failure.

Related open filing that does NOT carry this replay key: 20261104T055008Z-cloudwatch-elbv2-s3-5-net-new-root-causes.md owns the other 14 failures in this service (including the enable-time refusals); this check — the positive case, that a successful enable leaves a test file behind — is listed unmarked and survives.

Cause 1 of 1: a successful access-logs enable produces no object in the log bucket at all.
AWS: recorded behavior only for the object write; the operation is modeled. Smithy: elbv2 ModifyLoadBalancerAttributes(LoadBalancerArn required, Attributes required, @length 0..20) -> ModifyLoadBalancerAttributesOutput{Attributes}, errors InvalidConfigurationRequestException and LoadBalancerNotFoundException (both client, HTTP 400, awsQuery codes InvalidConfigurationRequest / LoadBalancerNotFound). Recorded AWS behavior: setting access_logs.s3.enabled=true with access_logs.s3.bucket=<bucket> and access_logs.s3.prefix=<prefix> makes the load balancer immediately write one plaintext marker object, keyed '<prefix>/AWSLogs/<account-id>/ELBAccessLogTestFile' (and 'AWSLogs/<account-id>/ELBAccessLogTestFile' when the prefix is empty) — non-empty, not gzip-compressed, i.e. not a log file. That write IS the permission probe: if it cannot be performed the enable is refused instead. Re-enabling with a different prefix ADDS a second test file at the new key alongside the first; it does not replace it. The key template's fixed parts are 'AWSLogs/', '/ELBAccessLogTestFile' and the '/'-joined prefix; the variable parts are the prefix from the request and the account id of the caller.
Emulator: the call succeeds, but the bucket stays empty. The check polls ListObjectsV2 for 120 s after enabling with prefix 'tigris' and reports `AssertionError: the test file after enabling with prefix 'tigris' not met within 120s; last []`, so the second leg (re-enable with an empty prefix must ADD a second key) is never reached either. ('tigris' is this run's prefix value filling the template, not a requirement.)
Why: the test file is the only synchronous, observable proof that log delivery is actually wired — it is what operators and IaC checks look for immediately after enabling, long before the first hourly log object appears. With nothing written, a caller cannot distinguish 'logging is configured and the bucket policy works' from 'configured but every future delivery will fail', and the accumulate-don't-replace behavior across prefixes cannot be observed at all.
Fix: the access-logs enable path of the ModifyLoadBalancerAttributes handler in the elbv2 service crate — on a transition to access_logs.s3.enabled=true, perform the delivery probe by writing a non-empty plaintext ELBAccessLogTestFile object at '<prefix>/AWSLogs/<account-id>/ELBAccessLogTestFile' into the configured bucket (empty prefix => 'AWSLogs/<account-id>/ELBAccessLogTestFile'), adding a new object per configured prefix rather than overwriting an existing one.
Traces: cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 202/554 passed; cloudwatch__elbv2__s3 1/16 (6%), 15 failed. 14 are marked OWNED by 20261104T055008Z-cloudwatch-elbv2-s3-5-net-new-root-causes.md; this one is not.

[mismatch] cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix:
  File "check_cloudwatch__elbv2__s3_enabling_access_logs_writes_the_test_file_under_the_configured_prefix.py", line 33, in run
    keys = _poll(lambda: _objects(s3, bucket), lambda k: k != [], timeout_s=120, ...)
  AssertionError: the test file after enabling with prefix 'tigris' not met within 120s; last []

The check mints a bucket, grants the logdelivery.elasticloadbalancing.amazonaws.com principal a bucket policy, asserts the bucket starts empty, creates an ALB, then enables access logs with prefix 'tigris'. Expected (check lines 53-66): the bucket holds exactly ['tigris/AWSLogs/<account>/ELBAccessLogTestFile'], whose body is non-empty and NOT gzip ('\x1f\x8b'); re-enabling with an empty prefix then leaves exactly ['AWSLogs/<account>/ELBAccessLogTestFile', 'tigris/AWSLogs/<account>/ELBAccessLogTestFile']. Actual: the bucket is still empty after 120 s, so neither the first key, its plaintext body, nor the second-prefix accumulation is observable.

Smithy consulted: elbv2 ModifyLoadBalancerAttributes(LoadBalancerArn, Attributes[0..20]) -> ModifyLoadBalancerAttributesOutput, errors InvalidConfigurationRequestException/LoadBalancerNotFoundException (client, 400).

## Steps to Reproduce
1. s3.create_bucket and attach a bucket policy granting logdelivery.elasticloadbalancing.amazonaws.com s3:PutObject on the bucket; confirm ListObjectsV2 is empty.
2. Create an ALB; then elbv2.modify_load_balancer_attributes(LoadBalancerArn=<lb>, Attributes=[{'Key':'access_logs.s3.enabled','Value':'true'},{'Key':'access_logs.s3.bucket','Value':<bucket>},{'Key':'access_logs.s3.prefix','Value':'tigris'}]).
3. Poll s3.list_objects_v2(Bucket=<bucket>): AWS shows exactly 'tigris/AWSLogs/<account-id>/ELBAccessLogTestFile'; get_object on it returns a non-empty, non-gzip body. The emulator shows no objects for 120 s.
4. Re-enable with access_logs.s3.prefix='' and poll again: AWS ADDS 'AWSLogs/<account-id>/ELBAccessLogTestFile' alongside the first key.
5. Re-replay cloudwatch__elbv2__s3.enabling-access-logs-writes-the-test-file-under-the-configured-prefix.
