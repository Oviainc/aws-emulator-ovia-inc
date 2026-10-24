Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.batch-update-firewall-rule-limit-exceeded-rejected route53resolver.create-firewall-domain-list-name-over-max-length-rejected route53resolver.create-firewall-rule-invalid-group-not-found route53resolver.e2e-firewall-domain-block-full-lifecycle route53resolver.e2e-firewall-domain-import-block-consequence route53resolver.firewall-rule-override-ttl-numeric-precision-round-trip route53resolver.import-firewall-domains-conflict-while-in-progress route53resolver.import-firewall-domains-conflict-while-in-progress-rejected route53resolver.import-firewall-domains-invalid-s3-uri-rejected route53resolver.list-firewall-domains-pagination-no-duplicates route53resolver.tag-resource-arn-not-found route53resolver.tag-untag-resource-lifecycle route53resolver.tag-value-unicode-and-empty-round-trip route53resolver.update-firewall-domains-replace-replaces-whole-set
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateFirewallDomainList|NotImplemented
Issue-Commit: df1261813c70037074c7525ba4fddd96df97f3b7

# Implement route53resolver CreateFirewallDomainList: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `df1261813c70037074c7525ba4fddd96df97f3b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallDomainList

## Description
`CreateFirewallDomainList` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_domain_list.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `CreateFirewallDomainList` call, so each one unblocks only when `CreateFirewallDomainList` works: `route53resolver.batch-update-firewall-rule-limit-exceeded-rejected`, `route53resolver.create-firewall-domain-list-name-over-max-length-rejected`, `route53resolver.create-firewall-rule-invalid-group-not-found`, `route53resolver.e2e-firewall-domain-block-full-lifecycle`, `route53resolver.e2e-firewall-domain-import-block-consequence`, `route53resolver.firewall-rule-override-ttl-numeric-precision-round-trip`, `route53resolver.import-firewall-domains-conflict-while-in-progress`, `route53resolver.import-firewall-domains-conflict-while-in-progress-rejected`, `route53resolver.import-firewall-domains-invalid-s3-uri-rejected`, `route53resolver.list-firewall-domains-pagination-no-duplicates`, `route53resolver.tag-resource-arn-not-found`, `route53resolver.tag-untag-resource-lifecycle`, `route53resolver.tag-value-unicode-and-empty-round-trip`, `route53resolver.update-firewall-domains-replace-replaces-whole-set`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_domain_list.rs`. Replaying the corpus, the emulator refused the `CreateFirewallDomainList` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateFirewallDomainList operation: operation CreateFirewallDomainList is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53resolver emulator
Call CreateFirewallDomainList and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
