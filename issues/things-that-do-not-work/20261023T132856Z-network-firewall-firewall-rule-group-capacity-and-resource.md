Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-firewall.describe-firewall-not-found-rejected network-firewall.firewall-deletion-removes-resource network-firewall.delete-protected-firewall-rejected network-firewall.rule-group-capacity-tracked-on-create network-firewall.update-rule-group-exceeds-capacity-rejected network-firewall.resource-policy-missing-required-action-rejected network-firewall.resource-policy-put-describe-delete-roundtrip
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall.describe-firewall-not-found-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: network-firewall.firewall-deletion-removes-resource|run#13|AssertionError|assert gone_message == expected_gone, f"expected message {expected_gone!r}, got {gone_message!r}"
Issue-Key: network-firewall.delete-protected-firewall-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: network-firewall.rule-group-capacity-tracked-on-create|run#7|AssertionError|assert message == (
Issue-Key: network-firewall.update-rule-group-exceeds-capacity-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: network-firewall.resource-policy-missing-required-action-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: network-firewall.resource-policy-put-describe-delete-roundtrip|run#21|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# Network Firewall firewall, rule-group capacity and resource-policy error paths return emulator-authored messages instead of the service's fixed/derived text

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFirewall

## Description
SIX net-new root causes, one service (network-firewall). Every one of these error paths returns the RIGHT HTTP status (400) and the RIGHT error code; only the Message diverges — but in each case the message is a requirement, not an incidental string: it is either a FIXED resource-agnostic constant that the service repeats verbatim across operations, or it is BUILT from the request with a fixed stem plus a bracketed parameter. The emulator instead composes per-handler prose that embeds request-derived content (firewall names, computed capacity numbers, action diffs, ARNs) where the service emits none. Each cause is its own replay cluster and each needs its own message corrected before this issue is submitted.

An open issue, "Network Firewall error responses replace AWS's fixed exception messages with emulator-authored text", already establishes this same constant-message rule for DescribeFirewallPolicy, DeleteFirewallPolicy, DescribeRuleGroup, DeleteRuleGroup, DescribeResourcePolicy, CreateRuleGroup's duplicate-name arm, UpdateRuleGroup's stale-token arm and CreateFirewallPolicy's bad-reference arm. The operations and arms below are NOT among the ones it lists (the single exception is called out in cause 6), so they are not covered by it; the ideal fix is one shared service-level constant that all of them draw from.

