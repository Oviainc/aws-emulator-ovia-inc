Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda__s3__stepfunctions.cftenant-distribution-naming-a-deleted-origin-access-control-rejected
Issue-Key: cloudfront__dynamodb__lambda__s3__stepfunctions.cftenant-distribution-naming-a-deleted-origin-access-control-rejected|_refusal#11|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# cloudfront__dynamodb__lambda__s3__sfn: CreateDistribution accepts an S3 origin naming a DELETED origin access control instead of refusing it with InvalidOriginAccessControl (1 cause)

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: CreateDistribution does not validate Origin.OriginAccessControlId against live origin access controls, so a config whose S3 origin points at an OAC that has been deleted is created instead of refused. Open issues that look adjacent do NOT carry this check's replay key and describe other configs: 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md, 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md, 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md and 20261023T140640Z-cloudfront-lambda-createdistribution-accepts-a-lambda-edge-association-pointing-at.md (unpublished Lambda@Edge version). This cause is specifically OAC liveness.
AWS: Smithy (cloudfront 2020-05-31) models InvalidOriginAccessControl on CreateDistribution with HTTP 400, documented 'The origin access control is not valid.'; the recorded refusal for an origin naming an OAC that no longer exists is HTTP 400, code InvalidOriginAccessControl, message (recorded example) 'The specified origin access control does not exist or is not valid.' The refusal must also create nothing — the refused config's Comment must not appear in ListDistributions. The same run shows GetOriginAccessControl on the deleted id answering NoSuchOriginAccessControl, so the resource is genuinely gone before CreateDistribution is called, and the identical config naming a live OAC is accepted (positive control), so the rule being tested is liveness of the referenced OAC, not syntax.
Emulator: CreateDistribution SUCCEEDS (HTTP 201 with a Distribution) for the config whose single S3 origin sets OriginAccessControlId to the deleted id. No error of any kind is raised, and a distribution is created from a config AWS rejects.
Why: a client gets a distribution that cannot work — the origin references an access control that does not exist, so on real AWS the deploy fails at create time and the client fixes its template, while on the emulator it succeeds and the fault is invisible until edge traffic (which would be refused by the bucket policy) is served. IaC parity is lost: Terraform/CDK runs that should fail with InvalidOriginAccessControl converge green, and the negative-path error handling in client code is never exercised.
Fix: the CreateDistribution handler's DistributionConfig validation — resolve each Origin's OriginAccessControlId against the origin-access-control store and refuse an unknown/deleted id with InvalidOriginAccessControl (HTTP 400) before any distribution state is written (UpdateDistribution needs the same check).
Traces: cloudfront__dynamodb__lambda__s3__sfn.cftenant-distribution-naming-a-deleted-origin-access-control-rejected.

## Evidence
cloudfront__dynamodb__lambda__s3__sfn.cftenant-distribution-naming-a-deleted-origin-access-control-rejected:
  setup (passes): CreateOriginAccessControl twice; DeleteOriginAccessControl(dead, IfMatch=<etag>); GetOriginAccessControl(dead) -> NoSuchOriginAccessControl, 'The specified origin access control does not exist.'
  expected: cf.create_distribution(DistributionConfig=<one S3 origin on the tenant bucket's regional domain, OriginAccessControlId=<dead>, one default behavior, CachingOptimized, disabled>) is refused with Code 'InvalidOriginAccessControl' (HTTP 400), message 'The specified origin access control does not exist or is not valid.', and the refused Comment is absent from the account's distributions.
  actual: AssertionError: the call was expected to be refused, but it succeeded (check line 54 / _refusal line 186) — the create returned success.
Model: CreateDistribution errors include InvalidOriginAccessControl (client, httpError 400).

## Steps to Reproduce
1) Create an S3 bucket and two origin access controls (live, dead).
2) DeleteOriginAccessControl on 'dead' with its ETag; confirm GetOriginAccessControl(dead) -> NoSuchOriginAccessControl.
3) Call CreateDistribution with a DistributionConfig containing a single S3 origin whose DomainName is <bucket>.s3.<region>.amazonaws.com, OriginAccessControlId=<dead id>, one default cache behavior (managed CachingOptimized policy 658327ea-f89d-4fab-a63d-7e88639e58f6), Enabled=false, and a unique Comment.
Expected: HTTP 400 InvalidOriginAccessControl and no distribution carrying that Comment. Observed: the call succeeds and a distribution is created.
4) Control: the identical config with OriginAccessControlId=<live id> must be accepted and read back with that OAC on the origin.
