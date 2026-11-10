Issue-Source: trace-sweep-divergence
Issue-Trace: migrationhub-config-create-home-region-control-dry-run-validation-ordering
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: migrationhub-config-create-home-region-control-dry-run-validation-ordering migrationhub-config-create-home-region-control-invalid-input migrationhub-config-create-home-region-control-invalid-region-pattern-rejected migrationhub-config-home-region-lifecycle migrationhub-config-describe-home-region-controls-filters-by-home-region migrationhub-config-describe-home-region-controls-nonexistent-control-id-returns-empty-list migrationhub-config-delete-home-region-control-invalid-input
Issue-Shape-Id: com.amazonaws.migrationhubconfig#AWSMigrationHubMultiAccountService
Issue-Key: migrationhub-config|CreateHomeRegionControl|4xx->5xx|$.Message
Issue-Key: migrationhub-config|CreateHomeRegionControl|4xx->5xx|$.__type
Issue-Key: migrationhub-config|CreateHomeRegionControl|4xx->5xx|status
Issue-Key: migrationhub-config|DescribeHomeRegionControls|2xx->5xx|$.HomeRegionControls
Issue-Key: migrationhub-config|DescribeHomeRegionControls|2xx->5xx|status
Issue-Key: migrationhub-config|GetHomeRegion|2xx->5xx|$
Issue-Key: migrationhub-config|GetHomeRegion|2xx->5xx|status
Issue-Key: migrationhub-config|CreateHomeRegionControl|2xx->5xx|$.HomeRegionControl
Issue-Key: migrationhub-config|CreateHomeRegionControl|2xx->5xx|status
Issue-Key: migrationhub-config|DeleteHomeRegionControl|4xx->5xx|$.Message
Issue-Key: migrationhub-config|DeleteHomeRegionControl|4xx->5xx|$.__type
Issue-Key: migrationhub-config|DeleteHomeRegionControl|4xx->5xx|status
Issue-Key: migrationhub-config|GetHomeRegion|2xx->5xx|$.HomeRegion
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# MigrationHub Config wire replay: every operation refused with AmbiguousService instead of serving the request

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHomeRegionControl

## Description
Wire replay of the MigrationHub Config fixtures scores 0/31 asserted steps (7 fixtures, 31 mismatched, 6 further steps blocked). Every one of the service's four operations answers HTTP 500 with an emulator-authored error whose __type is 'AmbiguousService' and whose body reports that two emulated services both claim requests for the signing name 'mgh'. Smithy service_info shows why that is not a legal basis for refusal: AWSMigrationHubMultiAccountService (version 2019-06-30, awsJson1_1) has awsService.endpointPrefix = 'migrationhub-config' but sigv4Name = 'mgh' and arnNamespace = 'mgh', while AWSMigrationHub has endpointPrefix = 'mgh' and the same sigv4Name 'mgh'. The signing name is shared by design; the endpoint prefix and the awsJson1_1 X-Amz-Target service-shape prefix are what distinguish the two. 'AmbiguousService' is not a modeled error of this service at all: the model's only errors are the service-common AccessDeniedException, InternalServerError, InvalidInputException, ServiceUnavailableException, ThrottlingException, plus DryRunOperation on CreateHomeRegionControl. The seven clusters below are the seven distinct expected responses the replay asserted; all of them are answered by the same refusal, so all seven must be closed.

Cause 1 of 7 - CreateHomeRegionControl rejects a malformed HomeRegion. AWS: Smithy CreateHomeRegionControl.HomeRegion is @required, @length min 1 max 50, @pattern ^([a-z]+)-([a-z]+)-([0-9]+)$; a value violating it is an InvalidInputException, httpError 400, and the recorded response's message is built from the request, naming the member and the constraint (recorded example: "1 validation error detected: Value 'Not_A_Region!' at 'homeRegion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-z]+)-([a-z]+)-([0-9]+)$"). Validation must also run before the DryRun short-circuit, which is what this fixture orders. Emulator: HTTP 500, __type 'AmbiguousService', no Message member. Why: a client that catches InvalidInputException to surface a bad region to its user instead sees an unmodeled 5xx that botocore treats as retryable, so it burns its retry budget and then reports a generic server failure. Fix: the shared request router / service-resolution layer, before any CreateHomeRegionControl validation is reached. Traces: migrationhub-config-create-home-region-control-dry-run-validation-ordering#task#1 (cluster of 8: also ...dry-run-validation-ordering#task#3, ...-invalid-input#task#1/#task#2/#task#3, ...-invalid-region-pattern-rejected#task#1, ...-home-region-lifecycle#task#11/#task#12).

