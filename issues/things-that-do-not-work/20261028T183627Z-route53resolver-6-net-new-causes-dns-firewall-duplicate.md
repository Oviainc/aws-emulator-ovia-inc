Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.associate-firewall-rule-group-duplicate-priority-conflict route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness route53resolver.update-firewall-rule-group-association-mutation-protected route53resolver.firewall-association-mutation-protection-priority-change-rejected route53resolver.concurrent-update-outpost-resolver-optimistic-lock route53resolver.batch-update-firewall-rule-limit-exceeded-rejected route53resolver.deploy-firewall-rule-group-cfn-association-active
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.associate-firewall-rule-group-duplicate-priority-conflict|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.update-firewall-rule-group-association-mutation-protected|run#19|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: route53resolver.firewall-association-mutation-protection-priority-change-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver|UpdateFirewallRuleGroupAssociation|ConflictException
Issue-Key: route53resolver.batch-update-firewall-rule-limit-exceeded-rejected|run#11|AssertionError|assert code == "LimitExceededException", f"expected LimitExceededException, got {code!r}"
Issue-Key: route53resolver.deploy-firewall-rule-group-cfn-association-active|run#25|AssertionError|assert dead_message == expected, f"expected message {expected!r}, got {dead_message!r}"
Issue-Commit: afab077872ee6c32a9626f154d3b33fea96b8a18

# route53resolver: 6 net-new causes — DNS Firewall duplicate-priority and MutationProtection refusals are emulator-authored (no RSLVR-02303/02203/02307), a second concurrent UpdateFirewallRuleGroupAssociation is refused ConflictException, BatchUpdateFirewallRule's whole-batch cap is unenforced, and RSLVR-02025 on an association omits the Trace Id clause

