Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-create-firewall-to-test-path-refusals
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-create-firewall-to-test-path-refusals network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes network-firewall-nfwr-traj-stateless-rule-priority-mistakes network-firewall-nfwr-traj-list-filter-and-paging-mistakes network-firewall-nfwr-traj-stateful-engine-options-mistakes network-firewall-nfwr-traj-stateless-default-action-spelling
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateFirewall|4xx->4xx|$.message
Issue-Key: network-firewall|CreateRuleGroup|4xx->4xx|$.message
Issue-Key: network-firewall|ListFirewalls|4xx->4xx|$.message
Issue-Key: network-firewall|ListRuleGroups|4xx->4xx|$.message
Issue-Key: network-firewall|ListFirewallPolicies|4xx->4xx|$.message
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->4xx|$.message
Issue-Commit: c1ca043e5c77dd90c2a96695b3dfa06a20838f7d

# Constraint-violation messages are built in the wrong shape and from the wrong constraint set

> _Note: this issue was identified against the code base at commit `c1ca043e5c77dd90c2a96695b3dfa06a20838f7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewall

## Description
Network Firewall's protocol-level input validation (the HTTP 400 body with a lowercase `message` member reading '<n> validation error detected: ...') is rendered in a different shape by the emulator: it interpolates the offending value and an indexed member path that AWS omits, and for collections and enums it reports only part of the constraint set. The status and the error are right in every case below; the message text is not. This bundle has 6 causes.

Cause 1 of 6 - missing required member (fixture network-firewall-nfwr-traj-create-firewall-to-test-path-refusals, step task#4).
AWS: CreateFirewall.FirewallPolicyArn is required (Smithy: target ResourceArn, required, length 1..256, pattern ^arn:aws). Omitting it yields HTTP 400 with message "1 validation error detected: Value at 'firewallPolicyArn' failed to satisfy constraint: Member must not be null". Fixed text: "<n> validation error detected: Value at '", "' failed to satisfy constraint: Member must not be null". Variable parts: the error count and the lowerCamel member path. For a null member AWS writes 'Value at' with no value rendered.
Emulator: "1 validation error detected: Value null at 'firewallPolicyArn' failed to satisfy constraint: Member must not be null" - it renders the literal 'null' as the value.

Cause 2 of 6 - missing required member, second instance (fixture network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes, step task#2; same cluster: that fixture's task#6 and network-firewall-nfwr-traj-stateless-rule-priority-mistakes#task#2).
AWS: CreateRuleGroup.Capacity is required (Smithy: target RuleCapacity, required) -> "1 validation error detected: Value at 'capacity' failed to satisfy constraint: Member must not be null".
Emulator: "... Value null at 'capacity' ..." - same defect as Cause 1, confirming it is in the shared message builder rather than one handler.

Cause 3 of 6 - list member constraints are reported partially and with an index (fixture network-firewall-nfwr-traj-list-filter-and-paging-mistakes, step task#1; cluster also covers task#2..task#4).
AWS: ListFirewalls.VpcIds targets VpcIds, whose member has length 1..128 and pattern ^vpc-[0-9a-f]+$ (Smithy: VpcIds list member -> VpcId). A bad entry yields "1 validation error detected: Value at 'vpcIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^vpc-[0-9a-f]+$]" - AWS names the list member ('vpcIds', no index), renders no value, and lists the member's whole constraint set in brackets.
Emulator: "1 validation error detected: Value 'DMZVPC' at 'vpcIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^vpc-[0-9a-f]+$" - it interpolates the value, appends an element index path, and reports only the pattern constraint.

Cause 4 of 6 - enum value set is reported from the modeled subset (fixture network-firewall-nfwr-traj-list-filter-and-paging-mistakes, step task#6).
AWS: ListRuleGroups.Type targets RuleGroupType. AWS's validation message enumerates the service-side value set: "1 validation error detected: Value at 'type' failed to satisfy constraint: Member must satisfy enum value set: [FIREWALL, STATELESS, IPS, STATEFUL, IPS_DOMAIN, STATEFUL_DOMAIN]" (recorded behavior - the public Smithy enum only lists STATELESS, STATEFUL, STATEFUL_DOMAIN, so the model is not the source for this text), again with no value rendered.
Emulator: "1 validation error detected: Value 'stateful' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [STATELESS, STATEFUL, STATEFUL_DOMAIN]" - renders the value and emits only the three modeled names.

Cause 5 of 6 - range constraint renders the value (fixture network-firewall-nfwr-traj-list-filter-and-paging-mistakes, step task#7).
AWS: ListFirewallPolicies.MaxResults targets PaginationMaxResults, range 1..100 (Smithy). Below range: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
Emulator: "... Value '0' at 'maxResults' ..." - same value-interpolation defect.

Cause 6 of 6 - nested enum path, value rendered (fixture network-firewall-nfwr-traj-stateful-engine-options-mistakes, step task#2; same cluster: network-firewall-nfwr-traj-stateless-default-action-spelling#task#5 and network-firewall-nfwr-traj-stateless-rule-priority-mistakes#task#5).
AWS: FirewallPolicy.StatefulEngineOptions.RuleOrder targets RuleOrder (Smithy enum: DEFAULT_ACTION_ORDER, STRICT_ORDER) -> "1 validation error detected: Value at 'firewallPolicy.statefulEngineOptions.ruleOrder' failed to satisfy constraint: Member must satisfy enum value set: [DEFAULT_ACTION_ORDER, STRICT_ORDER]". The nested lowerCamel path and the value set are right in the emulator; only the rendered value is wrong.
Emulator: "... Value 'strict_order' at 'firewallPolicy.statefulEngineOptions.ruleOrder' ...".

Why: these messages are the only machine-readable detail on a 400 from the pre-dispatch validator, and test suites, SDK retry wrappers and user-facing tooling match on them. A client that string-matches AWS's text gets no match here; worse, Causes 3 and 4 understate the constraint - a client told only about the regex, or told STATEFUL_DOMAIN is the widest type set, draws the wrong conclusion about what the API accepts. Rendering the submitted value also echoes caller input into an error body AWS deliberately keeps value-free.

Fix: the shared request-validation/message-rendering component in the network-firewall service crate (the builder that formats '<n> validation error detected: ...'): drop the value interpolation and the '.<index>.member' suffix, name the list member itself, emit the full constraint set in brackets for a constrained collection member, and source the enum value set for the message from the service's value list rather than the modeled public subset.

## Evidence
create-firewall-to-test-path-refusals#task#4: expected "Value at 'firewallPolicyArn'", actual "Value null at 'firewallPolicyArn'".
rule-group-capacity-and-type-mistakes#task#2: expected "Value at 'capacity'", actual "Value null at 'capacity'".
list-filter-and-paging-mistakes#task#1: expected "Value at 'vpcIds' ... Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^vpc-[0-9a-f]+$]", actual "Value 'DMZVPC' at 'vpcIds.1.member' ... Member must satisfy regular expression pattern: ^vpc-[0-9a-f]+$".
list-filter-and-paging-mistakes#task#6: expected enum value set [FIREWALL, STATELESS, IPS, STATEFUL, IPS_DOMAIN, STATEFUL_DOMAIN] and no value rendered; actual value 'stateful' rendered with set [STATELESS, STATEFUL, STATEFUL_DOMAIN].
list-filter-and-paging-mistakes#task#7: expected "Value at 'maxResults'", actual "Value '0' at 'maxResults'".
stateful-engine-options-mistakes#task#2: expected "Value at 'firewallPolicy.statefulEngineOptions.ruleOrder'", actual "Value 'strict_order' at ...". All six keep status 400 and the lowercase `message` member.

## Steps to Reproduce
Replay the listed fixture steps. Equivalently: CreateFirewall without FirewallPolicyArn; CreateRuleGroup without Capacity; ListFirewalls with VpcIds=['DMZVPC']; ListRuleGroups with Type='stateful'; ListFirewallPolicies with MaxResults=0; CreateFirewallPolicy with StatefulEngineOptions.RuleOrder='strict_order'. Compare the `message` member of each 400 body.
