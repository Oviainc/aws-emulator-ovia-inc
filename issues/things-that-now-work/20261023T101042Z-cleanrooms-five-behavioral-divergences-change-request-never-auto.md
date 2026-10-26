Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.collaboration-change-request-reapprove-rejected cleanrooms.configured-table-association-role-arn-pattern-rejected cleanrooms.create-membership-duplicate-conflict cleanrooms.create-privacy-budget-template-duplicate-conflict cleanrooms.delete-collaboration-cascades-membership-status
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms.collaboration-change-request-reapprove-rejected|run#10|AssertionError|assert status_before == "COMMITTED", status_before
Issue-Key: cleanrooms.configured-table-association-role-arn-pattern-rejected|run#19|AssertionError|assert error["Code"] == "AccessDeniedException", error
Issue-Key: cleanrooms.create-membership-duplicate-conflict|run#10|AssertionError|assert error["Message"] == (
Issue-Key: cleanrooms.create-privacy-budget-template-duplicate-conflict|run#13|AssertionError|assert error.get("Error", {}).get("Message") == (
Issue-Key: cleanrooms.delete-collaboration-cascades-membership-status|run#11|AssertionError|assert error["Message"] == (
Issue-Commit: 3b7cd50bec204a941815a8ace6b2a1778291c65c

# cleanrooms: five behavioral divergences — change-request never auto-commits for a sole-member collaboration, roleArn syntax refused as ValidationException instead of AccessDeniedException, and three wrong rejection-message rules

