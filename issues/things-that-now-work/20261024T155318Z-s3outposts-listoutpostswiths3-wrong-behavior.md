Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# ARNs are built for Outpost-id forms their own modeled pattern does not admit (12-digit and `ec2` branches)

_Filed: 2026-10-24T15:53:18Z_
_Operation: ListOutpostsWithS3 | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: every ARN the service reports satisfies the pattern the model declares for that member. The three ARN shapes deliberately admit DIFFERENT subsets of `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`:
- `Outpost$OutpostArn`: `...:outpost/(op-[a-f0-9]{17}|ec2)$` — no 12-digit branch
- `Outpost$S3OutpostArn`: `...:outpost/(op-[a-f0-9]{17}|\d{12})/s3$` — no `ec2` branch
- `Endpoint$EndpointArn` / `CreateEndpointResult$EndpointArn`: `...:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/[a-zA-Z0-9]{19}$` — no 12-digit branch
So the real service never emits `...:outposts:<region>:<acct>:outpost/111122223333` nor `...:s3-outposts:<region>:<acct>:outpost/ec2/s3`.

Actual: the ARN builders blindly interpolate whatever Outpost id the record carries, and the inventory accepts all three branches (`OutpostRecord::provisioned` -> `is_outpost_id`, which admits `ec2` and `\d{12}`). ListOutpostsWithS3 therefore reports, for an Outpost provisioned under the 12-digit branch, an `OutpostArn` that violates `OutpostArn`'s pattern, and for the `ec2` branch an `S3OutpostArn` that violates `S3OutpostArn`'s pattern. CreateEndpoint on a 12-digit-branch Outpost likewise returns an `EndpointArn` the pattern does not admit, and ListEndpoints repeats it. A client whose SDK validates response members against the model (or that parses the ARN to recover the Outpost id) sees a value the real service cannot produce. The service should either keep such Outposts out of the inventory or omit the ARN member it cannot form legally, not emit an out-of-pattern ARN.

## Evidence

Spec: smithy_query(query="shape", shape="Outpost,Endpoint") — OutpostArn `^arn:(aws|...):outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|ec2)$`, S3OutpostArn `^arn:(aws|...):s3-outposts:...:outpost/(op-[a-f0-9]{17}|\d{12})/s3$`, EndpointArn `...outpost/(op-[a-f0-9]{17}|ec2)/endpoint/[a-zA-Z0-9]{19}$`; OutpostId admits all three branches.
State/handler: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:162-178 (`outpost_arn` / `s3_outpost_arn` interpolate `self.outpost_id` unconditionally), :153-160 (`OutpostRecord::provisioned` accepts any `is_outpost_id`, i.e. `ec2` and `\d{12}` too, src/state.rs:491-499), :223-228 (`endpoint_arn`); reported by ListOutpostsWithS3 at src/ops/list_outposts_with_s3.rs:192 via `to_outpost`.
Tests: tests/all/unprovisioned_account.rs::only_what_was_provisioned_is_reported provisions exactly these two branches ("the ec2 branch" line 248, "the 12-digit branch" line 253) and then asserts BOTH ARNs in the generic `arn:aws:outposts:us-east-1:{owner}:outpost/{id}` / `.../outpost/{id}/s3` form (lines 336-347), so the out-of-pattern ARNs are enshrined rather than covered.

## Steps to Reproduce

1. Start the emulator with S3_OUTPOSTS_PROVISIONED_OUTPOSTS="000000000000:1099511627776,ec2:1099511627776"
2. Call ListOutpostsWithS3 (maxResults=100)
3. Expected: no reported OutpostArn/S3OutpostArn violates its modeled pattern
4. Observed: the 12-digit Outpost reports OutpostArn=arn:aws:outposts:us-east-1:<acct>:outpost/000000000000 (OutpostArn admits only op-<17 hex> or ec2), and the ec2 Outpost reports S3OutpostArn=arn:aws:s3-outposts:us-east-1:<acct>:outpost/ec2/s3 (S3OutpostArn admits only op-<17 hex> or 12 digits)
5. Call CreateEndpoint with OutpostId="000000000000", SubnetId="subnet-0a1b2c3d4e5f60718", SecurityGroupId="sg-0a1b2c3d4e5f60718"
6. Observed: EndpointArn=arn:aws:s3-outposts:us-east-1:<acct>:outpost/000000000000/endpoint/<id>, which EndpointArn's pattern does not admit; ListEndpoints reports the same value
