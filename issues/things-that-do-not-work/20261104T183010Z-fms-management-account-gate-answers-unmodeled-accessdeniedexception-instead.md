Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fms.get-admin-scope-malformed-account-rejected fms.list-admin-accounts-for-organization-unconfigured-not-found
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms.get-admin-scope-malformed-account-rejected|run#8|AssertionError|assert notfound_code == "InvalidOperationException", (
Issue-Key: fms.list-admin-accounts-for-organization-unconfigured-not-found|run#7|AssertionError|assert code == "InvalidOperationException", f"expected InvalidOperationException, got {code}"
Issue-Commit: e5b2e36fc94a41937ab22fe33196324420972c76

# FMS management-account gate answers unmodeled AccessDeniedException instead of the modeled InvalidOperationException on GetAdminScope and ListAdminAccountsForOrganization

> _Note: this issue was identified against the code base at commit `e5b2e36fc94a41937ab22fe33196324420972c76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAdminScope

## Description
Two causes, one shared mechanism: two organization-scoped Firewall Manager operations run the emulator's `require_administrator_at` gate and, when the store holds no administrator association, refuse with the error code `AccessDeniedException` and the message "No default admin could be found for account <account-id> in Region <region>". Real AWS refuses these two organization-management-account-only calls with the MODELED `InvalidOperationException` naming the caller as not the organization's management account. A fixer must close BOTH causes before submitting: they are separate operations with separate handlers and separate gate call sites, and fixing one does not move the other.

Shared requirement for the refusal both causes must produce (HTTP 400, awsJson1_1, `$.__type` = "InvalidOperationException"), message template:
  "<caller-account-id> is not the management account of the Organization but is attempting to perform an action that only the Organization management account can perform. The Organization management account is <management-account-id>"
Fixed text (exactly, including the single spaces): " is not the management account of the Organization but is attempting to perform an action that only the Organization management account can perform. The Organization management account is ". Variable parts: the CALLER's account id (the credentials' account, leading the message) and the organization's management account id, which is the same value Organizations `DescribeOrganization` reports as `Organization.MasterAccountId`. Neither id is part of the requirement as a literal; any account ids seen in one replay are that environment's values.

=== Cause 1 of 2: GetAdminScope for a well-formed account id when the caller is not the organization's management account ===
AWS: Smithy (services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json) models GetAdminScope with errors InternalErrorException | InvalidInputException | InvalidOperationException | LimitExceededException | ResourceNotFoundException, all HTTP 400; `AccessDeniedException` is in NONE of them and the service declares no common error carrying it. GetAdminScope.AdminAccount targets AWSAccountId (@length 1..1024, @pattern ^[0-9]+$). com.amazonaws.fms#InvalidOperationException is documented "The operation failed because there was nothing to do or the operation wasn't possible" — the class AWS uses when the call itself is not possible for this caller, as opposed to the resource being absent. Recorded behavior: a GetAdminScope whose AdminAccount satisfies ^[0-9]+$ (a 12-digit id, here one that is not a registered administrator), issued by a caller that is not the organization's management account, is refused HTTP 400 with code "InvalidOperationException" and the management-account message above. The same check's positive control passes: a malformed AdminAccount ("not-an-account-id") is still correctly refused with ValidationException and the exact constraint message, so input validation is not at issue here — only the refusal class for a WELL-FORMED id.
Emulator: HTTP 400 with error code "AccessDeniedException" and message "No default admin could be found for account <account-id> in Region <region>" (fixed text "No default admin could be found for account " / " in Region ", with the emulator account id and the signing Region as variable parts). The gate runs before the account is examined, so no InvalidOperationException branch is ever reached.
Why: a client branching on the error code sees an authorization/credential failure where AWS reports that this call is not possible from this account and names the account that CAN make it. The two demand opposite handling — InvalidOperationException is a terminal, self-describing answer that tells the caller to re-issue the call from the management account it names, while AccessDeniedException is a permissions problem to escalate and carries no such pointer — so delegated-admin discovery and onboarding flows that on AWS learn where to retry instead retry blindly, re-sign, or abort. Worse, AccessDeniedException is not in GetAdminScope's modeled error union at all, so a generated SDK cannot deserialize it into any modeled variant and surfaces an opaque/unmodeled service error.
Fix: GetAdminScope's handler and the gate it calls. The management-account check belongs in the gate's refusal construction, which must emit the operation's modeled InvalidOperationException with the management-account message rather than an injected AccessDeniedException; the handler must keep ResourceNotFoundException only for a well-formed account that is genuinely no administrator resource for an entitled caller.
Traces: check fms.get-admin-scope-malformed-account-rejected. Gate call site services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_admin_scope.rs:199; the not-found branch below it at get_admin_scope.rs:213-218; refusal constructor and gate in src/state.rs (`no_administrator_refusal` / `require_administrator_at`).

=== Cause 2 of 2: ListAdminAccountsForOrganization when the caller is not the organization's management account ===
AWS: Smithy models ListAdminAccountsForOrganization with errors InternalErrorException | InvalidOperationException | LimitExceededException | ResourceNotFoundException (HTTP 400); `AccessDeniedException` is not among them. The operation's own modeled documentation states the rule: "This operation can be called only from the organization's management account." Recorded behavior: called by an account that is not the organization's management account, it is refused HTTP 400 with code "InvalidOperationException" and the management-account message above. The same check's positive control passes: GetAdminAccount in the same unconfigured store correctly answers ResourceNotFoundException with an empty Message, so the store state itself is right and only this operation's refusal class diverges.
Emulator: HTTP 400 with error code "AccessDeniedException" and the "No default admin could be found for account <account-id> in Region <region>" message, produced by the gate before any pagination token or administrator set is examined.
Why: an enumerating client cannot distinguish "this call must be made from the management account, which is <id>" from "your credentials are denied". It loses the one piece of actionable information AWS supplies — the management account to call from — and receives a code outside the operation's four-error union that no SDK models for it, turning a recoverable control-plane answer into an opaque authorization failure.
Fix: ListAdminAccountsForOrganization's handler and the same gate; the organization-scoped refusal must be the modeled InvalidOperationException carrying the management-account message, not an injected AccessDeniedException.
Traces: check fms.list-admin-accounts-for-organization-unconfigured-not-found. Gate call site services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_admin_accounts_for_organization.rs:181.

Related but NOT covering: open issue 20261029T235353Z-fms-administrator-gate-answers-unmodeled-accessdeniedexception-instead-of.md concerns the same gate but a DIFFERENT required outcome and a different operation set — it requires ResourceNotFoundException with the fixed text "The referenced item does not exist." on GetAdminScope, ListComplianceStatus, GetComplianceDetail, GetProtectionStatus and GetViolationDetails. Neither cause here is satisfied by that: these two calls must answer InvalidOperationException with the management-account message, and ListAdminAccountsForOrganization is not in that issue's set at all. Closing that issue by making the gate answer ResourceNotFoundException would leave both causes here still diverging, so the two must be reconciled: the gate's refusal class depends on the operation.

## Evidence
Both causes: the emulator's HTTP status (400) matches; the awsJson1_1 `$.__type` error code and `$.Message` diverge.

Cause 1 — check fms.get-admin-scope-malformed-account-rejected (fms.GetAdminScope with AdminAccount a well-formed 12-digit id):
  expected code: "InvalidOperationException"
  actual code:   "AccessDeniedException"
  replay assertion: AssertionError: expected InvalidOperationException for well-formed unregistered account, got AccessDeniedException
  expected message: "<caller-account> is not the management account of the Organization but is attempting to perform an action that only the Organization management account can perform. The Organization management account is <Organization.MasterAccountId from organizations DescribeOrganization>"
  actual message:   "No default admin could be found for account <emulator account id> in Region <signing region>"
  Passing sub-assertion in the same check (NOT part of this defect): AdminAccount="not-an-account-id" is correctly refused with ValidationException and message "1 validation error detected: Value 'not-an-account-id' at 'adminAccount' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$".

Cause 2 — check fms.list-admin-accounts-for-organization-unconfigured-not-found (fms.ListAdminAccountsForOrganization, no arguments, store with no administrator association):
  expected code: "InvalidOperationException"
  actual code:   "AccessDeniedException"
  replay assertion: AssertionError: expected InvalidOperationException, got AccessDeniedException
  expected message: the same management-account template as Cause 1
  actual message:   "No default admin could be found for account <emulator account id> in Region <signing region>"
  Passing sub-assertion in the same check (NOT part of this defect): GetAdminAccount answers ResourceNotFoundException with an empty Message.

Model evidence that the emitted code is unmodeled (services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json):
  GetAdminScope                    errors: InternalErrorException, InvalidInputException, InvalidOperationException, LimitExceededException, ResourceNotFoundException
  ListAdminAccountsForOrganization errors: InternalErrorException, InvalidOperationException, LimitExceededException, ResourceNotFoundException
  AccessDeniedException appears in neither union nor in the service's common errors.

Source of the emulator's behavior: the gate `crate::state::require_administrator_at` returns the AccessDeniedException refusal built by `crate::state::no_administrator_refusal` (via crate::scaffolded::Scaffolded::access_denied); it is called at src/ops/get_admin_scope.rs:199 and src/ops/list_admin_accounts_for_organization.rs:181, in both cases before the operation's own logic. Account ids and Regions appearing in any one replay are per-environment values, not requirements; the requirements are the error code and the message's fixed text with the caller/management account ids filled in from the request credentials and Organizations state.

## Steps to Reproduce
Preconditions for both causes: boot the emulator with a fresh Firewall Manager store (no AssociateAdminAccount / PutAdminAccount, so no administrator association exists) and a caller that is not the organization's management account as reported by organizations DescribeOrganization -> Organization.MasterAccountId.

Cause 1:
1. Call organizations DescribeOrganization and note Organization.MasterAccountId.
2. Call fms GetAdminScope with AdminAccount="not-an-account-id" -> ValidationException with the exact pattern message (passes today; keep it passing).
3. Call fms GetAdminScope with AdminAccount set to a well-formed 12-digit account id that is not a registered administrator (e.g. any id matching ^[0-9]+$).
4. Observe HTTP 400 with code "AccessDeniedException" and message "No default admin could be found for account ... in Region ...". AWS answers HTTP 400 "InvalidOperationException" with "<caller account> is not the management account of the Organization but is attempting to perform an action that only the Organization management account can perform. The Organization management account is <MasterAccountId>".
5. Equivalently, replay behavioral check fms.get-admin-scope-malformed-account-rejected.

Cause 2:
1. Call fms GetAdminAccount -> ResourceNotFoundException with an empty Message (passes today; keep it passing).
2. Call organizations DescribeOrganization and note Organization.MasterAccountId.
3. Call fms ListAdminAccountsForOrganization with no arguments.
4. Observe HTTP 400 with code "AccessDeniedException" and the "No default admin could be found ..." message; AWS answers HTTP 400 "InvalidOperationException" with the management-account message above.
5. Equivalently, replay behavioral check fms.list-admin-accounts-for-organization-unconfigured-not-found.

Full check: re-replay both behavioral checks fms.get-admin-scope-malformed-account-rejected and fms.list-admin-accounts-for-organization-unconfigured-not-found; both must pass, with the ValidationException and GetAdminAccount positive controls still passing.
