Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-catalog-list-firewall-domain-lists
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-catalog-list-firewall-domain-lists route53resolver-dns-firewall-domain-list-lifecycle route53resolver-dns-firewall-rule-group-and-rules route53resolver-dns-firewall-rule-group-policy-and-tagging
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|ListFirewallDomainLists|2xx->2xx|$.FirewallDomainLists[Arn="arn:aws:route53resolver:us-west-2:000000000000:firewall-domain-list/rslvr-fdl-d57b208eeaea0f26"]
Issue-Key: route53resolver|ImportFirewallDomains|2xx->2xx|$.StatusMessage
Issue-Key: route53resolver|DeleteFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroup.Status
Issue-Key: route53resolver|DeleteFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroup.StatusMessage
Issue-Related: 20261107T012050Z-route53resolver-route-53-resolver-list-projections-drop-managed-domain.md
Issue-Commit: cf09345244e8a785c8046377cb5c67787ee78494

# Route 53 Resolver DNS Firewall responses diverge: managed domain-list catalog absent, ImportFirewallDomains and DeleteFirewallRuleGroup status fields wrong (3 causes)

> _Note: this issue was identified against the code base at commit `cf09345244e8a785c8046377cb5c67787ee78494`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFirewallDomainLists

## Description
THREE net-new wire causes in the Route 53 Resolver DNS Firewall control plane (services/service-com-amazonaws-route53resolver-route53resolver). They share one fault: the DNS Firewall response projection serves an incomplete or wrong picture of a firewall resource's identity/lifecycle state — the service-managed domain-list catalog is absent from the list projection, and the lifecycle members (Status / StatusMessage) that AWS stamps on a mutating response are either omitted or carry the wrong state. All three are reported by the replay as UNOWNED (owned_by empty) and none is stub-blocked, so no open issue's replay key gates them; the open issues that describe adjacent behavior in prose are named per cause and must NOT be relied on to close these steps. A fixer must close all three causes before re-replaying — closing one leaves the other two steps still diverging.

=== Cause 1 of 3: ListFirewallDomainLists omits the AWS-managed DNS Firewall domain lists ===
AWS: Smithy: ListFirewallDomainLists(MaxResults @range 1..100, NextToken) -> ListFirewallDomainListsResponse{NextToken, FirewallDomainLists: FirewallDomainListMetadataList}; errors AccessDeniedException | InternalServiceErrorException | ThrottlingException | ValidationException (all httpError 400); paginated items=FirewallDomainLists. Shape FirewallDomainListMetadata models Id, Arn, Name (@length 0..64, @pattern ^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$), CreatorRequestId, plus ManagedOwnerName (doc: "The owner of the list, used only for lists that are not managed by you. For example, the managed domain list AWSManagedDomainsMalwareDomainList has the managed owner name Route 53 Resolver DNS Firewall"), ManagedListType (enum DomainListType, doc "The type of the managed domain list, for example THREAT") and Category (doc "The category of the domain list"). WHICH lists are service-managed is recorded behavior only: every account/Region is served the service-managed threat lists IN ADDITION to the caller's own lists. The fixed, required part is the set of managed entries — Name and CreatorRequestId equal to the managed-list name (AWSManagedDomainsAmazonGuardDutyThreatList, AWSManagedDomainsBotnetCommandandControl, AWSManagedDomainsAggregateThreatList, AWSManagedDomainsMalwareDomainList), ManagedOwnerName "Route 53 Resolver DNS Firewall", ManagedListType THREAT, and the matching Category per list ("Amazon GuardDuty Threat List", "Botnet/Command and Control", "Aggregate Threat List", "Malware"). The recorded list ids, ARNs and owning account number are per-run values and are NOT requirements.
Emulator: HTTP 200 whose FirewallDomainLists contains no managed entry at all — the replay reports each managed ARN as "missing (expected {...}, got None)".
Why: referencing a service-managed threat list from a FirewallRule is the documented way to use DNS Firewall. A client that resolves a managed list by name before CreateFirewallRule finds no list to reference and cannot build a rule group at all; a client that renders or filters the account's domain lists sees a catalog AWS never serves.
Fix: the DNS Firewall domain-list catalog seeded by the crate's state module must contain the service-managed lists, and the ListFirewallDomainLists projection (shared with GetFirewallDomainList) must emit ManagedOwnerName, ManagedListType and Category for them. Open issue 20261107T012050Z-route53resolver-route-53-resolver-list-projections-drop-managed-domain.md describes this same behavior as its Cause 1 and open issue 20261029T234450Z-route53resolver-route-53-resolver-dns-firewall-state-managed-catalogs.md in prose, but the replay still reports this cluster as UNOWNED — their replay keys do not bind this step (the divergence key now carries an eighth, additional caller-owned ARN entry), so this cause must be closed and re-replayed on its own.
Traces: route53resolver-catalog-list-firewall-domain-lists#task#1

