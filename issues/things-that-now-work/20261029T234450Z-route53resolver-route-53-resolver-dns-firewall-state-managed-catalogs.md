Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-catalog-list-firewall-domain-lists
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-catalog-list-firewall-domain-lists route53resolver-dns-firewall-domain-list-lifecycle route53resolver-dns-firewall-rule-group-and-rules route53resolver-firewall-rule-override-ttl-numeric-precision-round-trip route53resolver-list-firewall-rules-priority-ordering
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn
Issue-Key: route53resolver|ListFirewallDomains|2xx->2xx|$.Domains[*]
Issue-Key: route53resolver|GetFirewallDomainList|2xx->2xx|$.FirewallDomainList.Status
Issue-Key: route53resolver|ListFirewallRules|2xx->2xx|$.FirewallRules[Name="sim2real-probe-route53resolver-rule-alert-e37ee8d7"]
Issue-Key: route53resolver|DeleteFirewallRuleGroup|2xx->4xx|$.FirewallRuleGroup
Issue-Key: route53resolver|DeleteFirewallRuleGroup|2xx->4xx|status
Issue-Key: route53resolver|DeleteFirewallDomainList|2xx->4xx|status
Issue-Related: 20261029T234450Z-route53resolver-route-53-resolver-dns-firewall-projections-omit-statusmessage.md
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# Route 53 Resolver DNS Firewall state: managed catalogs, domain canonicalization, import failure, rule listing and deletes all diverge (7 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFirewallDomainLists

## Description
Seven net-new wire causes in the DNS Firewall state keeping of services/service-com-amazonaws-route53resolver-route53resolver: the managed domain-list catalog is missing, written domains are not canonicalized, a failed import never reaches its terminal state, created rules are not listed, and two delete paths refuse a resource the emulator itself created.

=== Cause 1 of 7: ListFirewallDomainLists omits the AWS-managed domain lists ===
AWS: Smithy: ListFirewallDomainLists(MaxResults 1..100, NextToken) -> {NextToken, FirewallDomainLists: FirewallDomainListMetadataList}; the FirewallDomainList shape models ManagedOwnerName ("The owner of the list, used only for lists that are not managed by you. For example, the managed domain list AWSManagedDomainsMalwareDomainList has the managed owner name Route 53 Resolver DNS Firewall"), ManagedListType (enum DomainListType, e.g. THREAT) and Category. Recorded behavior: every account/Region is served the service-managed threat lists in addition to the caller's own — entries whose CreatorRequestId and Name are the managed-list name (e.g. AWSManagedDomainsAmazonGuardDutyThreatList, AWSManagedDomainsBotnetCommandandControl, AWSManagedDomainsAggregateThreatList, AWSManagedDomainsMalwareDomainList), ManagedOwnerName "Route 53 Resolver DNS Firewall", ManagedListType THREAT, and a Category per list ("Amazon GuardDuty Threat List", "Botnet/Command and Control", "Aggregate Threat List", "Malware"). The list ids are account-independent service ids; the names, categories, owner name and type are the fixed part.
Emulator: HTTP 200 whose FirewallDomainLists contains none of the managed entries (replay: four managed ARNs reported "missing ... got None").
Fix: the DNS Firewall domain-list catalog seeded in the crate's state module must include the service-managed lists, and ListFirewallDomainLists/GetFirewallDomainList must project them with ManagedOwnerName, ManagedListType and Category.
Why: a client that selects a managed threat list by name to reference from a FirewallRule (the documented way to use DNS Firewall) finds no list to reference, so rule creation against managed lists is impossible.
Traces: route53resolver-catalog-list-firewall-domain-lists#task#1

