Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group
Issue-Key: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group|_poll_until#38|Timeout|time.sleep(interval_s)
Issue-Commit: 3fc7e3e957d43d95045575b42258a9f9788f891e

# cloudfront__logs: CloudFront standard logging v2 delivers no access-log record, because the distribution's edge domain never answers a request to log (1 cause)

> _Note: this issue was identified against the code base at commit `3fc7e3e957d43d95045575b42258a9f9788f891e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
One net-new root cause grounded in the cloudfront__logs check below. Cause count: 1 — it must be closed before this issue is submitted. Both services involved are bootstrapped (cloudfront, cloudwatchlogs).

RELATED OPEN ISSUES (none covers this check): 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md covers the over-permissive ADMISSION of PutDeliverySource (it validates neither the resource, nor the logType, nor the region) — a different concern from the delivery producing no records. 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md, cause 8 of 10, records that cloudfront distributions serve nothing at their edge domain, grounded in five `cloudfront.`-prefixed checks; this check carries a different service token and a different replay key, and adds the standard-logging-v2 join that cause does not grade. 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md covers the cloudfront__s3 token. This check is marked neither ALREADY FILED nor OWNED in the replay report.

=== Cause 1 of 1: a CloudFront ACCESS_LOGS delivery source wired to a CloudWatch Logs log group never produces a log event, because no request is ever served at the edge to produce one ===
AWS: recorded behavior only — there is no model surface for the CloudFront edge data plane, and the CloudWatch Logs Smithy model describes only the control-plane envelopes (PutDeliverySource / PutDeliveryDestination / CreateDelivery, with CreateDelivery reporting deliveryDestinationType 'CWL' for a log-group destination). Recorded AWS behavior for the join these three calls establish: once GetDistribution reports Status 'Deployed', a GET of https://<DomainName>/<key> is answered from the S3 origin through the attached OriginAccessControl with HTTP 200 and the object's bytes; and within the delivery window an access-log event for that request appears in the destination log group, readable through logs FilterLogEvents, as a JSON record carrying cs-uri-stem == '/<key>', sc-status == 200 and x-host-header == the distribution's DomainName. The record's field VALUES all come from the request and from the distribution that served it; none of them is fixed text.
Emulator: the check never reaches any of those assertions. It is killed by the suite's 60 s cap while still polling for the FIRST HTTP 200 from the distribution's own DomainName (the check's own budget for that poll is 240 s, and its later log-record poll is 900 s, so the 60 s kill lands in the edge fetch). The control plane up to that point answers: the log group, the bucket and object, the OAC, the distribution, the log-group resource policy, PutDeliverySource (asserted service == 'cloudfront', logType == 'ACCESS_LOGS', resourceArns == [the distribution ARN]), PutDeliveryDestination and CreateDelivery (asserted deliveryDestinationType == 'CWL') all succeed — so all three vended-log envelopes are green and the pipeline still carries nothing. Note the edge host is the emulator's own announced distribution domain, not a third-party address, so this is the emulator's refusal to serve, not a runner egress or name-resolution fault.
Why: standard logging v2 is unverifiable end to end. A client can build the full source -> destination -> delivery triple, get HTTP 200 on every call, and then wait forever for records; nothing in the emulator ever tells it that neither the edge nor the delivery produced anything. Every trajectory that reads CloudFront access logs back out of CloudWatch Logs — log-driven alarms, 4xx/5xx triage, request correlation on cs-uri-stem or x-host-header — is unreachable, and the delivery's own recordFields contract cannot be exercised.
Fix: two components, both required for this check to pass. (a) A CloudFront data-plane listener for distribution domain names that resolves the matching cache behavior and origin for the request path, fetches from the S3 origin using the attached OriginAccessControl, and answers the viewer — the same missing component named by the related cloudfront-only issue. (b) The delivery leg in the CloudWatch Logs service: a CreateDelivery whose source is a cloudfront ACCESS_LOGS delivery source must, for each request the edge serves, append a JSON access-log event to the destination log group's stream in the delivery's configured outputFormat, with the record fields populated from the served request.
Traces: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group.

## Evidence
Replay output for cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group:

  Error: Check timed out after 60.0s

Where the 60 s cap lands, from the check body: every control-plane call before the edge fetch is a single non-polling request (logs CreateLogGroup / DescribeLogGroups, s3 CreateBucket / PutObject, cloudfront CreateOriginAccessControl / CreateDistribution / TagResource, s3 PutBucketPolicy, logs PutResourcePolicy / PutDeliverySource / PutDeliveryDestination / CreateDelivery), and each of the three vended-log envelopes is asserted immediately after it returns:
  assert src["service"] == "cloudfront" and src["logType"] == "ACCESS_LOGS"
  assert src["resourceArns"] == [dist_arn]
  assert delivery["deliveryDestinationType"] == "CWL"
None of those assertions is reported as the failure, so all three succeeded. The next two steps are the polls:
  _wait_distribution_deployed(cf, dist_id)                      # 1200 s budget
  served = _fetch_ok(session, f"{ctx.endpoint_url(dist['DomainName'])}/{key}")   # 240 s budget
and only after those:
  found = _poll_until(records, lambda r: len(r) >= 1, timeout_s=900.0, ...)      # 900 s budget
The check's own exhaustion path for the edge fetch raises "DATAPLANE_UNIMPLEMENTED: timed out waiting for 200 from <the distribution's own DomainName>" — it never got that far because the harness killed the check first.

Assertions that consequently never ran (the contract this issue is about):
  served.content == body
  rec["cs-uri-stem"] == "/<key>"
  str(rec["sc-status"]) == "200"
  rec["x-host-header"] == dist["DomainName"]

The log group, bucket, key and distribution id are all per-run values; the requirement is that the record names the request the edge served.

## Steps to Reproduce
Delivery plane pinned to us-east-1 (CloudFront's vended-log control plane region).
1. logs CreateLogGroup(logGroupName=<new group>) in us-east-1; read its ARN with DescribeLogGroups and strip a trailing ':*'.
2. s3 CreateBucket(<new bucket>) and PutObject(Bucket, Key=<probe>.txt, Body=<bytes>, ContentType='text/plain').
3. cloudfront CreateOriginAccessControl(OriginAccessControlOriginType='s3', SigningBehavior='always', SigningProtocol='sigv4').
4. cloudfront CreateDistribution: Enabled=True, one origin with DomainName='<bucket>.s3.<region>.amazonaws.com', S3OriginConfig.OriginAccessIdentity='' and OriginAccessControlId=<step 3>, DefaultCacheBehavior targeting it with ViewerProtocolPolicy='allow-all' and the managed CachingDisabled CachePolicyId. Note Id, ARN and DomainName.
5. s3 PutBucketPolicy allowing cloudfront.amazonaws.com s3:GetObject on the bucket with Condition AWS:SourceArn == the distribution ARN.
6. logs PutResourcePolicy(resourceArn=<step 1 bare ARN>) allowing delivery.logs.amazonaws.com logs:CreateLogStream and logs:PutLogEvents.
7. logs PutDeliverySource(name=<new>, resourceArn=<distribution ARN>, logType='ACCESS_LOGS') -> succeeds.
8. logs PutDeliveryDestination(name=<new>, outputFormat='json', deliveryDestinationConfiguration={'destinationResourceArn': <step 1 bare ARN>}) -> succeeds, deliveryDestinationType 'CWL'.
9. logs CreateDelivery(deliverySourceName=<step 7>, deliveryDestinationArn=<step 8 arn>) -> succeeds.
10. cloudfront GetDistribution until Status == 'Deployed'.
11. HTTP GET https://<DomainName>/<probe key>.
    Observed: no HTTP 200 ever arrives; the poll never terminates and the check is killed at the 60 s harness cap.
    Expected: HTTP 200 carrying the object bytes.
12. logs FilterLogEvents(logGroupName=<step 1 group>, filterPattern='"<probe key>"').
    Expected: at least one JSON event with cs-uri-stem == '/<probe key>', sc-status == 200 and x-host-header == <DomainName>. Unreachable today.
Or replay the check id in trace_ids.
