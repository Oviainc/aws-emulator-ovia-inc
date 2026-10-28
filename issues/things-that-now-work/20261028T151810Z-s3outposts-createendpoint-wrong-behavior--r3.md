Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# ARNs are built in partitions their own modeled pattern does not admit (aws-iso-f, aws-iso-e)

_Filed: 2026-10-28T15:18:10Z_
_Operation: CreateEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::arn_partition` maps the region prefixes `us-isof-` and `eu-isoe-` to the partition strings `aws-iso-f` and `aws-iso-e`. Neither string is in the alternation that `CreateEndpointResult$EndpointArn`, `Endpoint$EndpointArn`, `Outpost$OutpostArn` and `Outpost$S3OutpostArn` publish, so a request signed for such a Region gets back an ARN member whose value its own declared `@pattern` rejects.

This is the SAME rule the crate already implements for the other dimension of these patterns and skips here. For the `OutpostId` segment the crate gates the member on whether the pattern admits the value at all (`arn_admits_outpost` + `OUTPOST_ARN_BRANCHES` / `S3_OUTPOST_ARN_BRANCHES` / `ENDPOINT_ARN_BRANCHES`) and OMITS the optional member when it cannot form a legal value. For the partition segment the very same functions (`OutpostRecord::outpost_arn`, `OutpostRecord::s3_outpost_arn`, `EndpointRecord::endpoint_arn`) interpolate `arn_partition(..)` unconditionally, with no admission check, so the two halves of one pattern are treated by two different rules inside one function body.

This is NOT the already-fixed "every ARN is built in the `aws` partition regardless of the Region" gap: the Region->partition mapping is correct and must stay for the five partitions the patterns list. The defect is the missing admission gate for the two partition values the patterns do not list — for those the ARN member has no legal value at all and should be absent, exactly as it already is for an Outpost whose id branch the pattern rejects.

Symptom surface: `CreateEndpoint` answers with the out-of-pattern `EndpointArn`; `ListEndpoints` and `ListSharedEndpoints` repeat it; `ListOutpostsWithS3` reports out-of-pattern `OutpostArn` and `S3OutpostArn`.

## Evidence

Model (smithy_query query="shape" shape="com.amazonaws.s3outposts#Endpoint,com.amazonaws.s3outposts#Outpost") — the alternation lists FIVE partitions and no more:
- `Endpoint$EndpointArn` pattern: `^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):s3-outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/[a-zA-Z0-9]{19}$`
- `Outpost$OutpostArn` pattern: `^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|ec2)$`
- `Outpost$S3OutpostArn` pattern: `^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):s3-outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|\d{12})/s3$`
OPERATIONS.md:30 carries the same for the create result: "`EndpointArn` (string, pattern ^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):s3-outposts:...".

Code that emits a value outside that alternation — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:865-872:
```
    const PARTITION_PREFIXES: &[(&str, &str)] = &[
        ("cn-", "aws-cn"),
        ("us-gov-", "aws-us-gov"),
        ("us-isob-", "aws-iso-b"),
        ("us-isof-", "aws-iso-f"),
        ("eu-isoe-", "aws-iso-e"),
        ("us-iso-", "aws-iso"),
    ];
```
and it is interpolated with no admission gate — src/state.rs:360-369 (`EndpointRecord::endpoint_arn`):
```
        arn_admits_outpost(ENDPOINT_ARN_BRANCHES, &self.outpost_id).then(|| {
            format!(
                "arn:{}:s3-outposts:{home_region}:{}:outpost/{}/endpoint/{}",
                arn_partition(home_region),
```
(same shape at src/state.rs:283-291 `outpost_arn` and :299-307 `s3_outpost_arn`). The `then(..)` gate covers only the Outpost-id branch; `arn_partition(..)` is interpolated whatever it returns.

The crate's OWN stated rule, which this breaks — src/state.rs:73-82:
```
//! ## Why an ARN member can be ABSENT
//!
//! An id has to be one an ARN member's own pattern admits before that member
//! can name it. ... Each ARN member is optional, so the projection leaves
//! out the member it cannot form instead of reporting a string the member's own
//! pattern rejects; everything else about the resource is reported as usual.
```
and tests/all/arn_patterns.rs:3-11:
```
//! Every ARN this service reports is one the reporting member's own pattern admits.
//!
//! The rule under test: a response member whose declared pattern admits only SOME of the
//! forms an identifier can take carries a value that pattern admits, or carries nothing at
//! all — never a string the pattern rejects.
```
That same test file even narrows its own Region table to stay legal — tests/all/arn_patterns.rs:34-37: "A Region and the partition its ARNs name, limited to the partitions the three ARN patterns' own alternation lists (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`)".

Test coverage: tests/all/arn_partitions.rs:93-102 currently ASSERTS the out-of-pattern values:
```
    Row {
        case: "an isolated partition added after the ARN patterns were published",
        region: "us-isof-east-1",
        partition: "aws-iso-f",
    },
    Row {
        case: "the European isolated partition",
        region: "eu-isoe-west-1",
        partition: "aws-iso-e",
    },
```
so the divergence is enshrined, not covered; those two rows would move to the "member absent" half of the rule that tests/all/arn_patterns.rs already exercises for unadmitted id branches (`legal_or_absent`, arn_patterns.rs:196-198).

## Steps to Reproduce

1. Start the emulator with S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248:<emulator-account>:us-isof-east-1" so one S3-capable Outpost is installed in us-isof-east-1
2. Sign a CreateEndpoint call for Region us-isof-east-1: POST /S3Outposts/CreateEndpoint with {"OutpostId":"op-0a1b2c3d4e5f60718","SubnetId":"subnet-0a1b2c3d4e5f60718","SecurityGroupId":"sg-0a1b2c3d4e5f60718"}
3. Expected: either no EndpointArn member at all (the member cannot be formed legally, exactly as the crate already does for an Outpost whose id branch EndpointArn's pattern rejects), or a value the member's pattern admits
4. Observed: EndpointArn="arn:aws-iso-f:s3-outposts:us-isof-east-1:<account>:outpost/op-0a1b2c3d4e5f60718/endpoint/<id>" — the pattern's alternation is (aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b), so `aws-iso-f` is not admitted
5. GET /S3Outposts/ListEndpoints?maxResults=100 and GET /S3Outposts/ListSharedEndpoints?outpostId=op-0a1b2c3d4e5f60718&maxResults=100 in the same Region: both repeat the same out-of-pattern EndpointArn
6. GET /S3Outposts/ListOutpostsWithS3?maxResults=100 in the same Region: OutpostArn="arn:aws-iso-f:outposts:us-isof-east-1:<account>:outpost/op-0a1b2c3d4e5f60718" and S3OutpostArn="arn:aws-iso-f:s3-outposts:...:/s3", neither admitted by its own pattern
7. Repeat every step for Region eu-isoe-west-1, which yields the equally unadmitted partition `aws-iso-e`
