Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.security-group-protocol-keywords-are-case-insensitive-aliases ec2.security-group-all-protocols-egress-in-another-port-form-rejected-as-duplicate ec2.security-group-all-protocols-ingress-revokes-across-port-forms ec2.security-group-all-protocols-egress-revoke-ignores-the-port-range ec2.security-group-unknown-protocol-or-icmpv6-zero-ports-rejected ec2.security-group-rule-id-revoke-across-direction-or-group-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AuthorizeSecurityGroupIngress|InvalidParameterValue
Issue-Key: ec2.security-group-all-protocols-egress-in-another-port-form-rejected-as-duplicate|run#9|AssertionError|assert echoed == [(ctx.account, True, "-1", -1, -1, _PEER)], (
Issue-Key: ec2.security-group-all-protocols-ingress-revokes-across-port-forms|run#6|AssertionError|assert _perms(ec2, group_id, "IpPermissions") == [("-1", None, None, (_PEER,))], (
Issue-Key: ec2|RevokeSecurityGroupEgress|InvalidPermission.NotFound
Issue-Key: ec2.security-group-unknown-protocol-or-icmpv6-zero-ports-rejected|run#4|AssertionError|assert (code, msg) == want, f"protocol {protocol!r} must be refused {want!r}, got {(code, msg)!r}"
Issue-Key: ec2.security-group-rule-id-revoke-across-direction-or-group-rejected|run#13|AssertionError|assert (code, msg) == want, f"revoking an egress rule id as ingress must be {want!r}, got {(code, msg)!r}"
Issue-Commit: 563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3

# ec2: security-group rule representation and refusal fidelity — 5 net-new causes (all-protocols port normalization, protocol-keyword aliases, revoke-by-port, unknown-protocol and rule-id refusal wording)

> _Note: this issue was identified against the code base at commit `563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeSecurityGroupIngress

## Description
Five independent defects in the EC2 security-group rule path. Every one is grounded in a failing behavioral check listed under `### ec2`. None is marked ALREADY FILED or OWNED in the replay report. The open issue 20261103T021956Z-ec2-security-group-and-network-interface-refusals-wrong.md covers other security-group refusals and does NOT carry any of the replay keys below; 20261028T082553Z-ec2-the-rules-per-security-group-quota-is.md (fixed) concerned the per-group rule quota only.

Cause 1 of 5: the `all` protocol keyword is refused instead of being accepted as a case-insensitive alias of `-1`.
AWS: Smithy `IpPermission.IpProtocol` is a free `String` documented as "The IP protocol name (tcp, udp, icmp, icmpv6) or number ... Use -1 to specify all protocols." AWS additionally accepts the keyword `all` (any case) on Authorize/RevokeSecurityGroupIngress/Egress and stores it as `-1`; protocol names are matched case-insensitively, so `TCP`/`Tcp` are the same protocol as `tcp`. The second grant of an already-present rule is refused `InvalidPermission.Duplicate` with the message `the specified rule "peer: <cidr>, <PROTOCOL-UPPERCASE>, ALLOW" already exists` for an all-protocols rule and `the specified rule "peer: <cidr>, <PROTOCOL-UPPERCASE>, from port: <from>, to port: <to>, ALLOW" already exists` for a ported protocol (fixed text as quoted; the cidr, protocol token and port numbers come from the request).
Emulator: AuthorizeSecurityGroupIngress with `IpProtocol="all"` answers HTTP 400 `InvalidParameterValue` with the emulator-authored message `Invalid IP protocol 'all' for ipPermissions.1: use a protocol name (tcp, udp, icmp, icmpv6), a protocol number between 0 and 255, or -1 for all protocols` — so the rule is never created and the alias/duplicate/case-insensitivity contract behind it is unreachable.
Why: clients and tools that write `all` (the form the CLI and most templates emit) cannot author an allow-all rule at all, and anything relying on the uppercase-protocol duplicate message gets an unrelated code.
Fix: the IP-protocol parsing/validation shared by the AuthorizeSecurityGroup*/RevokeSecurityGroup* handlers, plus the duplicate-rule refusal message in the same validation.
Traces: ec2.security-group-protocol-keywords-are-case-insensitive-aliases.

