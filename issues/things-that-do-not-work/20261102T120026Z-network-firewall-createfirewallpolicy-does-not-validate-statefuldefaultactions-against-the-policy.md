Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-stateful-engine-options-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-stateful-engine-options-mistakes network-firewall-nfwr-traj-stateless-default-action-spelling
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->2xx|$.Message
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->2xx|$.__type
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->2xx|status
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->4xx|$.Message
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->4xx|$.__type
Issue-Key: network-firewall|CreateFirewallPolicy|2xx->4xx|$.FirewallPolicyResponse
Issue-Key: network-firewall|CreateFirewallPolicy|2xx->4xx|$.UpdateToken
Issue-Key: network-firewall|CreateFirewallPolicy|2xx->4xx|status
Issue-Commit: c1ca043e5c77dd90c2a96695b3dfa06a20838f7d

# CreateFirewallPolicy does not validate StatefulDefaultActions against the policy rule order

> _Note: this issue was identified against the code base at commit `c1ca043e5c77dd90c2a96695b3dfa06a20838f7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallPolicy

## Description
CreateFirewallPolicy accepts FirewallPolicy bodies that AWS refuses with InvalidRequestException, so invalid policies enter state and the valid policy that follows is then rejected for a name clash. This bundle has 3 causes.

Cause 1 of 3 - StatefulDefaultActions used with the wrong rule order is accepted (fixture network-firewall-nfwr-traj-stateful-engine-options-mistakes, step task#1; same cluster: network-firewall-nfwr-traj-stateless-default-action-spelling#task#1).
AWS: StatefulDefaultActions are only valid when FirewallPolicy.StatefulEngineOptions.RuleOrder is STRICT_ORDER (Smithy: RuleOrder enum values DEFAULT_ACTION_ORDER, STRICT_ORDER; the cross-member rule is recorded behavior). With DEFAULT_ACTION_ORDER in effect, AWS answers HTTP 400 with __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException' and Message 'StatefulDefaultActions has invalid rule order, parameter: [DEFAULT_ACTION_ORDER], context: StatefulDefaultActions'. Fixed text: 'StatefulDefaultActions has invalid rule order, parameter: [', '], context: StatefulDefaultActions'. Variable part: the effective RuleOrder value.
Emulator: HTTP 200 with a created policy; no __type, no Message.

Cause 2 of 3 - an unsupported StatefulDefaultActions value gets the wrong error and message (fixture network-firewall-nfwr-traj-stateful-engine-options-mistakes, step task#3; same cluster: four steps of network-firewall-nfwr-traj-stateless-default-action-spelling).
AWS: each StatefulDefaultActions entry must be one of the supported default actions; an unsupported one is refused with HTTP 400, __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException', Message 'StatefulDefaultActions has invalid action, parameter: [aws:DROP_ESTABLISHED], context: StatefulDefaultActions'. Fixed text: 'StatefulDefaultActions has invalid action, parameter: [', '], context: StatefulDefaultActions'. Variable part: the rejected action string taken verbatim from the request (aws:DROP_ESTABLISHED in this recording).
Emulator: HTTP 400 but __type 'InvalidOperationException' with Message 'A resource with the specified name already exists' - it never inspected the action; it only hit the policy name that Cause 1 wrongly created.

Cause 3 of 3 - the corrected CreateFirewallPolicy is refused (fixture network-firewall-nfwr-traj-stateful-engine-options-mistakes, step task#4; same cluster: network-firewall-nfwr-traj-stateless-default-action-spelling#task#7).
AWS: with no policy created by the refused attempts, the corrected request succeeds with HTTP 200 returning UpdateToken and FirewallPolicyResponse (Smithy: both required in CreateFirewallPolicy output; recording shows FirewallPolicyStatus ACTIVE, Consumed*Capacity values and EncryptionConfiguration Type AWS_OWNED_KMS_KEY).
Emulator: HTTP 400 with neither member.

Why: a client building a strict-order policy gets a false success for a policy AWS would never store, so its inspection engine configuration silently differs from production, and the modeled InvalidRequestException path is untested. An unmodeled InvalidOperationException whose message talks about a name clash actively misleads: the caller retries with a new name and hits the same wall. Cause 3 then makes the fix-and-retry flow impossible.

Fix: CreateFirewallPolicy's validation in the network-firewall service crate - validate StatefulDefaultActions against StatefulEngineOptions.RuleOrder and against the supported action set before mutating state, raising InvalidRequestException with the fully qualified __type and the templates above. Cause 3 should clear once invalid policies no longer reach state; re-replay all three bindings.

## Evidence
stateful-engine-options-mistakes#task#1 (and stateless-default-action-spelling#task#1): expected 400 __type ...#InvalidRequestException + Message 'StatefulDefaultActions has invalid rule order, parameter: [DEFAULT_ACTION_ORDER], context: StatefulDefaultActions'; actual 200 with no error members.
stateful-engine-options-mistakes#task#3 (5-member cluster): expected __type ...#InvalidRequestException + Message 'StatefulDefaultActions has invalid action, parameter: [aws:DROP_ESTABLISHED], context: StatefulDefaultActions'; actual __type 'InvalidOperationException', Message 'A resource with the specified name already exists'.
stateful-engine-options-mistakes#task#4 (and stateless-default-action-spelling#task#7): expected 200 with UpdateToken + FirewallPolicyResponse (FirewallPolicyStatus ACTIVE); actual 400, both missing.

## Steps to Reproduce
Replay fixtures network-firewall-nfwr-traj-stateful-engine-options-mistakes (task#1, task#3, task#4) and network-firewall-nfwr-traj-stateless-default-action-spelling. Equivalently: CreateFirewallPolicy with StatefulDefaultActions while RuleOrder is DEFAULT_ACTION_ORDER; then with an unsupported action value; then with a valid body under the same FirewallPolicyName.
