Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__s3.cfn-changeset-template-url-caller-lacks-s3-getobject-rejected
Issue-Key: cloudformation__iam__s3.cfn-changeset-template-url-caller-lacks-s3-getobject-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# cloudformation__iam__s3: CreateChangeSet with a TemplateURL never authorizes the template fetch, so a caller with no s3:GetObject on the template object is accepted

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1: CreateChangeSet does not authorize the S3 fetch of the TemplateURL object.
AWS: when CreateChangeSet is given a TemplateURL, CloudFormation reads that object as the requesting principal (or the stack's service role) and refuses the request when that principal is not granted s3:GetObject on it. The call is refused, not accepted; no change set is created.
Emulator: CreateChangeSet answers HTTP 200 and returns a change-set Id and StackId for a caller holding no s3:GetObject on the template object. Recorded shape: {'Id': 'arn:aws:cloudformation:<region>:<account>:changeSet/<name>/<uuid>', 'StackId': 'arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>'} with HTTPStatusCode 200.
Why: a client relying on CloudFormation to surface the missing template-read permission at change-set creation time gets a change set it believes is valid; the authorization defect is invisible and the negative control silently passes. It also means template bodies can be read through CloudFormation by principals S3 would refuse.
Fix: CreateChangeSet's template-fetch path — it must resolve TemplateURL through an authorized S3 read of the fetching principal and refuse on denial (as the sibling CreateStack path must; see the companion issue filed for cloudformation__iam__lambda__s3 cause 3, and the related open issue 20261029T180026Z-cloudformation-dynamodb-s3-cloudformation-s3-template-fetch-authorization-deletionpolicy-retain-on.md, which does not carry this replay key).
Traces: cloudformation__iam__s3.cfn-changeset-template-url-caller-lacks-s3-getobject-rejected

## Evidence
Expected: the CreateChangeSet call is refused (a ClientError). Actual: `AssertionError: expected the call to be refused, but it SUCCEEDED and returned {'Id': 'arn:aws:cloudformation:us-west-2:000000000000:changeSet/cs-b560eb9b8c9c/<uuid>', 'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-stk-<id>/<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. The ids and uuids above are per-run values; the requirement is the refusal, not these values.

## Steps to Reproduce
Upload a template object to S3. Create an IAM principal whose identity policy grants cloudformation:CreateChangeSet but NOT s3:GetObject on that object. Signed as that principal, call cloudformation:CreateChangeSet with StackName and TemplateURL pointing at the object. AWS refuses; the emulator returns 200 with a change-set Id.
