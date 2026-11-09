Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__s3.alb-access-log-lines-are-delivered-to-s3-with-the-documented-fields elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni
Issue-Key: elbv2__s3.alb-access-log-lines-are-delivered-to-s3-with-the-documented-fields|_client_error#27|AssertionError|raise AssertionError(
Issue-Key: elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni|_get#29|PublishedUrlRejected|ip = socket.gethostbyname(ctx.endpoint_host(dns, 443)[0])
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# elbv2__s3: 2 net-new causes — ModifyLoadBalancerAttributes enables access logs without validating the bucket's delivery permission, and the published DNSName is an unserved elb.amazonaws.com host

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
Two net-new root causes (Cause 1 of 2, Cause 2 of 2). Related open filing that does NOT carry these replay keys: 20261104T055008Z-elbv2-s3-an-https-alb-writes-no-access.md is under this heading but the suite marks neither id below OWNED; the access-log validation cause is also filed for a different heading in 20261104T055008Z-cloudwatch-elbv2-s3-5-net-new-root-causes.md, which carries no elbv2__s3 check id.

Cause 1 of 2: ModifyLoadBalancerAttributes accepts access_logs.s3.enabled=true with a bucket whose policy does not grant the load balancer delivery, answering 200 instead of refusing.
AWS: Smithy (elasticloadbalancingv2) ModifyLoadBalancerAttributes declares errors InvalidConfigurationRequestException (awsQueryError code InvalidConfigurationRequest, HTTP 400) and LoadBalancerNotFoundException. AWS validates access-log delivery synchronously at enable time: it writes an ELBAccessLogTestFile under the configured prefix, and when the bucket policy does not permit the write the call is refused with HTTP 400 InvalidConfigurationRequest whose message names the bucket and the missing permission (fixed text as AWS words it for that error class; the bucket name and prefix are values taken from the request). Only a bucket that grants delivery is accepted.
Emulator: the call returns HTTP 200 with the full attribute list echoed back — access_logs.s3.enabled 'true', access_logs.s3.bucket and access_logs.s3.prefix set to the requested values — with no validation of the bucket, its policy or the prefix.
Why: a client gets a success for a configuration that will silently deliver nothing. The one signal AWS gives at configuration time (a 400 naming the bucket) is replaced by a 200, so a misconfigured logging pipeline looks correct and the absence of log lines later has no attributable cause.
Fix: the ELBv2 ModifyLoadBalancerAttributes handler's validation for the access_logs.s3.* attribute family — resolve the bucket, perform the delivery-permission probe, and refuse with InvalidConfigurationRequest (HTTP 400) when the bucket is missing or its policy does not grant the write.
Traces: elbv2__s3.alb-access-log-lines-are-delivered-to-s3-with-the-documented-fields.

Cause 2 of 2: ELBv2 publishes LoadBalancer.DNSName in the real-AWS elb.amazonaws.com domain — an address other than the one the emulator is reached on.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer -> LoadBalancers[].DNSName, "The public DNS name of the load balancer"; LoadBalancer.Scheme documents that an internet-facing load balancer's DNS name resolves to the load balancer nodes, so a client's TLS request to that name is served by the HTTPS listener and the access-log record for it carries the negotiated ssl_protocol/ssl_cipher and the SNI host. Recorded behavior only for the wire details.
Emulator: DNSName is <lb-name>-<id>.<region>.elb.amazonaws.com (name and digits are per-run values; the defect is the domain) and the harness refuses it with simparity.published.PublishedUrlRejected "endpoint_host was given <published-host>:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address".
Why: no request can be sent through the load balancer under its own name, so the TLS fields and SNI an access-log record is supposed to capture are unreachable.
Fix: the ELBv2 load-balancer state/serializer path that mints and reports DNSName, plus the HTTPS listener socket path that must terminate TLS on that address and write the access-log record for the request.
Traces: elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni.

## Evidence
Replay of suite simparity-behavioral-aws-v1: elbv2__s3 0/3 (0%), 3 failed; one is OWNED, these two are not.

Cause 1:
[mismatch] elbv2__s3.alb-access-log-lines-are-delivered-to-s3-with-the-documented-fields
  File "check_...py", line 518, in run -> err = _client_error(lambda: _enable_access_logs(ctx, lb_arn, bucket, prefix))
  AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'Attributes': [{'Key': 'access_logs.s3.enabled', 'Value': 'true'}, {'Key': 'access_logs.s3.bucket', 'Value': '<bucket>'}, {'Key': 'access_logs.s3.prefix', 'Value': '<prefix>'}, ...], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected: HTTP 400 InvalidConfigurationRequest naming the bucket and the missing delivery permission. Actual: HTTP 200 with the attributes echoed.

Cause 2:
[mismatch] elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni
  simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-lel-e82b538149d8-8791379177.us-west-2.elb.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

Smithy consulted: elasticloadbalancingv2 ModifyLoadBalancerAttributes (input LoadBalancerArn + Attributes, errors InvalidConfigurationRequestException/InvalidConfigurationRequest 400 and LoadBalancerNotFoundException); CreateLoadBalancer output LoadBalancers[] (LoadBalancer.DNSName, LoadBalancer.Scheme).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; both ids in trace_ids reproduce.
2. Cause 1: create an S3 bucket with NO policy granting ELB log delivery, then elbv2.modify_load_balancer_attributes(LoadBalancerArn=<alb>, Attributes=[{'Key':'access_logs.s3.enabled','Value':'true'},{'Key':'access_logs.s3.bucket','Value':<bucket>},{'Key':'access_logs.s3.prefix','Value':<prefix>}]). Expect HTTP 400 InvalidConfigurationRequest; the emulator answers 200 and echoes the attributes.
3. Cause 2: create an HTTPS listener on the ALB and open a TLS connection to <DNSName>:443 with SNI. The emulator publishes <name>-<id>.<region>.elb.amazonaws.com, which is not one of its own hosts, so the probe is refused before connect.
4. Re-replay the ids in trace_ids to confirm the fix.
