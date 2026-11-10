Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.enum-wire-values-precede-resource-resolution ds.list-ad-assessments-returns-inventory ds.register-event-topic-absent-directory-rejected ds.tag-limit-exceeded-rejected ds.disable-sso-without-enable-is-an-idempotent-no-op ds.remove-region-access-denied ds.microsoft-ad-create-delete-lifecycle ds.ip-route-duplicate-cidr-rejected ds.describe-snapshots-returns-inventory ds.snapshot-create-delete-lifecycle ds.ip-route-add-remove-lifecycle ds.list-log-subscriptions-returns-inventory ds.state-machine-trust-create-verify-delete-404
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.enum-wire-values-precede-resource-resolution|run#12|AssertionError|assert msg == want, f"DescribeLDAPSSettings(Type='Client') message: {msg!r} != {want!r}"
Issue-Key: ds.list-ad-assessments-returns-inventory|run#20|AssertionError|assert message == expected, (
Issue-Key: ds.register-event-topic-absent-directory-rejected|run#15|AssertionError|assert registered_message == expected, (
Issue-Key: ds.tag-limit-exceeded-rejected|run#12|AssertionError|assert message == expected_message, (
Issue-Key: ds.disable-sso-without-enable-is-an-idempotent-no-op|run#9|AssertionError|assert error is None, (
Issue-Key: ds.remove-region-access-denied|run#18|AssertionError|assert denied_code == "AccessDeniedException", (
Issue-Key: directory-service|DescribeDirectories|EntityDoesNotExistException
Issue-Key: ds.ip-route-duplicate-cidr-rejected|_wait_route_status#10|Timeout|time.sleep(interval_s)
Issue-Key: ds.describe-snapshots-returns-inventory|_wait_for_inventory_status#6|Timeout|time.sleep(interval_s)
Issue-Key: ds.snapshot-create-delete-lifecycle|_wait_snapshot_status#6|Timeout|time.sleep(interval_s)
Issue-Key: ds.ip-route-add-remove-lifecycle|_wait_route_status#6|Timeout|time.sleep(interval_s)
Issue-Key: ds.list-log-subscriptions-returns-inventory|_wait_directory_stage#4|Timeout|time.sleep(interval_s)
Issue-Key: ds.state-machine-trust-create-verify-delete-404|_wait_directory_stage#5|Timeout|time.sleep(interval_s)
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Directory Service control plane: 7 behavioral divergences — error envelopes carry no RequestId, DisableSso is 501, RemoveRegion is never authorized, a deleted directory 404s instead of reporting Deleting, duplicate IP route misclassified, and six polling checks never converge

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableSso, RemoveRegion, DescribeDirectories, AddTagsToResource, AddIpRoutes, RegisterEventTopic, DescribeADAssessment, DescribeLDAPSSettings, CreateSnapshot, DescribeSnapshots, ListIpRoutes, ListLogSubscriptions, CreateTrust

## Description
Behavioral replay of the ds suite (simparity-behavioral-ga-prod-v1) left 14 ds.* failing checks after skipping the ones the report marks ALREADY FILED (scaffold stubs) or OWNED by an open issue. They reduce to SEVEN independent root causes. A fixer must close all seven before submitting.

Related open issues that do NOT cover these checks (named here per the clustering rules, they carry none of these behavioral check ids): 20261023T212506Z-ds-error-envelopes-omit-the-modeled-requestid-and.md (wire corpus; its Cause 1 is the same defect as Cause 1 below, observed on a different operation set), 20261022T175927Z-ds-directory-creation-skips-password-and-vpc-validation.md and 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly.md (directory-create validation / instant Active).