> _Note: this issue was identified against the code base at commit `afab077872ee6c32a9626f154d3b33fea96b8a18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFirewallRuleGroupAssociation

## Description
Six net-new root causes in the DNS Firewall control plane of service-com-amazonaws-route53resolver-route53resolver. Causes 1-3 and 6 are refusal-message fidelity: the emulator refuses with the right HTTP status and the right error CODE but with prose it authored itself instead of the RSLVR-coded sentence Route 53 Resolver returns. Causes 4-5 refuse calls AWS accepts.

=== Cause 1 of 6: AssociateFirewallRuleGroup's duplicate-priority refusal does not use AWS's RSLVR-02303 sentence and names the wrong variable ===
Failing check: route53resolver.associate-firewall-rule-group-duplicate-priority-conflict
AWS: Smithy models AssociateFirewallRuleGroup(CreatorRequestId, FirewallRuleGroupId, MutationProtection, Name, Priority, Tags, VpcId) with errors AccessDeniedException | ConflictException | InternalServiceErrorException | LimitExceededException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400 (defaulted); ValidationException carries only {Message}, so no message text is modeled — the wording is recorded behavior only. The recorded Route 53 Resolver response for a Priority another association of the SAME VpcId already holds is HTTP 400, Error.Code `ValidationException`, Error.Message of the form:
    [RSLVR-02303] This priority is the same with another association: "<Id of the association that already holds that priority>" . Try again with another priority number.
Message anatomy: FIXED text is `[RSLVR-02303] This priority is the same with another association: "` … `" . Try again with another priority number.` — note the service's space between the closing double quote and the period. The single VARIABLE part is the id of the EXISTING FirewallRuleGroupAssociation that already occupies the priority, read out of the service's own state; the requested Priority and the VpcId do not appear in AWS's message at all.
Emulator: HTTP 400 / ValidationException (correct class) with the self-authored sentence `Priority 300 is already in use by another rule group associated with VPC vpc-00000000000000022` — no RSLVR code, different wording, and it names the requested priority and the VPC instead of the colliding association's id, so the one datum a caller needs (which association to look at) is missing. Built inline in the handler (`format!("Priority {} is already in use by another rule group associated with VPC {}", ...)`).
Why: a client that matches, logs or branches on Resolver's Message cannot recognise the priority collision — the RSLVR-02303 code that error-handling and support tooling keys on is absent, and remediation tooling that would read the colliding association id out of the message has nothing to read. Negative tests written against AWS fail here, and tests written against the emulator fail in production.
Fix: AssociateFirewallRuleGroup's duplicate-priority validation, which must look up the colliding association and build the sentence through the shared RSLVR message formatter in crate::state rather than formatting its own string. The identical collision check on the update path (UpdateFirewallRuleGroupAssociation) repeats the same self-authored string and must take the same sentence, so the message belongs in one state.rs helper both handlers call.
Traces: route53resolver.associate-firewall-rule-group-duplicate-priority-conflict. File citations: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/associate_firewall_rule_group.rs:325-339 (the refusal) and services/service-com-amazonaws-route53resolver-route53resolver/src/ops/update_firewall_rule_group_association.rs:298-315 (the duplicated wording on the update path).

=== Cause 2 of 6: CreateFirewallRule's duplicate-priority refusal does not use AWS's RSLVR-02203 sentence ===
Failing check: route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness
AWS: Smithy models CreateFirewallRule(CreatorRequestId, FirewallRuleGroupId, FirewallDomainListId, Priority required, Action required, Name required, Qtype len 1..16, …) with errors AccessDeniedException | InternalServiceErrorException | LimitExceededException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400; ValidationException{Message} carries no modeled text, so the wording is recorded behavior only. Priority uniqueness is group-wide and is NOT scoped by Qtype (the check's whole point): a second rule in the same FirewallRuleGroupId at a priority already taken, even with a different Qtype, is refused HTTP 400 / `ValidationException` with:
    [RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "<FirewallRuleGroupId>". Provide another priority.
Message anatomy: FIXED text is `[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "` … `". Provide another priority.`; the single VARIABLE part is the FirewallRuleGroupId taken verbatim from the request.
Emulator: HTTP 400 / ValidationException (correct class) with `Priority 710 is already in use by another rule in rule group rslvr-frg-910a2dec89025cc1` — no RSLVR-02203 code and non-AWS wording.
Why: same client impact as Cause 1, on the rule-create path: the collision is unrecognisable by code, so SDK wrappers and CI negative tests that pin AWS's RSLVR-02203 string diverge in both directions.
Fix: CreateFirewallRule's duplicate-priority validation, routed through the shared RSLVR message formatter in crate::state (the per-handler `format!` must go).
Traces: route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness. File citation: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_rule.rs (the duplicate-priority branch of `handle`, after the enum/match-source validation at :269-320).

=== Cause 3 of 6: the MutationProtection refusal on UpdateFirewallRuleGroupAssociation is emulator-authored instead of AWS's RSLVR-02307 sentence ===
Failing checks: route53resolver.update-firewall-rule-group-association-mutation-protected, route53resolver.firewall-association-mutation-protection-priority-change-rejected
AWS: Smithy models UpdateFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId required, Priority, MutationProtection, Name) with errors AccessDeniedException | ConflictException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400; MutationProtection's own doc states the rule ("this setting disallows modification or removal of the association"), but no message text is modeled, so the wording is recorded behavior only. Changing Priority while MutationProtection is ENABLED is refused HTTP 400, Error.Code `ValidationException`, Error.Message:
    [RSLVR-02307] The DNS Firewall rule group association is under mutation protection. Remove the mutation protection and try again.
Message anatomy: the sentence is ENTIRELY fixed text — it has NO variable part; it never names the association id. When the caller sends an X-Amzn-Trace-Id, the service appends ` Trace Id: "<Root>"` after it (see Cause 6).
Emulator: HTTP 400 / ValidationException (correct class) with `The firewall rule group association with ID "rslvr-frgassoc-975835de1c9756ce" has MutationProtection ENABLED; disable it before modifying or removing the association` — no RSLVR-02307 code, entirely different wording, and it injects the association id AWS's message does not carry. Produced by `crate::state::mutation_protection_forbids_change(&input.firewall_rule_group_association_id)`, whose very signature bakes in the spurious id parameter.
Why: mutation protection exists to stop accidental change, so its refusal is the one a client is most likely to surface to an operator or match on; against the emulator the RSLVR-02307 code is absent and the text is unmatchable. Both failing checks abort at this assertion, so the rest of the contract they cover (priority unchanged after the refusal, and the identical change succeeding once protection is disabled) is never exercised.
Fix: `crate::state::mutation_protection_forbids_change` in state.rs — it must return the fixed RSLVR-02307 sentence through the shared RSLVR formatter and take no resource id; the caller in the update handler drops the argument.
Traces: route53resolver.update-firewall-rule-group-association-mutation-protected, route53resolver.firewall-association-mutation-protection-priority-change-rejected. File citation: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/update_firewall_rule_group_association.rs:275-293 and the `mutation_protection_forbids_change` helper in services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs. Note: route53resolver.firewall-association-mutation-protection-priority-change-rejected pins a caller-supplied X-Amzn-Trace-Id and therefore ALSO needs the trailing clause of Cause 6 before it passes; route53resolver.update-firewall-rule-group-association-mutation-protected sends no trace header and needs only this cause.

=== Cause 4 of 6: a second concurrent UpdateFirewallRuleGroupAssociation is refused ConflictException because the handler gates every update on COMPLETE ===
Failing check: route53resolver.concurrent-update-outpost-resolver-optimistic-lock
AWS: recorded behavior only — the model constrains nothing about concurrency; ConflictException is modeled on this operation but documented for an invalid state transition ("you can't delete a firewall domain list if it is in the process of being deleted"). Recorded Route 53 Resolver behavior: two concurrent UpdateFirewallRuleGroupAssociation calls on one association, each setting a full (Priority, Name) pair, BOTH return 200; the service serialises them and the association, once COMPLETE again, equals one racer's pair whole (last writer wins) — it never refuses the second writer merely because the first change has not settled, and it never leaves a mixed pair.
Emulator: one racer gets HTTP 400, Error.Code `ConflictException`, Error.Message `The requested state transition isn't valid.`, and the check dies on that call. The cause is self-inflicted: the handler writes `status = UPDATING` with `settled_at = now + PROVISIONING_DELAY` on every accepted update, and then refuses any update that arrives while `status_at(now) != COMPLETE`. Any two updates inside the synthetic provisioning window therefore collide, and the same gate makes an in-flight update unretryable for a normal sequential client too.
Why: a client that changes an association twice in quick succession (or any two concurrent writers — a reconciler plus an operator, two CI jobs) gets a hard 400 the real service never returns, with a message that claims an invalid state transition where the transition is perfectly valid. Callers must either insert sleeps that AWS does not require or treat a legitimate refusal class as retryable noise.
Fix: the state gate in UpdateFirewallRuleGroupAssociation's handler and the record's settling model in crate::state: an association whose own last update is still settling must accept a further update (restamping the change and the settle deadline, last writer wins, the whole (Priority, Name, MutationProtection) triple applied atomically) rather than answering ConflictException. The ConflictException path should stay only for the transitions the model documents (e.g. an association on its way out, DELETING).
Traces: route53resolver.concurrent-update-outpost-resolver-optimistic-lock. File citation: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/update_firewall_rule_group_association.rs:263-273 (the `status != COMPLETE` -> `state_transition_conflict()` gate) and :342-349 (the UPDATING/`settled_at` write that opens the window).

=== Cause 5 of 6: BatchUpdateFirewallRule answers 501 NotImplemented, so the whole-batch entry cap (LimitExceededException RSLVR-02052) is never enforced and no batch update is applied ===
Failing check: route53resolver.batch-update-firewall-rule-limit-exceeded-rejected
AWS: Smithy models BatchUpdateFirewallRule(UpdateFirewallRuleEntries: UpdateFirewallRuleEntries required) -> {UpdatedFirewallRules, UpdateErrors} with errors AccessDeniedException | InternalServiceErrorException | LimitExceededException | ThrottlingException | ValidationException, all httpError 400; LimitExceededException is documented "The request caused one or more limits to be exceeded" and is a WHOLE-CALL error, distinct from the per-entry `UpdateErrors` the operation uses for bad references. Recorded behavior for an oversized batch is HTTP 400, Error.Code `LimitExceededException`, Error.Message:
    [RSLVR-02052] The number of firewall rules in the update batch exceeds the allowed maximum. Provide fewer than "10" rules.
Message anatomy: FIXED text is `[RSLVR-02052] The number of firewall rules in the update batch exceeds the allowed maximum. Provide fewer than "` … `" rules.`; the VARIABLE part is the service's own per-request entry cap, which the recording shows as 10. The refusal is wholesale and ATOMIC: not even the batch's one legitimate entry is applied, so the target rule's ModificationTime is unchanged afterwards, and a correctly-sized batch on the same rule still succeeds and moves its Priority.
Emulator: every BatchUpdateFirewallRule call is refused with Error.Code `NotImplemented` — the handler body is still the scaffold (`Err(crate::scaffolded::Scaffolded::not_implemented("BatchUpdateFirewallRule"))`), so neither the cap, the atomicity, nor the success path exists.
Why: callers that update firewall rules in batches get an unmodelled error code for every call, and once the operation is written the >10-entry refusal must be the whole-call LimitExceededException above rather than per-entry UpdateErrors or a ValidationException, or batch clients will silently apply partial batches.
Fix: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/batch_update_firewall_rule.rs — implement the handler over the rule records in crate::state, refusing a batch past the per-request entry cap wholesale with LimitExceededException and the RSLVR-02052 sentence BEFORE any entry is applied. NOTE: open issue "Implement route53resolver BatchUpdateFirewallRule: unimplemented scaffold stub" (filed from the checkout for route53resolver.batch-update-and-delete-firewall-rule) covers the bare "it is a 501 stub" fact; this cause is recorded because that issue does not carry this check's replay key and does not state the cap/atomicity/message contract the check asserts. Close them together.
Traces: route53resolver.batch-update-firewall-rule-limit-exceeded-rejected. File citation: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/batch_update_firewall_rule.rs:151-169.

=== Cause 6 of 6: the RSLVR-02025 ResourceNotFoundException for a removed FirewallRuleGroupAssociation omits the trailing `Trace Id: "<Root>"` clause ===
Failing check: route53resolver.deploy-firewall-rule-group-cfn-association-active
AWS: Smithy models ResourceNotFoundException{Message, ResourceType} on UpdateFirewallRuleGroupAssociation, httpError 400; no message text is modeled, so the wording is recorded behavior only. For a FirewallRuleGroupAssociationId that no longer exists (just disassociated), Route 53 Resolver answers HTTP 400, Error.Code `ResourceNotFoundException`, Error.Message:
    [RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupAssociationId>". Trace Id: "<Root>"
Message anatomy: FIXED text is `[RSLVR-02025] Can’t find the resource with ID "` … `". Trace Id: "` … `"` (with the service's U+2019 right-single-quote in `Can’t` and the sentence-ending period BEFORE the Trace Id clause). VARIABLE parts are (a) the association id taken verbatim from the request and (b) the X-Ray trace Root, which is NOT per-run invention: it is read from the caller's `X-Amzn-Trace-Id: Root=<root>;Sampled=0` request header, so a caller that pins that header gets a byte-exact message — which is exactly how the failing check pins it (`_traced(client)` registers a fixed header on every route53resolver call). Only when the caller sends no such header does the service mint a Root itself.
Emulator: HTTP 400 / ResourceNotFoundException with the RSLVR-02025 prefix and the id correct, but the message stops at the period: `[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-1d0b14e4db018fed".` — the ` Trace Id: "..."` clause is absent, because the shared formatter has no trace parameter and the handler discards `ctx` for message purposes.
Why: the trace Root is how a caller correlates a Resolver refusal with its X-Ray trace and with a support case; against the emulator that correlation key is simply absent, and every client-side matcher written against AWS's full message (which ends in `Trace Id: "..."`) diverges. Here it also aborts a CloudFormation-deployed-rule-group check at its last assertion.
Fix: the shared RSLVR message formatter in crate::state plus plumbing the request's X-Amzn-Trace-Id Root (available on the handler's OperationContext request parts) into it, with a service-minted Root as the fallback — not this one handler. NOTE: this is the same shared-formatter defect as open issue 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing (which grounds it in GetFirewallRuleGroup / route53resolver.delete-firewall-rule-group-with-vpc-association-conflict); it is recorded here only because that issue does not carry this check's replay key, and because Cause 3's second check needs the same clause. One fix closes all of them.
Traces: route53resolver.deploy-firewall-rule-group-cfn-association-active. File citation: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/update_firewall_rule_group_association.rs:169 (`let _ = ctx;`) and :253-261 / :317-329 (`crate::state::firewall_rule_group_association_not_found`), plus the `rslvr_message` / `resource_not_found_message` helpers in services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs.

## Evidence
All six causes are grounded in the replay of simparity-behavioral-ga-prod-v1 against this checkout (route53resolver: 43/80 passed). Recorded ids, priorities, VPC ids, association ids and trace Roots below are this run's values filling the message templates — they are not part of any requirement.

Cause 1 — route53resolver.associate-firewall-rule-group-duplicate-priority-conflict [mismatch]
  Expected (AWS): HTTP 400, Error.Code "ValidationException", Error.Message
    '[RSLVR-02303] This priority is the same with another association: "<existing association Id>" . Try again with another priority number.'
  Actual (emulator): HTTP 400, Error.Code "ValidationException" (correct), Error.Message
    'Priority 300 is already in use by another rule group associated with VPC vpc-00000000000000022'
  Replay excerpt:
    File "check_route53resolver_associate_firewall_rule_group_duplicate_priority_conflict.py", line 233, in run
      assert message == expected, f"expected message {expected!r}, got {message!r}"
    AssertionError: expected message '[RSLVR-02303] This priority is the same with another association: "rslvr-frgassoc-1d0b14e4db018fed" . Try again with another priority number.', got 'Priority 300 is already in use by another rule group associated with VPC vpc-00000000000000022'
  Code: associate_firewall_rule_group.rs:330-338 builds the message with format!("Priority {} is already in use by another rule group associated with VPC {}", input.priority, input.vpc_id).

Cause 2 — route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness [mismatch]
  Expected (AWS): HTTP 400, "ValidationException",
    '[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "<FirewallRuleGroupId>". Provide another priority.'
  Actual (emulator): HTTP 400, "ValidationException" (correct),
    'Priority 710 is already in use by another rule in rule group rslvr-frg-910a2dec89025cc1'
  Replay excerpt:
    File "check_route53resolver_create_firewall_rule_qtype_does_not_scope_priority_uniqueness.py", line 160, in run
      assert message == expected, ...
    AssertionError: expected message '[RSLVR-02203] ... same priority: "rslvr-frg-910a2dec89025cc1". Provide another priority.', got 'Priority 710 is already in use by another rule in rule group rslvr-frg-910a2dec89025cc1'
  (The emulator DID refuse the differing-Qtype duplicate, so the uniqueness rule itself is right; only the message diverges.)

Cause 3 — route53resolver.update-firewall-rule-group-association-mutation-protected [mismatch] and route53resolver.firewall-association-mutation-protection-priority-change-rejected [mismatch]
  Expected (AWS), on UpdateFirewallRuleGroupAssociation(Priority=...) against an association with MutationProtection=ENABLED:
    ('ValidationException', '[RSLVR-02307] The DNS Firewall rule group association is under mutation protection. Remove the mutation protection and try again.')
    and, when the caller pins X-Amzn-Trace-Id, the same sentence followed by ' Trace Id: "<Root>"'.
  Actual (emulator):
    ('ValidationException', 'The firewall rule group association with ID "rslvr-frgassoc-975835de1c9756ce" has MutationProtection ENABLED; disable it before modifying or removing the association')
  Replay excerpts:
    check_route53resolver_update_firewall_rule_group_association_mutation_protected.py:80 — assert refusal == expected -> AssertionError: expected ('ValidationException', '[RSLVR-02307] The DNS Firewall rule group association is under mutation protection. Remove the mutation protection and try again.'), got ('ValidationException', 'The firewall rule group association with ID "rslvr-frgassoc-975835de1c9756ce" has MutationProtection ENABLED; disable it before modifying or removing the association')
    check_route53resolver_firewall_association_mutation_protection_priority_change_rejected.py:58 — AssertionError: expected message '[RSLVR-02307] ... try again. Trace Id: "1-6ae23b17-52d71ebda91048c2b88a7ecb"', got 'The firewall rule group association with ID "rslvr-frgassoc-975835de1c9756ce" has MutationProtection ENABLED; ...'
  Note the second check's positive control passed first: an UNPROTECTED association took Priority=202 cleanly and reported ENABLED after the protection was switched on, so only the refusal text diverges.
  Code: update_firewall_rule_group_association.rs:280-293 calls crate::state::mutation_protection_forbids_change(&input.firewall_rule_group_association_id) — the helper takes an id AWS's message does not contain.

Cause 4 — route53resolver.concurrent-update-outpost-resolver-optimistic-lock [error]
  Scenario (from the check): associate a rule group with a VPC at Priority=137, poll to COMPLETE, then submit TWO UpdateFirewallRuleGroupAssociation calls concurrently from a ThreadPoolExecutor, writer A = (Priority 4001, Name A), writer B = (Priority 4002, Name B); AWS returns 200 for both and the settled association equals one writer's pair whole.
  Actual (emulator): one of the two calls raises
    botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the UpdateFirewallRuleGroupAssociation operation: The requested state transition isn't valid.
  so fut.result() propagates and the check never reaches its no-interleaving assertion.
  Code: update_firewall_rule_group_association.rs:266-273 refuses whenever record.status_at(now) != FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE, while :347-349 sets status = UPDATING with settled_at = now + crate::state::PROVISIONING_DELAY on every accepted update — the emulator's own provisioning window is what makes the second writer collide.

Cause 5 — route53resolver.batch-update-firewall-rule-limit-exceeded-rejected [mismatch]
  Expected (AWS), BatchUpdateFirewallRule with 1001 entries: HTTP 400, Error.Code "LimitExceededException", Error.Message '[RSLVR-02052] The number of firewall rules in the update batch exceeds the allowed maximum. Provide fewer than "10" rules.'; the target rule's ModificationTime is unchanged afterwards, and a 1-entry batch then moves its Priority to 250.
  Actual (emulator): Error.Code "NotImplemented".
  Replay excerpt:
    File "check_route53resolver_batch_update_firewall_rule_limit_exceeded_rejected.py", line 62, in run
      assert code == "LimitExceededException", f"expected LimitExceededException, got {code!r}"
    AssertionError: expected LimitExceededException, got 'NotImplemented'
  Code: batch_update_firewall_rule.rs:158-168 — `let _ = (&input, ctx, &self.engine);` then `Err(crate::scaffolded::Scaffolded::not_implemented("BatchUpdateFirewallRule"))`; the SCAFFOLD marker is still at the top of the file.

Cause 6 — route53resolver.deploy-firewall-rule-group-cfn-association-active [mismatch]
  The check registers a before-sign hook putting a FIXED 'X-Amzn-Trace-Id: Root=<root>;Sampled=0' on every route53resolver call (check source comment: "Route 53 Resolver ends a DNS Firewall refusal message with the caller's trace Root, so a fixed Root makes the whole message exact"), deploys the rule group via CloudFormation, associates it, updates it to Priority 150, disassociates it, then updates the dead id.
  Expected (AWS): HTTP 400, "ResourceNotFoundException", '[RSLVR-02025] Can’t find the resource with ID "<assoc id>". Trace Id: "<Root sent by the caller>"'
  Actual (emulator): HTTP 400, "ResourceNotFoundException" (correct), '[RSLVR-02025] Can’t find the resource with ID "<assoc id>".' — clause absent.
  Replay excerpt:
    File "check_route53resolver_deploy_firewall_rule_group_cfn_association_active.py", line 127, in run
      assert dead_message == expected, ...
    AssertionError: expected message '[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-1d0b14e4db018fed". Trace Id: "1-6ae23e89-0624219a04ba4315978e3230"', got '[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-1d0b14e4db018fed".'
  Everything before that assertion passed (CFN provisioned the FirewallDomainList and FirewallRuleGroup, the association reached COMPLETE, the Priority update to 150 settled, and the disassociation genuinely removed the record), so this message clause is the only divergence in that whole flow.

Model references used: smithy_query(query="operation", operation="UpdateFirewallRuleGroupAssociation,CreateFirewallRule,BatchUpdateFirewallRule") on services/service-com-amazonaws-route53resolver-route53resolver/smithy-model/com-amazonaws-route53resolver-route53resolver.json, and the AssociateFirewallRuleGroup contract reproduced in associate_firewall_rule_group.rs:48-104. No RSLVR message text is modeled anywhere in this service, so every expected sentence above is recorded behavior.

## Steps to Reproduce
Common setup: boto3 clients for route53resolver, ec2 (and cloudformation for Cause 6) pointed at the emulator.

Cause 1 — AssociateFirewallRuleGroup duplicate priority (RSLVR-02303):
1. vpc_id = ec2.create_vpc(CidrBlock="10.90.0.0/16")["Vpc"]["VpcId"].
2. g1 = route53resolver.create_firewall_rule_group(CreatorRequestId=<unique>, Name=<unique>)["FirewallRuleGroup"]["Id"]; g2 likewise.
3. a1 = associate_firewall_rule_group(CreatorRequestId=<unique>, FirewallRuleGroupId=g1, VpcId=vpc_id, Priority=300, Name=<unique>) — succeeds; keep a1["FirewallRuleGroupAssociation"]["Id"].
4. associate_firewall_rule_group(CreatorRequestId=<unique>, FirewallRuleGroupId=g2, VpcId=vpc_id, Priority=300, Name=<unique>) — refused 400/ValidationException (correct), but Error.Message is 'Priority 300 is already in use by another rule group associated with VPC <vpc_id>' instead of AWS's '[RSLVR-02303] This priority is the same with another association: "<a1 id>" . Try again with another priority number.'

Cause 2 — CreateFirewallRule duplicate priority (RSLVR-02203):
1. g = create_firewall_rule_group(...)["FirewallRuleGroup"]["Id"]; dl = create_firewall_domain_list(...)["FirewallDomainList"]["Id"].
2. create_firewall_rule(CreatorRequestId=<unique>, FirewallRuleGroupId=g, FirewallDomainListId=dl, Priority=710, Action="BLOCK", BlockResponse="NXDOMAIN", Name=<unique>, Qtype="A") — succeeds.
3. create_firewall_rule(same group and domain list, Priority=710, Qtype="AAAA", new CreatorRequestId/Name) — correctly refused 400/ValidationException, but Error.Message is 'Priority 710 is already in use by another rule in rule group <g>' instead of '[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "<g>". Provide another priority.'

Cause 3 — MutationProtection refusal (RSLVR-02307):
1. g = create_firewall_rule_group(...); vpc_id = ec2.create_vpc(...).
2. assoc = associate_firewall_rule_group(CreatorRequestId=<unique>, FirewallRuleGroupId=g, VpcId=vpc_id, Priority=101, Name=<unique>, MutationProtection="ENABLED"); poll get_firewall_rule_group_association until Status == "COMPLETE".
3. update_firewall_rule_group_association(FirewallRuleGroupAssociationId=assoc id, Priority=200) — refused 400/ValidationException (correct) with the emulator's own 'The firewall rule group association with ID "..." has MutationProtection ENABLED; disable it before modifying or removing the association' instead of '[RSLVR-02307] The DNS Firewall rule group association is under mutation protection. Remove the mutation protection and try again.'
4. Repeat step 3 with a before-sign hook setting a fixed 'X-Amzn-Trace-Id: Root=<root>;Sampled=0': AWS's message additionally ends with ' Trace Id: "<root>"' (Cause 6); the emulator emits neither part.

Cause 4 — concurrent update refused:
1. vpc_id = ec2.create_vpc(CidrBlock="10.84.0.0/16"); g = create_firewall_rule_group(...).
2. assoc = associate_firewall_rule_group(..., VpcId=vpc_id, FirewallRuleGroupId=g, Priority=137, Name=<unique>); poll until Status == "COMPLETE".
3. From two threads at once call update_firewall_rule_group_association(FirewallRuleGroupAssociationId=assoc id, Priority=4001, Name="A") and (..., Priority=4002, Name="B").
4. One call fails with 400 ConflictException "The requested state transition isn't valid." AWS returns 200 for both and settles on one writer's (Priority, Name) pair whole. The same refusal reproduces SEQUENTIALLY: issue a second update immediately after the first (inside the emulator's PROVISIONING_DELAY window, before Status returns to COMPLETE).

Cause 5 — BatchUpdateFirewallRule cap:
1. Create a domain list with one domain, a rule group, and one rule in it (Priority=100, Action=BLOCK, BlockResponse=NXDOMAIN); record the rule's ModificationTime.
2. batch_update_firewall_rule(UpdateFirewallRuleEntries=[{FirewallRuleGroupId: g, FirewallDomainListId: dl, Priority: 100, Action: "BLOCK", BlockResponse: "NXDOMAIN"}] * 1001) — the emulator answers Error.Code "NotImplemented"; AWS answers 400 LimitExceededException with '[RSLVR-02052] The number of firewall rules in the update batch exceeds the allowed maximum. Provide fewer than "10" rules.' and leaves the rule's ModificationTime untouched.
3. batch_update_firewall_rule with a SINGLE entry at Priority=250 — AWS applies it (list_firewall_rules then reports Priority 250); the emulator again answers NotImplemented.

Cause 6 — missing Trace Id clause on an association not-found:
1. Register a before-sign hook on the route53resolver client that sets a FIXED 'X-Amzn-Trace-Id: Root=1-<8 hex>-<24 hex>;Sampled=0'; keep that Root.
2. Create a VPC, a rule group and an association; poll to COMPLETE; then disassociate_firewall_rule_group(FirewallRuleGroupAssociationId=assoc id) and poll until it is gone.
3. update_firewall_rule_group_association(FirewallRuleGroupAssociationId=<the dead id>, Priority=200) — refused 400/ResourceNotFoundException (correct), but Error.Message stops at '[RSLVR-02025] Can’t find the resource with ID "<dead id>".' where AWS appends ' Trace Id: "<the Root from step 1>"'.

Replay: run_behavioral_suite and re-run route53resolver.associate-firewall-rule-group-duplicate-priority-conflict, route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness, route53resolver.update-firewall-rule-group-association-mutation-protected, route53resolver.firewall-association-mutation-protection-priority-change-rejected, route53resolver.concurrent-update-outpost-resolver-optimistic-lock, route53resolver.batch-update-firewall-rule-limit-exceeded-rejected, route53resolver.deploy-firewall-rule-group-cfn-association-active.
