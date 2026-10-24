Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.s3-role-trust-policy-validation
Issue-Key: apigateway__cloudfront__s3.s3-role-trust-policy-validation|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# apigateway__cloudfront__s3: a deployed REST API's AWS-service S3 integration never answers, so the role-trust-policy check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
ONE root cause, covering the one failing check of this service that is not already owned by another issue.

Cause 1 of 1: traffic sent at a deployed REST API's invoke URL never completes, so an AWS-service (S3 GetObject) proxy integration that assumes an execution role can never be observed.
AWS: recorded behavior only (no model surface for the API Gateway data plane). A role whose trust policy lists both s3.amazonaws.com and apigateway.amazonaws.com is recorded by an S3 replication configuration AND is concretely assumable by an API Gateway AWS-service integration: GET <invoke_url>/object answers 200 with the S3 object's bytes.
Emulator: the control-plane half of the check passes (the replication configuration and the IAM trust document read back correctly), then `_poll_until(lambda: _invoke(f"{invoke_url}/object", "GET"), lambda r: r[0] == 200 and r[1] == content, timeout_s=90)` never sees a 200 and the check is killed by the suite's 60 s cap.
Why: no API-Gateway-fronted trajectory can be validated end to end; a client cannot tell a broken execution-role trust policy from an endpoint that answers nothing, because both look like silence.
Fix: the API Gateway data plane — serving a deployed stage's invoke URL and executing an AWS-service integration under the integration's `credentials` role (the apigateway service's data-plane/integration component).
Traces: apigateway__cloudfront__s3.s3-role-trust-policy-validation.

The open issue 20261023T210711Z-apigateway-cloudfront-s3-every-cloudfront-s3-integration-check.md describes this same data-plane gap for other checks in this service (and the resolved 20261023T125454Z-apigateway-traffic-driven-at-a-deployed-rest-api-and.md claimed the family fixed), but neither carries this check's replay key: this check is marked neither ALREADY FILED nor OWNED in the replay report. It is filed here so the replay key is recorded; close it together with that open issue.

## Evidence
Replay output, verbatim tail:
- apigateway__cloudfront__s3.s3-role-trust-policy-validation: Error: Check timed out after 60.0s.
The check (tagged "concern:data-plane", "dp:endpoint") first asserts the control plane successfully — `replication["Role"] == role_arn`, `replication["Rules"][0]["Destination"]["Bucket"] == dest_arn`, and that the role's AssumeRolePolicyDocument lists both 's3.amazonaws.com' and 'apigateway.amazonaws.com' — then builds a REST API with an `arn:aws:apigateway:<region>:s3:path/<bucket>/<key>` integration and the role as `credentials`, deploys it, and polls the invoke URL for a 200 with the object body. Expected (AWS): status 200 and body == the object's content. Actual (emulator): the poll never observes a 200 and the harness kills the check at 60 s.

## Steps to Reproduce
1. Create source/destination buckets and a role trusting both s3.amazonaws.com and apigateway.amazonaws.com; put a replication configuration naming the role.
2. Create a REST API, add a resource and a GET method, PutIntegration type=AWS with uri arn:aws:apigateway:<region>:s3:path/<bucket>/<key> and credentials=<role arn>, deploy a stage.
3. GET <invoke_url>/object repeatedly — no 200 ever arrives.
Or replay apigateway__cloudfront__s3.s3-role-trust-policy-validation.
