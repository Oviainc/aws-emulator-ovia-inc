Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.create-firewall-rule-invalid-group-not-found route53resolver.disassociate-resolver-rule-not-associated-invalid-request route53resolver.associate-resolver-rule-invalid-vpc-rejected route53resolver.e2e-outpost-resolver-full-lifecycle route53resolver.outpost-resolver-lifecycle-creating-to-operational route53resolver.outpost-resolver-lifecycle-creating-to-active-to-deleted
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.create-firewall-rule-invalid-group-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: route53resolver.disassociate-resolver-rule-not-associated-invalid-request|run#9|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: route53resolver.associate-resolver-rule-invalid-vpc-rejected|_client_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: route53resolver.e2e-outpost-resolver-full-lifecycle|_client_error#5|AssertionError|raise AssertionError("the call succeeded; expected AWS to refuse it with a ClientError")
Issue-Key: route53resolver.outpost-resolver-lifecycle-creating-to-operational|_assert_creation_refused_without_a_racked_outpost#4|AssertionError|assert code == "ValidationException", (
Issue-Key: route53resolver.outpost-resolver-lifecycle-creating-to-active-to-deleted|run#9|AssertionError|raise AssertionError(
Issue-Commit: 5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b

# Route 53 Resolver rule/firewall write path: DisassociateResolverRule and CreateFirewallRule are 501 stubs, and AssociateResolverRule / CreateOutpostResolver accept ids that do not exist

