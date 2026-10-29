Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-dns-firewall-domain-list-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-dns-firewall-domain-list-lifecycle route53resolver-dns-firewall-rule-group-and-rules route53resolver-route53resolver-readonly-list-configs route53resolver-firewall-domain-list-lifecycle-with-listing route53resolver-dns-firewall-rule-group-policy-and-tagging
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateFirewallDomainList|2xx->2xx|$.FirewallDomainList.StatusMessage
Issue-Key: route53resolver|GetFirewallDomainList|2xx->2xx|$.FirewallDomainList.StatusMessage
Issue-Key: route53resolver|UpdateFirewallDomains|2xx->2xx|$.Status
Issue-Key: route53resolver|UpdateFirewallDomains|2xx->2xx|$.StatusMessage
Issue-Key: route53resolver|DeleteFirewallDomainList|2xx->2xx|$.FirewallDomainList.StatusMessage
Issue-Key: route53resolver|GetFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroup.StatusMessage
Issue-Key: route53resolver|CreateFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroup.StatusMessage
Issue-Key: route53resolver|DeleteFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroup.Status
Issue-Key: route53resolver|DeleteFirewallRuleGroup|2xx->2xx|$.FirewallRuleGroup.StatusMessage
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# Route 53 Resolver DNS Firewall projections omit StatusMessage and report a transient Status (8 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallDomainList

## Description
Eight net-new wire causes in the DNS Firewall domain-list and rule-group response projections of services/service-com-amazonaws-route53resolver-route53resolver. The shared fault: the emulator never populates the modeled StatusMessage member on FirewallDomainList / FirewallRuleGroup / UpdateFirewallDomains responses, and on the two mutation paths it also projects the in-flight lifecycle Status instead of the settled one AWS answers. Each cause is one replayed divergence cluster.

=== Cause 1 of 8: CreateFirewallDomainList omits FirewallDomainList.StatusMessage ===
AWS: Smithy: CreateFirewallDomainList(CreatorRequestId required @length 1..255 @idempotencyToken, Name required @length 0..64 @pattern "^(?!^[0-9]+$)([a-zA-Z0-9\\-_' ']+)$", Tags) -> CreateFirewallDomainListResponse{FirewallDomainList}; shape FirewallDomainList has member StatusMessage (target StatusMessage, @length 0..255) documented "Additional information about the status of the list, if available." and Status (enum FirewallDomainListStatus: COMPLETE | COMPLETE_IMPORT_FAILED | IMPORTING | DELETING | UPDATING). Recorded behavior: a freshly created list is projected with Status COMPLETE and StatusMessage "Created Firewall Domain List" — fixed text keyed off the lifecycle state, with no variable part.
Emulator: HTTP 200 with FirewallDomainList.StatusMessage entirely absent from the body (replay: "$.FirewallDomainList.StatusMessage: missing (expected 'Created Firewall Domain List', got None)").
Why: a client that surfaces or logs the status reason sees nothing, and SDK round-trips that assert the member is present on a create fail; any code that distinguishes a healthy list from an import-failed one by its StatusMessage cannot.
Fix: the CreateFirewallDomainList handler's response projection (the FirewallDomainList serializer / the domain-list record in the crate's state module), which must carry the lifecycle status message alongside Status.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#setup#1 (cluster also covers route53resolver-dns-firewall-rule-group-and-rules#setup#2..#4 and route53resolver-route53resolver-readonly-list-configs#setup#1)

=== Cause 2 of 8: GetFirewallDomainList omits FirewallDomainList.StatusMessage ===
AWS: Smithy: GetFirewallDomainList(FirewallDomainListId required) -> {FirewallDomainList}; errors AccessDenied | InternalServiceError | ResourceNotFound | Throttling, all httpError 400 (defaulted). Recorded behavior: the read projects the same StatusMessage the create set — "Created Firewall Domain List" for a list in COMPLETE.
Emulator: HTTP 200, StatusMessage member missing.
Why: the read path is where waiters and consoles pick the status reason up; its permanent absence makes the member look optional when AWS always sets it for a managed lifecycle state.
Fix: same projection as cause 1, on the GetFirewallDomainList read path.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#2 (cluster also covers #task#5, #task#8 and route53resolver-firewall-domain-list-lifecycle-with-listing#task#3..#4)

