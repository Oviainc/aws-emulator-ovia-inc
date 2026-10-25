Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group
Issue-Key: cloudfront__logs|PutResourcePolicy|InvalidParameterException
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cloudfront__logs: logs PutResourcePolicy refuses a resourceArn-scoped policy with "policyName is required", so CloudFront standard logging v2 cannot be wired to a log group (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: PutResourcePolicy demands policyName even when the policy is scoped by resourceArn ===
AWS: logs PutResourcePolicy takes policyName, policyDocument, resourceArn and expectedRevisionId; neither policyName nor resourceArn is unconditionally required. The resource-scoped form — a policyDocument plus the resourceArn of the log group it attaches to, with no policyName — is accepted, and it is the form CloudFront standard logging v2 delivery needs so the delivery service may write into the target log group.
Emulator: that request is refused with HTTP 400 InvalidParameterException and message 'policyName is required'. Because the grant can never be installed, the CloudFront access-log record never reaches the log group and the check cannot proceed.
Why: resource-scoped CWL resource policies are unusable, which blocks every cross-service log delivery that depends on one (CloudFront standard logging v2 here; the same refusal is the recorded cause for the EventBridge bus-logging checks). A client following the AWS delivery-setup procedure is stopped at the first step by an error AWS does not raise. Open issue 20261023T113357Z-logs-cloudwatch-logs-accepts-requests-real-aws-refuses-putresourcepolicy.md names this PutResourcePolicy behaviour and carries other checks' replay keys; it does not carry this one.
Fix: PutResourcePolicy's input validation in the logs handler — policyName must be optional when resourceArn is supplied.
Traces: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group.

## Evidence
cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group: botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the PutResourcePolicy operation: policyName is required. Expected HTTP 200 with the resourceArn-scoped resource policy stored, followed by a CloudFront access-log record appearing in the target log group.

## Steps to Reproduce
logs PutResourcePolicy(policyDocument=<document allowing delivery.logs.amazonaws.com to PutLogEvents>, resourceArn='arn:aws:logs:<region>:<account>:log-group:<group>:*') with NO policyName — 400 InvalidParameterException 'policyName is required'. Expected 200.
