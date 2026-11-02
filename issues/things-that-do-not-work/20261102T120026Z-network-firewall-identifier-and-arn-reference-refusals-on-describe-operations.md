Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-firewall-identifier-mistakes
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-firewall-identifier-mistakes network-firewall-nfwr-traj-policy-and-logging-reads-on-wrong-references network-firewall-nfwr-traj-rule-group-lookup-needs-type
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|DescribeFirewall|4xx->4xx|$.Message
Issue-Key: network-firewall|DescribeFirewall|4xx->4xx|$.__type
Issue-Key: network-firewall|DescribeFirewallPolicy|4xx->4xx|$.Message
Issue-Key: network-firewall|DescribeFirewallPolicy|4xx->4xx|$.__type
Issue-Key: network-firewall|DescribeRuleGroup|4xx->4xx|$.Message
Issue-Commit: c1ca043e5c77dd90c2a96695b3dfa06a20838f7d

# Identifier and ARN reference refusals on Describe operations use the wrong error class and wording

> _Note: this issue was identified against the code base at commit `c1ca043e5c77dd90c2a96695b3dfa06a20838f7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFirewall

## Description
Every Network Firewall Describe operation takes a name-or-ARN pair and refuses a bad reference with InvalidRequestException and a specific message. The emulator refuses with the right status but its own wording, and in two cases the wrong error shape, so clients cannot distinguish a malformed reference from a missing resource. This bundle has 5 causes.

Cause 1 of 5 - neither firewall identifier supplied (fixture network-firewall-nfwr-traj-firewall-identifier-mistakes, step task#1).
AWS: DescribeFirewall.FirewallName and .FirewallArn are both optional in the model (Smithy: neither is required) but exactly one must be supplied; omitting both yields HTTP 400 InvalidRequestException with Message 'At least one of FirewallName or FirewallArn must be set' - fixed text in full, naming the two member names from the model.
Emulator: HTTP 400 with Message 'You must specify either FirewallName or FirewallArn.' - same meaning, different fixed text.

Cause 2 of 5 - malformed resource ARN (fixture network-firewall-nfwr-traj-firewall-identifier-mistakes, step task#2; cluster also covers task#3, task#4, task#5).
AWS: FirewallArn targets ResourceArn (Smithy: length 1..256, pattern ^arn:aws). An ARN that passes that prefix check but is not a well-formed resource ARN is refused with HTTP 400, __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException' and Message 'Resource ARN has an invalid format.' - fixed text in full.
Emulator: HTTP 400 but __type 'ValidationException' - a shape this service does not model (Smithy common_errors: InternalServerError, InvalidRequestException, ThrottlingException) - and with no Message member at all.

Cause 3 of 5 - ARN whose resource type does not match the operation (fixture network-firewall-nfwr-traj-policy-and-logging-reads-on-wrong-references, step task#2; same cluster: task#3).
AWS: DescribeFirewallPolicy given a well-formed ARN of a different resource type is refused as a bad reference, not as a missing resource: HTTP 400, __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException', Message 'Resource Type and Arn(Type in Arn) do not match' - fixed text in full.
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and Message 'No object with the provided name or resource identifier was found' - it resolved the ARN as a lookup miss instead of checking the type segment.

Cause 4 of 5 - neither policy identifier supplied (fixture network-firewall-nfwr-traj-policy-and-logging-reads-on-wrong-references, step task#4).
AWS: as Cause 1, for DescribeFirewallPolicy's FirewallPolicyName / FirewallPolicyArn pair: Message 'At least one of FirewallPolicyName or FirewallPolicyArn must be set'.
Emulator: 'You must specify either FirewallPolicyName or FirewallPolicyArn.'

Cause 5 of 5 - rule-group lookup by name without Type (fixture network-firewall-nfwr-traj-rule-group-lookup-needs-type, step task#2).
AWS: DescribeRuleGroup.Type is optional in the model (Smithy: target RuleGroupType, not required) but required when the group is referenced by RuleGroupName rather than RuleGroupArn; omitting it yields HTTP 400 InvalidRequestException with Message 'Type must be set when referring to RuleGroup by name' - fixed text in full.
Emulator: Message 'Type is required for requests that do not include the RuleGroupArn.'

Why: these are the refusals a client hits while wiring up references, and the distinction matters. An unmodeled 'ValidationException' with no Message (Cause 2) cannot be mapped by a generated SDK at all, so a caller sees an opaque error instead of InvalidRequestException and err.message() is empty. Collapsing a type-mismatched ARN into ResourceNotFoundException (Cause 3) inverts the diagnosis: the caller concludes the policy was deleted and retries creation, where AWS told it the ARN names the wrong kind of resource. The reworded messages in Causes 1, 4 and 5 break assertions and user-facing text that match AWS's wording.

Fix: the shared reference-resolution/validation path used by the Describe* handlers in the network-firewall service crate - one identifier-presence check per name/ARN pair, an ARN format check and an ARN resource-type check ahead of state lookup, all raising InvalidRequestException with the fully qualified __type and AWS's exact fixed text.

## Evidence
firewall-identifier-mistakes#task#1: expected Message 'At least one of FirewallName or FirewallArn must be set'; actual 'You must specify either FirewallName or FirewallArn.'
firewall-identifier-mistakes#task#2 (4-member cluster): expected __type ...#InvalidRequestException + Message 'Resource ARN has an invalid format.'; actual __type 'ValidationException', Message missing.
policy-and-logging-reads-on-wrong-references#task#2 (and task#3): expected __type ...#InvalidRequestException + Message 'Resource Type and Arn(Type in Arn) do not match'; actual __type 'ResourceNotFoundException', Message 'No object with the provided name or resource identifier was found'.
policy-and-logging-reads-on-wrong-references#task#4: expected 'At least one of FirewallPolicyName or FirewallPolicyArn must be set'; actual 'You must specify either FirewallPolicyName or FirewallPolicyArn.'
rule-group-lookup-needs-type#task#2: expected 'Type must be set when referring to RuleGroup by name'; actual 'Type is required for requests that do not include the RuleGroupArn.' All five return HTTP 400.

## Steps to Reproduce
Replay the listed fixture steps. Equivalently: DescribeFirewall with no identifier; DescribeFirewall with a malformed FirewallArn; DescribeFirewallPolicy with a rule-group ARN; DescribeFirewallPolicy with no identifier; DescribeRuleGroup by RuleGroupName with no Type. Compare __type and Message in each 400 body.