=== Cause 3 of 8: UpdateFirewallDomains answers Status UPDATING instead of COMPLETE and omits StatusMessage ===
AWS: Smithy: UpdateFirewallDomains(FirewallDomainListId required, Operation required enum, Domains required) -> {Id, Name, Status (FirewallDomainListStatus), StatusMessage @length 0..255}. Recorded behavior: an ADD/REMOVE of a handful of domains against a COMPLETE list is applied synchronously — the response reports Status COMPLETE with StatusMessage "Created Firewall Domain List"; UPDATING is reserved for an update that is genuinely still in flight.
Emulator: HTTP 200 with Status 'UPDATING' and no StatusMessage (replay: "$.Status: value_mismatch (expected 'COMPLETE', got 'UPDATING')" and "$.StatusMessage: missing").
Why: a client that polls until Status leaves UPDATING before issuing the next call waits on a state the emulator never leaves on this response, and a client that treats UPDATING as "not yet applied" will not read back the domains it just wrote.
Fix: the UpdateFirewallDomains handler's status computation — the list must be projected in its settled state once the write is committed — plus the StatusMessage projection.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#3

=== Cause 4 of 8: UpdateFirewallDomains omits StatusMessage on the REMOVE path ===
AWS: same operation/shape as cause 3; the recorded response carries a StatusMessage whose literal value the corpus asserts only by shape (a non-empty lifecycle sentence <=255 chars).
Emulator: StatusMessage absent (replay: "$.StatusMessage: missing (expected '<shaped-not-asserted>', got None)").
Why: as cause 1 — the modeled member is never on the wire, so no client can read the update's status reason.
Fix: the StatusMessage projection on the UpdateFirewallDomains response, for every Operation value.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#6

=== Cause 5 of 8: DeleteFirewallDomainList omits the delete-specific StatusMessage ===
AWS: Smithy: DeleteFirewallDomainList(FirewallDomainListId required) -> {FirewallDomainList}; errors AccessDenied | Conflict | InternalServiceError | ResourceNotFound | Throttling (400). Recorded behavior: the delete projects the list in its deleting state with StatusMessage "Deleting the Firewall Domain List" — fixed text, distinct from the create sentence, with no variable part.
Emulator: HTTP 200 with StatusMessage missing.
Why: the delete response is the only place a caller learns the deletion was accepted and is in progress; with the member absent, a caller cannot distinguish "delete accepted" from "list unchanged".
Fix: the DeleteFirewallDomainList handler's projection must set the delete-phase status message, not reuse or drop the create one.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#teardown#13 (cluster also covers route53resolver-firewall-domain-list-lifecycle-with-listing#teardown#6..#7 and route53resolver-route53resolver-readonly-list-configs#teardown#5)

=== Cause 6 of 8: GetFirewallRuleGroup omits FirewallRuleGroup.StatusMessage ===
AWS: Smithy: GetFirewallRuleGroup(FirewallRuleGroupId required) -> {FirewallRuleGroup}; shape FirewallRuleGroup has StatusMessage (@length 0..255, "Additional information about the status of the rule group, if available.") and Status (FirewallRuleGroupStatus). Recorded behavior: a rule group in COMPLETE reads back with StatusMessage "Created Firewall Rule Group" (fixed text).
Emulator: StatusMessage missing from the projected rule group.
Why: same client impact as cause 2, on the rule-group family.
Fix: the FirewallRuleGroup projection shared by the rule-group read paths.
Traces: route53resolver-dns-firewall-rule-group-and-rules#task#5 (cluster also covers #task#11)

=== Cause 7 of 8: CreateFirewallRuleGroup omits FirewallRuleGroup.StatusMessage ===
AWS: Smithy: CreateFirewallRuleGroup(CreatorRequestId required, Name required @pattern, Tags) -> {FirewallRuleGroup}. Recorded behavior: the create projects Status COMPLETE with StatusMessage "Created Firewall Rule Group".
Emulator: StatusMessage missing.
Why: as cause 1, for rule groups.
Fix: the CreateFirewallRuleGroup handler's projection / the rule-group record written in the crate's state module.
Traces: route53resolver-dns-firewall-rule-group-policy-and-tagging#task#1