=== Cause 2 of 3: the ImportFirewallDomains response omits StatusMessage entirely ===
AWS: Smithy: ImportFirewallDomains(FirewallDomainListId required @length 1..64, Operation required (FirewallDomainImportOperation), DomainFileUrl required @length 1..1024) -> ImportFirewallDomainsResponse{Id, Name, Status (enum FirewallDomainListStatus = COMPLETE | COMPLETE_IMPORT_FAILED | IMPORTING | DELETING | UPDATING), StatusMessage (@length 0..255)}; errors AccessDeniedException | ConflictException | InternalServiceErrorException | LimitExceededException | ResourceNotFoundException | ThrottlingException | ValidationException (all httpError 400). The accepted-import message is recorded behavior only: the import acknowledgement carries StatusMessage "Importing domains from provided file." — that sentence is entirely fixed text, with no variable part taken from the request or state.
Emulator: HTTP 200 whose body has NO StatusMessage member at all (replay: "$.StatusMessage: missing (expected 'Importing domains from provided file.', got None)"); Status itself matched, so only the modeled message member is dropped.
Why: the import is asynchronous, so the acknowledgement's Status/StatusMessage pair is the only thing a caller has to confirm the import was accepted and is in flight. A client that reads StatusMessage off the import response gets a missing key instead of a sentence and either crashes on the absent member or cannot tell an accepted import from a rejected one before it begins polling.
Fix: the ImportFirewallDomains handler/serializer must stamp the import-accepted status message onto the domain-list record and emit the modeled StatusMessage member in the response, as the sibling UpdateFirewallDomains response does. Open issue 20261101T021308Z-route53resolver-route-53-resolver-dns-firewall-statusmessage-is-frozen.md covers the SETTLED state after a FAILED import (its Cause 3, GetFirewallDomainList at task#10) and open issue 20261025T012620Z-route53resolver-importfirewalldomains-stub.md tracks the operation being a scaffold stub; neither covers the import response's own missing StatusMessage at task#9, and the replay reports this cluster unowned.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#9

=== Cause 3 of 3: DeleteFirewallRuleGroup projects Status DELETING with a delete message where AWS reports UPDATING ===
AWS: Smithy: DeleteFirewallRuleGroup(FirewallRuleGroupId required @length 1..64) -> DeleteFirewallRuleGroupResponse{FirewallRuleGroup}; errors AccessDeniedException | ConflictException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException (all httpError 400). FirewallRuleGroup.Status is enum FirewallRuleGroupStatus = COMPLETE | DELETING | UPDATING and StatusMessage is @length 0..255 ("Additional information about the status of the rule group, if available."). Which of those states a delete acknowledgement carries is recorded behavior only: AWS answers 200 with the deleted group projected at Status UPDATING and StatusMessage "Updating Firewall Rule Group" — fixed text with no variable part; the group id, ARN, name and timestamps in the same projection are per-run values.
Emulator: HTTP 200 with $.FirewallRuleGroup.Status = "DELETING" and $.FirewallRuleGroup.StatusMessage = "Deleting the Firewall Rule Group" — an emulator-authored state/message pair for the delete path.
Why: a client that deletes a rule group and then waits on the returned Status sees a state AWS never returns here, so a wait loop keyed on the documented transition (UPDATING -> gone) never starts, and a client that surfaces StatusMessage shows wording AWS does not emit. The two members disagree with AWS together, so neither can be used to drive teardown.
Fix: the DeleteFirewallRuleGroup handler plus the rule-group status machine in the crate's state module — the delete path must stamp the rule group's Status/StatusMessage to the update-class state AWS reports and the one FirewallRuleGroup serializer must project it.
Traces: route53resolver-dns-firewall-rule-group-and-rules#teardown#21 (cluster also covers route53resolver-dns-firewall-rule-group-policy-and-tagging#teardown#11). Open issue 20261029T234450Z-route53resolver-route-53-resolver-vpc-config-dnssec-query-log.md concerns rule-group delete answering 200 where AWS REFUSES with 400, not the status fields of a successful delete, and does not own this cluster.

## Evidence
Wire corpus simparity-wire-aws-v1 (trace set simparity-wire-aws-v1), service route53resolver (bootstrapped). Replay summary: emulator_up true, bootstrap_signal false, 167 steps total, 121 passed, 46 mismatched, 0 errored, 0 blocked, score 72.5%, 37 divergence clusters. Of those 37, 2 are blocked by unimplemented operations and 32 are owned_by an open issue; the 3 below are the only clusters that are neither stub-blocked nor owned.

Cause 1 — ListFirewallDomainLists, fixture route53resolver-catalog-list-firewall-domain-lists, step task#1, status 200 -> 200, verdict mismatch, 1 affected step:
  $.FirewallDomainLists[Arn=".../firewall-domain-list/rslvr-fdl-23584f0fb4c24ebb"]: missing (expected {'Category': 'Amazon GuardDuty Threat List', 'CreatorRequestId': 'AWSManagedDomainsAmazonGuardDutyThreatList', 'Name': 'AWSManagedDomainsAmazonGuardDutyThreatList', 'ManagedListType': 'THREAT', 'ManagedOwnerName': 'Route 53 Resolver DNS Firewall'}, got None)
  $.FirewallDomainLists[Arn=".../rslvr-fdl-c80983a1f0284c99"]: missing (Category 'Botnet/Command and Control', AWSManagedDomainsBotnetCommandandControl)
  $.FirewallDomainLists[Arn=".../rslvr-fdl-d252ee1944404e15"]: missing (Category 'Aggregate Threat List', AWSManagedDomainsAggregateThreatList)
  $.FirewallDomainLists[Arn=".../rslvr-fdl-fe17696f22a445f6"]: missing (Category 'Malware', AWSManagedDomainsMalwareDomainList)
  divergence_key: service=route53resolver|ListFirewallDomainLists|status=200->200|keys=$.FirewallDomainLists[Arn=...] (8 keys; the four account-owned ARNs in that key list carry per-run ids and are not themselves the requirement). owned_by: [], stub_path: "".

Cause 2 — ImportFirewallDomains, fixture route53resolver-dns-firewall-domain-list-lifecycle, step task#9, status 200 -> 200, verdict mismatch, 1 affected step:
  $.StatusMessage: missing (expected 'Importing domains from provided file.', got None)
  divergence_key: service=route53resolver|ImportFirewallDomains|status=200->200|keys=$.StatusMessage. owned_by: [], stub_path: "".

Cause 3 — DeleteFirewallRuleGroup, fixture route53resolver-dns-firewall-rule-group-and-rules, step teardown#21, status 200 -> 200, verdict mismatch, 2 affected steps:
  $.FirewallRuleGroup.Status: value_mismatch (expected 'UPDATING', got 'DELETING')
  $.FirewallRuleGroup.StatusMessage: value_mismatch (expected 'Updating Firewall Rule Group', got 'Deleting the Firewall Rule Group')
  affected: route53resolver-dns-firewall-rule-group-and-rules#teardown#21, route53resolver-dns-firewall-rule-group-policy-and-tagging#teardown#11
  divergence_key: service=route53resolver|DeleteFirewallRuleGroup|status=200->200|keys=$.FirewallRuleGroup.Status,$.FirewallRuleGroup.StatusMessage. owned_by: [], stub_path: "".

Fixed text vs variable parts: "Importing domains from provided file." and "Updating Firewall Rule Group" are fixed AWS text with no variable part. In cause 1 the fixed requirement is the four managed list Names/CreatorRequestIds, ManagedOwnerName "Route 53 Resolver DNS Firewall", ManagedListType THREAT and the per-list Category strings; the recorded list ids, ARNs, owning account number and timestamps are per-run values and are not requirements.

## Steps to Reproduce
Replay fixtures route53resolver-catalog-list-firewall-domain-lists (task#1), route53resolver-dns-firewall-domain-list-lifecycle (task#9) and route53resolver-dns-firewall-rule-group-and-rules (teardown#21, cluster also covers route53resolver-dns-firewall-rule-group-policy-and-tagging#teardown#11) from corpus simparity-wire-aws-v1. All three causes must be closed before re-replaying.

Cause 1: against a freshly started emulator (no domain lists created), call ListFirewallDomainLists with no parameters. The response must include the service-managed threat lists in addition to any caller-owned lists: entries whose Name and CreatorRequestId are AWSManagedDomainsAmazonGuardDutyThreatList / AWSManagedDomainsBotnetCommandandControl / AWSManagedDomainsAggregateThreatList / AWSManagedDomainsMalwareDomainList, each with ManagedOwnerName "Route 53 Resolver DNS Firewall", ManagedListType THREAT and the matching Category ("Amazon GuardDuty Threat List", "Botnet/Command and Control", "Aggregate Threat List", "Malware"). GetFirewallDomainList on one of those ids must read it back with the same managed metadata.

Cause 2: CreateFirewallDomainList(CreatorRequestId=<id>, Name=<name>), then ImportFirewallDomains(FirewallDomainListId=<id>, Operation=REPLACE, DomainFileUrl=<s3 url>). The 200 response must carry a StatusMessage member — the import-accepted sentence "Importing domains from provided file." — alongside Id, Name and Status, not omit it.

Cause 3: CreateFirewallRuleGroup(CreatorRequestId=<id>, Name=<name>), then DeleteFirewallRuleGroup(FirewallRuleGroupId=<id>). The 200 response's FirewallRuleGroup must project Status UPDATING with StatusMessage "Updating Firewall Rule Group", not Status DELETING with "Deleting the Firewall Rule Group".