Cause 1 of 6: DescribeFirewall's ResourceNotFoundException message names the resource type.
AWS: Smithy (com.amazonaws.networkfirewall#NetworkFirewall_20201112, awsJson1_0) models DescribeFirewall with ResourceNotFoundException (client, HTTP 400, member Message -> ErrorMessage) documented "Unable to locate a resource using the parameters that you provided." The model does not fix the wire text, so for the text this is recorded behavior only — but the recording is unambiguous and repeated: the Message is exactly 'No object with the provided name or resource identifier was found', with no resource name, type or ARN appended, and it is byte-identical across the service's other not-found paths. The rule is therefore one constant message for ResourceNotFoundException, carrying no request-derived content.
Emulator: HTTP 400 / ResourceNotFoundException with Message 'The firewall does not exist.'
Why: ResourceNotFoundException is shared by several conditions, so clients and idempotent teardown code routinely match on the service's not-found text; against the emulator that match fails. The per-handler text also makes the emulator's not-found message differ between operations, which the service's does not.
Fix: the DescribeFirewall handler's not-found arm, which should take its message from the one shared service-level constant instead of composing its own.
Traces: network-firewall.describe-firewall-not-found-rejected (DescribeFirewall of a never-created firewall), network-firewall.firewall-deletion-removes-resource (DescribeFirewall after the firewall is actually gone). The same assertion also sits downstream in network-firewall.delete-protected-firewall-rejected.

Cause 2 of 6: DeleteFirewall's delete-protection refusal message leaks the firewall name.
AWS: Smithy models DeleteFirewall with InvalidOperationException (client, HTTP 400, Message -> ErrorMessage) documented "The operation failed because it's not valid." The status and code are what the model fixes; for the text this is recorded behavior only, and the recorded refusal for a firewall created with DeleteProtection=True is the resource-agnostic constant 'Unable to delete the object because delete protection is enabled' — no firewall name, no ARN. (The same constant family appears on the in-use refusal, 'Unable to delete the object because it is still in use'.)
Emulator: HTTP 400 / InvalidOperationException (status and code correct) with Message 'Firewall <firewall-name> has DeleteProtection enabled and cannot be deleted.'
Why: the message grows request-derived content the service never emits, so message-based detection of the delete-protection refusal written against AWS does not match, and the requested resource name surfaces in error output where AWS emits none.
Fix: the DeleteFirewall handler's delete-protection guard — emit the shared constant.
Traces: network-firewall.delete-protected-firewall-rejected.

Cause 3 of 6: CreateRuleGroup's capacity-exceeded message uses the wrong form entirely.
AWS: Smithy models CreateRuleGroup with required Capacity (RuleCapacity) and the client error InvalidRequestException (HTTP 400, "The operation failed because of a problem with your request"). Here the message IS built from the request and its shape is visible in the recording: a fixed stem, the required capacity in parameter brackets, and the offending input path as context — 'StatefulRules capacity exceeded, parameter: [<required capacity units>], context: RulesSource.StatefulRules'. That format is the requirement; the number inside the brackets is derived from the request (for a stateful rule group, the rule count).
Emulator: HTTP 400 / InvalidRequestException (status and code correct) with Message "The rules in RulesSource.StatefulRules require <n> capacity units, which exceeds the rule group's capacity of <m>." — free prose, with neither the stem nor the 'parameter: [..], context: ..' structure.
Why: this is one of the few Network Firewall messages that is parseable by construction; a client that extracts the required capacity from the bracketed parameter in order to retry CreateRuleGroup with a sufficient Capacity cannot parse the emulator's text at all, so the self-correcting retry that works against AWS fails.
Fix: the CreateRuleGroup handler's capacity validation (the DryRun path takes the same branch) — emit the service's stem and 'parameter: [..], context: ..' structure.
Traces: network-firewall.rule-group-capacity-tracked-on-create.

Cause 4 of 6: UpdateRuleGroup's capacity-exceeded message, same divergence on the update path.
AWS: UpdateRuleGroup models InvalidRequestException (client, HTTP 400) and a rule group's Capacity is fixed at creation (RuleGroupResponse$Capacity: "Rule group capacity is fixed at creation. When you update a rule group, you are limited to this capacity."), so rules needing more than it are refused with the same built message form: 'StatefulRules capacity exceeded, parameter: [<required capacity units>], context: RulesSource.StatefulRules'.
Emulator: Message "The rules in RulesSource.StatefulRules require <n> capacity units, which exceeds the rule group's capacity of <m>."
Why: as cause 3 — and the update path is where a client most needs to read the required capacity back, since it cannot raise the rule group's Capacity and must decide whether to split the rules.
Fix: the UpdateRuleGroup handler's capacity validation; ideally the shared capacity-check helper that cause 3's handler also calls.
Traces: network-firewall.update-rule-group-exceeds-capacity-rejected.

Cause 5 of 6: PutResourcePolicy's InvalidResourcePolicyException message appends the emulator's computed action diff.
AWS: Smithy models PutResourcePolicy (required ResourceArn, required Policy -> PolicyString length 1..395000 pattern \S) with InvalidResourcePolicyException (client, HTTP 400) documented "The policy statement failed validation." For the text this is recorded behavior only: the refusal carries the constant 'The supplied policy does not match RAM managed permissions.' and nothing else — no list of offending or missing actions.
Emulator: HTTP 400 / InvalidResourcePolicyException (status and code correct) with Message 'The supplied policy does not match RAM managed permissions. Unexpected actions: [network-firewall:DeleteFirewallPolicy]; missing actions: [network-firewall:AssociateFirewallPolicy, network-firewall:ListFirewallPolicies].' — the right stem followed by validator internals derived from the request.
Why: the message grows request-dependent content AWS does not emit, so a client comparing or logging the policy-validation message behaves differently against the emulator, and the names of actions taken from the submitted policy document leak into error output.
Fix: the PutResourcePolicy handler's policy-validation arm (and the state module's policy-breach reporting it formats) — emit the constant stem only. Note: the open issue "Network Firewall PutResourcePolicy rejects a valid RAM subset policy, blocking the resource-policy lifecycle" covers a DIFFERENT defect on this operation (a policy granting a permitted SUBSET must be accepted, not refused); this cause is the message text on the path where refusal is correct, i.e. a policy naming an action outside the permitted set.
Traces: network-firewall.resource-policy-missing-required-action-rejected.

