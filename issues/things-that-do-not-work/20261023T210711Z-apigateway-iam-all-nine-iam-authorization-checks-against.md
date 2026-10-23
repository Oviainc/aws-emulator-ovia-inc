Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam.managed-get-policy-attached-to-the-post-scoped-role-admits-it-until-detached apigateway__iam.xsvc-apigateway-3-iam-identity-allow-invokes-only-the-granted-method apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role apigateway__iam.xsvc-apigateway-3-iam-resource-policy-explicit-deny-beats-identity-allow-denied apigateway__iam.xsvc-apigateway-3-iam-resource-policy-not-ip-address-deny-blocked apigateway__iam.xsvc-apigateway-3-iam-resource-policy-source-ip-any-cidr-allow-invokes apigateway__iam.xsvc-apigateway-3-iam-role-without-invoke-permission-denied apigateway__iam.xsvc-apigateway-3-iam-unsigned-request-missing-authentication-token-rejected apigateway__iam.xsvc-apigateway-3-iam-wrong-secret-signature-mismatch-rejected
Issue-Key: apigateway__iam.managed-get-policy-attached-to-the-post-scoped-role-admits-it-until-detached|_await#7|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-identity-allow-invokes-only-the-granted-method|_await#5|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role|_await#6|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-explicit-deny-beats-identity-allow-denied|_await#7|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-not-ip-address-deny-blocked|_await#4|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-source-ip-any-cidr-allow-invokes|_await#4|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-role-without-invoke-permission-denied|_await#4|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-unsigned-request-missing-authentication-token-rejected|_await#4|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-wrong-secret-signature-mismatch-rejected|_await#4|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__iam: all nine IAM-authorization checks against a deployed stage hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a SigV4-signed (or deliberately unsigned/mis-signed) request to a deployed REST API stage with AWS_IAM authorization never returns.
AWS: a method with authorizationType=AWS_IAM answers at the stage invoke URL. An identity-policy allow invokes only the granted method; a resource policy allow rescues an otherwise-unauthorized role; an explicit resource-policy deny beats an identity allow; NotIpAddress / aws:SourceIp conditions block or admit by source IP; a role without execute-api:Invoke is denied; an unsigned request is refused with MissingAuthenticationToken; a wrong-secret signature is refused as a signature mismatch. Recorded behavior only for the per-case statuses and codes; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives for any of the nine. Each exhausts the harness budget and is reported as "Check timed out after 60.0s".
Why: the entire execute-api authorization model — identity policy, resource policy, explicit deny precedence, source-IP conditions, signature verification — is unobservable, and a client stalls instead of being allowed or denied.
Fix: the execute-api data-plane request path — SigV4 verification plus identity/resource-policy evaluation at the stage must always produce a response.
Traces: the nine check ids in trace_ids.

## Evidence
Detail for all nine checks is exactly "Error: Check timed out after 60.0s" — no HTTP status line and no body ever arrives from the deployed stage.

## Steps to Reproduce
1. Deploy a REST API with a method whose authorizationType is AWS_IAM, plus an IAM role and a REST API resource policy.
2. Send signed, unsigned and mis-signed requests to the stage invoke URL and observe that none of them produces a response within 60 s.
3. Or replay the check ids in trace_ids.
