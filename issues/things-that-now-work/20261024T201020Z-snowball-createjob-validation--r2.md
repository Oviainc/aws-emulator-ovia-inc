Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# CreateJob never validates the AWS resources an order names (S3 buckets in Resources, RoleARN, KmsKeyARN) — only that a bucket entry is present

_Filed: 2026-10-24T20:10:20Z_
_Operation: CreateJob | Class: validation_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a CreateJob request for an IMPORT or EXPORT order is validated against the resources it names, not just against their presence. The real service resolves the S3 bucket(s) in `Resources.S3Resources[].BucketArn`, the `RoleARN` and the `KmsKeyARN` at create time, and refuses an order that names something the account does not hold — `InvalidResourceException` (HTTP 400, "The specified resource can't be found. Check the information you provided in your last request, and try again.") for a bucket/role that cannot be resolved, and `KMSRequestFailedException` / `Ec2RequestFailedException` for a key or AMI the service cannot use. That resolution is why those three errors are declared on this operation at all, and why the operation's own documentation opens with "Your Amazon Web Services account must have the right trust policies and permissions in place to create a job for a Snow device."

Actual: the handler resolves ONLY ids that live in its own store — `LongTermPricingId`, `ClusterId`, `AddressId`, `ForwardingAddressId` — and for a transfer order checks nothing more than that `Resources.S3Resources` is a non-empty list and `RoleARN` is a non-empty string (`crate::state::names_s3_bucket`, which returns true for any entry whatsoever). A request naming `arn:aws:s3:::bucket-that-does-not-exist`, a role ARN for a role nobody created, an `Ec2AmiResources` entry for a non-existent AMI, or a KMS key ARN that resolves to no key is accepted with HTTP 200 and a minted `JobId`, and `DescribeJob` then reports the order as a valid `New` job pointing at resources that do not exist. The same hole exists on the `Resources` member of `UpdateJob`/`CreateCluster`, so a fixer should share one check.

This repository already wires cross-service checks of exactly this kind (other service crates depend on `service-com-amazonaws-s3-amazons3`, `...-iam-...`, `...-kms-...` in their Cargo.toml), so the bucket existence check is reachable against the S3 emulator's own state rather than hypothetical.

## Evidence

Spec/model: `CreateJob` declares `InvalidResourceException` ("The specified resource can't be found…"), `KMSRequestFailedException` ("The provided Key Management Service key lacks the permissions to perform the specified CreateJob or UpdateJob action") and `Ec2RequestFailedException` ("Your user lacks the necessary Amazon EC2 permissions…") — see OPERATIONS.md 'CreateJob' and `smithy_query(query="operation", operation="CreateJob")`. `S3Resource$BucketArn` is documented as "The Amazon Resource Name (ARN) of an Amazon S3 bucket"; `CreateJobRequest$Resources`: "With IMPORT jobs, you specify the bucket or buckets that your transferred data will be imported into. With EXPORT jobs, you specify the bucket or buckets that your transferred data will be exported from." Well-established Snow Family behavior: a job cannot be ordered for a bucket the account does not own — the order is bound to real buckets and a real role at creation.

Handler: services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/create_job.rs, `handle` — store lookups at lines 294-308 (long-term pricing), 314-326 (cluster), 418-444 (address / forwarding address); the transfer check at lines 508-528 calls `crate::state::names_s3_bucket(input.resources.as_ref())` and tests `input.role_arn.as_deref().is_none_or(str::is_empty)` only. `crate::state::names_s3_bucket` (src/state.rs lines 171-178) returns true as soon as `s3_resources` is a non-empty list — no ARN is parsed, resolved or compared with any store. `kms_key_arn`, `role_arn` and `resources` are then moved verbatim into `StoredJob` (lines 557-559).

Tests: tests/all/job_order_validation.rs `create_job_refuses_a_transfer_order_that_names_no_bucket_or_no_role` (line 328 ff.) asserts only the presence rule — rows are "Resources naming no bucket list", "an empty bucket list", "buckets with no role". No test anywhere in tests/all asserts a refusal for a bucket, role, AMI or KMS key that does not exist (`grep -n 'BucketArn\|bucket' tests/all/job_order_validation.rs` shows presence cases only).

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSIESnowballJobManagementService.CreateAddress` and a deliverable address; keep the minted `AddressId`
2. POST / with `X-Amz-Target: AWSIESnowballJobManagementService.CreateJob` and body {"JobType":"IMPORT","SnowballType":"EDGE_S","ShippingOption":"SECOND_DAY","AddressId":"<id>","RoleARN":"arn:aws:iam::123456789012:role/role-that-was-never-created","Resources":{"S3Resources":[{"BucketArn":"arn:aws:s3:::bucket-that-does-not-exist"}]}} — observe HTTP 200 with a minted JobId instead of a 400 `InvalidResourceException`
3. POST / ...DescribeJob with that JobId — observe `JobMetadata.Resources.S3Resources[0].BucketArn` and `RoleARN` reported back as a valid `New` order against resources that do not exist
4. Repeat with "KmsKeyARN":"arn:aws:kms:us-west-2:123456789012:key/00000000-0000-0000-0000-000000000000" and with "Resources":{"S3Resources":[{"BucketArn":"arn:aws:s3:::b"}],"Ec2AmiResources":[{"AmiId":"ami-00000000000000000"}]} — both are accepted as well
5. For contrast, create a bucket in the S3 emulator mounted on the same engine and note that the Snowball handler behaves identically whether or not the named bucket exists
