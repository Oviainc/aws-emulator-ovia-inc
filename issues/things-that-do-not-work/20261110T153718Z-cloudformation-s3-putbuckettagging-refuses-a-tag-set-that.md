Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual
Issue-Key: cloudformation__s3|PutBucketTagging|InvalidTag
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# cloudformation__s3: PutBucketTagging refuses a tag set that merely re-sends CloudFormation's unchanged aws: system tags with InvalidTag "System tags cannot be added/updated by requester"

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketTagging

## Description
One net-new root cause in the cloudformation__s3 suite. It is not covered by any open issue: the open cloudformation__s3 issues (20261109T220008Z-cloudformation-s3-7-net-new-causes-s3express-types.md, 20261029T211510Z-cloudformation-s3-createchangeset-template-source-validation-7-divergent-refusals-for.md, 20261107T060626Z-cloudformation-s3-cloudformation-nested-stacks-no-nested-change-set-tree.md) describe change-set template-source validation, S3Express types and nested change sets — a different surface.

Cause 1 of 1: an S3 PutBucketTagging that re-sends the aws:cloudformation:* system tags unchanged, alongside one changed user tag, is refused InvalidTag instead of being accepted.
AWS: Smithy: PutBucketTagging — PUT /{Bucket}?tagging, input member Tagging (@required, httpPayload); the operation models NO errors, so the refusal rule is documented rather than modelled. S3's rule is that a requester may not ADD or UPDATE a tag in the reserved aws: namespace; it refuses only that, with HTTP 400 and error code InvalidTag whose fixed message is "System tags cannot be added/updated by requester". Because PutBucketTagging replaces the whole tag set, the documented and recorded way to change one user tag on a CloudFormation-managed bucket is GetBucketTagging, edit the one user tag, and PUT the whole set back — including the aws:cloudformation:stack-name / stack-id / logical-id tags CloudFormation itself set, at their existing values. That call ADDS and UPDATES nothing in the aws: namespace, so AWS answers 200 and the new tag set reads back from GetBucketTagging.
Emulator: the same call is refused HTTP 400 InvalidTag with the message "System tags cannot be added/updated by requester" — the validator rejects the mere PRESENCE of any aws:-prefixed key in the submitted TagSet rather than comparing it against the tag set already stored. The check never reaches its drift assertions.
Why: a client cannot change any user tag on a bucket CloudFormation created, because every correct read-modify-write of the tag set necessarily carries CloudFormation's own system tags back. Every out-of-band tag edit, every drift-detection workflow and every tagging-governance tool against a CFN-managed bucket is blocked by a refusal AWS does not issue, and the refusal arrives on a request AWS treats as a no-op in the reserved namespace.
Fix: the PutBucketTagging handler's tag validation — the aws: namespace check must compare the submitted aws:-prefixed entries against the bucket's currently stored tag set and refuse only entries that are new or whose value differs; identical re-sends must pass through. (The same rule applies to PutBucketTagging's sibling validators that share the system-tag check.)
Traces: cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual.

## Evidence
Check cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual, verdict [error].

The check creates a stack with one AWS::S3::Bucket carrying Tags [{Key: sim2real-env, Value: template}], waits CREATE_COMPLETE, confirms drift detects IN_SYNC, then does exactly:
  current = s3.get_bucket_tagging(Bucket=bucket)["TagSet"]
  new_set = [t if t["Key"] != "sim2real-env" else {"Key": "sim2real-env", "Value": "drifted"} for t in current]
  s3.put_bucket_tagging(Bucket=bucket, Tagging={"TagSet": new_set})

Expected (AWS): HTTP 200; GetBucketTagging then reports sim2real-env=drifted with the aws:cloudformation:* tags unchanged, and DetectStackDrift reports DRIFTED with a /Tags/0/Value PropertyDifference of DifferenceType NOT_EQUAL, ExpectedValue "template", ActualValue "drifted".

Actual (emulator):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (InvalidTag) when calling the PutBucketTagging operation: System tags cannot be added/updated by requester

Message anatomy: "System tags cannot be added/updated by requester" is AWS's fixed text for InvalidTag in the reserved-namespace class — it is correct text raised on the wrong request. There are no variable parts; the defect is the predicate that selects it, not the wording. The bucket name, stack name and tag values above are per-run values, not part of the requirement.

## Steps to Reproduce
1. CreateStack with a template holding one AWS::S3::Bucket whose Properties.Tags is [{"Key":"sim2real-env","Value":"template"}]; wait for CREATE_COMPLETE and read the bucket's PhysicalResourceId.
2. Call GetBucketTagging on that bucket and observe the aws:cloudformation:stack-name / stack-id / logical-id tags alongside sim2real-env.
3. Call PutBucketTagging with that exact TagSet, changing only sim2real-env's value to "drifted".
4. AWS answers 200; the emulator answers 400 InvalidTag "System tags cannot be added/updated by requester".
5. Re-replay the behavioral check cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual to confirm the fix (it then proceeds to the DetectStackDrift / DescribeStackResourceDrifts assertions).