Cause 1 of 7: every ds refusal omits the modeled RequestId and the ' : RequestId: <id>' message suffix.
AWS: Smithy: every ds error structure declares BOTH Message and RequestId — e.g. EntityDoesNotExistException, DirectoryDoesNotExistException and TagLimitExceededException each model RequestId with @pattern ^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$ (all client, HTTP 400). Recorded behavior: ds appends the same id to the human message as ' : RequestId: <id>'; the requirement is the PRESENCE of a UUID-shaped RequestId plus that suffix rule, built from the request — the particular id in any recording is an example.
Emulator: status (400) and error code match, but the RequestId member is absent on every refusal (botocore reports None) and no message carries the suffix. Observed on DescribeLDAPSSettings ("Directory: 'd-8cd06b93e3' does not exist." where AWS sends "... does not exist. : RequestId: <id>"), DescribeADAssessment ("Assessment Id da-000de0bfaf373d85ee does not exist."), RegisterEventTopic ("Directory: 'd-0be95295bd' does not exist.") and AddTagsToResource.
Why: the request id is the only correlation handle a caller, SDK retry log or support workflow has for a failed ds call; code reading e.response['Error']['RequestId'] or scraping the suffix gets nothing, and any client that compares ds diagnostics against AWS's text never matches.
Fix: the shared ds error-envelope construction used by the operation handlers in services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/ — populate the modeled RequestId from the per-request id and append ' : RequestId: <id>' when building Message (every handler currently constructs its errors with request_id: None).
Traces: ds.enum-wire-values-precede-resource-resolution, ds.list-ad-assessments-returns-inventory, ds.register-event-topic-absent-directory-rejected, ds.tag-limit-exceeded-rejected.

Cause 2 of 7: AddTagsToResource's over-limit refusal replaces ds's own diagnostic with an emulator-authored sentence.
AWS: Smithy: TagLimitExceededException (client, HTTP 400) — 'The maximum allowed number of tags was exceeded.' Recorded behavior: the message body is the fixed ds diagnostic 'Tag limit exceeded.' (plus the RequestId suffix of Cause 1). The rule is a fixed service-authored string, not a request-derived one.
Emulator: 400 with 'A resource may carry at most 50 tags; the request would leave 51' — a message this emulator invents, so neither the fixed text nor the suffix is produced.
Why: a client that classifies tag failures on the service's diagnostic (or a parity test asserting it) sees text AWS never emits; combined with Cause 1 the whole envelope is unrecognizable.
Fix: the AddTagsToResource handler's tag-limit refusal in src/ops/add_tags_to_resource.rs — emit ds's fixed 'Tag limit exceeded.' diagnostic.
Traces: ds.tag-limit-exceeded-rejected.

Cause 3 of 7: DisableSso answers NotImplemented instead of succeeding as an idempotent no-op.
AWS: Smithy: DisableSso takes DirectoryId (required) plus optional UserName/Password and returns an EMPTY DisableSsoResult; its modeled errors are AuthenticationFailedException, ClientException, EntityDoesNotExistException, InsufficientPermissionsException, ServiceException — no 'not supported' class. Recorded behavior: DisableSso on a directory whose SSO was never enabled SUCCEEDS (200, empty body) as a no-op.
Emulator: the call is refused with code NotImplemented ('operation ... is scaffolded but not implemented'), so the check's assertion that the error is None fails. Note: the suite did NOT mark this check ALREADY FILED, so no stub issue is carrying it.
Why: disable-then-verify is the standard teardown path; a 501 makes every idempotent cleanup and every 'turn SSO off' flow fail against the emulator even though AWS treats it as a no-op.
Fix: implement the DisableSso handler (src/ops/disable_sso.rs) — resolve the directory, clear SsoEnabled, and answer 200 with the empty result whether or not SSO was on.
Traces: ds.disable-sso-without-enable-is-an-idempotent-no-op.

Cause 4 of 7: RemoveRegion is never authorized — an explicitly denied caller gets a resource error instead of AccessDeniedException.
AWS: Smithy: RemoveRegion models AccessDeniedException (client, HTTP 400) alongside DirectoryDoesNotExistException; authorization is evaluated BEFORE the directory is resolved, so a caller whose IAM policy denies ds:RemoveRegion is refused with AccessDeniedException regardless of the directory id passed.
Emulator: the denied caller is answered DirectoryDoesNotExistException — the handler resolves the directory and never consults the caller's identity or policy, so no request is ever denied.
Why: a client cannot distinguish 'you may not do this' from 'that directory is gone'; least-privilege tests pass vacuously and a permissions regression is invisible, while callers are sent down a recreate path for what is really a policy denial.
Fix: the RemoveRegion handler (src/ops/remove_region.rs) and the ds request-authorization seam — evaluate the caller's permissions ahead of resource resolution and refuse with the modeled AccessDeniedException.
Traces: ds.remove-region-access-denied.

