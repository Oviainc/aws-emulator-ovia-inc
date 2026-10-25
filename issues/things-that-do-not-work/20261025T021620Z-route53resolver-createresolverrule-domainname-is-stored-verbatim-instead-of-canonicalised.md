Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.resolver-rule-domain-name-trailing-dot-normalized route53resolver.create-forward-resolver-rule-requires-endpoint-and-targets route53resolver.create-resolver-rule-system-forbids-target-ips-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.resolver-rule-domain-name-trailing-dot-normalized|run#8|AssertionError|assert created_name.endswith("."), (
Issue-Key: route53resolver.create-forward-resolver-rule-requires-endpoint-and-targets|run#11|AssertionError|assert err["Message"] == FORWARD_REQUIRES_MESSAGE, err
Issue-Key: route53resolver.create-resolver-rule-system-forbids-target-ips-rejected|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 8e40a1008c33b06d6b244a457efc673d78e6bf69

# CreateResolverRule: DomainName is stored verbatim instead of canonicalised to a trailing-dot FQDN, and the FORWARD/SYSTEM RuleType refusals omit Resolver's RSLVR-coded message

> _Note: this issue was identified against the code base at commit `8e40a1008c33b06d6b244a457efc673d78e6bf69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolverRule

## Description
THREE distinct root causes in the Route 53 Resolver rule write path, all in the route53resolver service and all in the same handler (services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_resolver_rule.rs). A fixer must close ALL THREE before submitting; each cause lists its own failing check ids. (Related but NOT covering these: open issue 20261025T012620Z-route53resolver-route-53-resolver-rule-firewall-write-path-disassociateresolverrule.md covers CreateFirewallRule/DisassociateResolverRule stubs and AssociateResolverRule/CreateOutpostResolver existence validation, not CreateResolverRule's own projection or its RuleType refusal messages; 20261024T062131Z-route53resolver-createresolverrule-stub.md is a stale stub issue — the handler IS implemented and answering, which is why these are fidelity defects rather than 501s.)

Cause 1 of 3: CreateResolverRule does not canonicalise DomainName to a fully-qualified name, so the caller's spelling is echoed and stored.
AWS: Smithy: CreateResolverRule.DomainName is a bare string member (target DomainName, @length 1..256) with no normalisation trait, so the canonicalisation is pure server-side semantics — recorded behavior only: Route 53 Resolver stores a rule's domain as a fully-qualified name with a TRAILING DOT and projects that same value from CreateResolverRule, GetResolverRule and ListResolverRules, so `d.example.com` and `d.example.com.` submitted as two rules yield the identical stored DomainName.
Emulator: the handler copies the request value straight into the record (create_resolver_rule.rs: `domain_name: input.domain_name.clone()`) and state.rs ResolverRuleRecord::to_rule clones it back out unchanged, so a rule created with `d-<id>.sim2real-behav.example.com` is projected as `d-<id>.sim2real-behav.example.com` with no trailing dot, and the dotted/dotless spellings of one domain stay two different values for the lifetime of the record.
Why: every consumer that matches rules by domain breaks — rule dedup, VPC-association conflict detection and infra-as-code drift detection all compare the projected DomainName against the canonical FQDN they expect, so a reconciler churns forever on a rule it already created, and two rules for the same domain look distinct.
Fix: canonicalise DomainName once in the CreateResolverRule handler (and anywhere else a rule domain is accepted, e.g. UpdateResolverRule) before it is written to crate::state, so Create/Get/List all project the one canonical trailing-dot form.
Traces: route53resolver.resolver-rule-domain-name-trailing-dot-normalized.

Cause 2 of 3: the refusal of a FORWARD rule with no ResolverEndpointId/TargetIps carries neither Resolver's RSLVR-00706 service error code nor its single combined diagnosis.
AWS: Smithy: TargetIps (TargetList, @length min 1) and ResolverEndpointId (ResourceId, @length 1..64) are plain OPTIONAL members of CreateResolverRuleRequest, so the requirement is server-side; the operation models InvalidRequestException (HTTP 400, "The request is invalid."). Recorded behavior: a FORWARD rule missing the endpoint, AND a FORWARD rule that supplies TargetIps but no ResolverEndpointId, are BOTH refused with HTTP 400 / InvalidRequestException whose message carries Resolver's own service error code, example text: '[RSLVR-00706] Resolver endpoint ID and target IP are required for FORWARD type resolver rule.' The requirement here is the RSLVR-nnnnn identifier inside the message — this emulator's own state.rs documents that convention ("Route 53 Resolver carries its own `RSLVR-nnnnn` code inside every exception message ... the code is what operators and tooling key on") and already has the rslvr_message()/RSLVR_CODE_ENDPOINT_NOT_FOUND helper for it; the quoted sentence is the recorded example.
Emulator: HTTP status and error code are correct (400 InvalidRequestException) but the message is a locally invented sentence with no RSLVR code, and it is split into two different diagnoses: create_resolver_rule.rs:231 returns 'A rule with RuleType FORWARD must specify TargetIps' and create_resolver_rule.rs:332 returns 'A rule with RuleType FORWARD must specify ResolverEndpointId'.
Why: operators and tooling key on the RSLVR-nnnnn code because the HTTP status (400) and shape name are identical for every Resolver fault; a client that routes or asserts on that code sees nothing to key on.
Fix: route both FORWARD-precondition refusals in the CreateResolverRule handler through crate::state::rslvr_message with code RSLVR-00706, as the endpoint-not-found path already does.
Traces: route53resolver.create-forward-resolver-rule-requires-endpoint-and-targets.

Cause 3 of 3: the refusal of a SYSTEM rule that carries TargetIps omits Resolver's RSLVR-00707 service error code.
AWS: Smithy: as above, TargetIps/ResolverEndpointId are optional members and the RuleType discriminator is not modelled; the operation models InvalidRequestException (HTTP 400). Recorded behavior: a SYSTEM rule with TargetIps is refused with HTTP 400 / InvalidRequestException carrying the RSLVR code, example text: '[RSLVR-00707] Cannot specify resolver endpoint ID and target IP for SYSTEM type resolver rule.'
Emulator: status and code are correct (400 InvalidRequestException); the message is create_resolver_rule.rs:242 'TargetIps is available only for a rule with RuleType FORWARD, not SYSTEM' — no RSLVR identifier.
Why: same as cause 2 — the only per-fault discriminator Resolver exposes is the RSLVR code, so error-handling keyed on it cannot distinguish this fault from any other 400 InvalidRequestException.
Fix: emit this refusal through crate::state::rslvr_message with code RSLVR-00707 in the CreateResolverRule handler's RuleType validation.
Traces: route53resolver.create-resolver-rule-system-forbids-target-ips-rejected.

## Evidence
Cause 1 — route53resolver.resolver-rule-domain-name-trailing-dot-normalized: CreateResolverRule(RuleType='SYSTEM', DomainName='d-f81a0407f380.sim2real-behav.example.com') -> AssertionError "Resolver canonicalises a rule domain to a fully-qualified name with a trailing dot; CreateResolverRule projected 'd-f81a0407f380.sim2real-behav.example.com' for input 'd-f81a0407f380.sim2real-behav.example.com'" (check line 102). Expected: the projected DomainName ends with '.' and is identical for the dotted and dotless spellings across Create/Get/List. Source: services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_resolver_rule.rs:429 stores input.domain_name unchanged; src/state.rs:730 projects it unchanged.

Cause 2 — route53resolver.create-forward-resolver-rule-requires-endpoint-and-targets: after a control SYSTEM rule is created successfully, CreateResolverRule(RuleType='FORWARD', DomainName=<zone>, no ResolverEndpointId, no TargetIps) -> AssertionError {'Message': 'A rule with RuleType FORWARD must specify TargetIps', 'Code': 'InvalidRequestException'} at check line 118. Expected HTTP 400 InvalidRequestException with an RSLVR-00706-coded message (recorded example: '[RSLVR-00706] Resolver endpoint ID and target IP are required for FORWARD type resolver rule.'); status and code matched, the RSLVR code is absent. The same check's second class (TargetIps=[{'Ip':'10.1.2.6','Port':53}] with no ResolverEndpointId) expects the SAME RSLVR-00706 message but the emulator produces a different sentence from create_resolver_rule.rs:332.

Cause 3 — route53resolver.create-resolver-rule-system-forbids-target-ips-rejected: after a control SYSTEM rule without TargetIps is accepted, CreateResolverRule(RuleType='SYSTEM', DomainName=<domain>, TargetIps=[...]) -> AssertionError "expected message '[RSLVR-00707] Cannot specify resolver endpoint ID and target IP for SYSTEM type resolver rule.', got 'TargetIps is available only for a rule with RuleType FORWARD, not SYSTEM'" (check line 148). Status and error code matched.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Cause 1: CreateResolverRule(CreatorRequestId=<unique>, Name=<name>, RuleType='SYSTEM', DomainName='d.example.com') and a second rule with DomainName='d.example.com.' -> observe the two rules project two different DomainName values and neither ends in '.', in the Create response and in GetResolverRule/ListResolverRules. Replay route53resolver.resolver-rule-domain-name-trailing-dot-normalized.
3. Cause 2: CreateResolverRule(RuleType='FORWARD', DomainName=<zone>) with no ResolverEndpointId and no TargetIps -> 400 InvalidRequestException, message 'A rule with RuleType FORWARD must specify TargetIps' instead of an RSLVR-00706-coded message; repeat with TargetIps supplied but no ResolverEndpointId -> a third, different sentence. Replay route53resolver.create-forward-resolver-rule-requires-endpoint-and-targets.
4. Cause 3: CreateResolverRule(RuleType='SYSTEM', DomainName=<domain>, TargetIps=[{'Ip':'10.1.2.6','Port':53}]) -> 400 InvalidRequestException, message 'TargetIps is available only for a rule with RuleType FORWARD, not SYSTEM' instead of an RSLVR-00707-coded message. Replay route53resolver.create-resolver-rule-system-forbids-target-ips-rejected.