=== Cause 2 of 7: ListFirewallDomains returns added domains without the canonical trailing dot ===
AWS: Smithy: UpdateFirewallDomains(..., Operation, Domains required) / ListFirewallDomains(FirewallDomainListId required, MaxResults 1..5000, NextToken) -> {NextToken, Domains}; FirewallDomains members carry no pattern, so canonicalization is recorded behavior only: a domain written to a FirewallDomainList is stored as a DNS-canonical, fully qualified name — rooted with a single trailing dot — and reads back in that form (idempotently: an already-rooted name does not gain a second dot).
Emulator: the submitted spelling is echoed verbatim and unrooted (replay: expected 'ads.example.com.', got 'ads.example.com'; likewise for the other entries).
Why: a client that compares its configured blocklist against ListFirewallDomains never matches its own entries, and two spellings of one name become two list entries where AWS collapses them, so DomainCount and membership both diverge.
Fix: the domain-canonicalization step on the UpdateFirewallDomains write path (or the domain writer in the crate's state module), so every reader projects the canonical rooted name.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#4

=== Cause 3 of 7: the same un-canonicalized domains persist after a REMOVE ===
AWS: as cause 2 — after UpdateFirewallDomains(Operation=REMOVE) the surviving entries are still the canonical rooted names.
Emulator: the remaining entries are still unrooted ('ads.example.com', 'tracker.example.net' instead of 'ads.example.com.', 'tracker.example.net.'), confirming the defect is in storage rather than in one response path.
Why: as cause 2; additionally a REMOVE submitted in canonical form will not match a stored unrooted entry, so removals can silently no-op.
Fix: the single canonicalizer shared by the ADD/REMOVE/REPLACE paths, applied on write and on the removal key lookup.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#7

=== Cause 4 of 7: a failed ImportFirewallDomains leaves the list COMPLETE instead of COMPLETE_IMPORT_FAILED ===
AWS: Smithy: FirewallDomainListStatus enum = COMPLETE | COMPLETE_IMPORT_FAILED | IMPORTING | DELETING | UPDATING; FirewallDomainList.StatusMessage is "Additional information about the status of the list, if available." Recorded behavior: when an import from an unreachable/invalid S3 URL fails, GetFirewallDomainList reports Status COMPLETE_IMPORT_FAILED with StatusMessage "Failed to download the file from provided url, please try with a smaller file or copy the file into a <region> S3 bucket." — fixed text whose only variable part is the Region of the request.
Emulator: the list stays Status 'COMPLETE' with no StatusMessage, and the fixture's poll terminates because COMPLETE is a fail-on state (replay: "poll: 'COMPLETE' is in fail_on after 1 attempt(s)").
Why: a client that waits for an import to settle and then checks whether it failed is told the import succeeded, so it proceeds with a domain list that was never populated.
Fix: the ImportFirewallDomains execution path and the domain-list status machine in the crate's state module — a failed import must terminate in COMPLETE_IMPORT_FAILED with the download-failure message. (Open issue 20261025T012620Z-route53resolver-importfirewalldomains-stub.md tracks the operation being a stub; this cause is the status/message outcome and is not covered by that issue's replay key.)
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#10

=== Cause 5 of 7: ListFirewallRules does not return a rule that CreateFirewallRule accepted ===
AWS: Smithy: ListFirewallRules(FirewallRuleGroupId required, Priority, Action, MaxResults 1..100, NextToken) -> {NextToken, FirewallRules}; each element carries Action, CreationTime, CreatorRequestId, FirewallDomainListId, FirewallDomainRedirectionAction, FirewallRuleGroupId, ModificationTime, Name, Priority. Recorded behavior: a rule created in a group is immediately returned by an unfiltered ListFirewallRules on that group.
Emulator: HTTP 200 whose FirewallRules omits the just-created ALERT rule (replay: "$.FirewallRules[Name=\"...rule-alert...\"]: missing ... got None").
Why: the rule group looks empty, so a client cannot verify, update or delete the rule it created, and rule-count/priority reconciliation loops rewrite rules forever.
Fix: the rule index the CreateFirewallRule write populates and that ListFirewallRules reads in the crate's state module (and the filter handling on that read).
Traces: route53resolver-dns-firewall-rule-group-and-rules#task#10 (cluster also covers #task#14)

=== Cause 6 of 7: DeleteFirewallRuleGroup refuses with 400 a group AWS deletes with 200 ===
AWS: Smithy: DeleteFirewallRuleGroup(FirewallRuleGroupId required) -> {FirewallRuleGroup}; errors AccessDenied | Conflict | InternalServiceError | ResourceNotFound | Throttling | ValidationException (all 400). Recorded behavior: once the group's rules have been deleted, the delete succeeds — HTTP 200 projecting the group (Id, Arn, Name, OwnerId, CreatorRequestId, RuleCount 0, ShareStatus NOT_SHARED, CreationTime/ModificationTime, Status, StatusMessage).
Emulator: HTTP 400 with no FirewallRuleGroup in the body, so the group can never be torn down.
Why: teardown is a dead end — the rule group leaks and every subsequent run collides with it; a caller following AWS's documented delete-rules-then-delete-group order is refused.
Fix: the precondition check in the DeleteFirewallRuleGroup handler (it must not treat a group whose rules were removed as still in use) and the rule-count bookkeeping it reads in the crate's state module.
Traces: route53resolver-dns-firewall-rule-group-and-rules#teardown#21

=== Cause 7 of 7: DeleteFirewallDomainList refuses with 400 a list AWS deletes with 200 ===
AWS: Smithy: DeleteFirewallDomainList(FirewallDomainListId required) -> {FirewallDomainList}; ConflictException is reserved for a list still referenced by a rule or mid-delete. Recorded behavior: after the referencing rules are gone, the delete answers HTTP 200.
Emulator: HTTP 400 (status: expected 200, got 400) on four teardown steps across two fixtures.
Why: domain lists cannot be deleted once they have ever been referenced, so the account accumulates undeletable lists and no client can complete a clean teardown.
Fix: the in-use/state precondition in the DeleteFirewallDomainList handler and the reference bookkeeping it consults in the crate's state module.
Traces: route53resolver-firewall-rule-override-ttl-numeric-precision-round-trip#teardown#8 (cluster also covers route53resolver-list-firewall-rules-priority-ordering#teardown#16..#18)

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). Seven mismatch clusters, none stub-blocked, none owned.