> _Note: this issue was identified against the code base at commit `3b7cd50bec204a941815a8ace6b2a1778291c65c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCollaborationChangeRequest

## Description
This issue bundles 5 distinct root causes in the cleanrooms emulator, all surfaced by the behavioral suite. Each cause below is independent and must be closed separately; the bundle is only for filing economy. Affected operations: CreateCollaborationChangeRequest, CreateConfiguredTableAssociation, CreateMembership, CreatePrivacyBudgetTemplate, GetCollaboration (shared authorization-message helper).

No open issue covers these. The two open cleanrooms issues (20261022T200707Z-cleanrooms-createanalysistemplate-stub, 20261022T200707Z-cleanrooms-listtagsforresource-stub) are unimplemented-stub work items for CreateAnalysisTemplate / ListTagsForResource and are unrelated to the causes below.

=== Cause 1 of 5: CreateCollaborationChangeRequest leaves a sole-member collaboration's own change request PENDING instead of COMMITTED ===
Failing check: cleanrooms.collaboration-change-request-reapprove-rejected
AWS: recorded behavior only. Smithy (com.amazonaws.cleanrooms) models CollaborationChangeRequest$status as the required enum ChangeRequestStatus = PENDING|APPROVED|CANCELLED|DENIED|COMMITTED and CollaborationChangeRequest$isAutoApproved as a required Boolean documented as "Whether the change request was automatically approved based on the collaboration's auto-approval settings"; the model states no rule for when a request collapses straight to COMMITTED. The recording shows that in a collaboration whose only party is the creator, CreateCollaborationChangeRequest returns status=COMMITTED and isAutoApproved=true in the CreateCollaborationChangeRequest 200 response, EVEN when the collaboration was created with autoApprovedChangeRequestTypes=[] — i.e. the collapse is forced by there being no other party whose approval is outstanding, not by the auto-approval setting.
Emulator: the created request comes back status=PENDING (check asserted status_before == "COMMITTED", got "PENDING"), and the change is therefore never applied to the collaboration.
Why: a client that creates a change request in a single-member collaboration and then reads/acts on the committed result (the invited member appearing, or a subsequent update being refused because the request is terminal) sees a request stuck in PENDING forever — against the emulator there is no second party that could ever approve it, so the whole change-request workflow is unreachable and any state machine driven off status diverges permanently.
Fix: the auto-approval/commit decision for this operation's handler — it is computed purely from the collaboration's declared auto-approved change types and ignores how many parties' approvals are actually outstanding.
Traces: cleanrooms.collaboration-change-request-reapprove-rejected. Code sites: services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_collaboration_change_request.rs lines 331-372 (is_auto_approved gates both status and apply_collaboration_change) and src/state.rs lines 934-943 (change_request_is_auto_approved: only tests record.auto_approved_change_types) with src/state.rs lines 722-731 (change_request_approvals).

=== Cause 2 of 5: CreateConfiguredTableAssociation refuses a non-ARN roleArn with 400 ValidationException where AWS answers 403 AccessDeniedException ===
Failing check: cleanrooms.configured-table-association-role-arn-pattern-rejected
AWS: recorded behavior only (the Smithy model does carry a syntax constraint, but AWS does not enforce it as the rejection). Smithy: CreateConfiguredTableAssociation.roleArn -> RoleArn, required, @length min 32 max 512, @pattern ^arn:aws:iam::[\w]+:role/[\w+=./@-]+$; modeled errors include AccessDeniedException (HTTP 403) and ValidationException (HTTP 400). The recording shows that a value which passes the length constraint but is not a well-formed IAM role ARN is NOT rejected on syntax: Clean Rooms attempts to use the value as a role and the failure surfaces as error code AccessDeniedException (HTTP 403), with the recorded message "Cross-account pass role is not allowed." as an example.
Emulator: the request is refused before the handler by the modeled @pattern check, returning code ValidationException (HTTP 400) with the example message "1 validation error detected: Value 'not-an-arn-but-long-enough-to-pass-client-side-length-check-1234567890' at 'roleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:iam::[\w]+:role/[\w+=./@-]+$".
Why: a client that distinguishes "my input was malformed" (400 ValidationException, do not retry, fix the field) from "the role could not be assumed / pass-role denied" (403 AccessDeniedException, an IAM/trust-policy problem, commonly retried while IAM propagates) takes the wrong branch. The suite's own helper retries on these codes with a sleep for IAM propagation, so the status/code swap changes control flow, not just text.
Note for the fixer: this operation is additionally still a 501 scaffold (services/.../src/ops/create_configured_table_association.rs line 4 carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and line 188 returns Scaffolded::not_implemented), so the check's positive leg (creating an association with a well-formed role ARN) cannot pass until the operation is implemented; the pre-handler @pattern refusal is what the check observed because it never reached the stub.
Fix: this operation's roleArn validation — the syntax constraint must not be the refusal path; the operation's handler must classify an unusable roleArn as AccessDeniedException (403).
Traces: cleanrooms.configured-table-association-role-arn-pattern-rejected. Code site: services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_configured_table_association.rs (modeled-constraint pre-validation; handler body at lines 171-189).

=== Cause 3 of 5: CreateMembership duplicate-membership ValidationException states the wrong rule ===
Failing check: cleanrooms.create-membership-duplicate-conflict
AWS: recorded behavior only; the message rule IS known because it is composed from the request. Smithy: CreateMembership models ValidationException (HTTP 400) and ConflictException (HTTP 409). The recording shows a second CreateMembership for the same account/collaboration pair is rejected with code ValidationException and a message built as "Cannot create membership for account: <caller account id>. Memberships can only be created for members in INVITED state" — the rule is the caller's account id from the resolved request plus the fixed explanation that a membership is only creatable for a member in INVITED state.
Emulator: status and error code are correct (ValidationException), but the message asserts a different rule: "Cannot create membership for account: 000000000000. The account is already a member of collaboration: <collab id>" — it names the collaboration id and claims membership already exists rather than stating the INVITED-state precondition.
Why: the message is the only place the API states WHY the call is refused and what would make it succeed (the account must be in INVITED state). A client or operator reading the emulator's text concludes the pair is permanently closed rather than state-dependent, and any tooling that matches on the documented rule text fails to classify the refusal.
Fix: this operation's rejection-message construction (the shared helper the handler calls).
Traces: cleanrooms.create-membership-duplicate-conflict. Code site: services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/state.rs lines 1001-1006 (duplicate_membership_message), used by src/ops/create_membership.rs.

=== Cause 4 of 5: CreatePrivacyBudgetTemplate duplicate ConflictException states the wrong rule ===
Failing check: cleanrooms.create-privacy-budget-template-duplicate-conflict
AWS: recorded behavior only; the message rule is known because it is composed from the request's privacyBudgetType and the collaboration. Smithy: CreatePrivacyBudgetTemplate models ConflictException (HTTP 409) whose shape carries message, resourceId, resourceType and reason (ConflictExceptionReason = ALREADY_EXISTS|SUBRESOURCES_EXIST|INVALID_STATE). The recording shows a second template of the SAME privacyBudgetType is rejected with code ConflictException, HTTP 409, reason ALREADY_EXISTS, and a message built as "A Privacy Budget Template for Differential Privacy already exists in collaboration <collaboration id>." — i.e. the budget TYPE and the collaboration id are what the message names.
Emulator: code ConflictException and HTTP 409 are correct, but the message is "collaboration <id> already has privacy budget template <template id>; each collaboration can have only one privacy budget template" — it names the existing template id and asserts a collaboration-wide single-template rule instead of the per-budget-type rule.
Why: the stated rule is what tells a client whether a second template of a DIFFERENT budget type would be accepted. Hypothesis: the emulator's uniqueness index is keyed by collaboration alone (collaboration id -> template id), so it is not merely the wording — a second template of a different privacyBudgetType would also be refused, which AWS's per-type rule permits. The check stopped at the message assertion, so only the same-type rejection is directly evidenced.
Fix: this operation's conflict detection and its message construction — the uniqueness key should include privacyBudgetType, and the message should name the budget type and collaboration.
Traces: cleanrooms.create-privacy-budget-template-duplicate-conflict. Code sites: services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_privacy_budget_template.rs and src/state.rs lines 465-475 + 629-641 (privacy_budget_template_by_collaboration / privacy_budget_template_of_collaboration).

=== Cause 5 of 5: collaboration AccessDeniedException is not the IAM-style authorization message AWS returns ===
Failing check: cleanrooms.delete-collaboration-cascades-membership-status
AWS: recorded behavior only; the message rule is known because it is composed from the caller identity and the request. Smithy: GetCollaboration models AccessDeniedException (HTTP 403), ValidationException, ThrottlingException, InternalServerException and NOTABLY no ResourceNotFoundException — so a collaboration the caller can no longer reach is an authorization failure, as the recording shows. The recorded message follows the standard IAM authorization-failure rule: "User: <caller ARN as returned by sts:GetCallerIdentity> is not authorized to perform: cleanrooms:<OperationName> on resource: <target resource ARN, or * where the operation is not resource-scoped>" — for GetCollaboration on a deleted collaboration the resource is the collaboration ARN; for CreateMembership against it the resource is *.
Emulator: code AccessDeniedException is correct, but the message is the emulator's own form "Account 000000000000 is not authorized to access collaboration: <collab id>" for every operation — it names an account id rather than the caller principal ARN, and names neither the attempted cleanrooms:<Operation> nor the resource ARN.
Why: the IAM message is a machine-parsed contract: clients, SDK error helpers and operators extract the principal, the denied action and the resource from it to decide which policy to change. The emulator's text carries none of those three, so a client relying on AWS's authorization-failure rule cannot tell which action on which resource was denied. The divergence is shared by every operation that resolves a collaborationIdentifier, so fixing it once fixes all of them.
Fix: the shared authorization-denial message helper in this service's state, plus the per-operation call sites that must pass the operation name and the resource ARN (or *).
Traces: cleanrooms.delete-collaboration-cascades-membership-status (fails on GetCollaboration's message; the same check also asserts the CreateMembership-against-deleted-collaboration message). Code sites: services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/state.rs lines 979-983 (collaboration_access_denied_message) and its callers, e.g. src/ops/create_collaboration_change_request.rs lines 196-209.

## Evidence
Behavioral suite: cleanrooms 4/21 passed (19%), 17 failed. The 5 causes above are grounded in these 5 failing checks (the remaining failures are unimplemented-stub-blocked and already filed, or deferred as noted).

Cause 1 — cleanrooms.collaboration-change-request-reapprove-rejected [mismatch]
  Expected (AWS): CreateCollaborationChangeRequest in a creator-only collaboration created with autoApprovedChangeRequestTypes=[] returns status="COMMITTED", isAutoApproved=true.
  Actual (emulator): status="PENDING".
  check_cleanrooms_collaboration_change_request_reapprove_rejected.py line 61: assert status_before == "COMMITTED", status_before -> AssertionError: PENDING

Cause 2 — cleanrooms.configured-table-association-role-arn-pattern-rejected [mismatch]
  Expected (AWS): code AccessDeniedException, HTTP 403 (example message "Cross-account pass role is not allowed.").
  Actual (emulator): code ValidationException, HTTP 400, message (example) "1 validation error detected: Value 'not-an-arn-but-long-enough-to-pass-client-side-length-check-1234567890' at 'roleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:iam::[\w]+:role/[\w+=./@-]+$".
  check line 69: assert error["Code"] == "AccessDeniedException"

Cause 3 — cleanrooms.create-membership-duplicate-conflict [mismatch]
  Code matches (ValidationException). Expected message rule: "Cannot create membership for account: <account>. Memberships can only be created for members in INVITED state".
  Actual: "Cannot create membership for account: 000000000000. The account is already a member of collaboration: c5519783-e5d6-49c3-a84a-75d2a80a956a" (ids are examples).
  check line 48 message assertion.

Cause 4 — cleanrooms.create-privacy-budget-template-duplicate-conflict [mismatch]
  Code/status match (ConflictException, HTTP 409, reason ALREADY_EXISTS present). Expected message rule: "A Privacy Budget Template for Differential Privacy already exists in collaboration <collaboration id>."
  Actual: "collaboration 503b5e68-84f4-4e7d-8774-df1363e776a2 already has privacy budget template 7c4843dc-93db-4b52-93d9-b8bc0724b56f; each collaboration can have only one privacy budget template" (ids are examples).
  check line 62 message assertion.

Cause 5 — cleanrooms.delete-collaboration-cascades-membership-status [mismatch]
  Code matches (AccessDeniedException). Expected message rule: "User: <caller ARN> is not authorized to perform: cleanrooms:GetCollaboration on resource: <collaboration ARN>" (and, for the next leg, "...cleanrooms:CreateMembership on resource: *").
  Actual: "Account 000000000000 is not authorized to access collaboration: c89407f9-cdf0-4128-96dd-0bca2fb6fb29" (ids are examples).
  check line 47 message assertion.

Supporting emulator code read from the checkout: src/state.rs:934-943 (change_request_is_auto_approved), :979-983 (collaboration_access_denied_message), :1001-1006 (duplicate_membership_message), :465-475/:629-641 (privacy budget uniqueness keyed by collaboration only); src/ops/create_collaboration_change_request.rs:331-372; src/ops/create_configured_table_association.rs:4,188 (still a 501 scaffold).

## Steps to Reproduce
Boot the emulator and replay the five checks named in trace_ids (run_behavioral_suite), or reproduce each cause directly with a botocore cleanrooms client pointed at the emulator:

Cause 1: create_collaboration(members=[], creatorMemberAbilities=[CAN_QUERY, CAN_RECEIVE_RESULTS], queryLogStatus=DISABLED, autoApprovedChangeRequestTypes=[]); create_membership on it; then create_collaboration_change_request(changes=[{specificationType: MEMBER, specification: {member: {accountId: 111122223333, memberAbilities: [], displayName: invited-member}}}]). Inspect collaborationChangeRequest.status / .isAutoApproved: AWS returns COMMITTED/true, emulator returns PENDING.

Cause 2: create_configured_table_association(membershipIdentifier=<membership>, name=<alias>, configuredTableIdentifier=<table>, roleArn="not-an-arn-but-long-enough-to-pass-client-side-length-check-1234567890") — a value >=32 chars that is not an IAM role ARN. AWS: 403 AccessDeniedException. Emulator: 400 ValidationException from the modeled roleArn @pattern.

Cause 3: create_membership twice for the same collaboration with the same caller account; read the second call's Error.Message.

Cause 4: create_privacy_budget_template(privacyBudgetType=DIFFERENTIAL_PRIVACY, autoRefresh=NONE, parameters={differentialPrivacy:{epsilon:10,usersNoisePerQuery:30}}), then repeat with epsilon 5 / usersNoisePerQuery 50; read the ConflictException Message.

Cause 5: create_collaboration, create_membership, delete_collaboration, then get_collaboration(collaborationIdentifier=<deleted id>) and create_membership against the same id; compare each Error.Message with the IAM-style rule "User: <caller ARN> is not authorized to perform: cleanrooms:<Operation> on resource: <ARN|*>".
