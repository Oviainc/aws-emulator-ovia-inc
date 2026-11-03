Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__ecr__events__iam__logs__s3__secretsmanager__sns__sqs__ssm.provider-refresh-discipline-23-resource-types
Issue-Key: cloudwatch__dynamodb__ecr__events__iam__logs__s3__secretsmanager__sns__sqs__ssm.provider-refresh-discipline-23-resource-types|run#53|AssertionError|assert wrong == [], (
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# PutBucketPolicy does not store the policy document verbatim: GetBucketPolicy returns a rewritten Principal and array-wrapped Action/Resource

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy

## Description
Cause 1 of 1: the bucket policy a caller writes does not read back as written — the emulator re-serializes the document instead of persisting the bytes it was given.

AWS: PutBucketPolicy's `Policy` member is an opaque string bound with `@httpPayload` (Smithy: PutBucketPolicy.Policy target `Policy`, httpPayload, required; operation models no errors), and GetBucketPolicy's output `Policy` is the same opaque `@httpPayload` string. S3 stores the policy document as submitted and GetBucketPolicy hands that same document back, so a write-then-read round-trips the document's structure exactly: a `Principal` given as `{"AWS": "arn:aws:iam::<account-id>:root"}` reads back as that ARN string, and an `Action` or `Resource` given as a JSON scalar string reads back as a scalar string, not as a one-element array. S3 performs no principal canonicalisation and no scalar-to-list widening on the stored document. Recorded behavior only for the normalisation specifics (the model describes `Policy` as an opaque payload and therefore constrains nothing about its contents).

Emulator: the document is parsed and re-emitted in a normalised form. For a statement submitted as `{"Sid": "<sid>", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::<account-id>:root"}, "Action": "s3:GetObject", "Resource": "arn:aws:s3:::<bucket>/obj/*"}`, GetBucketPolicy returns `{"Sid": "<sid>", "Effect": "Allow", "Principal": {"AWS": "<account-id>"}, "Action": ["s3:GetObject"], "Resource": ["arn:aws:s3:::<bucket>/obj/*"]}` — the `arn:aws:iam::<account-id>:root` principal is collapsed to the bare 12-digit account id, and the scalar `Action` and `Resource` are wrapped in single-element arrays. Only the shape is rewritten; the grant itself is retained.

Why: every declarative tool that refreshes state by reading a resource back and diffing it against the configuration sees a permanent, unresolvable drift. A Terraform/CloudFormation/CDK refresh of `aws_s3_bucket_policy` compares the configured document against the read document, finds the Principal and the Action/Resource cardinality changed, and reports the resource as modified outside the configuration on every single plan — then re-applies the identical document, which is rewritten again. Callers that assert on their own policy document after writing it (policy linters, drift detectors, compliance scanners keyed on the exact Principal ARN form) read a document S3 would never have returned.

Fix: the bucket-policy state path — PutBucketPolicy must persist the request payload bytes unchanged and GetBucketPolicy must serve those same bytes, rather than round-tripping the document through a normalising policy parser/serializer. The principal canonicalisation and the scalar-to-array widening both belong to that normalisation step and must not be applied to the stored document.

Related (does NOT cover this): 20261102T111455Z-s3-bucket-policy-plane-accepts-malformed-policies-and.md covers malformed-policy validation, the GetBucketPolicy content-type, BlockPublicPolicy classification and GetBucketPolicyStatus absence. It says nothing about the stored document being rewritten, so this verbatim-round-trip defect is net-new.

Note on attribution: the suite lists this check under the composite heading `cloudwatch__dynamodb__ecr__events__iam__logs__s3__secretsmanager__sns__sqs__ssm`, which is used verbatim as this issue's service token. The failing assertion is entirely within the S3 bucket-policy plane (`s3_bucket_policy:Policy`); the other services in the heading round-tripped and are not implicated.

Traces: cloudwatch__dynamodb__ecr__events__iam__logs__s3__secretsmanager__sns__sqs__ssm.provider-refresh-discipline-23-resource-types.

## Evidence
Replay of simparity-behavioral-aws-v1, check `provider-refresh-discipline-23-resource-types` (verdict: mismatch).

Assertion raised at check line 919, `assert wrong == []`:

  AssertionError: attributes did not round-trip with the value the caller set: ["s3_bucket_policy:Policy = {'Version': '2012-10-17', 'Statement': [{'Sid': 'sim2realE3', 'Effect': 'Allow', 'Principal': {'AWS': '000000000000'}, 'Action': ['s3:GetObject'], 'Resource': ['arn:aws:s3:::sim2real-xsvc-e3rd-e3d569070331-b/obj/*']}]}, expected {'Version': '2012-10-17', 'Statement': [{'Sid': 'sim2realE3', 'Effect': 'Allow', 'Principal': {'AWS': 'arn:aws:iam::000000000000:root'}, 'Action': 's3:GetObject', 'Resource': 'arn:aws:s3:::sim2real-xsvc-e3rd-e3d569070331-b/obj/*'}]}"]

Three independent divergences in one document, all of them shape rewrites of what the caller submitted:
  - Principal.AWS: expected the ARN form the caller supplied (`arn:aws:iam::<account-id>:root`), got the bare account id.
  - Action: expected the scalar string the caller supplied, got a one-element array.
  - Resource: expected the scalar string the caller supplied, got a one-element array.

The bucket name, the account id and the `Sid` in the message above are values from this run's request (the account id is the emulator's own) and are not part of the requirement; the requirement is that whatever document is submitted is the document returned. `s3_bucket_policy:Policy` is the only one of the 23 resource types in this check that failed to round-trip — the defect is specific to the bucket-policy document, not to the check's round-trip harness.

## Steps to Reproduce
1. Create a bucket.
2. Call PutBucketPolicy with a document whose statement uses the root-ARN principal form and scalar Action/Resource, e.g.:
   {"Version":"2012-10-17","Statement":[{"Sid":"roundtrip","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account-id>:root"},"Action":"s3:GetObject","Resource":"arn:aws:s3:::<bucket>/obj/*"}]}
3. Call GetBucketPolicy on the same bucket and JSON-parse the returned `Policy`.
4. Observe Principal.AWS has become "<account-id>" and Action/Resource have each become one-element arrays. AWS returns the document byte-for-byte as submitted.
5. Or replay the behavioral check `provider-refresh-discipline-23-resource-types` and read the `s3_bucket_policy:Policy` entry of the `wrong` list.
