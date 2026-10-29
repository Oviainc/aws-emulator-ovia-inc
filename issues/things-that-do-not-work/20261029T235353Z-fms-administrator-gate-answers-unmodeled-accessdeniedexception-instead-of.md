Issue-Source: trace-sweep-divergence
Issue-Trace: fms-admin-not-configured-reads
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fms-admin-not-configured-reads fms-policy-not-found-reads
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms|GetAdminScope|4xx->4xx|$.Message
Issue-Key: fms|GetAdminScope|4xx->4xx|$.__type
Issue-Key: fms|ListComplianceStatus|4xx->4xx|$.Message
Issue-Key: fms|ListComplianceStatus|4xx->4xx|$.__type
Issue-Key: fms|GetComplianceDetail|4xx->4xx|$.Message
Issue-Key: fms|GetComplianceDetail|4xx->4xx|$.__type
Issue-Key: fms|GetProtectionStatus|4xx->4xx|$.Message
Issue-Key: fms|GetProtectionStatus|4xx->4xx|$.__type
Issue-Key: fms|GetViolationDetails|4xx->4xx|$.Message
Issue-Key: fms|GetViolationDetails|4xx->4xx|$.__type
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# FMS administrator gate answers unmodeled AccessDeniedException instead of the modeled ResourceNotFoundException on five read operations

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAdminScope

## Description
Five distinct causes, one shared fault: five Firewall Manager read operations run the `require_administrator` / `require_administrator_at` administrator gate BEFORE their own lookup, so with no administrator association in the store they answer the error code `AccessDeniedException` — which this service's model declares for NONE of these five operations — in place of the `ResourceNotFoundException` the recordings show. A fixer must close ALL FIVE before submitting; they are separate operations with separate handlers and separate gate call sites, and fixing one does not move the others.

Shared mechanism (the same in each cause): crate::state::require_administrator_at() returns crate::state::no_administrator_refusal(), i.e. Scaffolded::access_denied("No default admin could be found for account {emulator_account_id} in Region {region}"). That constructor exists precisely BECAUSE no operation in this model declares AccessDeniedException (state.rs says so in its own doc comment), so it is injected outside every operation's modeled error union. state.rs further states the rule these five violate: "Which operations take this gate is decided PER OPERATION, by what each one answers, never by the resource it names: an operation whose recorded answer to a caller with no administrator association is the outcome of its own lookup is not gated here." The recordings put all five operations in exactly that second class, so the gate must not be applied to them; each must fall through to its own lookup and answer that lookup's modeled not-found.

On the expected message: $.Message is the service-wide not-found constant, whose text is entirely FIXED — "The referenced item does not exist." — with no variable parts, because this error never restates the identifier that was looked up (state.rs RESOURCE_NOT_FOUND_MESSAGE documents the same invariant from the recordings). On the actual message: "No default admin could be found for account <account-id> in Region <region>" has fixed text "No default admin could be found for account " / " in Region " with the calling account id and the request's signing Region as its variable parts; the specific account and Region in any one replay are per-environment values, not part of the requirement. The HTTP status is 400 on both sides in every cause; what diverges is the error code (the awsJson1_1 $.__type discriminator) and the message body.

=== Cause 1 of 5: GetAdminScope on an account that is no administrator ===
AWS: Smithy (services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json) models GetAdminScope with errors InternalErrorException | InvalidInputException | InvalidOperationException | LimitExceededException | ResourceNotFoundException, all HTTP 400, awsJson1_1; AccessDeniedException is NOT among them, and the service declares no common errors carrying it. com.amazonaws.fms#ResourceNotFoundException declares one member, Message (target ErrorMessage), "The specified resource was not found." Recorded behavior for a well-formed AdminAccount that names no administrator resource: HTTP 400, error code "ResourceNotFoundException", Message "The referenced item does not exist." The operation's own module doc already states this requirement: "An account that is neither is no administrator resource to report, which is the modelled ResourceNotFoundException."
Emulator: HTTP 400 with $.__type "AccessDeniedException" and $.Message "No default admin could be found for account <account-id> in Region <region>". The gate runs before the account is looked up, so the not-found branch below it is never reached when the store holds no administrator association.
Why: a client branching on the error code sees an authorization failure where AWS reports a missing resource. The two demand opposite handling — a not-found is a terminal answer about the named account (create/onboard it, or treat it as absent), an access-denied is a credential/permission problem to escalate and never a statement about the resource — so callers retry, re-sign, or fail onboarding flows that on AWS simply learn the account is not an administrator. Worse, the code is not in the operation's modeled error union at all, so a generated SDK cannot deserialize it into any modeled variant and surfaces an unmodeled/opaque service error.
Fix: GetAdminScope's handler must not take the administrator gate; it belongs in the class state.rs describes as answering from its own lookup. Remove that gate call from the handler and let the existing organization_admin_at miss produce the modeled ResourceNotFoundException.
Traces: fixture fms-admin-not-configured-reads step task#2. Gate call site services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_admin_scope.rs:199; the correct not-found branch it pre-empts is at get_admin_scope.rs:213-218; refusal constructor src/state.rs:2530-2535 and src/state.rs:2552-2562; not-found constant src/state.rs:2436-2449.

