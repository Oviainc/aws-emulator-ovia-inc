Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-catalog-list-firewall-domain-lists
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-catalog-list-firewall-domain-lists route53resolver-resolver-rule-system-lifecycle
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:000000000000:firewall-domain-list/rslvr-fdl-1bc724e346019cfb"]
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:000000000000:firewall-domain-list/rslvr-fdl-4ab5894cbe1429d4"]
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:000000000000:firewall-domain-list/rslvr-fdl-cb4f4cfd74cafd8a"]
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:229214028842:firewall-domain-list/rslvr-fdl-23584f0fb4c24ebb"]
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:229214028842:firewall-domain-list/rslvr-fdl-c80983a1f0284c99"]
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:229214028842:firewall-domain-list/rslvr-fdl-d252ee1944404e15"]
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:229214028842:firewall-domain-list/rslvr-fdl-fe17696f22a445f6"]
Issue-Key: route53resolver|ListResolverRules|2xx->2xx|$.ResolverRules[Arn="arn:aws:route53resolver:us-west-2:000000000000:resolver-rule/rslvr-rr-910a2dec89025cc1"].StatusMessage
Issue-Commit: 569322fd02c2429e9947d96bd3e38eb76e62453b

# Route 53 Resolver list projections drop managed domain-list catalog entries and ResolverRule.StatusMessage (2 causes)