Cause 2 of 7 - DescribeHomeRegionControls on a pristine account. AWS: Smithy DescribeHomeRegionControls returns DescribeHomeRegionControlsResult with HomeRegionControls (list, @length 0..100); with no control configured the recorded response is HTTP 200 with HomeRegionControls = []. Emulator: HTTP 500 and no HomeRegionControls member. Why: the list read is the service's pristine-state probe and its pagination surface, so neither the empty-list baseline nor the ControlId/HomeRegion filters can be observed. Fix: same router layer. Traces: migrationhub-config-create-home-region-control-dry-run-validation-ordering#task#2 (cluster of 13, spanning ...-invalid-region-pattern-rejected, ...-filters-by-home-region, ...-nonexistent-control-id-returns-empty-list and ...-home-region-lifecycle including its teardown#16).

Cause 3 of 7 - GetHomeRegion before any home region is set. AWS: Smithy GetHomeRegion takes no input and returns GetHomeRegionResult whose only member HomeRegion is optional; when unset the recorded response is HTTP 200 with an empty JSON body {}. Emulator: HTTP 500 whose body is the AmbiguousService error document (recorded example message: "2 emulated services all claim requests for 'mgh': mgh, mgh; the request is rejected rather than served by an arbitrary one"). Why: GetHomeRegion is the call other Migration Hub services make during initialization to discover the home region; a 500 here aborts that bootstrap instead of reporting 'not configured yet'. Fix: same router layer. Traces: migrationhub-config-create-home-region-control-invalid-input#task#4 (cluster of 3: also ...-home-region-lifecycle#task#1 and #teardown#15).

Cause 4 of 7 - CreateHomeRegionControl succeeds for a well-formed request. AWS: Smithy CreateHomeRegionControl returns CreateHomeRegionControlResult.HomeRegionControl; the recorded 200 response carries ControlId, HomeRegion ('us-west-2'), RequestedTime and Target {Id, Type: ACCOUNT} (ControlId and RequestedTime are generated, asserted only as shaped). Emulator: HTTP 500 and no HomeRegionControl member. Why: the home region can never be set, so every downstream workflow that depends on the singleton control is unreachable. Fix: same router layer. Traces: migrationhub-config-create-home-region-control-invalid-region-pattern-rejected#task#3 (cluster of 2: also ...-home-region-lifecycle#task#4).

Cause 5 of 7 - DeleteHomeRegionControl on an id that does not exist. AWS: Smithy DeleteHomeRegionControl.ControlId is @required, @pattern ^hrc-[a-z0-9]{12}$; a well-formed but unknown id is an InvalidInputException, httpError 400, with a message built from the request id (recorded example: 'Home Region Control ID "hrc-000000000000" does not exist'). Emulator: HTTP 500, __type 'AmbiguousService', no Message. Why: a client cannot distinguish 'already deleted / wrong id' from a server fault, so idempotent teardown logic retries a permanent error. Fix: same router layer. Traces: migrationhub-config-delete-home-region-control-invalid-input#task#1.

Cause 6 of 7 - CreateHomeRegionControl as fixture setup. AWS: as cause 4, HTTP 200 (this cluster asserts the status only). Emulator: HTTP 500. Why: the setup step cannot establish the preconditions for the Describe filter fixtures, so their assertions are unreachable even once routing is fixed if setup still fails. Fix: same router layer. Traces: migrationhub-config-describe-home-region-controls-filters-by-home-region#setup#1 (cluster of 2: also ...-nonexistent-control-id-returns-empty-list#setup#1).

Cause 7 of 7 - GetHomeRegion after the home region has been set. AWS: Smithy GetHomeRegionResult.HomeRegion, @pattern ^([a-z]+)-([a-z]+)-([0-9]+)$; the recorded response is HTTP 200 with HomeRegion equal to the region the control was created in ('us-west-2' in this fixture), and it must stay consistent with DescribeHomeRegionControls. Emulator: HTTP 500 and no HomeRegion member. Why: the read-after-write consistency guarantee between Create and Get cannot hold, so SDKs caching the home region get nothing. Fix: same router layer. Traces: migrationhub-config-home-region-lifecycle#task#5 (cluster of 2: also ...#task#13).

Related: the open behavioral issue 20261022T110819Z-migrationhub-config-is-entirely-unreachable-router-resolves-by.md diagnoses the same router root cause from the behavioral suite, but it carries only behavioral check ids and no wire fixture/step key, so no wire divergence here is owned by it (the replay reports owned_by = [] for all seven clusters). This issue records the wire replay keys so a fix can be confirmed against the recorded corpus.

## Evidence
Replay summary (trace_set simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, state_reset_failures 0): fixtures_replayed 7, steps_total 37, steps_passed 0, steps_mismatched 31, steps_blocked 6, score_pct 0.0, divergence_count 7 - all seven on service 'migrationhub-config'.

