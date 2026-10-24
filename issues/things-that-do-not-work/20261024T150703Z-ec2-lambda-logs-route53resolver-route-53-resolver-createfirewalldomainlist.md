Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__logs__route53resolver.firewall-rule-priority-decides-which-action-the-resolver-applies ec2__lambda__logs__route53resolver.deleting-a-firewall-domain-list-in-use-rejected-and-it-keeps-blocking
Issue-Key: ec2__lambda__logs__route53resolver|CreateFirewallDomainList|NotImplemented
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# ec2__lambda__logs__route53resolver: Route 53 Resolver CreateFirewallDomainList is an unimplemented stub, so DNS Firewall rule-priority and in-use-deletion behaviour is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallDomainList

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: route53resolver CreateFirewallDomainList answers NotImplemented ===
AWS: CreateFirewallDomainList creates the domain list that every DNS Firewall rule references; it returns a FirewallDomainList with an Id and ARN, after which ImportFirewallDomains / UpdateFirewallDomains populate it and CreateFirewallRule binds it into a rule group at a priority. Two behaviours depend on it and on nothing else: the rule at the lower numeric priority decides the action the resolver applies, and DeleteFirewallDomainList for a list still referenced by a rule is refused (the list keeps blocking).
Emulator: the call answers error code NotImplemented with message 'operation CreateFirewallDomainList is scaffolded but not implemented in this emulator', so neither check gets past its first setup step. No existing filed stub issue names CreateFirewallDomainList (the auto-filed route53resolver stubs are UpdateResolverDnssecConfig and CreateResolverQueryLogConfig).
Why: DNS Firewall cannot be configured at all. A client's priority ordering and referential-integrity expectations — the two things that decide whether a block actually takes effect — are entirely unverifiable.
Fix: the CreateFirewallDomainList handler in services/service-com-amazonaws-route53resolver-route53resolver/src/ops/, plus the in-use check on DeleteFirewallDomainList and the priority resolution in the rule evaluator the two checks then grade.
Traces: ec2__lambda__logs__route53resolver.firewall-rule-priority-decides-which-action-the-resolver-applies, ec2__lambda__logs__route53resolver.deleting-a-firewall-domain-list-in-use-rejected-and-it-keeps-blocking.

## Evidence
Both checks end in: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateFirewallDomainList operation: operation CreateFirewallDomainList is scaffolded but not implemented in this emulator.
Expected: HTTP 200 with FirewallDomainList{Id, Arn, Name, Status}; then the rule-priority check expects the lower-priority rule's action to be applied, and the in-use-deletion check expects DeleteFirewallDomainList on a referenced list to be refused with a 400 client error while the list keeps blocking resolution.

## Steps to Reproduce
route53resolver CreateFirewallDomainList(CreatorRequestId=<uuid>, Name='sim2real-list') — answers NotImplemented instead of 200. Without it, CreateFirewallRule cannot reference a domain list and neither the priority nor the in-use-deletion behaviour can be exercised.
