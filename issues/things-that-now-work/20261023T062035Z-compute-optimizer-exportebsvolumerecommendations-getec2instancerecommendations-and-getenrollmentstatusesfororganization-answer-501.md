Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer.export-to-nonexistent-bucket-rejected compute-optimizer.get-ec2-instance-recommendations-schema-and-account-scoping compute-optimizer.get-enrollment-statuses-for-organization-non-management-rejected
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer.export-to-nonexistent-bucket-rejected|run#5|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code}"
Issue-Key: compute-optimizer.get-ec2-instance-recommendations-schema-and-account-scoping|run#6|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code}"
Issue-Key: compute-optimizer.get-enrollment-statuses-for-organization-non-management-rejected|run#3|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code}"
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# compute-optimizer: ExportEBSVolumeRecommendations, GetEC2InstanceRecommendations and GetEnrollmentStatusesForOrganization answer 501 NotImplemented

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportEBSVolumeRecommendations

## Description
Three Compute Optimizer operations exercised by the behavioral suite are still scaffold stubs: the centralized request walk reaches the generated `handle` body, which returns `crate::scaffolded::Scaffolded::not_implemented(..)` — HTTP 501 with error code `NotImplemented`. Each is a distinct operation with its own handler file, so this issue carries THREE causes, all on compute-optimizer. All three must be implemented before closing.

Cause 1 of 3: ExportEBSVolumeRecommendations answers 501 NotImplemented instead of exporting (or refusing a bad destination).
  AWS: Smithy: ExportEBSVolumeRecommendations takes a required `s3DestinationConfig` (S3DestinationConfig) plus accountIds/filters/fieldsToExport/fileFormat/includeMemberAccounts, and returns ExportEBSVolumeRecommendationsResponse {jobId, s3Destination} on success (HTTP 200). Its modeled client errors include InvalidParameterValueException (HTTP 400, "The value supplied for the input parameter is out of range or not valid") and LimitExceededException (HTTP 400). Recorded behavior: a destination bucket that does not exist (or lacks the Compute Optimizer bucket policy) is refused with error code `InvalidParameterValueException` and no export job is created for it; a correctly-policied bucket is accepted with a jobId that is then discoverable through DescribeRecommendationExportJobs. Recorded message, as an example only: "The Amazon S3 destination provided does not exist. Confirm your S3 destination and try again."
  Emulator: every call returns HTTP 501 with error code `NotImplemented` and message "operation ExportEBSVolumeRecommendations is scaffolded but not implemented in this emulator". No destination validation, no job creation.
  Why: a client cannot export EBS volume recommendations at all, and a client that relies on the documented rejection to detect a mis-policied bucket gets a 501 whose code is not in the operation's modeled error set, so SDK error handling for InvalidParameterValueException never fires and the bad-destination path is silently indistinguishable from an emulator-wide outage.
  Fix: the ExportEBSVolumeRecommendations handler (services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/ops/export_ebs_volume_recommendations.rs) — destination validation against the S3 state, plus the export-job record in the service state so DescribeRecommendationExportJobs can see it.
  Traces: compute-optimizer.export-to-nonexistent-bucket-rejected.

Cause 2 of 3: GetEC2InstanceRecommendations answers 501 NotImplemented instead of returning account-scoped recommendations and refusing a cross-account ARN.
  AWS: Smithy: GetEC2InstanceRecommendations(instanceArns, nextToken, maxResults [range 0..1000], filters, accountIds, recommendationPreferences) -> {nextToken, instanceRecommendations, errors: GetRecommendationErrors}; modeled client errors include InvalidParameterValueException (400), OptInRequiredException (403), ResourceNotFoundException (404). Recorded behavior: with no instanceArns filter the response carries only recommendations whose accountId is the caller's own account and whose instanceArn is in the caller's account; an explicit instanceArns entry naming another account's instance is refused with error code `InvalidParameterValueException` (recorded message, as an example: "Invalid permission to access another account. If this is an organization account, request the management account to enable service access.").
  Emulator: every call returns HTTP 501 / `NotImplemented`.
  Why: the primary EC2 recommendation read is unavailable, and the account-scoping boundary is unobservable — a client cannot distinguish "no recommendations" from "operation absent", and tenancy-isolation logic that depends on the documented 400 refusal is never exercised.
  Fix: the GetEC2InstanceRecommendations handler (services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/ops/get_ec2_instance_recommendations.rs) — account scoping of the returned list, maxResults honoring, the modeled `errors` list, and the cross-account instanceArns rejection.
  Traces: compute-optimizer.get-ec2-instance-recommendations-schema-and-account-scoping.

