Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: account.start-primary-email-update-self-target-denied
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account.start-primary-email-update-self-target-denied|_require_no_organization#1|RuntimeError|raise RuntimeError(
Issue-Commit: 7a3273ef077d4460acbc0a5962255d7dc076ff50

# organizations DescribeOrganization reports an organization on a freshly booted emulator, so the emulated account is never standalone and account's self-target StartPrimaryEmailUpdate denial is unobservable

> _Note: this issue was identified against the code base at commit `7a3273ef077d4460acbc0a5962255d7dc076ff50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOrganization

## Description
One root cause.

Cause 1 of 1: the Organizations control plane establishes organization membership at mount time, so no caller can ever observe an account that belongs to no organization. Failing check: account.start-primary-email-update-self-target-denied.

AWS: Organizations is opt-in per account. An account that is a member of no organization answers every organization-scoped read with the modeled AWSOrganizationsNotInUseException. Smithy: DescribeOrganization input smithy.api#Unit, output DescribeOrganizationResponse{Organization}, errors AccessDeniedException (403), AWSOrganizationsNotInUseException (404), ConcurrentModificationException (409), ServiceException (500), TooManyRequestsException (429); AWSOrganizationsNotInUseException documentation: "Your account isn't a member of an organization. To make this request, you must use the credentials of an account that belongs to an organization." That sentence is entirely fixed text — the error carries no variable parts taken from the request or state. Membership comes into existence only through CreateOrganization (whose own refusal for a second create is AlreadyInOrganizationException); there is no implicit membership an account is born with, so a never-created organization means DescribeOrganization is a 404 AWSOrganizationsNotInUseException, not a 200.

Emulator: on a freshly booted emulator, with no CreateOrganization ever called over the wire, DescribeOrganization answers 200 with a fully populated Organization. The organization is written during service construction: services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/service.rs, EmulatedService::new, takes the store lock and calls store.create_organization(DEFAULT_FEATURE_SET) whenever the organization slot is None, before any request is served. The handler's own refusal arm (src/ops/describe_organization.rs, the organization.as_ref().ok_or_else(...) that raises AWSOrganizationsNotInUseException with state::NOT_IN_USE_MESSAGE) is therefore dead code at runtime: the slot is never None once the service is mounted. The emulator's wording of that refusal is correct; it is simply unreachable.

Why: a client cannot determine whether the emulated account is standalone. The idiomatic probe — call DescribeOrganization and treat AWSOrganizationsNotInUseException as "this account is not in an organization" — always takes the "in an organization" branch, so client logic that must choose between standalone and organization-member behaviour (consolidated billing, SCP-gated paths, management-account-only operations, enrolment flows that call CreateOrganization when the probe 404s) silently selects the organization path and, for CreateOrganization, is refused AlreadyInOrganizationException for an organization it never made. The same unconditional membership makes a whole class of standalone-identity behaviour unobservable: the account check account.start-primary-email-update-self-target-denied, which verifies that StartPrimaryEmailUpdate targeting the caller's own AccountId is refused AccessDeniedException and starts no update (GetPrimaryEmailUpdateStatus then answers ValidationException), never issues a single account call. It aborts in its own precondition because DescribeOrganization succeeded, so the account service's self-target rejection path, and the positive control that GetAccountInformation keeps working after it, are both left unexercised by this suite.

Fix: the Organizations service's construction seam — EmulatedService::new in the organizations crate's src/service.rs — should not establish membership. The organization slot should stay None until a CreateOrganization request writes it through state::create_organization, leaving DescribeOrganization (and the sibling organization-scoped reads such as ListRoots) to serve their existing AWSOrganizationsNotInUseException arm until then. No change is needed in the account crate itself; nothing about this cause tells us whether account StartPrimaryEmailUpdate's self-target gate is correct, only that it is unreachable from this suite. Related but not covering: the open issue 20261024T045911Z-billingconductor-createbillinggroup-event-propagation.md notes CreateOrganization is never observed for the payer path; it does not address the mount-time seed or DescribeOrganization's status.

Traces: account.start-primary-email-update-self-target-denied (verdict error, RuntimeError raised in the check's _require_no_organization precondition after organizations.describe_organization() returned success). Code citations: organizations crate src/service.rs lines 80-90 (the seed), src/ops/describe_organization.rs lines 133-143 (the unreachable refusal arm), src/state.rs line 623 (NOT_IN_USE_MESSAGE) and line 397 (create_organization).

## Evidence
Expected (real AWS, account that is a member of no organization):
  Organizations DescribeOrganization -> HTTP 404, Error.Code "AWSOrganizationsNotInUseException", message (all fixed text, no per-request values): "Your account isn't a member of an organization. To make this request, you must use the credentials of an account that belongs to an organization."
  With that precondition satisfied, the check then proceeds: account StartPrimaryEmailUpdate{AccountId=<caller's own account id>, PrimaryEmail=<any value>} -> AccessDeniedException, and account GetPrimaryEmailUpdateStatus -> ValidationException (no update was started), and account GetAccountInformation still returns the same AccountId.

Actual (emulator, freshly booted, no CreateOrganization ever sent):
  Organizations DescribeOrganization -> HTTP 200 with an Organization structure (id/arn/featureSet=ALL/master account fields, seeded at mount by EmulatedService::new). No AWSOrganizationsNotInUseException is ever produced.
  Consequence in the replay: check account.start-primary-email-update-self-target-denied terminates with
    File "check_account_start_primary_email_update_self_target_denied.py", line 20, in run
      _require_no_organization(ctx.client("organizations"))
    File "check_account_start_primary_email_update_self_target_denied.py", line 63, in _require_no_organization
      raise RuntimeError("precondition: the caller belongs to an organization; this check runs only in an account that belongs to none")
  so zero account operations were invoked and the self-target denial was never evaluated.

Source of the divergence (not a per-run value):
  services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/service.rs:80-90 — EmulatedService::new: `if store.organization.is_none() { store.create_organization(crate::state::DEFAULT_FEATURE_SET); }`, executed once at mount, before any request.

## Steps to Reproduce
1. Boot the emulator fresh (no prior requests, no CreateOrganization).
2. Call Organizations DescribeOrganization with no input.
   Expected: HTTP 404 AWSOrganizationsNotInUseException "Your account isn't a member of an organization. To make this request, you must use the credentials of an account that belongs to an organization."
   Actual: HTTP 200 with a populated Organization (featureSet ALL).
3. Call Organizations CreateOrganization: it is refused AlreadyInOrganizationException, confirming the membership was established without any client request.
4. Replay the behavioral check account.start-primary-email-update-self-target-denied: it aborts in _require_no_organization on step 2's success and issues no account call, so account StartPrimaryEmailUpdate's self-target AccessDeniedException and the follow-up GetPrimaryEmailUpdateStatus ValidationException are never exercised.