Cause 2 of 5: an all-protocols (`-1`) rule is stored and echoed with FromPort/ToPort 0/0 instead of being normalized port-less (-1/-1 on the rule echo, member omitted on describe).
AWS: Smithy `IpPermission.IpProtocol` documents "specifying -1 or a protocol number other than tcp, udp, icmp, or icmpv6 allows traffic on all ports, regardless of any port range you specify." AWS therefore drops the submitted port range for an all-protocols rule: AuthorizeSecurityGroupIngress/Egress echoes the created `SecurityGroupRule` with `FromPort=-1` and `ToPort=-1`, DescribeSecurityGroups omits `FromPort`/`ToPort` from that `IpPermission`, and DescribeSecurityGroupRules reports -1/-1.
Emulator: the submitted range is kept verbatim. The authorize echo is `('-1', 0, 0, '<cidr>')` where AWS returns `('-1', -1, -1, '<cidr>')`, and DescribeSecurityGroups reports `('-1', 0, 0, (<cidr>,))` where AWS reports the permission with no port members at all.
Why: a client cannot tell an allow-all rule from a "ports 0-0" rule, duplicate detection against the other port form of the same rule stops working, and any diff of the group against its template reports a permanent, unresolvable drift.
Fix: port normalization for all-protocols rules in the security-group rule state/serializer (authorize echo, DescribeSecurityGroups and DescribeSecurityGroupRules must agree).
Traces: ec2.security-group-all-protocols-egress-in-another-port-form-rejected-as-duplicate, ec2.security-group-all-protocols-ingress-revokes-across-port-forms.

Cause 3 of 5: revoking an all-protocols rule that names a port range is refused InvalidPermission.NotFound instead of matching the rule.
AWS: because the port range is not part of an all-protocols rule (same IpProtocol doc as cause 2), RevokeSecurityGroupEgress/Ingress naming `IpProtocol=-1` with any FromPort/ToPort matches the stored port-less rule and removes it, answering 200 with `Return=true`.
Emulator: RevokeSecurityGroupEgress answers HTTP 400 `InvalidPermission.NotFound` with `The specified rule does not exist in this security group.` — the rule survives.
Why: a client that revokes with the same permission structure it authorized (or that carries a port range from a template) can never remove an allow-all rule, so teardown and least-privilege tightening both fail.
Fix: rule matching in the RevokeSecurityGroupEgress/RevokeSecurityGroupIngress handlers — ignore the port range when the protocol is all-protocols.
Traces: ec2.security-group-all-protocols-egress-revoke-ignores-the-port-range.

Cause 4 of 5: the unknown-IP-protocol refusal message is emulator-authored prose.
AWS: a protocol token that is neither a recognised name, a 0-255 number nor -1 is refused `InvalidParameterValue` with the message `Invalid value '<protocol>' for IP protocol. Unknown protocol.` — fixed text exactly as quoted, with only `<protocol>` taken from the request.
Emulator: the same request is refused `InvalidParameterValue` (code correct) with `Invalid IP protocol '<protocol>' for ipPermissions.1: use a protocol name (tcp, udp, icmp, icmpv6), a protocol number between 0 and 255, or -1 for all protocols`, which is not AWS's wording and leaks a request-index token AWS does not use here.
Why: clients and test suites that assert on, log or parse the refusal text see a message that exists nowhere in AWS.
Fix: the unknown-protocol refusal message in the Authorize/Revoke security-group rule validation.
Traces: ec2.security-group-unknown-protocol-or-icmpv6-zero-ports-rejected.

Cause 5 of 5: revoking a rule id in the wrong direction answers a not-found code instead of the direction-mismatch refusal.
AWS: RevokeSecurityGroupIngress given a `SecurityGroupRuleId` that exists but is an egress rule is refused `InvalidParameterValue` with `The security group rule ID '<sgr-id>' is an egress rule and may not be specified in a revoke-security-group-ingress request.` (fixed text as quoted; only the rule id comes from the request). The symmetric ingress-id-in-revoke-egress refusal is worded the same way with the directions swapped. `InvalidSecurityGroupRuleId.NotFound` is reserved for an id that exists in no group.
Emulator: the call is refused `InvalidSecurityGroupRuleId.NotFound` with `The security group rule ID '<sgr-id>' does not exist` — the emulator looks the id up only within the requested direction, so an existing rule is reported as nonexistent.
Why: a caller cannot distinguish "you used the wrong API for this rule" from "this rule is gone", so a retry loop keyed on NotFound silently treats a live egress rule as already revoked.
Fix: rule-id resolution in the RevokeSecurityGroupIngress/RevokeSecurityGroupEgress handlers — resolve the id account-wide, then refuse on the direction/group mismatch.
Traces: ec2.security-group-rule-id-revoke-across-direction-or-group-rejected.

