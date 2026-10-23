Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-firewall-policy-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-firewall-policy-error-paths network-firewall-rule-group-error-paths network-firewall-rule-group-stateful-lifecycle network-firewall-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|DescribeFirewallPolicy|4xx->4xx|$.Message
Issue-Key: network-firewall|DeleteFirewallPolicy|4xx->4xx|$.Message
Issue-Key: network-firewall|DescribeRuleGroup|4xx->4xx|$.Message
Issue-Key: network-firewall|DeleteRuleGroup|4xx->4xx|$.Message
Issue-Key: network-firewall|DescribeResourcePolicy|4xx->4xx|$.Message
Issue-Key: network-firewall|CreateRuleGroup|4xx->4xx|$.Message
Issue-Key: network-firewall|UpdateRuleGroup|4xx->4xx|$.Message
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->4xx|$.Message
Issue-Commit: b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf

# Network Firewall error responses replace AWS's fixed exception messages with emulator-authored text

> _Note: this issue was identified against the code base at commit `b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFirewallPolicy

## Description
Every error path exercised by the corpus returns the RIGHT HTTP status (400) and the RIGHT error code (__type), but the Message member carries text this emulator authored rather than the text the service produces. Across three fixtures and eight operations the recordings show Network Firewall uses a FIXED, resource-agnostic message per exception type — the same string verbatim for DescribeFirewallPolicy, DeleteFirewallPolicy, DescribeRuleGroup, DeleteRuleGroup and DescribeResourcePolicy — so the message is a property of the exception, not of the request, and the emulator's per-handler strings additionally leak resource names and ARNs that AWS's text never contains. EIGHT causes, one service; each is its own replay cluster and each needs its own message corrected.

Cause 1 of 8: DescribeFirewallPolicy's ResourceNotFoundException message.
AWS: Smithy models ResourceNotFoundException (client error, HTTP 400, member Message -> ErrorMessage) with the doc "Unable to locate a resource using the parameters that you provided." The model does not fix the wire text, so for the text this is recorded behavior only — but the recording is unambiguous and repeated: the Message is exactly 'No object with the provided name or resource identifier was found', with no name, type or ARN appended, and it is byte-identical across five different operations in three fixtures. The rule is therefore: one constant message for ResourceNotFoundException, carrying no request-derived content.
Emulator: HTTP 400 / ResourceNotFoundException with $.Message = 'The firewall policy does not exist.'
Why: a client that matches or logs on the service's not-found message (a common pattern where the error code alone is shared by several conditions) does not match, and emulator-only text that embeds the requested name makes recorded request data appear in error output where AWS emits none.
Fix: the DescribeFirewallPolicy handler's not-found arm, which should take the message from one shared service-level constant rather than composing its own.
Traces: network-firewall-firewall-policy-error-paths#task#2 (also #teardown#7).

Cause 2 of 8: DeleteFirewallPolicy's ResourceNotFoundException message.
AWS: as cause 1 — Message is the same constant 'No object with the provided name or resource identifier was found'.
Emulator: $.Message = 'The firewall policy does not exist.'
Why: as cause 1; the delete path is where idempotent teardown code most often inspects the not-found message.
Fix: the DeleteFirewallPolicy handler's not-found arm (shared constant).
Traces: network-firewall-firewall-policy-error-paths#task#5.

Cause 3 of 8: DescribeRuleGroup's ResourceNotFoundException message.
AWS: the same constant message.
Emulator: $.Message = 'The rule group does not exist.'
Why: as cause 1.
Fix: the DescribeRuleGroup handler's not-found arm and the shared rule-group lookup-error macro it expands.
Traces: network-firewall-rule-group-error-paths#task#3 (also that fixture's #teardown#7 and network-firewall-rule-group-stateful-lifecycle#teardown#9).

Cause 4 of 8: DeleteRuleGroup's ResourceNotFoundException message.
AWS: the same constant message.
Emulator: $.Message = 'The rule group does not exist.'
Why: as cause 1.
Fix: the DeleteRuleGroup handler's not-found arm (shared constant).
Traces: network-firewall-rule-group-error-paths#task#5 (also network-firewall-rule-group-stateful-lifecycle#teardown#10).

Cause 5 of 8: DescribeResourcePolicy appends the resource ARN to the ResourceNotFoundException message.
AWS: the same constant message, with NO ARN suffix, when the named resource carries no policy.
Emulator: $.Message = 'No object with the provided name or resource identifier was found: arn:aws:network-firewall:<region>:<account>:stateful-rulegroup/<name>' — the right stem with the request's ARN interpolated.
Why: the message grows request-dependent content AWS does not emit, so a client comparing messages (or redacting ARNs from logs) behaves differently against the emulator; it also makes the emulator's not-found text differ between operations, which AWS's does not.
Fix: the DescribeResourcePolicy handler's not-found arm — drop the interpolated ARN and use the shared constant. The same interpolation exists on the PutResourcePolicy and DeleteResourcePolicy not-found arms and should be corrected with it.
Traces: network-firewall-resource-policy-lifecycle#task#2 (also that fixture's #task#6).

Cause 6 of 8: CreateRuleGroup's duplicate-name rejection message.
AWS: the recorded 400 for creating a rule group whose name and type already exist carries the constant Message 'A resource with the specified name already exists' — again resource-agnostic, naming neither the type nor the name (recorded behavior only for the text; the status and code are what the model fixes).
Emulator: $.Message = 'A STATEFUL rule group named <name> already exists.'
Why: as cause 5 — the emulator's text embeds the requested type and name, so message-based duplicate detection written against AWS does not match and request data surfaces in error output.
Fix: the CreateRuleGroup handler's already-exists arm (shared constant message).
Traces: network-firewall-rule-group-error-paths#task#2.

Cause 7 of 8: UpdateRuleGroup's stale-token rejection message.
AWS: InvalidTokenException is modelled as a client error (HTTP 400, member Message) documented "The token you provided is stale or isn't valid for the operation"; the recorded Message for an UpdateRuleGroup carrying a superseded UpdateToken is the constant "Update token isn't valid."
Emulator: $.Message = 'The UpdateToken you provided is stale; retrieve the rule group again to get a current one.'
Why: optimistic-locking clients retry on the token error and frequently key that retry on the service's message; emulator-only prose makes that branch untestable even though the status and code are right.
Fix: the UpdateRuleGroup handler's update-token check (shared constant message for InvalidTokenException).
Traces: network-firewall-rule-group-error-paths#task#4.

Cause 8 of 8: CreateFirewallPolicy's unknown-rule-group-reference message uses the wrong stem.
AWS: here the message IS built from the request and its shape is visible in the recording: 'rule group ARN does not exist, parameter: [<the referenced ARN>]' — a fixed stem plus the offending ARN in the parameter brackets. That format is the requirement; the ARN inside it is the request's own value.
Emulator: $.Message = 'FirewallPolicy.StatefulRuleGroupReferences references a rule group that does not exist, parameter: [<same ARN>]' — the parameter half matches, the stem does not and names the input member instead.
Why: this is the one error where the message is parseable by construction (fixed stem + bracketed parameter); a client that matches the stem to distinguish a bad rule-group reference from other CreateFirewallPolicy validation failures does not match the emulator's.
Fix: the CreateFirewallPolicy handler's rule-group-reference resolution — emit the service's stem, keeping the bracketed parameter.
Traces: network-firewall-firewall-policy-error-paths#task#3.

Relation to existing issues: the only open network-firewall issue, "Network Firewall responses omit default EncryptionConfiguration.KeyId and policy capacity counters", covers members missing from 200 responses and names none of these error paths.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200; state_reset_failures 0). In every cluster below the HTTP status and the __type matched; only $.Message diverged.

Cause 1 — network-firewall-firewall-policy-error-paths#task#2, DescribeFirewallPolicy, 400 -> 400:
  $.Message: value_mismatch (expected 'No object with the provided name or resource identifier was found', got 'The firewall policy does not exist.')  [cluster also covers #teardown#7]
Cause 2 — network-firewall-firewall-policy-error-paths#task#5, DeleteFirewallPolicy, 400 -> 400: same expected/actual pair.
Cause 3 — network-firewall-rule-group-error-paths#task#3, DescribeRuleGroup, 400 -> 400:
  $.Message: value_mismatch (expected 'No object with the provided name or resource identifier was found', got 'The rule group does not exist.')  [cluster also covers network-firewall-rule-group-error-paths#teardown#7 and network-firewall-rule-group-stateful-lifecycle#teardown#9]
Cause 4 — network-firewall-rule-group-error-paths#task#5, DeleteRuleGroup, 400 -> 400: same pair  [cluster also covers network-firewall-rule-group-stateful-lifecycle#teardown#10]
Cause 5 — network-firewall-resource-policy-lifecycle#task#2, DescribeResourcePolicy, 400 -> 400:
  $.Message: value_mismatch (expected 'No object with the provided name or resource identifier was found', got 'No object with the provided name or resource identifier was found: arn:aws:network-firewall:us-west-2:000000000000:stateful-rulegroup/<generated name>')  [cluster also covers #task#6]
Cause 6 — network-firewall-rule-group-error-paths#task#2, CreateRuleGroup, 400 -> 400:
  $.Message: value_mismatch (expected 'A resource with the specified name already exists', got 'A STATEFUL rule group named <generated name> already exists.')
Cause 7 — network-firewall-rule-group-error-paths#task#4, UpdateRuleGroup, 400 -> 400:
  $.Message: value_mismatch (expected "Update token isn't valid.", got 'The UpdateToken you provided is stale; retrieve the rule group again to get a current one.')
Cause 8 — network-firewall-firewall-policy-error-paths#task#3, CreateFirewallPolicy, 400 -> 400:
  $.Message: value_mismatch (expected 'rule group ARN does not exist, parameter: [<arn>]', got 'FirewallPolicy.StatefulRuleGroupReferences references a rule group that does not exist, parameter: [<arn>]')

The generated resource names, account id, region and ARNs above are environment-specific and are NOT part of any requirement; the requirement in causes 1-7 is the constant message text with no request-derived content, and in cause 8 the fixed stem plus the bracketed parameter.

Basis: Smithy (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json) fixes the status and classification only — ResourceNotFoundException (client, HTTP 400, Message -> ErrorMessage, "Unable to locate a resource using the parameters that you provided"), InvalidTokenException (client, HTTP 400, "The token you provided is stale or isn't valid for the operation"), InvalidOperationException (client, HTTP 400) — so the texts above are grounded in the recordings, whose repetition of one identical ResourceNotFoundException string across five operations and three fixtures is what establishes the constant-message rule.
Code citations: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/describe_rule_group.rs:195-203 (per-handler composed message "Rule group {key} does not exist.") and :228-237 (impl_resolve_error!/impl_rule_group_lookup_error! macros that generate the lookup messages), src/ops/describe_resource_policy.rs:160-172 (ARN interpolated into the not-found message), src/ops/put_resource_policy.rs:217-227 (same interpolation).

## Steps to Reproduce
1. aws network-firewall describe-firewall-policy --firewall-policy-name does-not-exist -> 400 ResourceNotFoundException. Emulator Message 'The firewall policy does not exist.'; AWS 'No object with the provided name or resource identifier was found'. Same for delete-firewall-policy (cause 2).
2. aws network-firewall describe-rule-group --rule-group-name does-not-exist --type STATEFUL -> Message 'The rule group does not exist.' vs the constant AWS string. Same for delete-rule-group (cause 4).
3. Create a stateful rule group, then aws network-firewall describe-resource-policy --resource-arn <RuleGroupArn> before attaching any policy -> emulator appends ': <arn>' to the constant message (cause 5).
4. Create a rule group, then create it again with the same name and type -> emulator Message 'A STATEFUL rule group named <name> already exists.' vs AWS 'A resource with the specified name already exists' (cause 6).
5. Describe a rule group to get its UpdateToken, update it once (token rotates), then call update-rule-group again with the OLD token -> emulator Message 'The UpdateToken you provided is stale; ...' vs AWS "Update token isn't valid." (cause 7).
6. aws network-firewall create-firewall-policy --firewall-policy-name p1 --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"],"StatefulRuleGroupReferences":[{"ResourceArn":"arn:aws:network-firewall:us-west-2:000000000000:stateful-rulegroup/nope"}]}' -> compare the message stem before ', parameter: [' (cause 8).