> _Note: this issue was identified against the code base at commit `569322fd02c2429e9947d96bd3e38eb76e62453b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFirewallDomainLists

## Description
Two net-new wire causes on the Route 53 Resolver list/read projections of services/service-com-amazonaws-route53resolver-route53resolver. They share one fault: a List* response is served as an incomplete projection - a whole class of catalog entries is absent from one list, and a modeled member is absent from the elements of the other. Both are reported by the replay as UNOWNED (no open issue carries their replay key), so neither is gated by an existing fix; two open issues describe the same behavior in prose but do not cover these steps and must not be relied on to close them (see each cause).

=== Cause 1 of 2: ListFirewallDomainLists omits the AWS-managed DNS Firewall domain lists ===
AWS: Smithy: ListFirewallDomainLists(MaxResults @range 1..100, NextToken) -> ListFirewallDomainListsResponse{NextToken, FirewallDomainLists: FirewallDomainListMetadataList}; errors AccessDeniedException | InternalServiceErrorException | ThrottlingException | ValidationException (all 400); paginated items=FirewallDomainLists. Shape FirewallDomainListMetadata models Id, Arn, Name, CreatorRequestId plus ManagedOwnerName (doc: "The owner of the list, used only for lists that are not managed by you. For example, the managed domain list AWSManagedDomainsMalwareDomainList has the managed owner name Route 53 Resolver DNS Firewall"), ManagedListType (enum DomainListType, "The type of the managed domain list, for example THREAT") and Category ("The category of the domain list"). Which lists are service-managed is recorded behavior only: every account and Region is served the service-managed threat lists in addition to the caller's own, each with Name and CreatorRequestId equal to the managed-list name (AWSManagedDomainsAmazonGuardDutyThreatList, AWSManagedDomainsBotnetCommandandControl, AWSManagedDomainsAggregateThreatList, AWSManagedDomainsMalwareDomainList), ManagedOwnerName "Route 53 Resolver DNS Firewall", ManagedListType THREAT, and a per-list Category ("Amazon GuardDuty Threat List", "Botnet/Command and Control", "Aggregate Threat List", "Malware"). The names, categories, owner name and type are the fixed part of the requirement; the recorded list ids and the ids/ARNs of the caller-owned entries in the same response are per-run values, not requirements.
Emulator: HTTP 200 whose FirewallDomainLists contains no managed entry at all - the replay reports every managed ARN as "missing ... got None".
Why: referencing a managed threat list from a FirewallRule is the documented way to use DNS Firewall; a client that resolves a managed list by name before CreateFirewallRule finds no list to reference and cannot build a rule group at all, and a client that renders or filters the account's domain lists sees a catalog AWS never serves.
Fix: the DNS Firewall domain-list catalog seeded by the crate's state module must contain the service-managed lists, and the ListFirewallDomainLists projection (shared with GetFirewallDomainList) must emit ManagedOwnerName, ManagedListType and Category for them. Open issue 20261029T234450Z-route53resolver-route-53-resolver-dns-firewall-state-managed-catalogs.md describes this behavior as its Cause 1, but the replay reports this cluster as unowned - its replay key does not cover this step, so this cause must be closed and re-replayed on its own.
Traces: route53resolver-catalog-list-firewall-domain-lists#task#1

=== Cause 2 of 2: ListResolverRules omits StatusMessage on each listed ResolverRule ===
AWS: Smithy: ListResolverRules(MaxResults @range 1..100, NextToken, Filters) -> ListResolverRulesResponse{NextToken, MaxResults, ResolverRules}; errors InternalServiceErrorException | InvalidNextTokenException | InvalidParameterException | InvalidRequestException | ThrottlingException (all 400). Each element is the full ResolverRule shape, which models StatusMessage (target StatusMessage, @length 0..255, doc "A detailed description of the status of a Resolver rule.") alongside Status (enum ResolverRuleStatus); the shape doc states the ResolverRule parameter appears in the response to CreateResolverRule, DeleteResolverRule, GetResolverRule, ListResolverRules and UpdateResolverRule - i.e. the list element is the same projection as the get. Recorded behavior: a rule listed just after creation carries the same non-empty created-state status sentence the create and get returned (the fixture binds the list element's StatusMessage to the create's value).
Emulator: HTTP 200 in which the listed rule matching the created rule's ARN has no StatusMessage member at all (replay: "$.ResolverRules[Arn=...].StatusMessage: missing (expected '{{rule_status_message_created}}', got None)"). The ARN and rule id in that key are per-run values; the requirement is that the member is present on every listed rule.
Why: a client that builds its view of Resolver rules from ListResolverRules (the only non-N+1 way to enumerate them) can never show why a rule is in its current Status, and code that reads StatusMessage off a listed rule gets a missing key rather than a sentence - so list and get disagree with AWS in the same way and neither can be used as the source of truth.
Fix: the ListResolverRules handler must project rules through the one ResolverRule serializer that emits StatusMessage from the rule record's status, rather than a reduced list-only projection. Open issue 20261029T234450Z-route53resolver-route-53-resolver-rule-projections-omit-resolverrule-statusmessage.md describes this as its Cause 3 but the replay reports this cluster as unowned - its replay key covers the create/get/update/delete steps and not this list step, so a fix there can pass without this step ever being exercised.
Traces: route53resolver-resolver-rule-system-lifecycle#task#3

## Evidence
Wire corpus simparity-wire-aws-v1, trace set simparity-wire-aws-v1, service route53resolver (bootstrapped). Replay summary: 167 steps, 113 passed, 54 mismatched, score 67.7%, emulator_up true, bootstrap_signal false. 44 divergence clusters, of which these two are the only ones neither stub-blocked nor owned by an open issue.

Cause 1 - ListFirewallDomainLists, fixture route53resolver-catalog-list-firewall-domain-lists, step task#1, status 200 -> 200 (verdict mismatch, 1 affected step):
  $.FirewallDomainLists[Arn=".../firewall-domain-list/rslvr-fdl-23584f0fb4c24ebb"]: missing (expected {'Category': 'Amazon GuardDuty Threat List', 'CreatorRequestId': 'AWSManagedDomainsAmazonGuardDutyThreatList', 'Name': 'AWSManagedDomainsAmazonGuardDutyThreatList', 'ManagedListType': 'THREAT', 'ManagedOwnerName': 'Route 53 Resolver DNS Firewall'}, got None)
  ... same for rslvr-fdl-c80983a1f0284c99 (Category 'Botnet/Command and Control', AWSManagedDomainsBotnetCommandandControl), rslvr-fdl-d252ee1944404e15 (Category 'Aggregate Threat List', AWSManagedDomainsAggregateThreatList) and rslvr-fdl-fe17696f22a445f6 (Category 'Malware', AWSManagedDomainsMalwareDomainList) - four managed entries, all absent.
  divergence_key: service=route53resolver|ListFirewallDomainLists|status=200->200|keys=$.FirewallDomainLists[Arn=...] (7 keys; the three account-owned ARNs in that key list carry per-run ids and are not themselves the requirement).
  owned_by: [] , stub_path: "".

Cause 2 - ListResolverRules, fixture route53resolver-resolver-rule-system-lifecycle, step task#3, status 200 -> 200 (verdict mismatch, 1 affected step):
  $.ResolverRules[Arn="arn:aws:route53resolver:us-west-2:000000000000:resolver-rule/rslvr-rr-910a2dec89025cc1"].StatusMessage: missing (expected '{{rule_status_message_created}}' - the value the create returned - got None)
  divergence_key: service=route53resolver|ListResolverRules|status=200->200|keys=$.ResolverRules[Arn=...].StatusMessage
  owned_by: [] , stub_path: "".

## Steps to Reproduce
Replay fixtures route53resolver-catalog-list-firewall-domain-lists (task#1) and route53resolver-resolver-rule-system-lifecycle (task#3) from corpus simparity-wire-aws-v1. Both causes must be closed before re-replaying; closing only one leaves the other step still diverging.

Cause 1: against a freshly started emulator (no domain lists created), call ListFirewallDomainLists with no parameters. The response must include the service-managed threat lists in addition to any caller-owned lists: entries whose Name and CreatorRequestId are AWSManagedDomainsAmazonGuardDutyThreatList / AWSManagedDomainsBotnetCommandandControl / AWSManagedDomainsAggregateThreatList / AWSManagedDomainsMalwareDomainList, each with ManagedOwnerName "Route 53 Resolver DNS Firewall", ManagedListType THREAT and the matching Category ("Amazon GuardDuty Threat List", "Botnet/Command and Control", "Aggregate Threat List", "Malware"). GetFirewallDomainList on one of those ids must read it back with the same managed metadata.

Cause 2: CreateResolverRule(CreatorRequestId=<id>, Name=<name>, RuleType=SYSTEM, DomainName=<domain>) and note the ResolverRule.StatusMessage it returns, then call ListResolverRules with no filters. The element whose Arn matches the created rule must carry a StatusMessage member equal to the value the create/get projection reports for that status - not an absent member.
