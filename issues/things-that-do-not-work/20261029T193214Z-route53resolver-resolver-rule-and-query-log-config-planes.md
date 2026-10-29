Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.create-resolver-rule-duplicate-creator-request-id route53resolver.unicode-resolver-rule-name-round-trip route53resolver.associate-resolver-query-log-config-nonexistent-rejected route53resolver.resolver-query-log-config-association-status-transitions route53resolver.resolver-rule-association-gone-after-disassociate route53resolver.resolver-rule-association-state-transition route53resolver.list-resolver-endpoints-invalid-next-token-rejected route53resolver.list-resolver-query-log-config-associations-invalid-filter-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.create-resolver-rule-duplicate-creator-request-id|_client_error#9|AssertionError|raise AssertionError("the call succeeded; expected AWS to refuse it with a ClientError")
Issue-Key: route53resolver.unicode-resolver-rule-name-round-trip|_error#12|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded and returned {result!r}")
Issue-Key: route53resolver.associate-resolver-query-log-config-nonexistent-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.resolver-query-log-config-association-status-transitions|run#20|AssertionError|assert (redisassociate_code, redisassociate_message) == ("ResourceNotFoundException", gone_message), (
Issue-Key: route53resolver.resolver-rule-association-gone-after-disassociate|run#16|AssertionError|assert gone == expected, f"expected {expected!r} after disassociate, got {gone!r}"
Issue-Key: route53resolver.resolver-rule-association-state-transition|run#24|AssertionError|assert gone == expected_gone, f"expected {expected_gone!r} after disassociate, got {gone!r}"
Issue-Key: route53resolver.list-resolver-endpoints-invalid-next-token-rejected|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.list-resolver-query-log-config-associations-invalid-filter-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# route53resolver resolver-rule and query-log-config planes: 7 net-new causes — CreateResolverRule accepts a replayed CreatorRequestId and a raw non-ASCII DomainName, and five RSLVR-coded refusal messages (00701/00711/01601/01602/00302/01004, GetResolverRuleAssociation) are emulator-authored

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolverRule

## Description
Seven net-new root causes outside the DNS Firewall surface of services/service-com-amazonaws-route53resolver-route53resolver. Causes 1-2 accept calls AWS refuses; causes 3-7 refuse with the right class but a sentence the emulator authored instead of Resolver's own.

=== Cause 1 of 7: CreateResolverRule accepts a replayed CreatorRequestId and mints a second rule instead of refusing ResourceExistsException RSLVR-00701 ===
Failing check: route53resolver.create-resolver-rule-duplicate-creator-request-id
AWS: Smithy models CreateResolverRule(CreatorRequestId required 1-255, Name, RuleType required, DomainName, TargetIps, ResolverEndpointId, Tags, DelegationRecord) with errors AccessDenied | InternalServiceError | InvalidParameter | InvalidRequest | LimitExceeded | ResourceExistsException | ResourceNotFound | ResourceUnavailable | Throttling (all httpError 400, defaulted). CreatorRequestId is the dedup token; the replay rule is recorded behavior only: re-using a CreatorRequestId with different content is refused HTTP 400, Error.Code `ResourceExistsException`, Error.Message `[RSLVR-00701] Resolver rule with creator request ID "<CreatorRequestId>" already exists.` Anatomy: FIXED text `[RSLVR-00701] Resolver rule with creator request ID "` … `" already exists.`, one VARIABLE part — the CreatorRequestId from the request. Resolver does NOT silently return the existing rule, and the original rule is left unmodified.
Emulator: the second CreateResolverRule SUCCEEDS (HTTP 200) and returns a brand-new ResolverRule id — the check fails at `the call succeeded; expected AWS to refuse it with a ClientError`.
Why: the token is not a dedup token at all here, so a client that retries a create after a timeout silently ends up with two rules (and two charges / two conflicting forwarding definitions) where AWS guarantees one; the conflict a caller is expected to handle never arrives.
Fix: CreatorRequestId replay detection on CreateResolverRule — the handler must look the token up in the rule index in src/state.rs and refuse with the RSLVR-00701 ResourceExistsException before creating.
Traces: route53resolver.create-resolver-rule-duplicate-creator-request-id

