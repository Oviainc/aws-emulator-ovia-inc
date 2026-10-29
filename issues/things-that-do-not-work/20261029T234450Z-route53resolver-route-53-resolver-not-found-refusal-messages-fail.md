Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-dns-firewall-domain-list-lifecycle
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-dns-firewall-domain-list-lifecycle route53resolver-firewall-domain-list-lifecycle-with-listing route53resolver-query-log-config-missing-resource-errors route53resolver-vpc-config-and-association-missing-resource-errors
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|GetFirewallDomainList|4xx->4xx|$.Message
Issue-Key: route53resolver|GetResolverQueryLogConfig|4xx->4xx|$.Message
Issue-Key: route53resolver|GetResolverQueryLogConfigAssociation|4xx->4xx|$.Message
Issue-Key: route53resolver|DisassociateResolverQueryLogConfig|4xx->4xx|$.Message
Issue-Key: route53resolver|AssociateFirewallRuleGroup|4xx->4xx|$.Message
Issue-Key: route53resolver|GetFirewallRuleGroupAssociation|4xx->4xx|$.Message
Issue-Key: route53resolver|UpdateFirewallRuleGroupAssociation|4xx->4xx|$.Message
Issue-Key: route53resolver|DisassociateFirewallRuleGroup|4xx->4xx|$.Message
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# Route 53 Resolver not-found refusal Messages fail the recorded shape: no RSLVR code and no trailing Trace Id clause (8 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFirewallDomainList

## Description
Eight net-new wire causes where the emulator returns the right HTTP status (400) and the right error class but a Message that does not conform to the recorded message shape. The shared fault: the crate's refusal text is assembled per-handler instead of by one service-wide formatter, so RSLVR-coded sentences stop at the final period (no ' Trace Id: "<Root>"' clause) and the query-log paths emit no RSLVR code at all. Service route53resolver, crate services/service-com-amazonaws-route53resolver-route53resolver.

Shared AWS grounding: these errors (ResourceNotFoundException{Message, ResourceType}, ConflictException{Message}, ValidationException{Message}, UnknownResourceException{Message}) model only a free-text Message, so the wording is recorded behavior only. Elsewhere in this same corpus the recorded literals show the service's shape explicitly — e.g. '[RSLVR-02035] ... shared with another AWS account. Trace Id: "1-6a76e074-570e69780dc8a349626ecf87"' and '[RSLVR-02103] The DNS Firewall rule group has an associated rule: "<id>".  Delete the rule and try again later. Trace Id: "<Root>"'. So a Resolver refusal Message is: '[RSLVR-nnnnn] ' + the service's fixed sentence (ids/values from the request interpolated) + ' Trace Id: "<Root of the request\'s X-Amzn-Trace-Id>"'. The Root is a per-request value; the PRESENCE of the clause, its exact '. Trace Id: "' framing and the RSLVR code are the requirement. Hypothesis: for the query-log config/association family the emulator's own [RSLVR-01602] shows the code family in use, so the corresponding config-level sentence is RSLVR-coded too — the code digits for the config read are not stated by the recording, only that the emulator's uncoded prose fails the shape.

=== Cause 1 of 8: GetFirewallDomainList's RSLVR-02025 not-found Message has no Trace Id clause ===
AWS: Smithy: GetFirewallDomainList(FirewallDomainListId required) -> {FirewallDomainList}; ResourceNotFoundException (400). Required form: '[RSLVR-02025] Can\u2019t find the resource with ID "<FirewallDomainListId>". Trace Id: "<Root>"' — fixed text with the typographic apostrophe; variable: the id from the request, plus the Root.
Emulator: '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-fdl-0123456789abcdef0".' — correct code, correct sentence, no trace clause.
Why: a client or support workflow that extracts the trace Root from a refusal to correlate it with the request has nothing to parse, and exact-message assertions fail.
Fix: the crate's RSLVR message formatter / error serializer must append the trace clause from the request's X-Amzn-Trace-Id on every RSLVR refusal, including this read path. Open issue 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md names the same omission for other operations but does not carry these fixture steps' replay keys; these causes are filed here and may be closed by that fix.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#12 (cluster also covers #teardown#14 and route53resolver-firewall-domain-list-lifecycle-with-listing#teardown#8..#9)