Per-cluster expected vs actual, verbatim from the replay detail:

1. CreateHomeRegionControl - status: expected 400, got 500; $.__type: expected 'InvalidInputException', got 'AmbiguousService'; $.Message: missing (expected "1 validation error detected: Value 'Not_A_Region!' at 'homeRegion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-z]+)-([a-z]+)-([0-9]+)$", got None). affected_count 8.
2. DescribeHomeRegionControls - status: expected 200, got 500; $.HomeRegionControls: missing (expected [], got None). affected_count 13.
3. GetHomeRegion - status: expected 200, got 500; $: expected {}, got {'__type': 'AmbiguousService', 'message': "2 emulated services all claim requests for 'mgh': mgh, mgh; the request is rejected rather than served by an arbitrary one"}. affected_count 3.
4. CreateHomeRegionControl - status: expected 200, got 500; $.HomeRegionControl: missing (expected {'ControlId': '<shaped-not-asserted>', 'HomeRegion': 'us-west-2', 'RequestedTime': '<shaped-not-asserted>', 'Target': {'Id': '000000000000', 'Type': 'ACCOUNT'}}, got None). affected_count 2.
5. DeleteHomeRegionControl - status: expected 400, got 500; $.__type: expected 'InvalidInputException', got 'AmbiguousService'; $.Message: missing (expected 'Home Region Control ID "hrc-000000000000" does not exist', got None). affected_count 1.
6. CreateHomeRegionControl - status: expected 200, got 500. affected_count 2.
7. GetHomeRegion - status: expected 200, got 500; $.HomeRegion: missing (expected 'us-west-2', got None). affected_count 2.

Model evidence for the misrouting (smithy_query, services/service-com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice/smithy-model/com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice.json):
  service AWSMigrationHubMultiAccountService, version 2019-06-30, protocols [awsJson1_1], authSchemes [sigv4]
  awsService.endpointPrefix = 'migrationhub-config', awsService.arnNamespace = 'mgh', sigv4Name = 'mgh'
  common_errors = [AccessDeniedException, InternalServerError, InvalidInputException, ServiceUnavailableException, ThrottlingException]; CreateHomeRegionControl adds DryRunOperation (httpError 400). 'AmbiguousService' appears nowhere in the model.
The sibling service AWSMigrationHub uses endpointPrefix 'mgh' with the same sigv4Name 'mgh'; the refusal message naming its two candidates as 'mgh, mgh' (the same token twice) shows the router keys candidates by signing name rather than by their distinct endpoint prefixes.

HTTP statuses and error codes above are requirements. The validation and not-found Message values are requirements only as rules (a validation error naming the member and its violated constraint; a not-found error naming the requested ControlId) - the quoted texts are recorded examples. The AmbiguousService message text is quoted as an example only.

## Steps to Reproduce
1. Boot the emulator from this checkout; both service-com-amazonaws-migrationhubconfig-awsmigrationhubmultiaccountservice and service-com-amazonaws-migrationhub-awsmigrationhub are bootstrapped.
2. Replay the simparity-wire-aws-v1 corpus. All 7 migrationhub-config fixtures fail; 31 asserted steps mismatch and 6 more are blocked.
3. Representative step: fixture migrationhub-config-create-home-region-control-dry-run-validation-ordering, step task#1 - an awsJson1_1 POST / with X-Amz-Target AWSMigrationHubMultiAccountService.CreateHomeRegionControl and HomeRegion 'Not_A_Region!'. Expect HTTP 400 with __type InvalidInputException and a Message naming the homeRegion pattern constraint; observe HTTP 500 with __type 'AmbiguousService' and no Message.
4. Re-replay the other six causes by their keys: ...-dry-run-validation-ordering#task#2 (DescribeHomeRegionControls -> expect 200 {"HomeRegionControls": []}), ...-create-home-region-control-invalid-input#task#4 (GetHomeRegion unset -> expect 200 {}), ...-create-home-region-control-invalid-region-pattern-rejected#task#3 (CreateHomeRegionControl -> expect 200 with HomeRegionControl), ...-delete-home-region-control-invalid-input#task#1 (DeleteHomeRegionControl hrc-000000000000 -> expect 400 InvalidInputException), ...-describe-home-region-controls-filters-by-home-region#setup#1 (CreateHomeRegionControl setup -> expect 200), ...-home-region-lifecycle#task#5 (GetHomeRegion after create -> expect 200 HomeRegion 'us-west-2'). Each returns HTTP 500 AmbiguousService.
5. Equivalently with a client: point boto3's 'migrationhub-config' client at the emulator and call describe_home_region_controls() - ClientError (AmbiguousService) after botocore exhausts its retries, instead of an empty HomeRegionControls list.
6. All seven causes must pass on re-replay before this issue is closed.