=== Cause 2 of 7: CreateResolverRule stores a DomainName containing raw non-ASCII bytes instead of refusing InvalidParameterException RSLVR-00711 ===
Failing check: route53resolver.unicode-resolver-rule-name-round-trip
AWS: DomainName is modelled only as a 1-256 length string, so the IDNA requirement is recorded behavior only: an internationalized label must be spelled in IDNA/punycode (ACE). The ACE spelling (`xn--mnchen-3ya.<zone>.example.`) is accepted and round-trips byte-for-byte through GetResolverRule/ListResolverRules, while the raw-unicode spelling of the identical label is refused outright with HTTP 400, Error.Code `InvalidParameterException`, Error.Message `[RSLVR-00711] Invalid domain or delegation record` (entirely FIXED text — nothing from the request is echoed), and nothing is created.
Emulator: CreateResolverRule(RuleType=SYSTEM, DomainName='münchen.<zone>.example.') SUCCEEDS and stores the raw-unicode name — the response projects `'DomainName': 'münchen.sim2real-r53r-<n>.example.'` with Status COMPLETE, so the rejected-and-absent invariant is broken too (the rule shows up in ListResolverRules).
Why: the emulator accepts a rule a real account cannot hold, so a client's happy path validates here and then fails in production; worse, an emulator-built rule carries a DomainName no DNS resolver can match, silently making the forwarding rule inert rather than refusing it up front.
Fix: DomainName validation on CreateResolverRule (and the matching UpdateResolverRule path) — reject non-ASCII bytes with the RSLVR-00711 InvalidParameterException from the shared RSLVR formatter, before any state is written.
Traces: route53resolver.unicode-resolver-rule-name-round-trip

=== Cause 3 of 7: AssociateResolverQueryLogConfig's unknown-config refusal does not use AWS's RSLVR-01601 sentence ===
Failing check: route53resolver.associate-resolver-query-log-config-nonexistent-rejected
AWS: Smithy models AssociateResolverQueryLogConfig(ResolverQueryLogConfigId required, ResourceId required) with errors AccessDenied | InternalServiceError | InvalidParameter | InvalidRequest | LimitExceeded | ResourceExists | ResourceNotFoundException | Throttling (all 400); ResourceNotFoundException carries only {Message}, so the wording is recorded behavior only: for a well-formed-but-nonexistent config id the response is HTTP 400, `ResourceNotFoundException`, Message `[RSLVR-01601] The specified query logging configuration doesn't exist.` — entirely FIXED text; AWS does NOT echo the id.
Emulator: HTTP 400 / ResourceNotFoundException (correct class) with the self-authored, id-echoing sentence `Resolver query log config with ID "<id>" does not exist.` (recorded id `rqlc-0123456789abcdef0`).
Why: no RSLVR-01601 for a client to key on, and the emulator leaks the requested id into a message AWS keeps fixed, so exact-message handling diverges in both directions.
Fix: the not-found refusal in AssociateResolverQueryLogConfig's handler, taking its sentence from the shared RSLVR formatter. Related open issue 20261027T062807Z-route53resolver-5-net-new-causes-query-log-config.md covers other query-log-config causes but not this check's replay key.
Traces: route53resolver.associate-resolver-query-log-config-nonexistent-rejected

=== Cause 4 of 7: the gone-association refusal on the query-log-config association path does not use AWS's RSLVR-01602 sentence ===
Failing check: route53resolver.resolver-query-log-config-association-status-transitions
AWS: DisassociateResolverQueryLogConfig(ResolverQueryLogConfigId, ResourceId) and GetResolverQueryLogConfigAssociation both model ResourceNotFoundException {Message} only at httpError 400, so the wording is recorded behavior only: once the association has finished DELETING, a read of it and a second disassociate both answer HTTP 400, `ResourceNotFoundException`, Message `[RSLVR-01602] The specified query logging configuration association doesn't exist.` — entirely FIXED text, naming neither the config nor the VPC.
Emulator: the right code with the self-authored sentence `Resolver query log config association between Resolver query log config "<config-id>" and VPC "<vpc-id>" does not exist.` — no RSLVR-01602, and it echoes two ids AWS does not name.
Why: a client cannot distinguish "association gone" (RSLVR-01602) from "config gone" (RSLVR-01601, cause 3) by code, which is exactly the distinction Resolver draws; idempotent-teardown code that treats only RSLVR-01602 as already-done breaks.
Fix: the not-found refusal shared by DisassociateResolverQueryLogConfig and GetResolverQueryLogConfigAssociation, routed through the shared RSLVR formatter.
Traces: route53resolver.resolver-query-log-config-association-status-transitions

