Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__cloudformation.deploy-appstream-fleet-stack-association-round-trip-and-delete appstream__cloudformation.deploy-appstream-imagebuilder-bad-image-rejected-and-valid-image-round-trip-and-delete
Issue-Key: appstream__cloudformation|DescribeImages|NotImplemented
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# appstream__cloudformation: DescribeImages is an unimplemented scaffold stub, so no AppStream image can be resolved and no fleet/image-builder stack can be validated

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Cause 1 of 1: the AppStream DescribeImages handler answers NotImplemented.

AWS: recorded behavior only — AppStream 2.0 DescribeImages answers 200 with the images visible to the account (the AWS-published base images plus any private ones), so a client can pick an ImageName for a fleet or image builder and tell a valid image from an invalid one. It is a normal read operation and never a server error.
Emulator: `An error occurred (NotImplemented) when calling the DescribeImages operation: operation DescribeImages is scaffolded but not implemented in this emulator`. Both failing checks stop there: the fleet/stack-association round-trip cannot resolve an image to deploy against, and the image-builder check cannot establish that a good image exists before probing a bad one. (Neither check is marked ALREADY FILED in the replay report, so no stub issue covers them.)
Why: a client cannot enumerate AppStream images at all, so it cannot choose an image, validate one, or deploy any fleet or image builder — the whole AppStream provisioning path depends on this read.
Fix: implement the DescribeImages handler in the appstream crate's ops module over the image catalog (AWS-published base images plus images created in-emulator), honouring the Names/Arns/Type filters and NextToken paging the operation models.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: DescribeImages 200 with an Images list.
Actual (both checks): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeImages operation: operation DescribeImages is scaffolded but not implemented in this emulator

## Steps to Reproduce
1. Call AppStream DescribeImages with no filters.
2. Observe NotImplemented instead of 200 with an Images list.
3. Re-replay the 2 checks in trace_ids.
