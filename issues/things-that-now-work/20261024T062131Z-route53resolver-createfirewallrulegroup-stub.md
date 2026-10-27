Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.associate-firewall-rule-group-duplicate-priority-conflict route53resolver.batch-create-firewall-rule-partial-failure route53resolver.batch-delete-firewall-rule-removes-targeted-rules route53resolver.batch-update-and-delete-firewall-rule route53resolver.concurrent-update-outpost-resolver-optimistic-lock route53resolver.create-firewall-rule-group-all-digit-name-rejected route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness route53resolver.delete-firewall-rule-group-with-vpc-association-conflict route53resolver.firewall-association-mutation-protection-priority-change-rejected route53resolver.firewall-rule-group-policy-lifecycle route53resolver.put-firewall-rule-group-policy-self-share-rejected route53resolver.update-firewall-rule-group-association-mutation-protected route53resolver.update-firewall-rule-group-association-priority
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateFirewallRuleGroup|NotImplemented
Issue-Commit: df1261813c70037074c7525ba4fddd96df97f3b7

# Implement route53resolver CreateFirewallRuleGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `df1261813c70037074c7525ba4fddd96df97f3b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallRuleGroup

## Description
`CreateFirewallRuleGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_rule_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `CreateFirewallRuleGroup` call, so each one unblocks only when `CreateFirewallRuleGroup` works: `route53resolver.associate-firewall-rule-group-duplicate-priority-conflict`, `route53resolver.batch-create-firewall-rule-partial-failure`, `route53resolver.batch-delete-firewall-rule-removes-targeted-rules`, `route53resolver.batch-update-and-delete-firewall-rule`, `route53resolver.concurrent-update-outpost-resolver-optimistic-lock`, `route53resolver.create-firewall-rule-group-all-digit-name-rejected`, `route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness`, `route53resolver.delete-firewall-rule-group-with-vpc-association-conflict`, `route53resolver.firewall-association-mutation-protection-priority-change-rejected`, `route53resolver.firewall-rule-group-policy-lifecycle`, `route53resolver.put-firewall-rule-group-policy-self-share-rejected`, `route53resolver.update-firewall-rule-group-association-mutation-protected`, `route53resolver.update-firewall-rule-group-association-priority`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_rule_group.rs`. Replaying the corpus, the emulator refused the `CreateFirewallRuleGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateFirewallRuleGroup operation: operation CreateFirewallRuleGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53resolver emulator
Call CreateFirewallRuleGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