=== Cause 2 of 8: GetResolverQueryLogConfig's not-found Message is uncoded emulator prose ===
AWS: Smithy: GetResolverQueryLogConfig(ResolverQueryLogConfigId required) -> {ResolverQueryLogConfig}; errors include ResourceNotFound (400). Required form: an RSLVR-coded sentence naming the missing query logging configuration, ending with the Trace Id clause.
Emulator: 'Resolver query log config with ID "rqlc-0123456789abcdef0" does not exist.' — no RSLVR code, emulator wording, no trace clause.
Why: clients that branch on the RSLVR code cannot recognise the refusal at all on this path.
Fix: route this handler's not-found through the shared RSLVR formatter.
Traces: route53resolver-query-log-config-missing-resource-errors#task#4

=== Cause 3 of 8: GetResolverQueryLogConfigAssociation's RSLVR-01602 Message has no Trace Id clause ===
AWS: Smithy: GetResolverQueryLogConfigAssociation(ResolverQueryLogConfigAssociationId required) -> {ResolverQueryLogConfigAssociation}. Required form: "[RSLVR-01602] The specified query logging configuration association doesn't exist. Trace Id: \"<Root>\"" — the sentence is entirely fixed text (the id is not echoed), the Root is per-request.
Emulator: the sentence and code are right; the Message ends at the period.
Why: as cause 1 — the last field of the message is missing.
Fix: the shared formatter's trace clause on this path.
Traces: route53resolver-query-log-config-missing-resource-errors#task#6

=== Cause 4 of 8: DisassociateResolverQueryLogConfig's not-found Message is uncoded emulator prose ===
AWS: Smithy: DisassociateResolverQueryLogConfig(ResolverQueryLogConfigId required, ResourceId required) -> {ResolverQueryLogConfigAssociation}; errors include ResourceNotFound (400). Required form: the RSLVR-coded query-logging-configuration sentence plus the Trace Id clause.
Emulator: 'Resolver query log config with ID "rqlc-0123456789abcdef0" does not exist.' — as cause 2.
Why: a teardown path that ignores a specific coded refusal (already-disassociated) cannot identify it.
Fix: as cause 2, on the disassociate handler.
Traces: route53resolver-query-log-config-missing-resource-errors#task#7

=== Cause 5 of 8: AssociateFirewallRuleGroup's RSLVR-02025 Message has no Trace Id clause ===
AWS: Smithy: AssociateFirewallRuleGroup(CreatorRequestId required, FirewallRuleGroupId required, VpcId required, Priority required, Name required, MutationProtection, Tags) -> {FirewallRuleGroupAssociation}; errors include ResourceNotFound (400). Required form: '[RSLVR-02025] Can\u2019t find the resource with ID "<FirewallRuleGroupId>". Trace Id: "<Root>"'.
Emulator: the coded sentence naming the rule group id, with the trace clause missing.
Why: as cause 1.
Fix: the shared formatter's trace clause on the associate path.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#9

=== Cause 6 of 8: GetFirewallRuleGroupAssociation's RSLVR-02025 Message has no Trace Id clause ===
AWS: Smithy: GetFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId required) -> {FirewallRuleGroupAssociation}; ResourceNotFoundException (400). Required form as cause 5, with the association id as the variable part.
Emulator: coded sentence, no trace clause.
Why: as cause 1.
Fix: as cause 5.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#10

=== Cause 7 of 8: UpdateFirewallRuleGroupAssociation's RSLVR-02025 Message has no Trace Id clause ===
AWS: Smithy: UpdateFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId required, Priority, MutationProtection, Name) -> {FirewallRuleGroupAssociation}; ResourceNotFoundException (400). Required form as cause 6.
Emulator: coded sentence, no trace clause.
Why: as cause 1.
Fix: as cause 5.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#11

