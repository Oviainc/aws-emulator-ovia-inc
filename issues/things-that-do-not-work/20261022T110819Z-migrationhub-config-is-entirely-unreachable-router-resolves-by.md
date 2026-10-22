Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: migrationhub-config.concurrent-create-home-region-control-singleton-invariant migrationhub-config.concurrent-delete-home-region-control-not-fenced-to-single-winner migrationhub-config.create-home-region-control-access-denied-without-iam-permission migrationhub-config.create-home-region-control-dry-run-does-not-bypass-input-validation migrationhub-config.create-home-region-control-dry-run-reports-success-without-creating migrationhub-config.create-home-region-control-foreign-account-target-rejected migrationhub-config.create-home-region-control-invalid-region-pattern-rejected migrationhub-config.create-home-region-control-missing-permission-rejected migrationhub-config.create-home-region-control-non-numeric-target-id-rejected migrationhub-config.create-home-region-control-sets-state migrationhub-config.create-home-region-control-unsupported-region-rejected migrationhub-config.create-home-region-control-when-already-set-rejected migrationhub-config.delete-home-region-control-double-delete-rejected migrationhub-config.delete-home-region-control-malformed-control-id-rejected migrationhub-config.delete-home-region-control-nonexistent-id-rejected migrationhub-config.delete-home-region-control-removes-from-describe migrationhub-config.describe-and-get-home-region-consistent migrationhub-config.describe-home-region-controls-filters-by-control-id migrationhub-config.describe-home-region-controls-filters-by-home-region migrationhub-config.describe-home-region-controls-lists-created-control migrationhub-config.describe-home-region-controls-malformed-control-id-filter-rejected migrationhub-config.describe-home-region-controls-malformed-home-region-filter-rejected migrationhub-config.describe-home-region-controls-max-results-single-page migrationhub-config.describe-home-region-controls-nonexistent-control-id-returns-empty-list migrationhub-config.describe-home-region-controls-rejects-multiple-simultaneous-filters migrationhub-config.describe-home-region-controls-stable-across-repeated-reads migrationhub-config.describe-home-region-controls-target-filter-matches-calling-account migrationhub-config.e2e-home-region-control-delete-recreate-workflow migrationhub-config.get-home-region-after-create-matches migrationhub-config.get-home-region-returns-empty-when-unset migrationhub-config.get-home-region-transitions-from-unset-to-set migrationhub-config.home-region-control-consistent-across-apis migrationhub-config.home-region-control-cycle-survives-delete-then-recreate migrationhub-config.home-region-control-requested-time-timestamp-round-trip-preserved
Issue-Shape-Id: com.amazonaws.migrationhubconfig#AWSMigrationHubMultiAccountService
Issue-Key: migrationhub-config|DescribeHomeRegionControls|AmbiguousService
Issue-Commit: 92ad2730926ec3062b662e938e8ed9f3fac68577

# MigrationHub Config is entirely unreachable: router resolves by sigv4 signing name, so migrationhub-config and migrationhub both claim 'mgh' and every request is refused with AmbiguousService

> _Note: this issue was identified against the code base at commit `92ad2730926ec3062b662e938e8ed9f3fac68577`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeHomeRegionControls

## Description
Cause 1 of 1: the emulator's service-resolution/routing layer keys an incoming request on the sigv4 signing name instead of on the endpoint prefix (host label) or the awsJson1_1 X-Amz-Target service-shape prefix. Two emulated services legitimately share the signing name 'mgh', so the router finds two candidates, declares the request ambiguous and refuses it. The result is that NOT ONE MigrationHub Config operation is reachable: all 34 behavioral checks for this service fail on their very first call (CreateHomeRegionControl, DescribeHomeRegionControls, GetHomeRegion and DeleteHomeRegionControl are all affected — the error surfaces on DescribeHomeRegionControls because that is the first call every check makes, directly or via its pristine-state precondition).

AWS: Smithy (services/service-com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice) — service AWSMigrationHubMultiAccountService, version 2019-06-30, protocol awsJson1_1, awsService.endpointPrefix = "migrationhub-config", awsService.arnNamespace = "mgh", sigv4Name = "mgh". Smithy (services/service-com-amazonaws-migrationhub-awsmigrationhub) — service AWSMigrationHub, version 2017-05-31, protocol awsJson1_1, awsService.endpointPrefix = "mgh", awsService.arnNamespace = "mgh", sigv4Name = "mgh". The signing name is therefore NOT a unique service key in AWS: two distinct services share 'mgh' by design. AWS disambiguates them on the wire by endpoint host (migrationhub-config.<region>.amazonaws.com vs mgh.<region>.amazonaws.com) and, for awsJson1_1, by the X-Amz-Target header's service-shape prefix (AWSMigrationHubMultiAccountService.DescribeHomeRegionControls vs AWSMigrationHub.<Op>). A well-formed DescribeHomeRegionControls call against AWS returns HTTP 200 with a HomeRegionControls list (empty on a pristine account); its modeled failures are the service-common errors only: AccessDeniedException, InternalServerError, InvalidInputException, ServiceUnavailableException, ThrottlingException. 'AmbiguousService' is not a modeled error of this service and does not exist in AWS at all.