Cause 5 of 7: DeleteDirectory drops the record synchronously, so DescribeDirectories 400s instead of reporting the directory in Deleting.
AWS: Smithy: DescribeDirectories returns DirectoryDescriptions and models EntityDoesNotExistException for ids it cannot resolve; DirectoryStage carries Deleting and Deleted values. Recorded behavior: immediately after DeleteDirectory the directory is still described, with Stage transitioned OUT of Active into Deleting (full removal happens much later).
Emulator: DescribeDirectories(DirectoryIds=[<just-deleted id>]) raises EntityDoesNotExistException 'Directory: 'd-003ba5fe5a' does not exist.' — the delete removes the record outright, so the Deleting stage is never observable.
Why: every delete-and-wait client polls DescribeDirectories for Deleting/Deleted; against this emulator that poll raises instead of reporting progress, and code that treats the 400 as 'never existed' cannot tell a successful delete from a bad id.
Fix: the DeleteDirectory handler and the directory record/clock in src/ops/delete_directory.rs + src/state.rs — move the directory to Stage=Deleting and keep describing it until the removal transition falls due.
Traces: ds.microsoft-ad-create-delete-lifecycle.

Cause 6 of 7: AddIpRoutes refuses an already-routed CIDR with the wrong error class.
AWS: recorded behavior (stated by the check as measured on real AWS, us-west-2): re-adding a CIDR the directory already routes is refused with ClientException and the message 'The IP route(s) [<cidr>] already exist. If you want to recreate them, please delete the existing routes first.' (plus the RequestId suffix of Cause 1) — explicitly NOT EntityAlreadyExistsException. Smithy: AddIpRoutes models both ClientException and EntityAlreadyExistsException (both client, HTTP 400), so the model alone does not pick one; the recording does.
Emulator: src/ops/add_ip_routes.rs returns EntityAlreadyExistsException with 'Directory <id> already routes the address block <cidr>'. Status matches (400); the error code a client catches does not.
Why: the error code is what SDKs map to an exception class, so a caller's duplicate-route branch (keyed on ClientException) never runs and a benign duplicate surfaces as an unexpected entity conflict.
Fix: the duplicate-block branch of the AddIpRoutes handler (src/ops/add_ip_routes.rs) — refuse with ClientException carrying the route-already-exists diagnostic.
Traces: ds.ip-route-duplicate-cidr-rejected.

Cause 7 of 7: six ds checks that must wait for something never converge and exhaust the 60 s harness window.
AWS: these checks drive ordinary AWS sequences — build a VPC with two subnets, CreateMicrosoftAD, then poll: CreateSnapshot -> DescribeSnapshots Status=Completed -> DeleteSnapshot -> absent (ds.describe-snapshots-returns-inventory, ds.snapshot-create-delete-lifecycle); AddIpRoutes -> ListIpRoutes IpRouteStatusMsg=Added -> RemoveIpRoutes -> absent (ds.ip-route-add-remove-lifecycle, ds.ip-route-duplicate-cidr-rejected); CreateLogSubscription -> unfiltered ListLogSubscriptions (ds.list-log-subscriptions-returns-inventory); CreateTrust -> VerifyTrust -> Verified -> DeleteTrust (ds.state-machine-trust-create-verify-delete-404). On AWS each step answers and the polled state advances.
Emulator: every one of these six reports only 'Check timed out after 60.0s' — no call errored, the sequence simply never completes, so none of the asserted states is ever reached. The ds state transitions themselves are NOT the long pole (src/state.rs TRANSITION_SPAN = 500 ms for Requested->Active, Creating->Completed, Adding->Added), and the sibling checks that create a directory WITHOUT any fixture polling (e.g. ds.disable-sso-without-enable-is-an-idempotent-no-op) reach their assertions, so the stall is in a wait these six share and the others do not. Hypothesis (not stated by the model or any recording): the shared fixture's tag-filtered lookup/waiter loop never converges — these checks locate or build their VPC with DescribeVpcs(Filters=[{tag:Name}]) retry loops and the vpc_available / subnet_available waiters, and a sibling check in the same suite shows this emulator's DescribeVpcs rejecting a filter outright (ds.concurrent-add-ip-routes-conflict-detected: InvalidParameterValue "The filter 'isDefault' is invalid").
Why: no client can complete a create-then-poll lifecycle against this emulator: snapshots, IP routes, log subscriptions and trusts are all unverifiable end to end, and any SDK waiter in that path hangs instead of returning.
Fix: re-replay the six checks listed below and locate the non-converging wait, then fix it where it lives — the ds reporting handlers/clock (src/state.rs advance_clock, src/ops/describe_snapshots.rs, list_ip_routes.rs, list_log_subscriptions.rs, describe_trusts.rs) if a polled status never advances, or the EC2 describe/filter path the fixture polls if the VPC/subnet lookup is what stalls.
Traces: ds.describe-snapshots-returns-inventory, ds.snapshot-create-delete-lifecycle, ds.ip-route-add-remove-lifecycle, ds.ip-route-duplicate-cidr-rejected, ds.list-log-subscriptions-returns-inventory, ds.state-machine-trust-create-verify-delete-404.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, total 7/66 passed (10.6%); ds 5/51 (10%), 46 failed. Checks marked ALREADY FILED (DescribeCAEnrollmentPolicy, CreateInternetGateway, CreateConditionalForwarder, CreateComputer, ShareDirectory, ListCertificates, RegisterCertificate, CreateLogSubscription, CreateTrust stubs) and OWNED (ds.concurrent-add-ip-routes-conflict-detected, ds.ds-data-is-a-distinct-validated-surface) were skipped.

