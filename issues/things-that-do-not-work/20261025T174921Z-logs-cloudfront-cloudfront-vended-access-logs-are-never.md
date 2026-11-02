Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery
Issue-Key: cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery|_poll_until#32|Timeout|time.sleep(interval_s)
Issue-Commit: 3fc7e3e957d43d95045575b42258a9f9788f891e

# logs__cloudfront: CloudFront vended access logs are never delivered and DeleteDelivery's severing contract is unreachable, because the edge serves no request to log (1 cause)

> _Note: this issue was identified against the code base at commit `3fc7e3e957d43d95045575b42258a9f9788f891e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
One net-new root cause grounded in the logs__cloudfront check below. Cause count: 1 — it must be closed before this issue is submitted. Both services involved are bootstrapped (cloudwatchlogs, cloudfront).

RELATED OPEN ISSUES (none covers this check): 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md owns the sibling check logs__cloudfront.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected, but its six causes are all over-permissive ADMISSION on PutDeliverySource / PutSubscriptionFilter / PutTransformer — none of them is about a delivery producing no records or about DeleteDelivery's effect. 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md, cause 8 of 10, records the missing cloudfront edge data plane from five `cloudfront.`-prefixed checks; this check carries a different service token and a different replay key, and grades the vended-log join and the DeleteDelivery cut-off that cause does not. This check is marked neither ALREADY FILED nor OWNED in the replay report.

=== Cause 1 of 1: one delivery source feeding two log-group deliveries delivers nothing, so neither the per-request record contract nor DeleteDelivery's single-delivery severing can be observed ===
AWS: recorded behavior only for the join — the CloudWatch Logs model describes the control-plane envelopes (PutDeliverySource, PutDeliveryDestination, CreateDelivery with recordFields, GetDelivery, DeleteDelivery) and there is no model surface for the CloudFront edge. Recorded AWS behavior: (i) once the distribution is Deployed, a GET of https://<DomainName>/<key> for an object the attached OriginAccessControl can read is answered HTTP 200, and a GET for a key that is absent is answered HTTP 403; every such response carries an X-Amz-Cf-Id header minted per request and an X-Cache header whose value ends in " from cloudfront" and whose first word is 'Miss' for a CachingDisabled 200 and 'Error' for the OAC-denied 403. (ii) Each served request becomes one JSON access-log record in EVERY live delivery's destination log group, carrying exactly the delivery's configured recordFields and no others, with x-edge-request-id equal to that response's X-Amz-Cf-Id, cs-uri-stem equal to the requested path, sc-status equal to the status the edge answered, and x-edge-result-type equal to the first word of the X-Cache the viewer saw. All of these are values taken from the request and from the response the edge gave it; none is fixed text. (iii) DeleteDelivery(id) severs exactly one delivery: GetDelivery on that id is then ResourceNotFoundException with message "Requested Delivery does not exist in this account." (fixed text for this error class), a second DeleteDelivery on the same id is the same ResourceNotFoundException rather than an idempotent 200, the sibling delivery on the same source is untouched and keeps carrying later requests, records already delivered to the deleted delivery's group are not purged, and delivery to that group stops (eventually) and stays stopped.
Emulator: the check never reaches any of (ii) or (iii). It is killed by the suite's 60 s cap while still polling for the first edge response (the check's own budget for that poll is 240 s, and each log-record poll is 900 s, so the kill lands in the edge fetch). The control plane up to that point answers: the bucket and object, the OriginAccessControl, the distribution, TagResource, the bucket policy, both log groups, PutDeliverySource (asserted service == 'cloudfront' and resourceArns == [the distribution ARN]), both PutDeliveryDestination calls (asserted deliveryDestinationType == 'CWL') and both CreateDelivery calls all succeed. So one source, two destinations and two deliveries are all green and the pipeline carries nothing. The edge host is the emulator's own announced distribution domain, so this is the emulator's refusal to serve, not a runner egress or name-resolution fault.
Why: the entire causal claim of vended logging is unverifiable. A client gets HTTP 200 from all five control-plane calls, so it believes two independent log pipelines are live; no record ever arrives and no error is ever reported. Because nothing is ever delivered, DeleteDelivery's semantics are equally unobservable: a client cannot tell whether deleting one delivery severed only that one, whether a repeat delete is an error or an idempotent success, or whether already-delivered records survive. Request-level correlation (X-Amz-Cf-Id joined to x-edge-request-id) — the only way to tie a viewer response to its log record — does not exist here at all.
Fix: two components, both required for this check to pass. (a) A CloudFront data-plane listener for distribution domain names that resolves the cache behavior and origin for the request path, fetches through the attached OriginAccessControl, answers 200 for a readable object and 403 for one the OAC cannot read, and emits per-response X-Amz-Cf-Id and X-Cache ('Miss ... from cloudfront' / 'Error ... from cloudfront') headers. (b) The delivery leg in the CloudWatch Logs service: each live CreateDelivery whose source is a cloudfront ACCESS_LOGS delivery source must append one JSON event per served request to its destination log group's stream, restricted to exactly the delivery's configured recordFields and populated from that request/response (including x-edge-request-id == the X-Amz-Cf-Id returned to the viewer); and DeleteDelivery must stop only that delivery's emission, leave sibling deliveries on the same source emitting, leave already-written events in place, and make GetDelivery and a repeat DeleteDelivery on the deleted id answer ResourceNotFoundException with the message above.
Traces: logs__cloudfront.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery.

## Evidence
Replay output for logs__cloudfront.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery:

  Error: Check timed out after 60.0s

Where the 60 s cap lands, from the check body: all of the control-plane setup is single non-polling calls (sts GetCallerIdentity, s3 CreateBucket/PutObject, cloudfront CreateOriginAccessControl/CreateDistribution/TagResource, s3 PutBucketPolicy, logs CreateLogGroup x2, logs PutDeliverySource, logs PutDeliveryDestination x2, logs CreateDelivery x2), and the envelopes are asserted immediately:
  assert src["service"] == "cloudfront" and src["resourceArns"] == [dist_arn]
  assert dest["deliveryDestinationType"] == "CWL"
Neither is reported as the failure, so all five calls succeeded. The next steps are the polls:
  _wait_deployed(cf, dist_id)                                  # 1500 s budget
  rid, result = _request(base + path, status)                   # 240 s budget per request
  _poll_until(... _records(logs, group) ...)                    # 900 s budget per group
The harness killed the check at 60 s inside the first _request, whose predicate is
  lambda r: r[0] == expect_status and "x-amz-cf-id" in r[1]
against the distribution's own DomainName.

Assertions that consequently never ran (the contract this issue is about):
  assert len(phase1) == 3                       # three requests must mint three distinct X-Amz-Cf-Id values
  assert {v[2] for v in phase1.values()} == {"Miss", "Error"}
  assert x_cache.endswith(" from cloudfront")
  assert set(rec) == set(RECORD_FIELDS)         # ['date','time','cs-uri-stem','sc-status','x-edge-result-type','x-edge-request-id']
  assert rec["cs-uri-stem"] == path
  assert str(rec["sc-status"]) == str(status)   # 200 for the present key, 403 for the absent one
  assert rec["x-edge-result-type"] == result    # must equal the first word of the X-Cache the viewer saw
  GetDelivery(deleted id) == ("ResourceNotFoundException", "Requested Delivery does not exist in this account.")
  a second DeleteDelivery(same id) == the same ResourceNotFoundException, not an idempotent 200
  the sibling delivery on the same source still carries later requests
  records already delivered to the deleted delivery's group are not purged

Bucket, key, log-group names, distribution id/domain and the minted X-Amz-Cf-Id values are per-run; the requirement is the join between the response the edge gave and the record the delivery wrote.

## Steps to Reproduce
Delivery plane pinned to us-east-1 (CloudFront's vended-log control plane region).
1. s3 CreateBucket(<new bucket>); PutObject(Key=<present>.txt, Body=<bytes>). Choose a second key <absent>.txt that is never uploaded.
2. cloudfront CreateOriginAccessControl(OriginAccessControlOriginType='s3', SigningBehavior='always', SigningProtocol='sigv4').
3. cloudfront CreateDistribution: Enabled=True, one origin DomainName='<bucket>.s3.<region>.amazonaws.com' with S3OriginConfig.OriginAccessIdentity='' and OriginAccessControlId=<step 2>, DefaultCacheBehavior with ViewerProtocolPolicy='allow-all' and the managed CachingDisabled CachePolicyId. Note Id, ARN, DomainName.
4. s3 PutBucketPolicy allowing cloudfront.amazonaws.com s3:GetObject with Condition AWS:SourceArn == the distribution ARN.
5. logs CreateLogGroup twice in us-east-1: a 'primary' group and a 'barrier' group.
6. logs PutDeliverySource(name=<new>, resourceArn=<distribution ARN>, logType='ACCESS_LOGS') -> succeeds.
7. logs PutDeliveryDestination twice (outputFormat='json', destinationResourceArn='arn:aws:logs:us-east-1:<account>:log-group:<group>') -> both succeed as 'CWL'.
8. logs CreateDelivery twice on the SAME deliverySourceName, one per destination, recordFields=['date','time','cs-uri-stem','sc-status','x-edge-result-type','x-edge-request-id'] -> both succeed.
9. cloudfront GetDistribution until Status == 'Deployed'.
10. HTTP GET https://<DomainName>/<present>.txt twice and https://<DomainName>/<absent>.txt once.
    Observed: no response ever arrives with the expected status and an X-Amz-Cf-Id header; the poll never terminates and the check is killed at the 60 s harness cap.
    Expected: 200, 200 and 403, each with a distinct X-Amz-Cf-Id and an X-Cache ending in ' from cloudfront' whose first word is Miss, Miss, Error.
11. logs DescribeLogStreams + GetLogEvents over BOTH groups.
    Expected: one JSON record per request in each group, keys exactly the configured recordFields, x-edge-request-id == the X-Amz-Cf-Id from step 10, cs-uri-stem == the requested path, sc-status == the answered status, x-edge-result-type == the first word of X-Cache. Unreachable today.
12. logs DeleteDelivery(id=<the primary delivery>), then GetDelivery(same id) and DeleteDelivery(same id) again.
    Expected: both answer ResourceNotFoundException, message "Requested Delivery does not exist in this account."; GetDelivery on the sibling still returns it; later requests keep landing in the barrier group and stop landing in the primary group while the records from step 11 remain there. Unreachable today.
Or replay the check id in trace_ids.