> _Note: this issue was identified against the code base at commit `5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResolverRule

## Description
FOUR distinct root causes in the Route 53 Resolver control plane, all in the route53resolver service. A fixer must close ALL FOUR before submitting; each cause lists its own failing check ids. (Related but NOT covering these: open issue 20261022T084101Z-route53resolver-route-53-resolver-endpoint-ops-skip-resource-validation.md concerns the *endpoint* operations only; the per-operation stub issues already open for this service name other operations — CreateFirewallRuleGroup, CreateResolverQueryLogConfig, GetFirewallConfig, etc. — and neither CreateFirewallRule nor DisassociateResolverRule is among them.)

Cause 1 of 4: CreateFirewallRule is an unimplemented scaffold stub.
AWS: Smithy: CreateFirewallRule takes CreatorRequestId, FirewallRuleGroupId (ResourceId, @length 1..64, required), FirewallDomainListId, Priority, Action (ALERT|ALLOW|BLOCK), Name, and returns FirewallRule; its modeled errors include ResourceNotFoundException (HTTP 400, "The specified resource doesn't exist."), which is what a nonexistent FirewallRuleGroupId must produce. The recording shows a 400 ResourceNotFoundException for a bogus group id (message example: '[RSLVR-02025] Can\u2019t find the resource with ID "<id>".'), and a 200 with the created rule for a real group.
Emulator: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_rule.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() returns `Scaffolded::not_implemented("CreateFirewallRule")`, so EVERY call answers HTTP 501 with error code `NotImplemented` regardless of input.
Why: no DNS Firewall rule can ever be created, so nothing downstream of a rule (priority ordering, block/alert actions, block responses) can be exercised, and a client that expects ResourceNotFoundException for a bad group id instead sees an unmodeled 501 code its error handling does not recognise.
Fix: implement the handler in services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_rule.rs (create and store the rule in the service state keyed by rule group, reject an unknown FirewallRuleGroupId with ResourceNotFoundException) and delete the SCAFFOLDED marker.
Traces: route53resolver.create-firewall-rule-invalid-group-not-found.

Cause 2 of 4: DisassociateResolverRule is an unimplemented scaffold stub.
AWS: Smithy: DisassociateResolverRule(VPCId required, ResolverRuleId required) -> ResolverRuleAssociation; modeled errors include ResourceNotFoundException (HTTP 400). The recording shows a VPC/rule pair that was never associated being refused with HTTP 400 ResourceNotFoundException (message example: '[RSLVR-00800] Resolver Rule Association between Resolver Rule "<rule>" and VPC "<vpc>" does not exist.'), and a real association being removed successfully.
Emulator: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/disassociate_resolver_rule.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker; every call answers HTTP 501 `NotImplemented`.
Why: a rule association can be created but never removed, so VPC/rule association state is write-only and cleanup/teardown paths fail; the refusal for a non-existent association is also unobservable.
Fix: implement the handler in services/service-com-amazonaws-route53resolver-route53resolver/src/ops/disassociate_resolver_rule.rs against the association state written by AssociateResolverRule, returning ResourceNotFoundException when no association exists for the pair, and delete the SCAFFOLDED marker.
Traces: route53resolver.disassociate-resolver-rule-not-associated-invalid-request.

Cause 3 of 4: AssociateResolverRule does not validate that VPCId names an existing VPC — it accepts any well-formed id.
AWS: Smithy: AssociateResolverRule.VPCId is a required ResourceId (@length 1..64) and the operation models InvalidParameterException, InvalidRequestException, ResourceExistsException, ResourceNotFoundException and ResourceUnavailableException (all HTTP 400). Recorded behavior: a syntactically valid but nonexistent VPC id is refused with HTTP 400 and error code InvalidParameterException — AWS classifies an unresolvable VPC id as a malformed parameter rather than ResourceNotFoundException (reserved for an unknown rule/association id) or ResourceExistsException (duplicate association).
Emulator: the call returns HTTP 200 with a fabricated association, e.g. {'ResolverRuleAssociation': {'Id': 'rslvr-rrassoc-…', 'ResolverRuleId': 'rslvr-rr-…', 'VPCId': 'vpc-0123456789abcdef0', 'Status': 'CREATING'}} — no VPC lookup happens at all.
Why: the emulator is overly permissive on a negative path. A client's error handling for a bad/stale VPC id is never exercised, and the emulator accumulates associations pointing at VPCs that do not exist, so later reads (ListResolverRuleAssociations, DisassociateResolverRule) describe phantom topology that AWS would never have created.
Fix: in the AssociateResolverRule handler, resolve VPCId against the EC2 VPC state before creating the association and return InvalidParameterException (HTTP 400) when it does not resolve.
Traces: route53resolver.associate-resolver-rule-invalid-vpc-rejected.

Cause 4 of 4: CreateOutpostResolver does not validate that OutpostArn names an Outpost registered in the account — it creates a resolver for any ARN matching the pattern.
AWS: Smithy: CreateOutpostResolver.OutpostArn is required with pattern ^arn:aws([a-z-]+)?:outposts:[a-z\\d-]+:\\d{12}:outpost/op-[a-f0-9]{17}$, and the operation models ValidationException and ResourceNotFoundException (HTTP 400). Recorded behavior: an ARN that matches the pattern but names no racked Outpost in the account is refused deterministically, before any provisioning, with HTTP 400 ValidationException identifying the missing Outpost (message example: 'Cannot find the Outpost with Arn "<arn>"'), and no OutpostResolver is created.
Emulator: the call SUCCEEDS (HTTP 200) and an OutpostResolver is created for the nonexistent Outpost — the checks report 'the call succeeded; expected AWS to refuse it with a ClientError' and 'expected ValidationException for a non-racked OutpostArn, got None'. The pattern constraint is enforced but existence is not.
Why: the only deterministic contract an unracked account can observe for Outpost resolvers is inverted: the emulator reports success for hardware that does not exist, so a client cannot distinguish "your Outpost is not racked" from "your resolver is being provisioned", and the emulator leaks resolvers the caller was never handed an id to clean up.
Fix: in the CreateOutpostResolver handler, look OutpostArn up in the Outposts service state (the same set ListOutposts answers from) and return ValidationException (HTTP 400) naming the ARN when it is absent, creating nothing.
Traces: route53resolver.e2e-outpost-resolver-full-lifecycle, route53resolver.outpost-resolver-lifecycle-creating-to-operational, route53resolver.outpost-resolver-lifecycle-creating-to-active-to-deleted.

## Evidence
Cause 1 — route53resolver.create-firewall-rule-invalid-group-not-found: CreateFirewallRule(FirewallRuleGroupId=<bogus>, FirewallDomainListId=<real>, Priority=1, Action=ALLOW) -> AssertionError 'expected ResourceNotFoundException, got NotImplemented'. Expected HTTP 400 ResourceNotFoundException; actual HTTP 501 code NotImplemented. src/ops/create_firewall_rule.rs:4 carries the SCAFFOLDED marker and line 261 returns Scaffolded::not_implemented("CreateFirewallRule").

Cause 2 — route53resolver.disassociate-resolver-rule-not-associated-invalid-request: DisassociateResolverRule(VPCId=<real vpc>, ResolverRuleId=<real rule, never associated>) -> AssertionError 'expected ResourceNotFoundException, got NotImplemented'. Expected HTTP 400 ResourceNotFoundException; actual HTTP 501 NotImplemented. src/ops/disassociate_resolver_rule.rs:3-6 carries the SCAFFOLDED marker.

Cause 3 — route53resolver.associate-resolver-rule-invalid-vpc-rejected: AssociateResolverRule(ResolverRuleId=<real rslvr-rr-…>, VPCId='vpc-0123456789abcdef0') -> AssertionError 'expected a ClientError, but the call succeeded and returned {ResolverRuleAssociation: {Id: rslvr-rrassoc-63033b0ca389c35a, ResolverRuleId: rslvr-rr-6e73e372e2338aca, VPCId: vpc-0123456789abcdef0, Status: CREATING}, ResponseMetadata: {HTTPStatusCode: 200, …}}'. Expected HTTP 400 InvalidParameterException.

Cause 4 — route53resolver.e2e-outpost-resolver-full-lifecycle: after ListOutposts returns no Outposts, CreateOutpostResolver(OutpostArn=arn:aws:outposts:<region>:<account>:outpost/op-<17 hex>, PreferredInstanceType=…, InstanceCount=…) -> AssertionError 'the call succeeded; expected AWS to refuse it with a ClientError'. route53resolver.outpost-resolver-lifecycle-creating-to-operational -> AssertionError 'expected ValidationException for a non-racked OutpostArn, got None (None)'. route53resolver.outpost-resolver-lifecycle-creating-to-active-to-deleted -> AssertionError 'CreateOutpostResolver against a nonexistent Outpost ARN unexpectedly succeeded -- a real OutpostResolver is now on file that this check has no resolver id to clean up'. Expected HTTP 400 ValidationException in all three.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Cause 1: create a firewall domain list (CreateFirewallDomainList succeeds), then call CreateFirewallRule with a FirewallRuleGroupId that does not exist -> observe HTTP 501 NotImplemented instead of HTTP 400 ResourceNotFoundException. Replay route53resolver.create-firewall-rule-invalid-group-not-found.
3. Cause 2: create a VPC, an OUTBOUND resolver endpoint and a FORWARD resolver rule, then call DisassociateResolverRule(VPCId, ResolverRuleId) for that never-associated pair -> HTTP 501 NotImplemented instead of HTTP 400 ResourceNotFoundException. Replay route53resolver.disassociate-resolver-rule-not-associated-invalid-request.
4. Cause 3: with that same rule, call AssociateResolverRule(ResolverRuleId=<rule>, VPCId='vpc-0123456789abcdef0') (a well-formed id for no existing VPC) -> HTTP 200 with a new association instead of HTTP 400 InvalidParameterException. Replay route53resolver.associate-resolver-rule-invalid-vpc-rejected.
5. Cause 4: confirm ListOutposts returns an empty Outposts list, then call CreateOutpostResolver with an OutpostArn matching the modeled pattern but naming no registered Outpost -> HTTP 200 with an OutpostResolver instead of HTTP 400 ValidationException. Replay route53resolver.e2e-outpost-resolver-full-lifecycle, route53resolver.outpost-resolver-lifecycle-creating-to-operational and route53resolver.outpost-resolver-lifecycle-creating-to-active-to-deleted.
