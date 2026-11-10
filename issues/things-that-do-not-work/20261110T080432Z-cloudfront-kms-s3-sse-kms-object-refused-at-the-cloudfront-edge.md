Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied
Issue-Key: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied|run#38|AssertionError|assert _s3_error_message(denied) == expected_message, (
Issue-Commit: 7f92290cd08a3e8e509afee1d943cf5282438c9c

# SSE-KMS object refused at the CloudFront edge returns KMS's authorization message instead of S3's "Access Denied"

> _Note: this issue was identified against the code base at commit `7f92290cd08a3e8e509afee1d943cf5282438c9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One net-new root cause.

Cause 1 of 1: an S3 GetObject that fails because S3 cannot kms:Decrypt the object's SSE-KMS key serves KMS's internal authorization sentence as the S3 error Message, instead of S3's own refusal message.

AWS: a viewer request through an OAC-protected distribution reaches S3; when the object is SSE-KMS and the key policy does not admit cloudfront.amazonaws.com (or admits it only for another distribution's aws:SourceArn), S3 answers HTTP 403 with an <Error> body whose <Code> is AccessDenied and whose <Message> is exactly the fixed text "Access Denied" — S3 never surfaces the KMS principal, the key ARN or the denied KMS action to the viewer. Smithy: s3#GetObject models only InvalidObjectState (403) and NoSuchKey (404), so the AccessDenied body is recorded behavior only; the recording shows status 403, code AccessDenied, message "Access Denied" (fixed text, no variable parts).
Emulator: the status (403) and the code (AccessDenied) are right, but the Message is KMS's authorization sentence passed through verbatim: "User: cloudfront.amazonaws.com is not authorized to perform: kms:Decrypt on resource: <key ARN> because no resource-based policy allows the kms:Decrypt action" (the principal, the KMS action and the key ARN in that text are values from the emulator's own state; none of them belong in an S3 error body).
Why: a client or SDK that matches S3's refusal on its message (log scrapers, CloudFront error-page rules, tests asserting S3's contract) sees a message AWS never returns, and an anonymous viewer at the edge is told which KMS key the private origin uses and which KMS action was denied — information AWS deliberately does not disclose in the S3 403.
Fix: the S3 GetObject handler's SSE-KMS decrypt path — where a KMS authorization failure is mapped into the S3 error, it must be translated to S3's AccessDenied/"Access Denied" rather than relaying the KMS error's message; the S3 error serializer for the edge-served response is the component that must own the wording.
Related open issue (not a duplicate): 20261109T164957Z-cloudfront-kms-s3-a-distribution-s-domainname-is.md reports the edge being unreachable; the edge now serves (the SSE-S3 barrier object returns 200 and the SSE-KMS object returns 403), and this message divergence is the next, distinct failure on the same check, which that issue's replay key does not carry.
Traces: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied.

## Evidence
From the behavioral replay of cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied [mismatch]:

AssertionError: expected S3's refusal message 'Access Denied' passed through the edge, got 'User: cloudfront.amazonaws.com is not authorized to perform: kms:Decrypt on resource: arn:aws:kms:us-west-2:000000000000:key/01b10778-8ba5-425d-85f5-b27484a0e392 because no resource-based policy allows the kms:Decrypt action'

- Expected: HTTP 403, <Code>AccessDenied</code>, <Message>Access Denied</Message>.
- Actual: HTTP 403, Code AccessDenied (correct), Message = the KMS authorization sentence above. The key ARN and the region/account in it are per-run values; the defect is that a KMS-authored message appears at all where AWS returns the fixed text "Access Denied".
- The preceding assertions in the same beat passed: the status is 403 and the code is AccessDenied, and the sibling SSE-S3 object is still served 200 in the same instant.

## Steps to Reproduce
Replay cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied. In outline: create a bucket with one SSE-S3 object and one SSE-KMS object under a customer key whose key policy does NOT name cloudfront.amazonaws.com; create an OAC and a distribution with that bucket as origin plus a bucket policy allowing the cloudfront.amazonaws.com service principal for this distribution's ARN; once the SSE-S3 object is served 200 at the edge, GET the SSE-KMS object at the edge and read the 403 body's Code and Message.
