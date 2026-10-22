Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-admin.account-assignment-full-lifecycle sso-admin.application-provider-not-found-describe-rejected sso-admin.create-instance-duplicate-singleton-conflict-rejected
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin.account-assignment-full-lifecycle|run#14|AssertionError|assert create_code == "ValidationException", create_code
Issue-Key: sso-admin.application-provider-not-found-describe-rejected|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: sso-admin.create-instance-duplicate-singleton-conflict-rejected|run#7|AssertionError|assert code == "ServiceQuotaExceededException", f"expected ServiceQuotaExceededException, got {code!r}"
Issue-Commit: 692676341d77292a02c0c0941a8d422bd5873aeb

# sso-admin: 3 divergences — account-instance restriction not modeled on the account-assignment surface, DescribeApplicationProvider and CreateInstance answer 501 NotImplemented

> _Note: this issue was identified against the code base at commit `692676341d77292a02c0c0941a8d422bd5873aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccountAssignment

## Description
Three distinct root causes in the IAM Identity Center (sso-admin) emulator, all grounded in failing behavioral checks. Each must be closed before this issue is submitted.

Cause 1 of 3: the account-assignment / permission-set surface does not model the account-instance restriction, so refusals are misclassified as ResourceNotFoundException.
Failing check: sso-admin.account-assignment-full-lifecycle
AWS: Smithy (services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json) models ValidationException (HTTP 400, "The request failed because it contains a syntax error") on CreateAccountAssignment, DeleteAccountAssignment, DescribeAccountAssignmentCreationStatus, DescribeAccountAssignmentDeletionStatus and ListAccountAssignments, alongside ResourceNotFoundException (HTTP 404). The CreateInstance model documentation states an instance created for a standalone account that is not managed by Organizations is an *account* instance; such an instance supports no permission sets, so the whole permission-set/account-assignment surface is withdrawn on it. Recorded behavior for this account: every one of those five operations is rejected with error code ValidationException and the documented message "This operation is not supported for account instances of IAM Identity Center." (that fixed message is the documented account-instance refusal text; the requirement is the 400/ValidationException classification, the text is AWS's documented example).
Emulator: CreateAccountAssignment answered error code ResourceNotFoundException instead — the check asserted `create_code == "ValidationException"` and got `ResourceNotFoundException`. The state store seeds exactly one instance with permission sets unconditionally enabled (services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs: SEED_INSTANCE_ID, SEED_PERMISSION_SETS_ENABLED = true) and carries no instance-type (account vs organization) attribute at all, so the handler falls straight through to its "no permission set with that ARN" 404 path. The same gap exists on the other four operations of the surface, which the check asserts immediately after the first failing assertion.
Why: a client that branches on the error — treating ValidationException as "this instance type cannot do assignments at all, stop" and ResourceNotFoundException as "create the permission set first and retry" — takes the wrong branch, retries forever, or concludes the permission set merely needs creating on an instance where no permission set can ever exist. The HTTP status also diverges (404 vs 400), so status-based retry/alarm logic misclassifies a permanent restriction as a missing resource.
Fix: model the instance type in the service state and gate the permission-set/account-assignment surface on it, then return the modeled ValidationException from each of the five operations' handlers (create_account_assignment.rs, delete_account_assignment.rs, describe_account_assignment_creation_status.rs, describe_account_assignment_deletion_status.rs, list_account_assignments.rs) when the resolved instance is an account instance — the restriction check must precede the existence checks.
Traces: sso-admin.account-assignment-full-lifecycle (assertion at check line 56: expected ValidationException from CreateAccountAssignment, got ResourceNotFoundException).

Cause 2 of 3: DescribeApplicationProvider is an unimplemented scaffold stub and answers 501 NotImplemented for every input.
Failing check: sso-admin.application-provider-not-found-describe-rejected
AWS: Smithy DescribeApplicationProvider takes required ApplicationProviderArn (len 10..1224, pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$) and models ResourceNotFoundException (HTTP 404, "Indicates that a requested resource is not found"); on success it returns ApplicationProviderArn (required) plus optional DisplayData, FederationProtocol and ResourceServerConfig. Recorded behavior: a syntactically valid but non-built-in provider ARN is rejected with ResourceNotFoundException (recorded message example: "No application provider with arn '<arn>' was found"), while a built-in provider ARN such as arn:aws:sso::aws:applicationProvider/custom describes successfully and echoes back the same ApplicationProviderArn.
Emulator: the check asserted `code == "ResourceNotFoundException"` and got `NotImplemented` — a 501 from the scaffold stub, so both the negative path and the built-in positive path are unavailable.
Why: a client cannot distinguish a nonexistent provider from an existing one, and a 501 is not a modeled error for this operation, so SDK error handling (which only expects the modeled client errors) sees an unmodeled server failure and either retries a permanently failing call or aborts; no caller can enumerate or validate application providers.
Fix: implement the handler in services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/describe_application_provider.rs — back it with the set of built-in application providers in src/state.rs, return the modeled DescribeApplicationProviderOutput for a known provider ARN and ResourceNotFoundException for an unknown one, and delete the SCAFFOLDED marker.
Traces: sso-admin.application-provider-not-found-describe-rejected (assertion at check line 20).

Cause 3 of 3: CreateInstance is an unimplemented scaffold stub, so the one-instance-per-account singleton quota is never enforced.
Failing check: sso-admin.create-instance-duplicate-singleton-conflict-rejected
AWS: Smithy CreateInstance models ServiceQuotaExceededException (HTTP 402, "Indicates that the principal has crossed the permitted number of resources that can be created") and ConflictException (HTTP 409); its documentation states only one instance may exist per account across all Regions and that the request is rejected when an instance already exists in the same account. Recorded behavior: with the sole instance already present, CreateInstance is rejected with error code ServiceQuotaExceededException and the existing instance is left untouched (ListInstances still returns exactly one entry at the same InstanceArn, still ACTIVE).
Emulator: the check asserted `code == "ServiceQuotaExceededException"` and got `NotImplemented` — the handler is still the scaffold stub returning 501 (services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_instance.rs line 187). The emulator seeds its single instance in state instead, so the duplicate-create refusal path does not exist.
Why: a client provisioning Identity Center cannot tell "an instance already exists, adopt it" (402/ServiceQuotaExceededException) from "the service is unavailable" (501). Bootstrap logic that creates-then-adopts fails outright, and a 501 is not a modeled error for the operation so SDK retry classification is wrong too.
Fix: implement services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_instance.rs — on a request made while the account already holds an instance return the modeled ServiceQuotaExceededException, otherwise create the instance in src/state.rs (honoring the @idempotencyToken ClientToken and Name/Tags inputs) and return its InstanceArn; delete the SCAFFOLDED marker.
Traces: sso-admin.create-instance-duplicate-singleton-conflict-rejected (assertion at check line 28).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), sso-admin 0/23 passed.

Cause 1 — sso-admin.account-assignment-full-lifecycle [mismatch]:
  check_sso_admin_account_assignment_full_lifecycle.py, line 56:
    assert create_code == "ValidationException", create_code
  AssertionError: ResourceNotFoundException
  Expected: CreateAccountAssignment (and DeleteAccountAssignment, DescribeAccountAssignmentCreationStatus, DescribeAccountAssignmentDeletionStatus, ListAccountAssignments) -> HTTP 400 ValidationException, message "This operation is not supported for account instances of IAM Identity Center."
  Actual: HTTP 404 ResourceNotFoundException (handler's "No permission set found with the ARN ..." path, create_account_assignment.rs lines 208-222).

Cause 2 — sso-admin.application-provider-not-found-describe-rejected [mismatch]:
  check_sso_admin_application_provider_not_found_describe_rejected.py, line 20:
    assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
  AssertionError: expected ResourceNotFoundException, got 'NotImplemented'
  Expected: bogus arn:aws:sso::aws:applicationProvider/... -> HTTP 404 ResourceNotFoundException; built-in arn:aws:sso::aws:applicationProvider/custom -> HTTP 200 echoing ApplicationProviderArn.
  Actual: HTTP 501 NotImplemented (describe_application_provider.rs line 173, SCAFFOLDED marker at line 4).

Cause 3 — sso-admin.create-instance-duplicate-singleton-conflict-rejected [mismatch]:
  check_sso_admin_create_instance_duplicate_singleton_conflict_rejected.py, line 28:
    assert code == "ServiceQuotaExceededException", f"expected ServiceQuotaExceededException, got {code!r}"
  AssertionError: expected ServiceQuotaExceededException, got 'NotImplemented'
  Expected: CreateInstance with an instance already present -> HTTP 402 ServiceQuotaExceededException, sole instance unchanged.
  Actual: HTTP 501 NotImplemented (create_instance.rs line 187, SCAFFOLDED marker at line 4).

Smithy model consulted: services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json (operations CreateAccountAssignment, DeleteAccountAssignment, DescribeAccountAssignmentCreationStatus, DescribeAccountAssignmentDeletionStatus, ListAccountAssignments, DescribeApplicationProvider, CreateInstance and their modeled errors).

## Steps to Reproduce
1. Boot the emulator and point an sso-admin client at it.

Cause 1:
2. instance_arn = sso.list_instances()["Instances"][0]["InstanceArn"]; instance_id = instance_arn.rsplit("/", 1)[-1].
3. Call sso.create_account_assignment(InstanceArn=instance_arn, TargetId=<12-digit account id>, TargetType="AWS_ACCOUNT", PermissionSetArn=f"arn:aws:sso:::permissionSet/{instance_id}/ps-0123456789abcdef", PrincipalType="GROUP", PrincipalId=<identitystore group id>).
4. Observe ResourceNotFoundException; AWS answers ValidationException (HTTP 400) with the account-instance message. Repeat with delete_account_assignment, describe_account_assignment_creation_status, describe_account_assignment_deletion_status and list_account_assignments for the same surface.

Cause 2:
5. Call sso.describe_application_provider(ApplicationProviderArn="arn:aws:sso::aws:applicationProvider/sim2real-behav-not-a-real-provider") -> 501 NotImplemented instead of 404 ResourceNotFoundException.
6. Call sso.describe_application_provider(ApplicationProviderArn="arn:aws:sso::aws:applicationProvider/custom") -> 501 NotImplemented instead of a 200 echoing that ARN.

Cause 3:
7. With exactly one instance listed by sso.list_instances(), call sso.create_instance(Name="dup-attempt") -> 501 NotImplemented instead of 402 ServiceQuotaExceededException, and confirm ListInstances still returns the single original instance.

Replay: sso-admin.account-assignment-full-lifecycle, sso-admin.application-provider-not-found-describe-rejected, sso-admin.create-instance-duplicate-singleton-conflict-rejected.
