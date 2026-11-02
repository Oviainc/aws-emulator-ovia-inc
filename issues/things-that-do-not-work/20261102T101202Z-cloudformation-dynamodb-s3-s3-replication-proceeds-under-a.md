Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.cfn-deploy-iam-role-trust-mismatched-principal-negative
Issue-Key: cloudformation__dynamodb__s3.cfn-deploy-iam-role-trust-mismatched-principal-negative|run#22|AssertionError|assert code == "404", (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/dynamodb/s3: S3 replication proceeds under a role whose trust policy names the wrong principal

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Cause 1 of 1: a replication role whose trust policy does not trust s3.amazonaws.com is still able to replicate.
AWS: recorded behavior only — when the CloudFormation-deployed replication role's AssumeRolePolicyDocument names a mismatched principal, S3 cannot assume it, so the object written to the source bucket is never copied and HeadObject on the destination answers 404 (NoSuchKey / Not Found).
Emulator: HeadObject on the destination succeeds — the object was replicated even though the role could not have been assumed.
Why: the negative control for a trust-policy misconfiguration passes silently; a client testing that its IAM wiring is required sees it is not, and the emulator cannot be used to validate least-privilege replication setups.
Fix: the replication path must assume the configured role (trust-policy evaluation) before copying — i.e. the S3 replication executor's authorization, not the bucket state.
Traces: cloudformation__dynamodb__s3.cfn-deploy-iam-role-trust-mismatched-principal-negative.

## Evidence
AssertionError: "the object written under the mismatched-trust role must never replicate: HeadObject on the destination returned '<call succeeded>', expected 404".

## Steps to Reproduce
Replay cloudformation__dynamodb__s3.cfn-deploy-iam-role-trust-mismatched-principal-negative: deploy a stack with a source bucket, a destination bucket and a replication configuration whose IAM role's trust policy names a principal other than s3.amazonaws.com; PutObject into the source, then HeadObject the same key in the destination.