=== Cause 8 of 8: DeleteFirewallRuleGroup answers Status DELETING instead of COMPLETE and omits StatusMessage ===
AWS: Smithy: DeleteFirewallRuleGroup(FirewallRuleGroupId required) -> {FirewallRuleGroup}. Recorded behavior: deleting an unassociated, ruleless group answers HTTP 200 projecting the group with Status COMPLETE and StatusMessage "Created Firewall Rule Group" — i.e. the group's last settled state, not a transient DELETING.
Emulator: Status 'DELETING' and no StatusMessage (replay: "$.FirewallRuleGroup.Status: value_mismatch (expected 'COMPLETE', got 'DELETING')").
Why: a caller that keys teardown completion on the Status it gets back from the delete reads a state AWS does not report here, so cleanup logic branches differently against the emulator than against AWS.
Fix: the DeleteFirewallRuleGroup handler's status/StatusMessage projection.
Traces: route53resolver-dns-firewall-rule-group-policy-and-tagging#teardown#11

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped), emulator up (/__admin/health 200). All eight clusters are verdict=mismatch with no stub_path and no owning issue.

Cause 1 — CreateFirewallDomainList, route53resolver-dns-firewall-domain-list-lifecycle#setup#1, status 200 -> 200:
  $.FirewallDomainList.StatusMessage: missing (expected 'Created Firewall Domain List', got None)   [5 affected steps]
Cause 2 — GetFirewallDomainList, ...#task#2, 200 -> 200:
  $.FirewallDomainList.StatusMessage: missing (expected 'Created Firewall Domain List', got None)   [5 affected steps]
Cause 3 — UpdateFirewallDomains, ...#task#3, 200 -> 200:
  $.Status: value_mismatch (expected 'COMPLETE', got 'UPDATING')
  $.StatusMessage: missing (expected 'Created Firewall Domain List', got None)
Cause 4 — UpdateFirewallDomains, ...#task#6, 200 -> 200:
  $.StatusMessage: missing (expected '<shaped-not-asserted>', got None)
Cause 5 — DeleteFirewallDomainList, ...#teardown#13, 200 -> 200:
  $.FirewallDomainList.StatusMessage: missing (expected 'Deleting the Firewall Domain List', got None)   [4 affected steps]
Cause 6 — GetFirewallRuleGroup, route53resolver-dns-firewall-rule-group-and-rules#task#5, 200 -> 200:
  $.FirewallRuleGroup.StatusMessage: missing (expected 'Created Firewall Rule Group', got None)   [2 affected steps]
Cause 7 — CreateFirewallRuleGroup, route53resolver-dns-firewall-rule-group-policy-and-tagging#task#1, 200 -> 200:
  $.FirewallRuleGroup.StatusMessage: missing (expected 'Created Firewall Rule Group', got None)
Cause 8 — DeleteFirewallRuleGroup, route53resolver-dns-firewall-rule-group-policy-and-tagging#teardown#11, 200 -> 200:
  $.FirewallRuleGroup.Status: value_mismatch (expected 'COMPLETE', got 'DELETING')
  $.FirewallRuleGroup.StatusMessage: missing (expected 'Created Firewall Rule Group', got None)

## Steps to Reproduce
Replay fixture route53resolver-dns-firewall-domain-list-lifecycle (steps setup#1, task#2, task#3, task#6, teardown#13), route53resolver-dns-firewall-rule-group-and-rules (task#5) and route53resolver-dns-firewall-rule-group-policy-and-tagging (task#1, teardown#11) against the emulator. Minimal reproductions:
1/2. CreateFirewallDomainList(CreatorRequestId=<id>, Name=<name>) then GetFirewallDomainList(FirewallDomainListId=<id>) -> both responses must carry FirewallDomainList.StatusMessage.
3/4. UpdateFirewallDomains(FirewallDomainListId=<id>, Operation=ADD, Domains=[...]) then again with Operation=REMOVE -> Status must be the settled COMPLETE and StatusMessage must be present.
5. DeleteFirewallDomainList(FirewallDomainListId=<id>) -> StatusMessage must be the delete-phase sentence.
6/7. CreateFirewallRuleGroup(CreatorRequestId=<id>, Name=<name>) then GetFirewallRuleGroup(FirewallRuleGroupId=<id>) -> FirewallRuleGroup.StatusMessage must be present.
8. DeleteFirewallRuleGroup on a group with no rules and no associations -> FirewallRuleGroup.Status must be the settled COMPLETE with StatusMessage present.
