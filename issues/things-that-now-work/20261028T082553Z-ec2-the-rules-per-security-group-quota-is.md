Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.security-group-rules-quota-is-enforced-per-direction-at-the-accounts-own-limit-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AuthorizeSecurityGroupIngress|RulesPerSecurityGroupLimitExceeded
Issue-Commit: 52a79640a1ceceb858c6fc834fcbcb6a886924f9

# ec2: the rules-per-security-group quota is one pooled per-direction counter, so AuthorizeSecurityGroupIngress refuses RulesPerSecurityGroupLimitExceeded for a rule AWS accepts (1 cause)

> _Note: this issue was identified against the code base at commit `52a79640a1ceceb858c6fc834fcbcb6a886924f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeSecurityGroupIngress

## Description
Cause 1 of 1: the inbound/outbound rules-per-security-group budget is counted as ONE bucket per direction, with no separation by IP address family, so a group whose IPv4 inbound rules have reached the account's quota can no longer take ANY further inbound rule — including an IPv6 one — and AuthorizeSecurityGroupIngress answers RulesPerSecurityGroupLimitExceeded where AWS answers 200.

AWS: the Amazon VPC quota "Inbound or outbound rules per security group" (service code `vpc`, adjustable, default 60) is published per DIRECTION and, per the Amazon VPC quotas page, is applied separately to IPv4 rules and to IPv6 rules; the inbound and the outbound buckets are also independent, so the default egress allow-all rule a new group carries never consumes inbound capacity. Smithy: EC2's model declares NO error shapes for AuthorizeSecurityGroupIngress (`smithy_query(query="operation", operation="AuthorizeSecurityGroupIngress", model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")` — errors: none), so the code and message are recorded behavior only: HTTP 400 with code `RulesPerSecurityGroupLimitExceeded` and the message `The maximum number of rules per security group has been reached.` That message is ALL fixed text — it interpolates no group id, no limit and no value from the request — and the check pins it literally; AWS only emits it once the bucket actually named in the request (that group, that direction, that address family) is full.

Emulator: the quota gate is `ComAmazonawsEc2Amazonec2State::has_security_group_rule_capacity` — it takes only `(group_id, is_egress, additional)` and weighs `security_group_rule_count(group_id, is_egress) + additional` against a single `MAX_SECURITY_GROUP_RULES_PER_DIRECTION`, counting every rule stored on that side of the group whatever address family it grants. `AuthorizeSecurityGroupIngress` asks that one question and, when it answers false, returns `state::rules_per_security_group_limit_exceeded()`. On replay the check sees an uncaught `botocore.exceptions.ClientError: An error occurred (RulesPerSecurityGroupLimitExceeded) when calling the AuthorizeSecurityGroupIngress operation: The maximum number of rules per security group has been reached.` on a call it expects AWS to ACCEPT — the refusal arrives on a request that is outside the bucket the check had filled. Hypothesis (neither the model nor the recording states which of the two it is, and both come from the same pooled counter): either the IPv6 inbound rule added while the IPv4 inbound bucket is full is refused because the two families share one counter, or the fill to the limit this emulator's own service-quotas surface reports for `vpc` / "rules per security group" is refused early because `MAX_SECURITY_GROUP_RULES_PER_DIRECTION` is not that same value.

Why: a client that provisions a dual-stack security group — IPv4 CIDRs to the quota plus any IPv6 rule — is told its group is full when on AWS it is not, and the refusal is terminal: there is no request shape that recovers it other than revoking an IPv4 rule the client still needs. The same pooled count also makes the quota unobservable: a caller cannot predict from Service Quotas how many rules it may add, and a quota-aware provisioner that reads the account's adjustable limit and fills to it gets a hard failure mid-run.

Fix: the EC2 security-group quota gate — `has_security_group_rule_capacity` must bucket by (group, direction, address family) rather than by (group, direction) alone, and its ceiling must be the value this emulator's service-quotas surface reports for `vpc` / "Inbound or outbound rules per security group" so the two surfaces cannot disagree. Both `AuthorizeSecurityGroupIngress` and `AuthorizeSecurityGroupEgress`, and `insert_security_group_rules`, ask the same question and inherit the change.

Traces: ec2.security-group-rules-quota-is-enforced-per-direction-at-the-accounts-own-limit-rejected.

## Evidence
Expected (AWS): with the account's `vpc` / "rules per security group" quota reported as N, exactly N IPv4 inbound rules are accepted on a fresh group; the N+1st IPv4 inbound rule is refused HTTP 400 `RulesPerSecurityGroupLimitExceeded` / `The maximum number of rules per security group has been reached.` with nothing stored; an IPv6 inbound rule and an IPv4 OUTBOUND rule are still accepted (200) while that IPv4 inbound bucket is full; after revoking one IPv4 inbound rule the previously refused CIDR is accepted.

Actual (emulator): an AuthorizeSecurityGroupIngress call the check expects to succeed is refused —
  botocore.exceptions.ClientError: An error occurred (RulesPerSecurityGroupLimitExceeded) when calling the AuthorizeSecurityGroupIngress operation: The maximum number of rules per security group has been reached.
raised uncaught out of the check, so the quota legs after it never run.

Code: services/service-com-amazonaws-ec2-amazonec2/src/state.rs lines 4846-4864 (`has_security_group_rule_capacity`: `security_group_rule_count(group_id, is_egress) + additional <= MAX_SECURITY_GROUP_RULES_PER_DIRECTION`, no address-family term) and lines 4836-4844 (`security_group_rule_count` filters on group id and `is_egress` only); services/service-com-amazonaws-ec2-amazonec2/src/ops/authorize_security_group_ingress.rs lines 309-316 (the gate and `state::rules_per_security_group_limit_exceeded()`).

Status/code/message quoted exactly: 400, `RulesPerSecurityGroupLimitExceeded`, `The maximum number of rules per security group has been reached.` — entirely fixed text, no request- or state-derived parts.

## Steps to Reproduce
1. Read the account's quota: service-quotas ListServiceQuotas(ServiceCode='vpc') and take the quota whose name contains 'rules per security group'; call its Value N.
2. ec2 CreateVpc(CidrBlock='10.78.0.0/16'), wait vpc_available, ec2 CreateSecurityGroup in that VPC.
3. AuthorizeSecurityGroupIngress with N distinct IPv4 /32 CIDRs on tcp/4433 (N-1 in one call, then the last one alone). Expect all accepted and DescribeSecurityGroupRules to count N inbound rules.
4. AuthorizeSecurityGroupIngress with one more distinct IPv4 CIDR. Expect 400 RulesPerSecurityGroupLimitExceeded / 'The maximum number of rules per security group has been reached.' and no new rule.
5. AuthorizeSecurityGroupIngress with Ipv6Ranges=[{'CidrIpv6':'2001:db8:5e2:1::/64'}] on tcp/4433 — AWS accepts this (separate IPv4/IPv6 buckets); the emulator refuses it with the same RulesPerSecurityGroupLimitExceeded.
6. AuthorizeSecurityGroupEgress with one IPv4 CIDR on tcp/4433 — must be accepted (separate direction).
7. RevokeSecurityGroupIngress of one IPv4 rule, then re-send the CIDR refused in step 4 — must now be accepted.
Replay: ec2.security-group-rules-quota-is-enforced-per-direction-at-the-accounts-own-limit-rejected