## Evidence
Cause 1 — ec2.security-group-protocol-keywords-are-case-insensitive-aliases [error]
  expected: authorize_security_group_ingress(IpProtocol="all", FromPort=0, ToPort=0) succeeds and echoes ('-1', -1, -1, '10.9.0.0/16')
  actual:   botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the AuthorizeSecurityGroupIngress operation: Invalid IP protocol 'all' for ipPermissions.1: use a protocol name (tcp, udp, icmp, icmpv6), a protocol number between 0 and 255, or -1 for all protocols

Cause 2 — ec2.security-group-all-protocols-egress-in-another-port-form-rejected-as-duplicate [mismatch]
  AssertionError: the authorize must echo the rule with ports normalized to -1, got [('000000000000', True, '-1', 0, 0, '10.0.0.0/8')]
       ec2.security-group-all-protocols-ingress-revokes-across-port-forms [mismatch]
  AssertionError: [0/0 then port-less] the ingress rule must be stored port-less, got [('-1', 0, 0, ('10.0.0.0/8',))]

Cause 3 — ec2.security-group-all-protocols-egress-revoke-ignores-the-port-range [error]
  botocore.exceptions.ClientError: An error occurred (InvalidPermission.NotFound) when calling the RevokeSecurityGroupEgress operation: The specified rule does not exist in this security group.

Cause 4 — ec2.security-group-unknown-protocol-or-icmpv6-zero-ports-rejected [mismatch]
  AssertionError: protocol 'esp' must be refused ('InvalidParameterValue', "Invalid value 'esp' for IP protocol. Unknown protocol."), got ('InvalidParameterValue', "Invalid IP protocol 'esp' for ipPermissions.1: use a protocol name (tcp, udp, icmp, icmpv6), a protocol number between 0 and 255, or -1 for all protocols")

Cause 5 — ec2.security-group-rule-id-revoke-across-direction-or-group-rejected [mismatch]
  AssertionError: revoking an egress rule id as ingress must be ('InvalidParameterValue', "The security group rule ID 'sgr-...' is an egress rule and may not be specified in a revoke-security-group-ingress request."), got ('InvalidSecurityGroupRuleId.NotFound', "The security group rule ID 'sgr-...' does not exist")

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: create a VPC and a security group, then AuthorizeSecurityGroupIngress with IpPermissions=[{IpProtocol:"all", FromPort:0, ToPort:0, IpRanges:[{CidrIp:"10.9.0.0/16"}]}]. Expect 200 with the rule echoed as IpProtocol "-1", FromPort -1, ToPort -1; observe InvalidParameterValue.
3. Cause 2: AuthorizeSecurityGroupEgress/Ingress with IpProtocol "-1" and FromPort/ToPort 0/0, then read the echo, DescribeSecurityGroups and DescribeSecurityGroupRules. Expect -1/-1 on the echo and the rules read, and no FromPort/ToPort on the DescribeSecurityGroups permission; observe 0/0 everywhere.
4. Cause 3: after authorizing that all-protocols egress rule, RevokeSecurityGroupEgress naming IpProtocol "-1" with a port range. Expect 200 and the rule removed; observe InvalidPermission.NotFound.
5. Cause 4: AuthorizeSecurityGroupIngress with IpProtocol "esp". Expect InvalidParameterValue "Invalid value 'esp' for IP protocol. Unknown protocol."; observe the emulator's own prose.
6. Cause 5: authorize an egress rule, take its SecurityGroupRuleId, and call RevokeSecurityGroupIngress with that id. Expect InvalidParameterValue naming it as an egress rule; observe InvalidSecurityGroupRuleId.NotFound.