Cause 6 of 6: DescribeResourcePolicy's not-found message appends the resource ARN.
AWS: the same constant as cause 1 — 'No object with the provided name or resource identifier was found', with NO ARN suffix, when the named resource carries no policy (including after DeleteResourcePolicy has detached one).
Emulator: HTTP 400 / ResourceNotFoundException (status and code correct) with Message 'No object with the provided name or resource identifier was found: arn:aws:network-firewall:<region>:<account>:firewall-policy/<policy-name>' — the right stem with the request's ARN interpolated.
Why: as cause 1; the interpolated ARN makes message comparison and log redaction behave differently, and makes the emulator's not-found text operation-dependent.
Fix: the DescribeResourcePolicy handler's not-found arm — drop the interpolated ARN and use the shared constant (the same interpolation exists on the PutResourcePolicy and DeleteResourcePolicy not-found arms). Note: cause 5 of the open issue "Network Firewall error responses replace AWS's fixed exception messages with emulator-authored text" already names this exact interpolation for DescribeResourcePolicy from the wire corpus; this cause is recorded because the behavioral check below is an independent witness of it on the firewall-policy resource type and must be re-replayed to confirm the fix. If that issue's cause 5 is fixed first, this cause closes with it — but it must be re-replayed, not assumed.
Traces: network-firewall.resource-policy-put-describe-delete-roundtrip.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the built emulator. Per-service: network-firewall 4/22 (18%), 18 failed. In every cluster below the HTTP status (400) and the error code MATCHED; only the Message diverged. Generated resource names, the account id 000000000000, the region and ARNs are environment-specific and are NOT part of any requirement: the requirement in causes 1, 2, 5 and 6 is the constant text with no request-derived content, and in causes 3 and 4 the fixed stem plus the 'parameter: [..], context: ..' structure.

Cause 1 — network-firewall.describe-firewall-not-found-rejected [mismatch], DescribeFirewall, 400 / ResourceNotFoundException:
  AssertionError: expected message 'No object with the provided name or resource identifier was found', got 'The firewall does not exist.'
  Same pair at network-firewall.firewall-deletion-removes-resource [mismatch], check line 49, asserting the message DescribeFirewall returns once the deleted firewall is actually gone.

Cause 2 — network-firewall.delete-protected-firewall-rejected [mismatch], DeleteFirewall on a firewall created with DeleteProtection=True, 400 / InvalidOperationException:
  AssertionError: expected message 'Unable to delete the object because delete protection is enabled', got 'Firewall sim2real-nfw-940d38c0f607 has DeleteProtection enabled and cannot be deleted.'

Cause 3 — network-firewall.rule-group-capacity-tracked-on-create [mismatch], CreateRuleGroup with Type=STATEFUL, Capacity=1, DryRun=True and two StatefulRules, 400 / InvalidRequestException:
  AssertionError: unexpected capacity-rejection message: "The rules in RulesSource.StatefulRules require 2 capacity units, which exceeds the rule group's capacity of 1."  (expected 'StatefulRules capacity exceeded, parameter: [2], context: RulesSource.StatefulRules')

Cause 4 — network-firewall.update-rule-group-exceeds-capacity-rejected [mismatch], UpdateRuleGroup on a Capacity=10 rule group with 199 rules, 400 / InvalidRequestException:
  AssertionError: expected message 'StatefulRules capacity exceeded, parameter: [199], context: RulesSource.StatefulRules', got "The rules in RulesSource.StatefulRules require 199 capacity units, which exceeds the rule group's capacity of 10."

Cause 5 — network-firewall.resource-policy-missing-required-action-rejected [mismatch], PutResourcePolicy, 400 / InvalidResourcePolicyException:
  AssertionError: expected message 'The supplied policy does not match RAM managed permissions.', got 'The supplied policy does not match RAM managed permissions. Unexpected actions: [network-firewall:DeleteFirewallPolicy]; missing actions: [network-firewall:AssociateFirewallPolicy, network-firewall:ListFirewallPolicies].'

