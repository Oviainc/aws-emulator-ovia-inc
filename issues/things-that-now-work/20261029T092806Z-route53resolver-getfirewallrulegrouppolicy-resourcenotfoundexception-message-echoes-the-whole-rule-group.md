Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.firewall-rule-group-policy-lifecycle
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.firewall-rule-group-policy-lifecycle|run#21|AssertionError|assert message == '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-frg-00000000000000000".', message
Issue-Commit: 934d4b192e4a1d8926d0f8f396fb86d9a2693e80

# GetFirewallRuleGroupPolicy ResourceNotFoundException message echoes the whole rule-group ARN where Route 53 Resolver names only the rule-group ID (RSLVR-02025)

> _Note: this issue was identified against the code base at commit `934d4b192e4a1d8926d0f8f396fb86d9a2693e80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFirewallRuleGroupPolicy

## Description
Cause 1 of 1: when GetFirewallRuleGroupPolicy is called with an `Arn` that names a DNS Firewall rule group which does not exist, the emulator refuses with the right status and the right error code, but builds the RSLVR-02025 message out of the full ARN it was handed instead of the rule-group ID inside that ARN.

AWS: Smithy models GetFirewallRuleGroupPolicy(Arn: Arn, required, @length 1..255) -> GetFirewallRuleGroupPolicyResponse{FirewallRuleGroupPolicy: @length 0..30000}, errors AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400 (defaulted); ResourceNotFoundException carries {Message, ResourceType} and the model states no message text, so the wording is recorded behavior only. The recorded Route 53 Resolver response for an unknown rule group on this operation is HTTP 400, Error.Code `ResourceNotFoundException`, Error.Message:

    [RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupId>".

Message anatomy: the FIXED text is `[RSLVR-02025] Can’t find the resource with ID "` ... `".` (note the service's U+2019 right single quote in `Can’t`, the double quotes around the value, and the sentence-ending period). The single VARIABLE part is the RULE GROUP ID — the `rslvr-frg-...` resource-id component of the submitted ARN, i.e. the segment after `firewall-rule-group/` — never the ARN as submitted, and never a per-run value the service invents. Resolver identifies the resource the same way in this message whether the caller addressed it by id (GetFirewallRuleGroup takes FirewallRuleGroupId) or by ARN (this operation takes Arn): the message always names the bare id. (Real Resolver also appends a trailing ` Trace Id: "<Root>"` clause; that separate omission is already filed as open issue 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing, and the failing check here strips that suffix before comparing, so it is NOT this cause — the divergence below is in the identifier the sentence names.)

Emulator: the refusal is HTTP 400 with Error.Code `ResourceNotFoundException` (correct), but Error.Message is `[RSLVR-02025] Can’t find the resource with ID "arn:aws:route53resolver:<region>:<account>:firewall-rule-group/<FirewallRuleGroupId>".` — the whole ARN appears where AWS puts the bare id. The handler looks the group up by ARN and then hands the same ARN string straight to the not-found message builder, so no id is ever extracted.

Why: a client that matches, parses or asserts on Resolver's refusal gets a string no real Resolver response can produce. The message is how a caller recovers WHICH resource was missing (error-triage tooling and CLI wrappers commonly pull the quoted id out of the sentence and correlate it with their own records of `rslvr-frg-` ids); against the emulator they extract an ARN instead of an id, so that correlation breaks silently. Negative tests written against AWS's exact message fail against the emulator, and tests written against the emulator's message fail in production. In this suite the divergence aborts the check at its LAST assertion — everything before it (the empty-policy document on a fresh group, and both ValidationException RSLVR-02035 refusals) passes — so the not-found contract of the policy operations is the only part left unverified.

Fix: the GetFirewallRuleGroupPolicy handler's not-found path, which must resolve the submitted ARN to its rule-group id before composing the message rather than passing the ARN through; the shared message builder keeps its single id parameter, so sibling ARN-addressed operations (PutFirewallRuleGroupPolicy, and the resolver-query-log-config policy operations that address resources by ARN) need the same resolution rather than a local patch here. File and line citations are under Traces.

Traces: route53resolver.firewall-rule-group-policy-lifecycle. Citations: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/get_firewall_rule_group_policy.rs:165-172 (looks up via `store.firewall_rule_group_by_arn(&input.arn)` and on miss calls `crate::state::firewall_rule_group_not_found(&input.arn)` — the ARN, not the id), and services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs:486-504 / :1195-1200 (`rslvr_message`, `resource_not_found_message`, `firewall_rule_group_not_found`, which quote their argument verbatim).

## Evidence
Cause 1 — route53resolver.firewall-rule-group-policy-lifecycle [mismatch]

The check creates a rule group, reads its empty policy, asserts the two RSLVR-02035 ValidationException refusals of an invalid policy document (all of which PASS against the emulator), then addresses a non-existent group by rewriting the real group's ARN's last segment to `rslvr-frg-00000000000000000` and calls GetFirewallRuleGroupPolicy on it.

Request (check source):
  bogus_arn = group_arn.rsplit("/", 1)[0] + "/rslvr-frg-00000000000000000"
  r53r.get_firewall_rule_group_policy(Arn=bogus_arn)

Expected (AWS): HTTP 400, Error.Code = "ResourceNotFoundException",
  Error.Message = '[RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupId>".'
  i.e. only the `rslvr-frg-...` segment of the submitted ARN is quoted.

Actual (emulator): HTTP 400, Error.Code = "ResourceNotFoundException" (correct),
  Error.Message = '[RSLVR-02025] Can’t find the resource with ID "arn:aws:route53resolver:<region>:<account>:firewall-rule-group/<FirewallRuleGroupId>".'

Replay excerpt:
  File "check_route53resolver_firewall_rule_group_policy_lifecycle.py", line 78, in run
    assert message == '[RSLVR-02025] Can\u2019t find the resource with ID "rslvr-frg-00000000000000000".', message
  AssertionError: [RSLVR-02025] Can’t find the resource with ID "arn:aws:route53resolver:us-west-2:000000000000:firewall-rule-group/rslvr-frg-00000000000000000".

The region `us-west-2`, account `000000000000` and the all-zero id above are this run's values filling the template; the requirement is that the quoted value is the rule-group id component only, not the ARN. The check's `_error()` helper already strips any trailing ` Trace Id: "..."` clause before comparing, so the trailing-clause omission filed in 20261028T035605Z-... is not what fails this assertion.

Model: GetFirewallRuleGroupPolicy input {Arn: Arn required, @length 1..255}; output {FirewallRuleGroupPolicy @length 0..30000}; errors AccessDeniedException, InternalServiceErrorException, ResourceNotFoundException, ThrottlingException, ValidationException — every one httpError 400 (defaulted). ResourceNotFoundException{Message: String, ResourceType: String}; no message text is modeled, so the wording is recorded behavior only.

Code: get_firewall_rule_group_policy.rs `let Some(record) = store.firewall_rule_group_by_arn(&input.arn) else { return Err(... ResourceNotFoundException(crate::state::firewall_rule_group_not_found(&input.arn)) ...) }` — the ARN is forwarded as the message's "ID".

## Steps to Reproduce
1. r53r = boto3 client for route53resolver against the emulator.
2. group = r53r.create_firewall_rule_group(CreatorRequestId=<unique>, Name=<unique>)["FirewallRuleGroup"]; group_arn = group["Arn"].
3. Build an ARN for a rule group that does not exist, keeping the real ARN's prefix: bogus_arn = group_arn.rsplit("/", 1)[0] + "/rslvr-frg-00000000000000000".
4. r53r.get_firewall_rule_group_policy(Arn=bogus_arn). It is refused with HTTP 400 / ResourceNotFoundException (correct). Inspect Error.Message: the emulator returns '[RSLVR-02025] Can’t find the resource with ID "<the whole bogus ARN>".' while AWS returns '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-00000000000000000".' — the bare rule-group id.
5. Compare with the id-addressed sibling, which already names the bare id: r53r.get_firewall_rule_group(FirewallRuleGroupId="rslvr-frg-00000000000000000") — same RSLVR-02025 sentence, quoting the id. AWS's message is identical in both cases; the emulator's differs only because this operation is addressed by ARN.
6. Same ARN-echo is expected on the other ARN-addressed policy entry point: r53r.put_firewall_rule_group_policy(Arn=bogus_arn, FirewallRuleGroupPolicy='{}').

Replay: run_behavioral_suite and re-run route53resolver.firewall-rule-group-policy-lifecycle.
