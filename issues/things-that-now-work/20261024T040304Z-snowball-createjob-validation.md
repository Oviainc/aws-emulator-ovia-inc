Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceac96a2525794129e4bd0e80b1424d3e5c0e5c2

# CreateJob accepts a job description AWS refuses: no JobType/SnowballType/ShippingOption, and a LongTermPricingId that names nothing

_Filed: 2026-10-24T04:03:04Z_
_Operation: CreateJob | Class: validation_

> _Note: this issue was identified against the code base at commit `ceac96a2525794129e4bd0e80b1424d3e5c0e5c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A standalone (non-cluster) CreateJob on the real service must describe the device order: the job type, the device type, the shipping speed, the IAM role and (for IMPORT/EXPORT) the S3 resources. The emulator validates only that the submitted AddressId (and ForwardingAddressId) names a stored address; every other member is optional and simply stored.

Two concrete consequences:

1. `CreateJob {"AddressId": "ADID..."}` — no JobType, no SnowballType, no ShippingOption, no RoleARN, no Resources — answers 200 with a minted JobId, and DescribeJob then reports a job in state `New` with no JobType, no SnowballType and no ShippingDetails. On AWS such a request is refused (the modeled `InvalidInputCombinationException` — "Job or cluster creation failed. One or more inputs were invalid. Confirm that the CreateClusterRequest$SnowballType value supports your CreateJobRequest$JobType, and try again." — is the error this operation declares for exactly this class of failure). No JobType/SnowballType compatibility check happens at all: any value of the two enums can be combined (e.g. `JobType=EXPORT` with `SnowballType=SNC1_SSD`).

2. `LongTermPricingId` is stored unchecked. The handler deliberately refuses any `ClusterId` because no cluster can exist in this emulator (CreateCluster is still a 501 stub), but applies no equivalent rule to `LongTermPricingId` even though `CreateLongTermPricing` is also a stub, so every well-formed LTPID necessarily names a resource that does not exist. Expected: `InvalidResourceException` ("The specified resource can't be found"); actual: 200, and DescribeJob reports `LongTermPricingId` for a long-term-pricing record that was never created.

## Evidence

Spec: CreateJob declares `InvalidInputCombinationException` and `InvalidResourceException` (OPERATIONS.md 'CreateJob'; smithy_query operation=CreateJob), with the combination error documented as the SnowballType/JobType mismatch error. Handler: services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/create_job.rs lines 264-351 — the only checks are ClusterId present (always refused), AddressId present + known, ForwardingAddressId known; `input.job_type`, `input.snowball_type`, `input.shipping_option`, `input.role_arn`, `input.resources` and `input.long_term_pricing_id` are copied into StoredJob with no validation. Tests: tests/all/job_lifecycle.rs `create_job_refuses_a_resource_no_record_answers_to` covers only unknown AddressId/ForwardingAddressId/ClusterId; `job_rows()` rows all carry a JobType, so nothing asserts a refusal for an under-specified job or an unknown LongTermPricingId.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSIESnowballJobManagementService.CreateAddress and a deliverable address; keep the minted AddressId
2. POST / with X-Amz-Target: ...CreateJob and body {"AddressId": "<id>"} — observe 200 with a minted JobId instead of a 400 refusal
3. DescribeJob on that JobId — observe JobMetadata with no JobType, no SnowballType and no ShippingDetails
4. POST / CreateJob with {"AddressId": "<id>", "JobType": "IMPORT", "SnowballType": "EDGE_S", "ShippingOption": "SECOND_DAY", "LongTermPricingId": "LTPID00000000-0000-0000-0000-000000000000"} — observe 200, and DescribeJob echoing a LongTermPricingId no CreateLongTermPricing ever minted (expected InvalidResourceException)
