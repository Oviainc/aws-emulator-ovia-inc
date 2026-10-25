Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.associate-resolver-rule-invalid-vpc-rejected route53resolver.concurrent-resolver-rule-association-same-vpc-race route53resolver.create-forward-resolver-rule-requires-endpoint-and-targets route53resolver.create-resolver-rule-system-forbids-target-ips-rejected route53resolver.creator-request-id-idempotency-token-preserved route53resolver.delete-resolver-rule-in-use-rejected route53resolver.disassociate-resolver-rule-not-associated-invalid-request route53resolver.e2e-resolver-rule-forward-and-query-log-flow route53resolver.get-resolver-rule-policy-without-policy route53resolver.list-resolver-rule-associations-stable-ordering route53resolver.resolver-rule-domain-name-trailing-dot-normalized route53resolver.resolver-rule-target-ip-list-order-preserved route53resolver.resolver-rule-target-ip-port-round-trip route53resolver.system-resolver-rule-and-policy
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateResolverRule|NotImplemented
Issue-Commit: df1261813c70037074c7525ba4fddd96df97f3b7

# Implement route53resolver CreateResolverRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `df1261813c70037074c7525ba4fddd96df97f3b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolverRule

## Description
`CreateResolverRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_resolver_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `CreateResolverRule` call, so each one unblocks only when `CreateResolverRule` works: `route53resolver.associate-resolver-rule-invalid-vpc-rejected`, `route53resolver.concurrent-resolver-rule-association-same-vpc-race`, `route53resolver.create-forward-resolver-rule-requires-endpoint-and-targets`, `route53resolver.create-resolver-rule-system-forbids-target-ips-rejected`, `route53resolver.creator-request-id-idempotency-token-preserved`, `route53resolver.delete-resolver-rule-in-use-rejected`, `route53resolver.disassociate-resolver-rule-not-associated-invalid-request`, `route53resolver.e2e-resolver-rule-forward-and-query-log-flow`, `route53resolver.get-resolver-rule-policy-without-policy`, `route53resolver.list-resolver-rule-associations-stable-ordering`, `route53resolver.resolver-rule-domain-name-trailing-dot-normalized`, `route53resolver.resolver-rule-target-ip-list-order-preserved`, `route53resolver.resolver-rule-target-ip-port-round-trip`, `route53resolver.system-resolver-rule-and-policy`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_resolver_rule.rs`. Replaying the corpus, the emulator refused the `CreateResolverRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateResolverRule operation: operation CreateResolverRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53resolver emulator
Call CreateResolverRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
