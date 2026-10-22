Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-firewall.encryption-configuration-defaults-to-aws-owned-key network-firewall.firewall-policy-update-replaces-rule-group-reference network-firewall.list-rule-groups-pagination-reflects-created network-firewall.rule-group-referenced-by-policy-delete-rejected network-firewall.rule-group-update-persists-new-rules network-firewall.tag-and-untag-resource-roundtrip network-firewall.update-rule-group-exceeds-capacity-rejected
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateRuleGroup|NotImplemented
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Implement network-firewall CreateRuleGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRuleGroup

## Description
`CreateRuleGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/create_rule_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreateRuleGroup` call, so each one unblocks only when `CreateRuleGroup` works: `network-firewall.encryption-configuration-defaults-to-aws-owned-key`, `network-firewall.firewall-policy-update-replaces-rule-group-reference`, `network-firewall.list-rule-groups-pagination-reflects-created`, `network-firewall.rule-group-referenced-by-policy-delete-rejected`, `network-firewall.rule-group-update-persists-new-rules`, `network-firewall.tag-and-untag-resource-roundtrip`, `network-firewall.update-rule-group-exceeds-capacity-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/create_rule_group.rs`. Replaying the corpus, the emulator refused the `CreateRuleGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRuleGroup operation: operation CreateRuleGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the network-firewall emulator
Call CreateRuleGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
