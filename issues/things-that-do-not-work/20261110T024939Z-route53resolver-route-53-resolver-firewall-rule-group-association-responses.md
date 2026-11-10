Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-dns-firewall-override-answers-in-vpc
Issue-Step: task#18
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-dns-firewall-override-answers-in-vpc
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|AssociateFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroupAssociation.StatusMessage
Issue-Key: route53resolver|GetFirewallRuleGroupAssociation|2xx->2xx|$.FirewallRuleGroupAssociation.StatusMessage
Issue-Commit: 109411407d03b40f255761f7a04c49ae72d75d8c

# Route 53 Resolver firewall rule group association responses omit FirewallRuleGroupAssociation.StatusMessage (2 causes)

> _Note: this issue was identified against the code base at commit `109411407d03b40f255761f7a04c49ae72d75d8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateFirewallRuleGroup

## Description
TWO net-new wire causes in the Route 53 Resolver DNS Firewall control plane (services/service-com-amazonaws-route53resolver-route53resolver). They share ONE fault: the single FirewallRuleGroupAssociation projection the emulator serves never emits the modeled StatusMessage member, so every operation that returns an association reports its lifecycle state with Status alone and drops the sentence that explains it. Both clusters are reported by the replay as UNOWNED (owned_by empty) and neither is stub-blocked, so no open issue's replay key gates them. A fixer must close BOTH causes before re-replaying — closing one leaves the other step still diverging.

=== Cause 1 of 2: the AssociateFirewallRuleGroup response omits StatusMessage ===
AWS: Smithy: AssociateFirewallRuleGroup(CreatorRequestId required @length 1..255 @idempotencyToken, FirewallRuleGroupId required @length 1..64, VpcId required @length 1..64, Priority required, Name required @length 0..64 @pattern ^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$, MutationProtection, Tags @length 0..200) -> AssociateFirewallRuleGroupResponse{FirewallRuleGroupAssociation}; errors AccessDeniedException | ConflictException | InternalServiceErrorException | LimitExceededException | ResourceNotFoundException | ThrottlingException | ValidationException (all httpError 400). Shape FirewallRuleGroupAssociation models Status (enum FirewallRuleGroupAssociationStatus = COMPLETE | DELETING | UPDATING) alongside StatusMessage (@length 0..255, doc: "Additional information about the status of the response, if available."). The specific sentence an in-flight association carries is recorded behavior only: the association acknowledgement answers 200 with StatusMessage "Creating Firewall Rule Group Association". That whole sentence is FIXED text — it has no variable part taken from the request or from emulator state (the association id, ARN, VPC id, rule-group id, name, priority and timestamps in the same projection ARE per-run values and are not requirements).
Emulator: HTTP 200 whose body has no StatusMessage member at all under FirewallRuleGroupAssociation — replay reports "$.FirewallRuleGroupAssociation.StatusMessage: missing (expected 'Creating Firewall Rule Group Association', got None)". Status itself matched, so only the modeled message member is dropped.
Why: associating a rule group with a VPC is asynchronous, so the Status/StatusMessage pair on the acknowledgement is all a caller has to confirm the association was accepted and is still settling. A client that reads StatusMessage off the associate response gets an absent key instead of a sentence — it either faults on the missing member or cannot distinguish an association that is being created from one that failed, before it starts polling GetFirewallRuleGroupAssociation.
Fix: the AssociateFirewallRuleGroup handler must stamp the creating-class status message onto the association record it writes into the crate's state module, and the one shared FirewallRuleGroupAssociation serializer must project the modeled StatusMessage member. Open issue 20261108T111412Z-route53resolver-route-53-resolver-dns-firewall-responses-diverge-managed.md covers the SAME class of defect on other DNS Firewall resources (ImportFirewallDomains' missing StatusMessage, DeleteFirewallRuleGroup's wrong Status/StatusMessage) but not the rule-group ASSOCIATION projection, and the replay reports this cluster unowned.
Traces: route53resolver-dns-firewall-override-answers-in-vpc#task#18

=== Cause 2 of 2: the GetFirewallRuleGroupAssociation read-back omits StatusMessage on a settled association ===
AWS: Smithy: GetFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId required @length 1..64) -> GetFirewallRuleGroupAssociationResponse{FirewallRuleGroupAssociation}; errors AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException (all httpError 400). The returned structure models the same StatusMessage (@length 0..255) member as the associate response. The settled sentence is recorded behavior only: once the association has finished settling AWS answers 200 with StatusMessage "Finished rule group association update" — again entirely FIXED text with no variable part drawn from the request or state.
Emulator: HTTP 200 whose FirewallRuleGroupAssociation has no StatusMessage member — replay reports "$.FirewallRuleGroupAssociation.StatusMessage: missing (expected 'Finished rule group association update', got None)". Status matched here too, so the read path drops exactly the same modeled member as the write path.
Why: this is the operation a client polls to learn that DNS Firewall filtering is actually live for the VPC. With StatusMessage absent, a poll loop that keys on the settled message never terminates, and any client that surfaces the association's state to a user shows a lifecycle with no explanation at all — the one member AWS uses to say which transition completed is missing on both the write and the read.
Fix: the shared FirewallRuleGroupAssociation projection used by GetFirewallRuleGroupAssociation (and by ListFirewallRuleGroupAssociations / UpdateFirewallRuleGroupAssociation / DisassociateFirewallRuleGroup, which return the same shape) must emit StatusMessage, and the association state machine in the crate's state module must persist the settled status message when the association transitions out of its creating state.
Traces: route53resolver-dns-firewall-override-answers-in-vpc#task#19

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped; get_bootstrap_status reports an empty pending set). Replay summary: emulator_up true, bootstrap_signal false, score 73.9%, 203 steps total, 150 passed, 48 mismatched, 0 errored, 5 blocked, 40 divergence clusters. Of those 40 clusters, 2 are blocked by unimplemented operations and 36 are owned_by an open issue; the 2 below are the only clusters that are neither stub-blocked nor owned (stub_path "", owned_by []).

Cause 1 — operation AssociateFirewallRuleGroup, fixture route53resolver-dns-firewall-override-answers-in-vpc, step task#18, verdict mismatch, expected_status 200 -> actual_status 200, affected_count 1:
  $.FirewallRuleGroupAssociation.StatusMessage: missing (expected 'Creating Firewall Rule Group Association', got None)
  divergence_key: service=route53resolver|AssociateFirewallRuleGroup|status=200->200|keys=$.FirewallRuleGroupAssociation.StatusMessage
  affected: route53resolver-dns-firewall-override-answers-in-vpc#task#18
  owned_by: [], stub_path: ""

Cause 2 — operation GetFirewallRuleGroupAssociation, fixture route53resolver-dns-firewall-override-answers-in-vpc, step task#19, verdict mismatch, expected_status 200 -> actual_status 200, affected_count 1:
  $.FirewallRuleGroupAssociation.StatusMessage: missing (expected 'Finished rule group association update', got None)
  divergence_key: service=route53resolver|GetFirewallRuleGroupAssociation|status=200->200|keys=$.FirewallRuleGroupAssociation.StatusMessage
  affected: route53resolver-dns-firewall-override-answers-in-vpc#task#19
  owned_by: [], stub_path: ""

Expected vs actual, in short: AWS answers 200 with the modeled FirewallRuleGroupAssociation.StatusMessage member present on both the associate acknowledgement and the subsequent read-back; the emulator answers 200 with that member absent in both. No HTTP status or error code diverges here — the status (200) and the Status enum value both matched — so the defect is purely the dropped modeled member.

Fixed text vs variable parts: "Creating Firewall Rule Group Association" and "Finished rule group association update" are each entirely fixed AWS text with NO variable substitution. All other values in the same projection (Id, Arn, FirewallRuleGroupId, VpcId, Name, Priority, CreatorRequestId, CreationTime, ModificationTime) are per-run and are NOT part of the requirement. Smithy citation: FirewallRuleGroupAssociation.StatusMessage targets StatusMessage @length 0..255, doc "Additional information about the status of the response, if available."; FirewallRuleGroupAssociationStatus enum = COMPLETE | DELETING | UPDATING.

## Steps to Reproduce
Replay fixture route53resolver-dns-firewall-override-answers-in-vpc from corpus simparity-wire-aws-v1, steps task#18 and task#19. BOTH causes must be closed before re-replaying; closing one leaves the other step diverging.

Cause 1: CreateFirewallRuleGroup(CreatorRequestId=<id>, Name=<name>), then AssociateFirewallRuleGroup(CreatorRequestId=<id>, FirewallRuleGroupId=<group id>, VpcId=<vpc id>, Priority=<int>, Name=<assoc name>). The 200 response's FirewallRuleGroupAssociation must carry a StatusMessage member alongside Status — the creating-class sentence "Creating Firewall Rule Group Association" — not omit the member entirely.

Cause 2: with that association in place and settled, call GetFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId=<assoc id>). The 200 response's FirewallRuleGroupAssociation must carry StatusMessage "Finished rule group association update" alongside Status, not omit it. The same shape is returned by ListFirewallRuleGroupAssociations, UpdateFirewallRuleGroupAssociation and DisassociateFirewallRuleGroup, so verify StatusMessage is projected on those paths too once the shared serializer is fixed.