Cause 3 of 3: GetEnrollmentStatusesForOrganization answers 501 NotImplemented instead of the organization-boundary rejection.
  AWS: Smithy: GetEnrollmentStatusesForOrganization(filters, nextToken, maxResults [range 0..1000]) -> {accountEnrollmentStatuses, nextToken}, paginated; modeled client errors include AccessDeniedException (403) and InvalidParameterValueException (400). Recorded behavior: called with credentials that are not an AWS Organizations management account, the operation is refused with error code `InvalidParameterValueException` rather than returning an empty list, while the account-scoped GetEnrollmentStatus on the same client still succeeds.
  Emulator: every call returns HTTP 501 / `NotImplemented`.
  Why: the organization/standalone-account boundary for this operation does not exist in the emulator. A client that handles the documented rejection to decide whether it is a management account instead sees an unmodeled 501, and multi-account enrollment code paths cannot be tested.
  Fix: the GetEnrollmentStatusesForOrganization handler (services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/ops/get_enrollment_statuses_for_organization.rs) — the management-account check that produces the modeled 400, and the paginated success shape for the management-account case.
  Traces: compute-optimizer.get-enrollment-statuses-for-organization-non-management-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against this checkout; compute-optimizer 4/25 passed. Each of these three checks failed on a call whose response was HTTP 501 / error code `NotImplemented`.

Cause 1 — check compute-optimizer.export-to-nonexistent-bucket-rejected [mismatch]:
  call: ExportEBSVolumeRecommendations(s3DestinationConfig={bucket: <freshly generated, nonexistent bucket name>})
  expected: error code InvalidParameterValueException (HTTP 400)
  actual:   AssertionError: expected InvalidParameterValueException, got NotImplemented
  source: services/.../src/ops/export_ebs_volume_recommendations.rs:209 returns Scaffolded::not_implemented("ExportEBSVolumeRecommendations"); the SCAFFOLDED marker is still on line 4 of that file.

Cause 2 — check compute-optimizer.get-ec2-instance-recommendations-schema-and-account-scoping [mismatch]:
  call: GetEC2InstanceRecommendations(instanceArns=[arn:aws:ec2:<region>:111122223333:instance/i-0ffffffffffffffff])
  expected: error code InvalidParameterValueException (HTTP 400); a following unfiltered call returning instanceRecommendations scoped to the caller account
  actual:   AssertionError: expected InvalidParameterValueException, got NotImplemented
  source: services/.../src/ops/get_ec2_instance_recommendations.rs carries the SCAFFOLDED, NOT IMPLEMENTED marker (line 4).

Cause 3 — check compute-optimizer.get-enrollment-statuses-for-organization-non-management-rejected [mismatch]:
  call: GetEnrollmentStatusesForOrganization() from non-management credentials
  expected: error code InvalidParameterValueException (HTTP 400)
  actual:   AssertionError: expected InvalidParameterValueException, got NotImplemented
  source: services/.../src/ops/get_enrollment_statuses_for_organization.rs carries the SCAFFOLDED, NOT IMPLEMENTED marker (line 4).

Model evidence (smithy_query on services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/smithy-model/com-amazonaws-computeoptimizer-computeoptimizerservice.json): all three operations model InvalidParameterValueException with httpError 400; none models any error that would be serialized as `NotImplemented`.

Note: separate scaffold issues are already filed for other stubbed compute-optimizer operations (ExportLambdaFunctionRecommendations, DescribeRecommendationExportJobs, GetAutoScalingGroupRecommendations, GetEBSVolumeRecommendations, GetECSServiceRecommendations, GetIdleRecommendations, GetLambdaFunctionRecommendations, GetRDSDatabaseRecommendations, GetRecommendationSummaries). The three operations in THIS issue are not covered by any of those.

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite simparity-behavioral-aws-v1 (service compute-optimizer).
2. Cause 1: replay check compute-optimizer.export-to-nonexistent-bucket-rejected — ensure the account is opted in (UpdateEnrollmentStatus status=Active), then call ExportEBSVolumeRecommendations with s3DestinationConfig={bucket: <a bucket name that does not exist>}. Expect a 400 whose error code is InvalidParameterValueException and no export job referencing that bucket; observe HTTP 501 error code NotImplemented. Then repeat against a bucket created with the Compute Optimizer bucket policy and expect 200 with jobId + s3Destination.bucket echoing the request.
3. Cause 2: replay check compute-optimizer.get-ec2-instance-recommendations-schema-and-account-scoping — call GetEC2InstanceRecommendations(instanceArns=["arn:aws:ec2:<region>:111122223333:instance/i-0ffffffffffffffff"]). Expect error code InvalidParameterValueException; observe NotImplemented. Then call GetEC2InstanceRecommendations(maxResults=25) and expect 200 with at most 25 instanceRecommendations, every element's accountId equal to the caller's account and instanceArn prefixed arn:aws:ec2:<region>:<caller account>:instance/.
4. Cause 3: replay check compute-optimizer.get-enrollment-statuses-for-organization-non-management-rejected — call GetEnrollmentStatusesForOrganization() with non-management-account credentials. Expect error code InvalidParameterValueException; observe NotImplemented. GetEnrollmentStatus on the same client must still succeed.
5. All three causes must pass before closing: re-replay compute-optimizer.export-to-nonexistent-bucket-rejected, compute-optimizer.get-ec2-instance-recommendations-schema-and-account-scoping and compute-optimizer.get-enrollment-statuses-for-organization-non-management-rejected.
