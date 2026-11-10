Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53globalresolver.create-firewall-domain-list-idempotency-token-does-not-mask-rejection
Issue-Shape-Id: com.amazonaws.route53globalresolver#EC2DNSGlobalResolverCustomerAPI
Issue-Key: route53globalresolver.create-firewall-domain-list-idempotency-token-does-not-mask-rejection|run#8|AssertionError|assert first_code == "ResourceNotFoundException", (
Issue-Commit: 63738de0101ebf6c323b1ab59b247c4705f0deaf

# Implement route53globalresolver CreateFirewallDomainList: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `63738de0101ebf6c323b1ab59b247c4705f0deaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallDomainList

## Description
`CreateFirewallDomainList` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi/src/ops/create_firewall_domain_list.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateFirewallDomainList` call, so each one unblocks only when `CreateFirewallDomainList` works: `route53globalresolver.create-firewall-domain-list-idempotency-token-does-not-mask-rejection`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi/src/ops/create_firewall_domain_list.rs`. Replaying the corpus, the emulator refused the `CreateFirewallDomainList` call: File "run.py", line 365, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_route53globalresolver_create_firewall_domain_list_idempotency_token_does_not_mask_rejection.py", line 40, in run
    assert first_code == "ResourceNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException on the first call, got NotImplemented: 'operation CreateFirewallDomainList is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the route53globalresolver emulator
Call CreateFirewallDomainList and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
