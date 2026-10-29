Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.associate-firewall-rule-group-duplicate-priority-conflict route53resolver.concurrent-update-outpost-resolver-optimistic-lock route53resolver.firewall-association-mutation-protection-priority-change-rejected route53resolver.update-firewall-rule-group-association-mutation-protected route53resolver.update-firewall-rule-group-association-priority
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|AssociateFirewallRuleGroup|NotImplemented
Issue-Commit: b164246765c3b0c82033950b5a9f668e079ece5f

# Implement route53resolver AssociateFirewallRuleGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b164246765c3b0c82033950b5a9f668e079ece5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateFirewallRuleGroup

## Description
`AssociateFirewallRuleGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/associate_firewall_rule_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `AssociateFirewallRuleGroup` call, so each one unblocks only when `AssociateFirewallRuleGroup` works: `route53resolver.associate-firewall-rule-group-duplicate-priority-conflict`, `route53resolver.concurrent-update-outpost-resolver-optimistic-lock`, `route53resolver.firewall-association-mutation-protection-priority-change-rejected`, `route53resolver.update-firewall-rule-group-association-mutation-protected`, `route53resolver.update-firewall-rule-group-association-priority`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/associate_firewall_rule_group.rs`. Replaying the corpus, the emulator refused the `AssociateFirewallRuleGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociateFirewallRuleGroup operation: operation AssociateFirewallRuleGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53resolver emulator
Call AssociateFirewallRuleGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