Cause 6 — network-firewall.resource-policy-put-describe-delete-roundtrip [mismatch], DescribeResourcePolicy after DeleteResourcePolicy, 400 / ResourceNotFoundException (check line 70):
  AssertionError: expected message 'No object with the provided name or resource identifier was found', got 'No object with the provided name or resource identifier was found: arn:aws:network-firewall:us-west-2:000000000000:firewall-policy/sim2real-nfw-policy-01a6893a8956'

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): DescribeFirewall (errors InternalServerError/InvalidRequestException/ResourceNotFoundException/ThrottlingException), DeleteFirewall (errors incl. InvalidOperationException), CreateRuleGroup (required Capacity -> RuleCapacity, required Type, DryRun default false; errors incl. InvalidRequestException), UpdateRuleGroup (required UpdateToken; errors incl. InvalidRequestException), PutResourcePolicy (required ResourceArn, required Policy -> PolicyString length 1..395000 pattern \S; errors InvalidRequestException/InvalidResourcePolicyException/ResourceNotFoundException), RuleGroupResponse$Capacity ("Rule group capacity is fixed at creation"), InvalidOperationException (client, HTTP 400, Message -> ErrorMessage), InvalidRequestException (client, HTTP 400), ResourceNotFoundException (client, HTTP 400). The model fixes the status and classification only, so the texts above are grounded in the recordings.

## Steps to Reproduce
Cause 1:
1. aws network-firewall describe-firewall --firewall-name does-not-exist -> 400 ResourceNotFoundException. Compare Message: emulator 'The firewall does not exist.', AWS 'No object with the provided name or resource identifier was found'.
2. Create a firewall, delete it, poll describe-firewall until it reports ResourceNotFoundException, and compare the same Message.

Cause 2:
1. aws network-firewall create-firewall --firewall-name fw1 --firewall-policy-arn <policy> --vpc-id <vpc> --subnet-mappings SubnetId=<subnet> --delete-protection
2. aws network-firewall delete-firewall --firewall-arn <arn> -> 400 InvalidOperationException. Emulator Message names the firewall; AWS Message is the constant 'Unable to delete the object because delete protection is enabled'.
3. aws network-firewall update-firewall-delete-protection --firewall-arn <arn> --no-delete-protection, then delete again -> must succeed (positive control).

Cause 3:
1. aws network-firewall create-rule-group --rule-group-name rg-under --type STATEFUL --capacity 1 --dry-run --rule-group '{"RulesSource":{"StatefulRules":[{"Action":"DROP","Header":{"Protocol":"TCP","Source":"ANY","SourcePort":"ANY","Direction":"FORWARD","Destination":"192.0.2.0/24","DestinationPort":"445"},"RuleOptions":[{"Keyword":"sid:1"}]},{"Action":"ALERT","Header":{"Protocol":"UDP","Source":"ANY","SourcePort":"ANY","Direction":"ANY","Destination":"198.51.100.0/24","DestinationPort":"53"},"RuleOptions":[{"Keyword":"sid:2"}]}]}}'
2. -> 400 InvalidRequestException. Compare the Message against 'StatefulRules capacity exceeded, parameter: [2], context: RulesSource.StatefulRules'.

Cause 4:
1. Create a STATEFUL rule group with --capacity 10 and one rule; note its UpdateToken.
2. aws network-firewall update-rule-group --rule-group-arn <arn> --update-token <token> --type STATEFUL --rule-group <199 StatefulRules> -> 400 InvalidRequestException. Compare the Message against 'StatefulRules capacity exceeded, parameter: [199], context: RulesSource.StatefulRules'.

Cause 5:
1. Create a firewall policy; note its FirewallPolicyArn.
2. aws network-firewall put-resource-policy --resource-arn <FirewallPolicyArn> --policy '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"<account-id>"},"Action":["network-firewall:DeleteFirewallPolicy"],"Resource":"<FirewallPolicyArn>"}]}' -> 400 InvalidResourcePolicyException. The Message must be exactly 'The supplied policy does not match RAM managed permissions.' with no action lists appended.

Cause 6:
1. Create a firewall policy, put-resource-policy a valid policy on it, then delete-resource-policy.
2. aws network-firewall describe-resource-policy --resource-arn <FirewallPolicyArn> -> 400 ResourceNotFoundException. The Message must be the constant with no ': <arn>' suffix.
3. aws network-firewall delete-resource-policy --resource-arn <FirewallPolicyArn> again -> 400 ResourceNotFoundException with the same constant Message.