Cause 1 — expected vs actual (status 400 and error code matched in all four):
- ds.enum-wire-values-precede-resource-resolution: AssertionError "DescribeLDAPSSettings(Type='Client') message: \"Directory: 'd-8cd06b93e3' does not exist.\" != \"Directory: 'd-8cd06b93e3' does not exist. : RequestId: None\"" (the expected string is built from the response's own RequestId, which the emulator left absent).
- ds.list-ad-assessments-returns-inventory: "DescribeADAssessment of the deleted assessment must refuse with 'Assessment Id da-000de0bfaf373d85ee does not exist. : RequestId: None', got 'Assessment Id da-000de0bfaf373d85ee does not exist.'"
- ds.register-event-topic-absent-directory-rejected: "RegisterEventTopic must refuse with \"Directory: 'd-0be95295bd' does not exist. : RequestId: None\", got \"Directory: 'd-0be95295bd' does not exist.\""
- ds.tag-limit-exceeded-rejected: "the over-limit tag must be refused with ds's exact message 'Tag limit exceeded. : RequestId: None', got 'A resource may carry at most 50 tags; the request would leave 51'".
Smithy (model_path services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): EntityDoesNotExistException / DirectoryDoesNotExistException / TagLimitExceededException each = {Message: ExceptionMessage, RequestId: @pattern UUID}, errorKind client, httpError 400.

Cause 2 — same check as above; emulator message is authored in src/ops/add_tags_to_resource.rs (the ds state module's MAX_TAGS = 50 bound), not ds's fixed 'Tag limit exceeded.'

Cause 3 — ds.disable-sso-without-enable-is-an-idempotent-no-op: AssertionError "expected DisableSso on a never-enabled directory to succeed as a no-op on real AWS, got NotImplemented". Smithy DisableSso: input {DirectoryId required, UserName, Password}, output DisableSsoResult (empty), errors AuthenticationFailedException/ClientException/EntityDoesNotExistException/InsufficientPermissionsException/ServiceException.

Cause 4 — ds.remove-region-access-denied: AssertionError "expected AccessDeniedException for the IAM-denied caller, got 'DirectoryDoesNotExistException'". Smithy RemoveRegion errors include AccessDeniedException (client, 400).

Cause 5 — ds.microsoft-ad-create-delete-lifecycle: botocore.errorfactory.EntityDoesNotExistException "An error occurred (EntityDoesNotExistException) when calling the DescribeDirectories operation: Directory: 'd-003ba5fe5a' does not exist." raised from the check's post-DeleteDirectory wait for Stage in (Deleting, Deleted) (timeout 60 s, interval 5 s).

Cause 6 — src/ops/add_ip_routes.rs duplicate-block branch returns AddIpRoutesErrorModeled::EntityAlreadyExistsException("Directory {id} already routes the address block {block}"); ds.ip-route-duplicate-cidr-rejected asserts code == 'ClientException' with message 'The IP route(s) [<cidr>] already exist. If you want to recreate them, please delete the existing routes first. : RequestId: <id>' and documents that this was confirmed against real AWS ("not EntityAlreadyExistsException -- confirmed against real AWS"). Smithy AddIpRoutes models both classes, so the model does not disambiguate; the recording does.

Cause 7 — all six report exactly 'Error: Check timed out after 60.0s' with no ClientError. src/state.rs: TRANSITION_SPAN = Duration::from_millis(500) governs Requested->Active, Creating->Completed and Adding->Added, so the polled ds statuses are not what takes minutes. Contrast: ds.disable-sso-without-enable-is-an-idempotent-no-op creates a VPC + two subnets + a Microsoft AD with NO waiter/tag-filter loop and reaches its assertion inside the window; every timed-out check goes through a shared fixture that does DescribeVpcs(Filters=[{'Name': 'tag:Name', ...}]) retry loops and/or ec2 get_waiter('vpc_available')/get_waiter('subnet_available'). Sibling evidence that this emulator's DescribeVpcs filtering diverges: ds.concurrent-add-ip-routes-conflict-detected failed with 'An error occurred (InvalidParameterValue) when calling the DescribeVpcs operation: The filter 'isDefault' is invalid'.

## Steps to Reproduce
1. git_clone the repo, then run_behavioral_suite() to build and boot the emulator and replay the ds suite.
2. Cause 1: call any ds operation on an absent entity (e.g. DescribeLDAPSSettings(DirectoryId='d-8cd06b93e3', Type='Client'), DescribeADAssessment(AssessmentId='da-0000000000'), RegisterEventTopic on an absent DirectoryId). Inspect the 400 body: no RequestId member, and Message has no ' : RequestId: <id>' suffix. Re-replay ds.enum-wire-values-precede-resource-resolution, ds.list-ad-assessments-returns-inventory, ds.register-event-topic-absent-directory-rejected.
3. Cause 2: AddTagsToResource with 51 tags on a directory. Expect 400 TagLimitExceededException 'Tag limit exceeded. : RequestId: <id>'; observe the emulator's own 'A resource may carry at most 50 tags; the request would leave 51'. Re-replay ds.tag-limit-exceeded-rejected.
4. Cause 3: CreateMicrosoftAD, then DisableSso(DirectoryId=...) without ever calling EnableSso. Expect 200 with an empty body; observe NotImplemented. Re-replay ds.disable-sso-without-enable-is-an-idempotent-no-op.
5. Cause 4: call RemoveRegion as a principal whose policy denies ds:RemoveRegion. Expect AccessDeniedException; observe DirectoryDoesNotExistException. Re-replay ds.remove-region-access-denied.
6. Cause 5: CreateMicrosoftAD, wait for Active, DeleteDirectory, then DescribeDirectories(DirectoryIds=[id]). Expect a description with Stage=Deleting (or Deleted); observe EntityDoesNotExistException. Re-replay ds.microsoft-ad-create-delete-lifecycle.
7. Cause 6: AddIpRoutes with a CIDR, wait for Added, then AddIpRoutes with the same CIDR. Expect 400 ClientException 'The IP route(s) [<cidr>] already exist. ...'; observe 400 EntityAlreadyExistsException. Re-replay ds.ip-route-duplicate-cidr-rejected.
8. Cause 7: re-replay ds.describe-snapshots-returns-inventory, ds.snapshot-create-delete-lifecycle, ds.ip-route-add-remove-lifecycle, ds.ip-route-duplicate-cidr-rejected, ds.list-log-subscriptions-returns-inventory and ds.state-machine-trust-create-verify-delete-404 with per-call logging to find which wait never returns (candidates: the fixture's tag-filtered DescribeVpcs / vpc_available / subnet_available waits, DescribeSnapshots Status, ListIpRoutes IpRouteStatusMsg, ListLogSubscriptions inventory, DescribeTrusts TrustState).
9. After fixing, re-replay every check id listed in this issue's trace ids — each cause has its own.