Emulator: every MigrationHub Config request is rejected with an emulator-authored error whose code is AmbiguousService and whose message is e.g. "2 emulated services all claim requests for 'mgh': mgh, mgh; the request is rejected rather than served by an arbitrary one" (message quoted as a recorded example; the requirement is the error code and the fact that the request must not be refused at all). botocore classifies it as retryable and exhausts its retries, so each call also fails only after "reached max retries: 3".

Why: a client cannot use MigrationHub Config against this emulator in any way. Home-region setup is the gate for the whole Migration Hub control plane, so SDK code that calls GetHomeRegion/CreateHomeRegionControl during initialization aborts; the error code is unmodeled, so client error handling that switches on the documented errors falls through to a generic failure, and because it is treated as retryable the client burns its full retry budget before surfacing anything. No positive behavior (singleton home-region invariant, Describe filters, pagination, timestamp round-trip) and no negative behavior (InvalidInputException on malformed ControlId/HomeRegion, double-delete rejection, AccessDeniedException without IAM permission) can be observed.

Fix: the request router / service-resolution layer shared by all emulated services (not any migrationhub-config handler — the handlers are never reached). Resolution must use the endpoint prefix taken from the request host, falling back to the X-Amz-Target service-shape prefix for awsJson1_1, and treat the sigv4 signing name as non-unique; the AmbiguousService refusal should remain only for genuinely indistinguishable requests. This is the same router defect already filed per-service for other signing-name collisions (e.g. 20261022T031220Z-wisdom-all-wisdom-requests-rejected-with-ambiguousservice, 20261022T081518Z-apigateway-api-gateway-is-unreachable-the-router-refuses-every, 20261022T092619Z-greengrassv2-every-request-rejected-with-ambiguousservice-because-greengrass); none of those issues carries the 'mgh' pair or any migrationhub-config replay key, so this one records it.

Traces: all 34 migrationhub-config behavioral checks, listed in trace_ids; each fails identically on its first MigrationHub Config call.

## Evidence
Suite result: migrationhub-config 0/34 passed (0%), 34 errored, 0 mismatched, 0 skipped.

Expected (AWS, per Smithy AWSMigrationHubMultiAccountService 2019-06-30, awsJson1_1):
  POST / to migrationhub-config.<region>.amazonaws.com
  X-Amz-Target: AWSMigrationHubMultiAccountService.DescribeHomeRegionControls
  -> HTTP 200, {"HomeRegionControls": []} on a pristine account
  Modeled errors for this service are only: AccessDeniedException, InternalServerError,
  InvalidInputException, ServiceUnavailableException, ThrottlingException.

Actual (emulator), identical for all 34 checks:
  botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the
  DescribeHomeRegionControls operation (reached max retries: 3): 2 emulated services all
  claim requests for 'mgh': mgh, mgh; the request is rejected rather than served by an
  arbitrary one

Root-cause evidence from the on-disk models (two services, one signing name):
  AWSMigrationHubMultiAccountService: endpointPrefix="migrationhub-config", sigv4Name="mgh", arnNamespace="mgh"
  AWSMigrationHub:                    endpointPrefix="mgh",                  sigv4Name="mgh", arnNamespace="mgh"
Both are bootstrapped in this checkout, which is why the router sees two claimants for 'mgh'.
The error message's "mgh, mgh" (the same token twice) shows the candidates are being named by
signing name rather than by their distinct endpoint prefixes.

## Steps to Reproduce
1. Boot the emulator from this checkout (both service-com-amazonaws-migrationhub-awsmigrationhub and service-com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice are bootstrapped).
2. Point a botocore/boto3 client for service 'migrationhub-config' at the emulator endpoint in any region.
3. Call the simplest read: mhc.describe_home_region_controls().
4. Observe ClientError with error code AmbiguousService and the message "2 emulated services all claim requests for 'mgh': mgh, mgh; ...", after botocore exhausts 3 retries, instead of HTTP 200 with an (empty) HomeRegionControls list.
5. The same refusal occurs for create_home_region_control, get_home_region and delete_home_region_control — no MigrationHub Config operation can be invoked.