=== Cause 8 of 8: DisassociateFirewallRuleGroup's RSLVR-02025 Message has no Trace Id clause ===
AWS: Smithy: DisassociateFirewallRuleGroup(FirewallRuleGroupAssociationId required) -> {FirewallRuleGroupAssociation}; ResourceNotFoundException (400). Required form as cause 6.
Emulator: coded sentence, no trace clause.
Why: as cause 1.
Fix: as cause 5. (Behavioral issue 20261029T193214Z-route53resolver-dns-firewall-control-plane-10-net-new.md describes the same omission for DisassociateFirewallRuleGroup from check ids; it carries no fixture step, so this wire cause is filed here and may be closed by the same fix.)
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#12

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). Eight mismatch clusters, all status 400 -> 400, none stub-blocked, none owned. In each the corpus asserts the Message by shape ('<shaped-not-asserted>') and the emulator's value fails it.

Cause 1 — GetFirewallDomainList, route53resolver-dns-firewall-domain-list-lifecycle#task#12:
  got '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-fdl-0123456789abcdef0".'  (no trailing ' Trace Id: "<Root>"')   [4 affected steps]
Cause 2 — GetResolverQueryLogConfig, route53resolver-query-log-config-missing-resource-errors#task#4:
  got 'Resolver query log config with ID "rqlc-0123456789abcdef0" does not exist.'  (no RSLVR code, no trace clause)
Cause 3 — GetResolverQueryLogConfigAssociation, ...#task#6:
  got "[RSLVR-01602] The specified query logging configuration association doesn't exist."  (no trace clause)
Cause 4 — DisassociateResolverQueryLogConfig, ...#task#7:
  got 'Resolver query log config with ID "rqlc-0123456789abcdef0" does not exist.'
Cause 5 — AssociateFirewallRuleGroup, route53resolver-vpc-config-and-association-missing-resource-errors#task#9:
  got '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-frg-0123456789abcdef0".'
Cause 6 — GetFirewallRuleGroupAssociation, ...#task#10:
  got '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-frgassoc-0123456789abcdef0".'
Cause 7 — UpdateFirewallRuleGroupAssociation, ...#task#11:
  got '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-frgassoc-0123456789abcdef0".'
Cause 8 — DisassociateFirewallRuleGroup, ...#task#12:
  got '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-frgassoc-0123456789abcdef0".'

Reference literals from the same corpus showing the required shape: '[RSLVR-02035] ... shared with another AWS account. Trace Id: "1-6a76e074-570e69780dc8a349626ecf87"' (PutFirewallRuleGroupPolicy) and '[RSLVR-02103] ... Delete the rule and try again later. Trace Id: "1-6a76e073-7375192c5bab22b5782fa6c7"' (DeleteFirewallRuleGroup). The Root value is per-request; the clause is the requirement.

## Steps to Reproduce
Send X-Amzn-Trace-Id: Root=<root>;Sampled=0 on the client, then replay fixtures route53resolver-dns-firewall-domain-list-lifecycle (task#12), route53resolver-query-log-config-missing-resource-errors (task#4, task#6, task#7) and route53resolver-vpc-config-and-association-missing-resource-errors (task#9..task#12). Minimal reproductions, each against a well-formed id that was never created:
1. GetFirewallDomainList(FirewallDomainListId='rslvr-fdl-0123456789abcdef0')
2. GetResolverQueryLogConfig(ResolverQueryLogConfigId='rqlc-0123456789abcdef0')
3. GetResolverQueryLogConfigAssociation(ResolverQueryLogConfigAssociationId='rqlcassoc-0123456789abcdef0')
4. DisassociateResolverQueryLogConfig(ResolverQueryLogConfigId='rqlc-0123456789abcdef0', ResourceId=<vpc>)
5. AssociateFirewallRuleGroup(CreatorRequestId=<id>, FirewallRuleGroupId='rslvr-frg-0123456789abcdef0', VpcId=<vpc>, Priority=150, Name=<n>)
6. GetFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId='rslvr-frgassoc-0123456789abcdef0')
7. UpdateFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId=same, Priority=200)
8. DisassociateFirewallRuleGroup(FirewallRuleGroupAssociationId=same)
In every case the Error.Message must carry the RSLVR code for that refusal and end with ' Trace Id: "<root>"'.
