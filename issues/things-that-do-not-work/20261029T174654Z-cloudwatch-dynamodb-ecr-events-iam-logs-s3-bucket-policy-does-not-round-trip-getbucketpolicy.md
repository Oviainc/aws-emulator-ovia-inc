Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__ecr__events__iam__logs__s3__secretsmanager__sns__sqs__ssm.provider-refresh-discipline-23-resource-types
Issue-Key: cloudwatch__dynamodb__ecr__events__iam__logs__s3__secretsmanager__sns__sqs__ssm.provider-refresh-discipline-23-resource-types|run#53|AssertionError|assert wrong == [], (
Issue-Commit: b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7

# S3 bucket policy does not round-trip: GetBucketPolicy rewrites the Principal to a bare account id and expands single-string Action/Resource into arrays

> _Note: this issue was identified against the code base at commit `b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketPolicy

## Description
ONE net-new root cause. NOTE on attribution: this check's id carries a composite suite token rather than a single service token; the defect it exposes is in the S3 bucket-policy plane (the failing row is `s3_bucket_policy:Policy`), and the issue is filed under the check id's leading token as required.

Cause 1 of 1 — a policy document written by PutBucketPolicy is served back in a re-normalised form, so it does not equal what the caller set.
AWS: recorded behavior — S3 stores a bucket policy as the caller supplied it and GetBucketPolicy returns that document; a read of an untouched resource round-trips the attributes the caller set. In particular a Principal written as {"AWS": "arn:aws:iam::<account>:root"} comes back as that ARN, and members written as single JSON strings ("Action": "s3:GetObject", "Resource": "arn:aws:s3:::<bucket>/obj/*") come back as strings, not as one-element arrays. (The IAM-policy grammar permits both spellings, but the service does not rewrite the caller's spelling; nothing in the model authorises the rewrite — recorded behavior only.)
Emulator: GetBucketPolicy returns a document in which the Principal has been collapsed from "arn:aws:iam::<account>:root" to the bare account id "<account>", and the single-string Action and Resource members have been promoted to one-element lists. The Sid, Effect and Version survive.
Why: any client that compares the policy it set with the policy it reads — Terraform/CloudFormation drift detection being the canonical case, which is exactly what this check models — sees permanent spurious drift and re-applies the policy forever. Code that reads Action/Resource expecting the spelling it wrote (a string) breaks on a list, and a Principal rewritten to a bare account id loses the distinction between the account-root ARN and other principal spellings.
Fix: S3 bucket-policy state/serialization — store the supplied policy document verbatim and serve it back unmodified on GetBucketPolicy (normalisation, if any, belongs only in the evaluator, never in the read path).
Traces: cloudwatch__dynamodb__ecr__events__iam__logs__s3__secretsmanager__sns__sqs__ssm.provider-refresh-discipline-23-resource-types.

## Evidence
[mismatch] provider-refresh-discipline-23-resource-types, assertion at check line 942 ("attributes did not round-trip with the value the caller set"), one offending row of the 23 resource types:
s3_bucket_policy:Policy = {'Version': '2012-10-17', 'Statement': [{'Sid': 'sim2realE3', 'Effect': 'Allow', 'Principal': {'AWS': '000000000000'}, 'Action': ['s3:GetObject'], 'Resource': ['arn:aws:s3:::sim2real-xsvc-e3rd-<id>-b/obj/*']}]}
expected {'Version': '2012-10-17', 'Statement': [{'Sid': 'sim2realE3', 'Effect': 'Allow', 'Principal': {'AWS': 'arn:aws:iam::000000000000:root'}, 'Action': 's3:GetObject', 'Resource': 'arn:aws:s3:::sim2real-xsvc-e3rd-<id>-b/obj/*'}]}
Three divergences in one document: Principal.AWS ARN -> bare account id; Action string -> one-element list; Resource string -> one-element list. The account id and bucket name are per-run values; the requirement is byte-for-byte round-tripping of the supplied document. All other 22 resource types in the check round-tripped.

## Steps to Reproduce
1) s3 CreateBucket(<bucket>). 2) PutBucketPolicy with {"Version":"2012-10-17","Statement":[{"Sid":"sim2realE3","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"s3:GetObject","Resource":"arn:aws:s3:::<bucket>/obj/*"}]}. 3) GetBucketPolicy and json.loads the returned Policy: Principal.AWS is "<account>", and Action/Resource are one-element lists.