=== Cause 2 of 5: ListComplianceStatus for a policy id that names no stored policy ===
AWS: Smithy models ListComplianceStatus with errors InternalErrorException | ResourceNotFoundException ONLY — a two-error union, so AccessDeniedException is unmodeled here by an especially wide margin. Recorded behavior for a PolicyId matching the modeled pattern ^[a-z0-9A-Z-]{36}$ but naming no policy: HTTP 400, "ResourceNotFoundException", Message "The referenced item does not exist."
Emulator: HTTP 400, $.__type "AccessDeniedException", $.Message "No default admin could be found for account <account-id> in Region <region>".
Why: the paginating caller cannot distinguish "this policy does not exist" from "you may not call Firewall Manager", and receives an error code outside the operation's two-error union, which no SDK models for it. A compliance-reporting client that enumerates policies and tolerates not-found per policy instead aborts the whole sweep as an authorization failure.
Fix: ListComplianceStatus's handler must not take the administrator gate; drop the gate call so the existing store.policies miss answers the modeled ResourceNotFoundException.
Traces: fixture fms-policy-not-found-reads step task#3. Gate call site src/ops/list_compliance_status.rs:183; pre-empted not-found branch list_compliance_status.rs:187-192.

=== Cause 3 of 5: GetComplianceDetail for a policy id that names no stored policy ===
AWS: Smithy models GetComplianceDetail with errors InternalErrorException | InvalidInputException | InvalidOperationException | ResourceNotFoundException; AccessDeniedException is not among them. Recorded behavior for an unknown PolicyId (with a well-formed MemberAccount): HTTP 400, "ResourceNotFoundException", Message "The referenced item does not exist."
Emulator: HTTP 400, $.__type "AccessDeniedException", $.Message "No default admin could be found for account <account-id> in Region <region>".
Why: same misclassification — an unmodeled authorization code replaces the modeled resource verdict, so a client cannot tell a missing policy (or a member account outside the policy's scope, which AWS answers with the same not-found) from a denied call, and gets an error its SDK has no variant for.
Fix: GetComplianceDetail's handler must not take the administrator gate; let its own policy miss and its out-of-scope member-account check answer the modeled ResourceNotFoundException.
Traces: fixture fms-policy-not-found-reads step task#4. Gate call site src/ops/get_compliance_detail.rs:186; pre-empted not-found branches get_compliance_detail.rs:190-195 and 205-210.

=== Cause 4 of 5: GetProtectionStatus for a policy id that names no stored policy ===
AWS: Smithy models GetProtectionStatus with errors InternalErrorException | InvalidInputException | ResourceNotFoundException; AccessDeniedException is not among them. Recorded behavior for an unknown PolicyId: HTTP 400, "ResourceNotFoundException", Message "The referenced item does not exist."
Emulator: HTTP 400, $.__type "AccessDeniedException", $.Message "No default admin could be found for account <account-id> in Region <region>".
Why: as above; a DDoS/protection-status client that treats not-found as "policy gone, stop polling" instead sees an authorization error and keeps retrying with fresh credentials, and the code lies outside the operation's modeled union.
Fix: GetProtectionStatus's handler must not take the administrator gate; answer from its own policy lookup.
Traces: fixture fms-policy-not-found-reads step task#5. Gate call site in src/ops/get_protection_status.rs (the crate::state::require_administrator* call preceding the store.policies lookup); refusal constructor src/state.rs:2530-2535.

=== Cause 5 of 5: GetViolationDetails for a policy id that names no stored policy ===
AWS: Smithy models GetViolationDetails with errors InternalErrorException | InvalidInputException | ResourceNotFoundException; AccessDeniedException is not among them. Recorded behavior for an unknown PolicyId (with well-formed MemberAccount, ResourceId and ResourceType): HTTP 400, "ResourceNotFoundException", Message "The referenced item does not exist."
Emulator: HTTP 400, $.__type "AccessDeniedException", $.Message "No default admin could be found for account <account-id> in Region <region>".
Why: as above; the remediation workflow that reads violation details cannot distinguish an absent policy/violation from a denied call, and receives an unmodeled error code.
Fix: GetViolationDetails' handler must not take the administrator gate; answer from its own policy/violation lookup.
Traces: fixture fms-policy-not-found-reads step task#6. Gate call site in src/ops/get_violation_details.rs (the crate::state::require_administrator* call preceding the lookup); refusal constructor src/state.rs:2530-2535.

Related but NOT covering: open issue 20261029T182723Z-fms-getadminaccount-s-unconfigured-resourcenotfoundexception-carries-the-service.md concerns a different defect on a different operation — GetAdminAccount DOES answer ResourceNotFoundException there, and the complaint is only that its Message carries the service-wide text instead of being empty. These five operations do not reach ResourceNotFoundException at all, and their recordings require the non-empty constant text, so that issue neither owns nor fixes any cause here. The gate is correct for the operations whose recorded refusal really is AccessDeniedException; this issue is only about the five that are mis-assigned to it.

## Evidence
All five causes: HTTP status matches (400 expected, 400 actual); the awsJson1_1 $.__type discriminator and $.Message diverge.

Cause 1 — fms-admin-not-configured-reads#task#2, GetAdminScope:
  expected: $.__type = "ResourceNotFoundException", $.Message = "The referenced item does not exist."
  actual:   $.__type = "AccessDeniedException",      $.Message = "No default admin could be found for account 000000000000 in Region us-west-2"
  (replay detail: "$.__type: value_mismatch (expected 'ResourceNotFoundException', got 'AccessDeniedException'); $.Message: value_mismatch (expected 'The referenced item does not exist.', got 'No default admin could be found for account 000000000000 in Region us-west-2')")

Cause 2 — fms-policy-not-found-reads#task#3, ListComplianceStatus: identical expected/actual pair as Cause 1.
Cause 3 — fms-policy-not-found-reads#task#4, GetComplianceDetail: identical expected/actual pair.
Cause 4 — fms-policy-not-found-reads#task#5, GetProtectionStatus: identical expected/actual pair.
Cause 5 — fms-policy-not-found-reads#task#6, GetViolationDetails: identical expected/actual pair.

The account id 000000000000 and Region us-west-2 in the actual messages are this environment's emulator account and the replay's signing Region — per-run values, not part of the requirement. The requirement is the error code and the fixed not-found text.

Model evidence that the actual code is unmodeled (services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json):
  GetAdminScope        errors: InternalErrorException, InvalidInputException, InvalidOperationException, LimitExceededException, ResourceNotFoundException
  ListComplianceStatus errors: InternalErrorException, ResourceNotFoundException
  GetComplianceDetail  errors: InternalErrorException, InvalidInputException, InvalidOperationException, ResourceNotFoundException
  GetProtectionStatus  errors: InternalErrorException, InvalidInputException, ResourceNotFoundException
  GetViolationDetails  errors: InternalErrorException, InvalidInputException, ResourceNotFoundException
  AccessDeniedException appears in none of these unions, nor in the service's common errors — which is why src/state.rs:2494 reaches for crate::scaffolded::Scaffolded::access_denied to emit it outside the modeled channel.

Source of the emulator's behavior: src/state.rs:2530-2535 no_administrator_refusal() builds the AccessDeniedException; src/state.rs:2552-2562 require_administrator_at() returns it when store.admin_account is None or inactive at `now`; the five handlers call it before their own lookups (get_admin_scope.rs:199, list_compliance_status.rs:183, get_compliance_detail.rs:186, and the corresponding sites in get_protection_status.rs and get_violation_details.rs).

## Steps to Reproduce
Cause 1:
1. Boot the emulator with a fresh store (no AssociateAdminAccount / PutAdminAccount, so no administrator association exists).
2. Call fms GetAdminScope with AdminAccount set to a well-formed 12-digit account id that is not an administrator.
3. Observe HTTP 400 with $.__type "AccessDeniedException" and the "No default admin could be found for account ... in Region ..." message; AWS answers HTTP 400 ResourceNotFoundException with Message "The referenced item does not exist."
4. Equivalently, replay fixture fms-admin-not-configured-reads step task#2.

Causes 2-5 (all against a fresh store with no administrator association):
1. Call fms ListComplianceStatus with PolicyId set to a 36-character id matching ^[a-z0-9A-Z-]{36}$ that names no stored policy -> expect ResourceNotFoundException "The referenced item does not exist.", observe AccessDeniedException. Replay fms-policy-not-found-reads step task#3.
2. Call fms GetComplianceDetail with that same unknown PolicyId and a well-formed MemberAccount -> same divergence. Replay fms-policy-not-found-reads step task#4.
3. Call fms GetProtectionStatus with that same unknown PolicyId -> same divergence. Replay fms-policy-not-found-reads step task#5.
4. Call fms GetViolationDetails with that same unknown PolicyId plus well-formed MemberAccount, ResourceId and ResourceType -> same divergence. Replay fms-policy-not-found-reads step task#6.

Full corpus check: replay the simparity-wire-aws-v1 fixtures fms-admin-not-configured-reads and fms-policy-not-found-reads; all five diverging steps must report the modeled ResourceNotFoundException once fixed.
