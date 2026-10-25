Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# Every ARN is built in the `aws` partition regardless of the Region the request was signed for

_Filed: 2026-10-24T05:40:24Z_
_Operation: CreateEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: an ARN names the partition of its Region. The model spells the admissible partitions out in every ARN pattern of this service — `^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):s3-outposts:...` for `EndpointArn`, and the same alternation for `OutpostArn` / `S3OutpostArn` — so a request signed for `us-gov-west-1` must yield `arn:aws-us-gov:s3-outposts:us-gov-west-1:...`, one signed for `cn-north-1` must yield `arn:aws-cn:...`, and an iso Region must yield `arn:aws-iso...`.

Actual: the ARN builders interpolate the Region read from the SigV4 credential scope into a literal `arn:aws:` prefix, so a GovCloud, China or ISO Region produces a cross-partition ARN that the real service never emits (`arn:aws:s3-outposts:us-gov-west-1:...`). This affects `CreateEndpointResult$EndpointArn` and, through the same helpers, `Endpoint$EndpointArn` in ListEndpoints/ListSharedEndpoints and `Outpost$OutpostArn` / `Outpost$S3OutpostArn` in ListOutpostsWithS3 — a client that parses the partition out of a returned ARN (to build a dependent ARN or an IAM policy) gets the wrong one.

## Evidence

Spec: smithy_query(query="shape", shape="Endpoint,Outpost") — EndpointArn `^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):s3-outposts:[a-z\-0-9]*:[0-9]{12}:outpost/...`, OutpostArn and S3OutpostArn carry the same partition alternation; well-established AWS ARN semantics tie the partition to the Region.
Handler/state: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:223-228 (`EndpointRecord::endpoint_arn` — `format!("arn:aws:s3-outposts:{region}:...")`), :164-178 (`outpost_arn` / `s3_outpost_arn`, same literal `arn:aws:`); the Region itself comes from `request_region` (src/state.rs:459-463) and is used verbatim, so Region and partition can disagree. CreateEndpoint returns this value at src/ops/create_endpoint.rs:268-275.
Tests: tests/all/endpoint_lifecycle.rs::every_arn_names_the_region_the_request_was_signed_for exercises only `aws`-partition Regions (us-east-1, eu-west-1, ap-southeast-2, me-central-1) and hardcodes `arn:aws:`, so no test covers a gov/cn/iso Region.

## Steps to Reproduce

1. Start the emulator with an Outpost provisioned (S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248")
2. Sign a CreateEndpoint request for Region us-gov-west-1 (SigV4 credential scope .../us-gov-west-1/s3-outposts/aws4_request) with OutpostId="op-0a1b2c3d4e5f60718", SubnetId="subnet-0a1b2c3d4e5f60718", SecurityGroupId="sg-0a1b2c3d4e5f60718"
3. Expected EndpointArn prefix: arn:aws-us-gov:s3-outposts:us-gov-west-1:<account>:outpost/...
4. Observed: arn:aws:s3-outposts:us-gov-west-1:<account>:outpost/...
5. Repeat ListOutpostsWithS3 in the same Region: OutpostArn/S3OutpostArn carry the same `arn:aws:` prefix instead of `arn:aws-us-gov:`
