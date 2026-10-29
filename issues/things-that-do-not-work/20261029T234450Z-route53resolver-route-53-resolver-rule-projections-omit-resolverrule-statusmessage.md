Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-resolver-rule-system-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-resolver-rule-system-lifecycle route53resolver-catalog-list-resolver-rules
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateResolverRule|2xx->2xx|$.ResolverRule.StatusMessage
Issue-Key: route53resolver|GetResolverRule|2xx->2xx|$.ResolverRule.StatusMessage
Issue-Key: route53resolver|ListResolverRules|2xx->2xx|$.ResolverRules[Arn="arn
Issue-Key: route53resolver|UpdateResolverRule|2xx->2xx|$.ResolverRule.StatusMessage
Issue-Key: route53resolver|DeleteResolverRule|2xx->2xx|$.ResolverRule.StatusMessage
Issue-Key: route53resolver|ListResolverRules|2xx->2xx|$.ResolverRules[*]
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# Route 53 Resolver rule projections omit ResolverRule.StatusMessage and ListResolverRules omits the autodefined Internet Resolver rule (6 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolverRule

## Description
Six net-new wire causes on the Resolver-rule read/write surface of services/service-com-amazonaws-route53resolver-route53resolver. Causes 1-5 share one fault: the modeled ResolverRule.StatusMessage member is never serialized on any rule projection. Cause 6 is the rule catalog: the account's autodefined recursive rule is absent from ListResolverRules.

=== Cause 1 of 6: CreateResolverRule omits ResolverRule.StatusMessage ===
AWS: Smithy: CreateResolverRule -> CreateResolverRuleResponse{ResolverRule}; shape ResolverRule declares StatusMessage (target StatusMessage, @length 0..255) documented "A detailed description of the status of a Resolver rule." alongside Status (enum ResolverRuleStatus). Recorded behavior: every ResolverRule projection carries a non-empty status sentence describing the rule's current state (the corpus asserts it by shape on the create path).
Emulator: HTTP 200 with ResolverRule.StatusMessage absent (replay: "$.ResolverRule.StatusMessage: missing (expected '<shaped-not-asserted>', got None)").
Why: a client that reports why a rule is in its current Status has nothing to show, and a round-trip assertion that the member exists on a created rule fails.
Fix: the ResolverRule response projection (the rule serializer / the rule record in the crate's state module) must emit the status message for the rule's Status.
Traces: route53resolver-resolver-rule-system-lifecycle#task#1

=== Cause 2 of 6: GetResolverRule omits ResolverRule.StatusMessage ===
AWS: Smithy: GetResolverRule(ResolverRuleId required) -> {ResolverRule}. Recorded behavior: the read projects the same created-state status sentence the create returned (the corpus binds it to the create's value via a fixture variable).
Emulator: StatusMessage missing.
Why: the read path is where clients poll a rule's state; the reason text is permanently unavailable.
Fix: same projection as cause 1, on the GetResolverRule path.
Traces: route53resolver-resolver-rule-system-lifecycle#task#2 (cluster also covers #task#5)

=== Cause 3 of 6: ListResolverRules omits StatusMessage on each listed rule ===
AWS: Smithy: ListResolverRules(MaxResults 1..100, NextToken, Filters) -> {NextToken, MaxResults, ResolverRules}; each element is the full ResolverRule shape including StatusMessage.
Emulator: the listed rule matching the created ARN has no StatusMessage member.
Why: list and get disagree with AWS in the same way, so a client cannot build its view from either.
Fix: the list projection must reuse the single ResolverRule serializer that carries StatusMessage.
Traces: route53resolver-resolver-rule-system-lifecycle#task#3

=== Cause 4 of 6: UpdateResolverRule omits ResolverRule.StatusMessage ===
AWS: Smithy: UpdateResolverRule(ResolverRuleId required, Config required) -> {ResolverRule}; the projection includes StatusMessage.
Emulator: StatusMessage missing on the updated rule.
Why: as cause 1, on the write path a client uses to confirm its change.
Fix: the UpdateResolverRule handler's projection.
Traces: route53resolver-resolver-rule-system-lifecycle#task#4

=== Cause 5 of 6: DeleteResolverRule omits ResolverRule.StatusMessage ===
AWS: Smithy: DeleteResolverRule(ResolverRuleId required) -> {ResolverRule}; the deleted rule is projected with its status and status message.
Emulator: StatusMessage missing.
Why: the delete response is the caller's only confirmation of the rule's final state reason.
Fix: the DeleteResolverRule handler's projection.
Traces: route53resolver-resolver-rule-system-lifecycle#task#11

=== Cause 6 of 6: ListResolverRules does not include the account's autodefined recursive rule ===
AWS: Smithy: ListResolverRules returns ResolverRules, each a full ResolverRule. Recorded behavior only (the rule is service-created, not modellable): every account/Region has a pre-existing autodefined rule which ListResolverRules returns as the first element even when the caller created nothing — Id "rslvr-autodefined-rr-internet-resolver", Name "Internet Resolver", DomainName ".", RuleType RECURSIVE, Status COMPLETE, ShareStatus NOT_SHARED, OwnerId "Route 53 Resolver", CreatorRequestId "", and an ARN of the form arn:aws:route53resolver:<region>::autodefined-rule/rslvr-autodefined-rr-internet-resolver (no account id in the ARN). The id, name and shape are fixed; only the Region in the ARN varies.
Emulator: HTTP 200 with an empty/short ResolverRules list — the autodefined entry is absent (replay: "$.ResolverRules[0]: missing").
Why: a client that enumerates rules to find or skip the default Internet Resolver (for example before creating a FORWARD rule for ".", or when rendering the rule list) sees a catalog AWS never serves, and code that indexes the autodefined rule by id fails outright.
Fix: the Resolver-rule catalog seeded in the crate's state module — the autodefined recursive rule must exist per Region and be projected by ListResolverRules (and readable by GetResolverRule).
Traces: route53resolver-catalog-list-resolver-rules#task#1

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). Six mismatch clusters, none stub-blocked, none owned by an open issue.

Cause 1 — CreateResolverRule, route53resolver-resolver-rule-system-lifecycle#task#1, 200 -> 200:
  $.ResolverRule.StatusMessage: missing (expected '<shaped-not-asserted>', got None)
Cause 2 — GetResolverRule, ...#task#2, 200 -> 200:
  $.ResolverRule.StatusMessage: missing (expected '{{rule_status_message_created}}', got None)   [2 affected steps]
Cause 3 — ListResolverRules, ...#task#3, 200 -> 200:
  $.ResolverRules[Arn="arn:aws:route53resolver:us-west-2:000000000000:resolver-rule/rslvr-rr-910a2dec89025cc1"].StatusMessage: missing (expected '{{rule_status_message_created}}', got None)
Cause 4 — UpdateResolverRule, ...#task#4, 200 -> 200:
  $.ResolverRule.StatusMessage: missing (expected '<shaped-not-asserted>', got None)
Cause 5 — DeleteResolverRule, ...#task#11, 200 -> 200:
  $.ResolverRule.StatusMessage: missing (expected '<shaped-not-asserted>', got None)
Cause 6 — ListResolverRules, route53resolver-catalog-list-resolver-rules#task#1, 200 -> 200:
  $.ResolverRules[0]: missing (expected {'Arn': 'arn:aws:route53resolver:us-west-2::autodefined-rule/rslvr-autodefined-rr-internet-resolver', 'CreatorRequestId': '', 'DomainName': '.', 'Id': 'rslvr-autodefined-rr-internet-resolver', 'Name': 'Internet Resolver', 'OwnerId': 'Route 53 Resolver', 'RuleType': 'RECURSIVE', 'ShareStatus': 'NOT_SHARED', 'Status': 'COMPLETE'}, got None)

## Steps to Reproduce
Replay fixtures route53resolver-resolver-rule-system-lifecycle (task#1, task#2, task#3, task#4, task#11) and route53resolver-catalog-list-resolver-rules (task#1). Minimal reproductions:
1-5. CreateResolverRule(CreatorRequestId=<id>, Name=<n>, RuleType=SYSTEM, DomainName=<d>), then GetResolverRule, ListResolverRules, UpdateResolverRule and DeleteResolverRule on it -> each ResolverRule projection must carry StatusMessage.
6. On a clean account, call ListResolverRules with no filters -> the response must include the autodefined rule rslvr-autodefined-rr-internet-resolver (Name "Internet Resolver", DomainName ".", RuleType RECURSIVE, OwnerId "Route 53 Resolver").