Cause 1 — ListFirewallDomainLists, route53resolver-catalog-list-firewall-domain-lists#task#1, 200 -> 200:
  $.FirewallDomainLists[Arn=".../firewall-domain-list/rslvr-fdl-23584f0fb4c24ebb"]: missing (expected {Category 'Amazon GuardDuty Threat List', CreatorRequestId/Name 'AWSManagedDomainsAmazonGuardDutyThreatList', ManagedListType 'THREAT', ManagedOwnerName 'Route 53 Resolver DNS Firewall'}, got None); same for rslvr-fdl-c80983a1f0284c99 (Botnet/Command and Control), rslvr-fdl-d252ee1944404e15 (Aggregate Threat List), rslvr-fdl-fe17696f22a445f6 (Malware)
Cause 2 — ListFirewallDomains, route53resolver-dns-firewall-domain-list-lifecycle#task#4, 200 -> 200:
  $.Domains[0]: expected 'ads.example.com.', got 'ads.example.com'; [1] expected 'malware.example.org.', got 'malware.example.org'; [2] expected 'tracker.example.net.', got 'tracker.example.net'
Cause 3 — ListFirewallDomains, ...#task#7, 200 -> 200:
  $.Domains[0]: expected 'ads.example.com.', got 'ads.example.com'; [1] expected 'tracker.example.net.', got 'tracker.example.net'
Cause 4 — GetFirewallDomainList, ...#task#10, 200 -> 200:
  $.FirewallDomainList.Status: value_mismatch (expected 'COMPLETE_IMPORT_FAILED', got 'COMPLETE')
  $.FirewallDomainList.StatusMessage: missing (expected 'Failed to download the file from provided url, please try with a smaller file or copy the file into a us-west-2 S3 bucket.', got None)
  poll: 'COMPLETE' is in fail_on after 1 attempt(s)
Cause 5 — ListFirewallRules, route53resolver-dns-firewall-rule-group-and-rules#task#10, 200 -> 200:
  $.FirewallRules[Name="sim2real-probe-route53resolver-rule-alert-e37ee8d7"]: missing (expected the ALERT rule with Priority 100, FirewallDomainListId, FirewallDomainRedirectionAction INSPECT_REDIRECTION_DOMAIN, got None)   [2 affected steps]
Cause 6 — DeleteFirewallRuleGroup, route53resolver-dns-firewall-rule-group-and-rules#teardown#21:
  status: expected 200, got 400; $.FirewallRuleGroup: missing (expected the projected group with RuleCount 0, got None)
Cause 7 — DeleteFirewallDomainList, route53resolver-firewall-rule-override-ttl-numeric-precision-round-trip#teardown#8:
  status: expected 200, got 400   [4 affected steps]

## Steps to Reproduce
Replay fixtures route53resolver-catalog-list-firewall-domain-lists (task#1), route53resolver-dns-firewall-domain-list-lifecycle (task#4, task#7, task#10), route53resolver-dns-firewall-rule-group-and-rules (task#10, teardown#21) and route53resolver-firewall-rule-override-ttl-numeric-precision-round-trip (teardown#8). Minimal reproductions:
1. ListFirewallDomainLists on a clean account -> the AWS-managed THREAT lists must be present with ManagedOwnerName/ManagedListType/Category.
2/3. CreateFirewallDomainList, UpdateFirewallDomains(Operation=ADD, Domains=['ads.example.com','malware.example.org','tracker.example.net']), ListFirewallDomains -> every entry must read back rooted with a trailing dot; repeat with Operation=REMOVE for one entry.
4. ImportFirewallDomains(FirewallDomainListId=<id>, Operation=REPLACE, DomainFileUrl=<unreachable s3 url>), then poll GetFirewallDomainList -> Status must settle at COMPLETE_IMPORT_FAILED with the download-failure StatusMessage.
5. CreateFirewallRuleGroup + CreateFirewallDomainList + CreateFirewallRule(Action=ALERT, Priority=100), then ListFirewallRules(FirewallRuleGroupId=<group>) -> the rule must be returned.
6. Delete the group's rules, then DeleteFirewallRuleGroup(FirewallRuleGroupId=<group>) -> must be 200 with the group projected, not 400.
7. After the referencing rules are deleted, DeleteFirewallDomainList(FirewallDomainListId=<list>) -> must be 200, not 400.