=== Cause 5 of 7: GetResolverRuleAssociation's not-found refusal does not use AWS's `ResolverRuleAssociation "<id>" does not Exist.` sentence ===
Failing checks: route53resolver.resolver-rule-association-gone-after-disassociate, route53resolver.resolver-rule-association-state-transition
AWS: Smithy models GetResolverRuleAssociation(ResolverRuleAssociationId required) with errors InternalServiceError | InvalidParameter | ResourceNotFoundException | Throttling (all 400); ResourceNotFoundException carries only {Message}, so the text is recorded behavior only: after DisassociateResolverRule completes, the read answers HTTP 400, `ResourceNotFoundException`, Message `ResolverRuleAssociation "<ResolverRuleAssociationId>" does not Exist.` Anatomy: FIXED text `ResolverRuleAssociation "` … `" does not Exist.` — note AWS's capital E in "Exist" and the absence of any RSLVR code on this particular message — with the association id from the request as the one VARIABLE part.
Emulator: `Resolver rule association with ID "<id>" does not exist.` — correct code, different fixed text (different noun phrase, lowercase "exist").
Why: exact-message assertions and log-matching against AWS fail, and the two spellings are not interchangeable for clients that parse the message.
Fix: the not-found message for GetResolverRuleAssociation (and the second-disassociate path that reuses it) in the handler/state helper; it must be the one sentence AWS returns, not the crate's generic "<resource> with ID ... does not exist" template.
Traces: route53resolver.resolver-rule-association-gone-after-disassociate, route53resolver.resolver-rule-association-state-transition

=== Cause 6 of 7: ListResolverEndpoints' invalid-NextToken refusal does not use AWS's RSLVR-00302 sentence ===
Failing check: route53resolver.list-resolver-endpoints-invalid-next-token-rejected
AWS: Smithy models ListResolverEndpoints(MaxResults 1-100, NextToken, Filters) as @paginated with errors InternalServiceError | InvalidNextTokenException | InvalidParameter | InvalidRequest | Throttling (all 400). The wording is recorded behavior only: a token the service never issued is refused HTTP 400, Error.Code `InvalidNextTokenException`, Error.Message `[RSLVR-00302] Invalid next token: "<the NextToken sent>", invalid version.` Anatomy: FIXED text `[RSLVR-00302] Invalid next token: "` … `", invalid version.` with the submitted token as the one VARIABLE part.
Emulator: the correct InvalidNextTokenException code but the self-authored `The NextToken you provided was not issued by this service for this query` — no RSLVR-00302, no echo of the token, and no "invalid version" clause.
Why: pagination error handling keyed on RSLVR-00302 (or on the echoed token for diagnostics) gets nothing.
Fix: the NextToken decode-failure refusal for ListResolverEndpoints — and the shared pagination-token validator this crate's listings call — must emit the RSLVR-00302 sentence.
Traces: route53resolver.list-resolver-endpoints-invalid-next-token-rejected

=== Cause 7 of 7: ListResolverQueryLogConfigAssociations' invalid-filter refusal does not use AWS's RSLVR-01004 sentence ===
Failing check: route53resolver.list-resolver-query-log-config-associations-invalid-filter-rejected
AWS: Smithy models ListResolverQueryLogConfigAssociations(MaxResults, NextToken, Filters, SortBy, SortOrder) with errors AccessDenied | InternalServiceError | InvalidParameterException | InvalidRequest | LimitExceeded | Throttling (all 400). The filter-name vocabulary is not modelled, so this is recorded behavior only: an unknown Filters[].Name is refused HTTP 400, Error.Code `InvalidParameterException`, Error.Message `[RSLVR-01004] The filter key "<Filters[].Name as sent>" is invalid.` Anatomy: FIXED text `[RSLVR-01004] The filter key "` … `" is invalid.` with the submitted filter name as the one VARIABLE part; AWS does not name the operation in the message. A valid filter name on the same call still succeeds.
Emulator: the correct InvalidParameterException code with the self-authored `'<name>' is not a valid filter name for ListResolverQueryLogConfigAssociations` — no RSLVR-01004, single quotes instead of AWS's double quotes, and an operation name AWS never prints.
Why: clients that key on RSLVR-01004 to distinguish a bad filter key from any other InvalidParameterException on the same listing cannot do so.
Fix: the filter-name validation shared by this crate's Filters-accepting listings, emitting the RSLVR-01004 sentence from the shared RSLVR formatter.
Traces: route53resolver.list-resolver-query-log-config-associations-invalid-filter-rejected

## Evidence
Behavioral suite, service route53resolver (bootstrapped), 8 failing checks in this bundle.

