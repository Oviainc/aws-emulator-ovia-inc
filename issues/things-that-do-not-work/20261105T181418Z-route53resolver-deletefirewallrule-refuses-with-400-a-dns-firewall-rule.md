Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-dns-firewall-rule-group-and-rules
Issue-Step: teardown#20
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-dns-firewall-rule-group-and-rules
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|DeleteFirewallRule|2xx->4xx|$.FirewallRule
Issue-Key: route53resolver|DeleteFirewallRule|2xx->4xx|status
Issue-Commit: 475d7014ed423f1edb047e4bfa101e91e3c4464d

# DeleteFirewallRule refuses with 400 a DNS Firewall rule the emulator itself created, instead of 200 projecting the deleted rule

> _Note: this issue was identified against the code base at commit `475d7014ed423f1edb047e4bfa101e91e3c4464d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFirewallRule

## Description
One net-new wire cause in the DNS Firewall rule plane of services/service-com-amazonaws-route53resolver-route53resolver: a rule that CreateFirewallRule accepted cannot be deleted — DeleteFirewallRule answers a 4xx refusal with no body instead of deleting the rule and projecting it.

=== Cause 1 of 1: DeleteFirewallRule answers 400 with no FirewallRule for a rule that exists ===
AWS: Smithy: DeleteFirewallRule(FirewallRuleGroupId required @length 1..64, FirewallDomainListId @length 1..64, FirewallThreatProtectionId @length 1..64, Qtype @length 1..16) -> DeleteFirewallRuleResponse{FirewallRule}; modeled errors AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException, every one HTTP 400. Recorded behavior: deleting an existing rule of an existing rule group succeeds with HTTP 200 and the response carries the deleted rule's full projection — Action, CreationTime, CreatorRequestId, FirewallDomainListId, FirewallDomainRedirectionAction, FirewallRuleGroupId, ModificationTime, Name and Priority. The 4xx refusal classes are reserved for a rule-group id or domain-list id that does not identify a rule; a rule the service itself created and has not deleted is not one of those cases. The rule-group id, domain-list id, rule name, creator request id, priority and the two timestamps in the recording are per-run values that fill that projection, not part of the requirement.
Emulator: HTTP 400 (replay: "status: expected 200, got 400") with no FirewallRule member at all ("$.FirewallRule: missing ... got None"), for the ALERT rule the same fixture created earlier in the same run against the same rule group.
Why: the rule plane has no delete path at all for a live rule. A client that follows AWS's documented teardown order (delete the group's rules, then the group, then the domain list) is refused at the first step, so the rule, its group and the domain list it references all leak and every later run collides with them; a reconciliation loop that deletes and recreates rules to change priority or action can never converge, and a caller cannot distinguish this refusal from a genuine ResourceNotFoundException.
Fix: the DeleteFirewallRule handler and the rule index it looks the rule up in, in the route53resolver crate's state module — the lookup must find a rule written by CreateFirewallRule, remove it, decrement the owning group's RuleCount, and serialize the removed rule into the response's FirewallRule member; only a genuinely absent rule may take a 400 refusal class.
Related open issues that do NOT cover this step: 20261029T234450Z-route53resolver-route-53-resolver-dns-firewall-state-managed-catalogs.md carries the read-side symptom of the same broken rule index (cause 5, ListFirewallRules omitting a created rule, at task#10) and the group delete (cause 6, DeleteFirewallRuleGroup at teardown#21), and 20261029T193214Z-route53resolver-dns-firewall-control-plane-10-net-new.md covers RSLVR wording and delete-while-in-use; none of them carries this step's replay key, so the DeleteFirewallRule write path is unexercised by their replays.
Traces: route53resolver-dns-firewall-rule-group-and-rules#teardown#20

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). One mismatch cluster, not stub-blocked, not owned by any open issue.

Cause 1 — DeleteFirewallRule, route53resolver-dns-firewall-rule-group-and-rules#teardown#20, expected 200 -> actual 400:
  status: expected 200, got 400
  $.FirewallRule: missing (expected the deleted rule's projection {Action 'ALERT', CreationTime, CreatorRequestId, FirewallDomainListId, FirewallDomainRedirectionAction 'INSPECT_REDIRECTION_DOMAIN', FirewallRuleGroupId, ModificationTime, Name, Priority 100}, got None)

The same fixture created that rule earlier in the run (CreateFirewallRule, Action=ALERT, Priority=100, against the group and domain list it had just created), so the rule existed when the delete was issued. The emulator's 400 therefore cannot be a modeled ResourceNotFoundException outcome. Note the read side of the same state is already known-broken (ListFirewallRules does not return the created rule), which is consistent with the delete failing on a rule-index lookup rather than on a precondition.

## Steps to Reproduce
Replay fixture route53resolver-dns-firewall-rule-group-and-rules, step teardown#20. Minimal reproduction:
1. CreateFirewallRuleGroup(Name=<group>) -> note FirewallRuleGroupId.
2. CreateFirewallDomainList(Name=<list>) -> note FirewallDomainListId.
3. CreateFirewallRule(FirewallRuleGroupId=<group>, FirewallDomainListId=<list>, Action=ALERT, Priority=100, Name=<rule>) -> 200.
4. DeleteFirewallRule(FirewallRuleGroupId=<group>, FirewallDomainListId=<list>) -> must answer HTTP 200 with FirewallRule projecting the deleted rule (Action, CreationTime, CreatorRequestId, FirewallDomainListId, FirewallDomainRedirectionAction, FirewallRuleGroupId, ModificationTime, Name, Priority). The emulator answers HTTP 400 with no FirewallRule.
5. Follow with DeleteFirewallRuleGroup(<group>) and DeleteFirewallDomainList(<list>) to confirm the teardown chain completes once the rule delete succeeds.
