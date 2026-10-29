Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.update-firewall-rule-invalid-action-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.update-firewall-rule-invalid-action-rejected|run#8|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Implement route53resolver UpdateFirewallRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFirewallRule

## Description
`UpdateFirewallRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/update_firewall_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateFirewallRule` call, so each one unblocks only when `UpdateFirewallRule` works: `route53resolver.update-firewall-rule-invalid-action-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/update_firewall_rule.rs`. Replaying the corpus, the emulator refused the `UpdateFirewallRule` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_route53resolver_update_firewall_rule_invalid_action_rejected.py", line 49, in run
    assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected ('ValidationException', "[RSLVR-02010] You can't provide a block option if the ACTION is ALLOW or ALERT."), got ('NotImplemented', 'operation UpdateFirewallRule is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the route53resolver emulator
Call UpdateFirewallRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