Cause 1 — create-resolver-rule-duplicate-creator-request-id:
  expected ResourceExistsException '[RSLVR-00701] Resolver rule with creator request ID "<CreatorRequestId>" already exists.'
  got      HTTP 200, a second ResolverRule minted — AssertionError: the call succeeded; expected AWS to refuse it with a ClientError

Cause 2 — unicode-resolver-rule-name-round-trip:
  expected ('InvalidParameterException', '[RSLVR-00711] Invalid domain or delegation record') for DomainName='münchen.<zone>.example.'
  got      HTTP 200: {'Id': 'rslvr-rr-...', 'DomainName': 'münchen.sim2real-r53r-b1918cfa1c7e.example.', 'Status': 'COMPLETE', 'RuleType': 'SYSTEM', ...}

Cause 3 — associate-resolver-query-log-config-nonexistent-rejected:
  expected message "[RSLVR-01601] The specified query logging configuration doesn't exist."
  got            'Resolver query log config with ID "rqlc-0123456789abcdef0" does not exist.'

Cause 4 — resolver-query-log-config-association-status-transitions:
  expected ('ResourceNotFoundException', "[RSLVR-01602] The specified query logging configuration association doesn't exist.")
  got      ('ResourceNotFoundException', 'Resolver query log config association between Resolver query log config "rqlc-910a2dec89025cc1" and VPC "vpc-00000000000000022" does not exist.')

Cause 5 — resolver-rule-association-gone-after-disassociate / resolver-rule-association-state-transition:
  expected ('ResourceNotFoundException', 'ResolverRuleAssociation "rslvr-rrassoc-975835de1c9756ce" does not Exist.')
  got      ('ResourceNotFoundException', 'Resolver rule association with ID "rslvr-rrassoc-975835de1c9756ce" does not exist.')
  (second check identical with id rslvr-rrassoc-63033b0ca389c35a)

Cause 6 — list-resolver-endpoints-invalid-next-token-rejected:
  expected message '[RSLVR-00302] Invalid next token: "not-a-real-next-token-!!!", invalid version.' (code InvalidNextTokenException)
  got            'The NextToken you provided was not issued by this service for this query'

Cause 7 — list-resolver-query-log-config-associations-invalid-filter-rejected:
  expected message '[RSLVR-01004] The filter key "NotARealResolverFilterName" is invalid.' (code InvalidParameterException)
  got            "'NotARealResolverFilterName' is not a valid filter name for ListResolverQueryLogConfigAssociations"

## Steps to Reproduce
Replay the 8 behavioral checks listed in trace_ids against the emulator (service route53resolver). Minimal reproductions per cause:
1. CreateResolverRule(CreatorRequestId=T, Name=n1, RuleType=SYSTEM, DomainName=a.example.internal.), then CreateResolverRule(CreatorRequestId=T, Name=n2, RuleType=SYSTEM, DomainName=b.example.internal.) -> must be ResourceExistsException RSLVR-00701, and GetResolverRule on the first rule must still report DomainName a.example.internal.
2. CreateResolverRule(RuleType=SYSTEM, DomainName='xn--mnchen-3ya.<zone>.example.') must succeed and round-trip; CreateResolverRule(RuleType=SYSTEM, DomainName='münchen.<zone>.example.') must be InvalidParameterException RSLVR-00711 and must leave no rule in ListResolverRules.
3. AssociateResolverQueryLogConfig(ResolverQueryLogConfigId='rqlc-0123456789abcdef0', ResourceId=<vpc>) -> compare Error.Message with the RSLVR-01601 sentence; no dangling association may be created.
4. CreateResolverQueryLogConfig + AssociateResolverQueryLogConfig, poll to CREATED, DisassociateResolverQueryLogConfig, poll until gone, then GetResolverQueryLogConfigAssociation and a second DisassociateResolverQueryLogConfig -> both must answer the RSLVR-01602 sentence.
5. CreateResolverRule + AssociateResolverRule(VpcId=<vpc>), poll COMPLETE, DisassociateResolverRule, then GetResolverRuleAssociation(ResolverRuleAssociationId=<assoc>) -> Message must be 'ResolverRuleAssociation "<assoc>" does not Exist.'
6. ListResolverEndpoints(NextToken='not-a-real-next-token-!!!') -> InvalidNextTokenException with the RSLVR-00302 sentence echoing the token; ListResolverEndpoints(MaxResults=n) with no token must still succeed.
7. ListResolverQueryLogConfigAssociations(Filters=[{'Name':'NotARealResolverFilterName','Values':['whatever']}]) -> InvalidParameterException with the RSLVR-01004 sentence; a valid filter name on the same call must still succeed.
