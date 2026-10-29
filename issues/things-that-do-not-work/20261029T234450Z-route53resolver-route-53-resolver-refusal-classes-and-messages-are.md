Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-list-operations-invalid-token-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-list-operations-invalid-token-errors route53resolver-route53resolver-readonly-list-configs route53resolver-dns-firewall-domain-list-lifecycle route53resolver-dns-firewall-rule-group-and-rules route53resolver-query-log-config-missing-resource-errors route53resolver-vpc-config-and-association-missing-resource-errors
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|ListFirewallConfigs|4xx->4xx|$.Message
Issue-Key: route53resolver|ListFirewallDomainLists|4xx->4xx|$.Message
Issue-Key: route53resolver|CreateFirewallDomainList|4xx->4xx|$.message
Issue-Key: route53resolver|CreateFirewallRule|4xx->4xx|$.Message
Issue-Key: route53resolver|AssociateResolverQueryLogConfig|4xx->4xx|$.Message
Issue-Key: route53resolver|AssociateResolverQueryLogConfig|4xx->4xx|$.__type
Issue-Key: route53resolver|AssociateResolverRule|4xx->4xx|$.Message
Issue-Key: route53resolver|AssociateResolverRule|4xx->4xx|$.__type
Issue-Key: route53resolver|GetResolverRuleAssociation|4xx->4xx|$.Message
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# Route 53 Resolver refusal classes and messages are emulator-authored on pagination, name validation and association paths (7 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFirewallConfigs

## Description
Seven net-new wire causes where the emulator returns HTTP 400 with the right-ish status but the wrong error identity: a self-authored message instead of the service's text, a missing message member, or the wrong error class entirely. Service route53resolver, crate services/service-com-amazonaws-route53resolver-route53resolver.

=== Cause 1 of 7: ListFirewallConfigs' invalid-NextToken refusal is emulator-authored ===
AWS: Smithy: ListFirewallConfigs(MaxResults @range 5..10, NextToken) -> {NextToken, FirewallConfigs}; errors AccessDenied | InternalServiceError | Throttling | ValidationException (all 400); ValidationException carries only {Message}, so the text is recorded behavior only. Recorded behavior: a NextToken the service never issued is refused HTTP 400 with the EC2-proxied validation sentence 'Value ( <token> ) for parameter NextToken is invalid. The token is invalid. (Service: AmazonEC2; Status Code: 400; Error Code: InvalidParameterValue; Request ID: <request id>; Proxy: null) Trace Id: "<trace Root>"'. Fixed text: 'Value ( ', ' ) for parameter NextToken is invalid. The token is invalid. (Service: AmazonEC2; Status Code: 400; Error Code: InvalidParameterValue; Request ID: ', '; Proxy: null) Trace Id: "'. Variable: the submitted token (from the request), the request id and the trace Root (per request).
Emulator: HTTP 400 with 'The NextToken you provided was not issued by this service for this query' — a single self-authored sentence, no EC2 proxy envelope, no token echo, no trace clause.
Why: a client that parses the embedded error code or echoes the offending token back to the user gets neither; paginator tests written against AWS fail, and the two distinct Resolver token-refusal formats (this one and cause 2) become indistinguishable.
Fix: the NextToken validation/refusal rendering for the firewall-config list path, which must produce the service's proxied sentence from the shared message formatter instead of a local string.
Traces: route53resolver-list-operations-invalid-token-errors#task#1 (cluster also covers route53resolver-route53resolver-readonly-list-configs#task#3)

=== Cause 2 of 7: ListFirewallDomainLists' invalid-NextToken refusal omits the RSLVR-00302 sentence ===
AWS: Smithy: ListFirewallDomainLists(MaxResults 1..100, NextToken) -> {NextToken, FirewallDomainLists}; same 400 error set. Recorded behavior: an undecodable token is refused HTTP 400 with Message '[RSLVR-00302] Invalid next token: "<token>", invalid version. Trace Id: "<trace Root>"' — fixed text with the RSLVR-00302 prefix and the ', invalid version.' clause; variable: the submitted token and the per-request trace Root.
Emulator: the same generic 'The NextToken you provided was not issued by this service for this query' as cause 1 — one message for two different service refusals.
Why: clients cannot key on RSLVR-00302, and the emulator loses the distinction AWS draws between its DNS Firewall list tokens and the EC2-proxied config tokens.
Fix: the NextToken decoder's refusal on the domain-list path, rendering the RSLVR-00302 sentence (with the token echoed) through the shared RSLVR formatter.
Traces: route53resolver-list-operations-invalid-token-errors#task#2 (cluster also covers route53resolver-route53resolver-readonly-list-configs#task#2)

=== Cause 3 of 7: CreateFirewallDomainList's Name-pattern refusal carries no message member at all ===
AWS: Smithy: CreateFirewallDomainList.Name is required with @length 0..64 and @pattern "^(?!^[0-9]+$)([a-zA-Z0-9\\-_' ']+)$"; the operation models ValidationException (400). Recorded behavior: a Name violating the pattern is refused HTTP 400 whose body carries the lowercase member `message` with the standard constraint sentence '1 validation error detected: Value \'<submitted name>\' at \'name\' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!^[0-9]+$)([a-zA-Z0-9\\-_\' \']+)' — fixed text '1 validation error detected: Value ', ' at ', ' failed to satisfy constraint: Member must satisfy regular expression pattern: ' plus the pattern WITHOUT its ^ and $ anchors; variable: the submitted value and the member name ('name') from the request.
Emulator: HTTP 400 (right status) but the body has no `message` member at all (replay: "$.message: missing ... got None").
Why: the SDK surfaces an error with an empty message, so the caller is told only that something was invalid — not which member, which value or which constraint; this is the one refusal a user-facing form must render.
Fix: the constraint-validation error serializer for this crate must emit the `message` member with the standard constraint sentence (and quote the pattern without anchors) on CreateFirewallDomainList's Name.
Traces: route53resolver-dns-firewall-domain-list-lifecycle#task#11

=== Cause 4 of 7: CreateFirewallRule's duplicate-priority refusal does not use AWS's RSLVR-02203 sentence ===
AWS: Smithy: CreateFirewallRule(..., FirewallRuleGroupId required, Priority required, Action required, Name required) -> {FirewallRule}; errors include ValidationException (400). Priority uniqueness within a rule group is not modellable, so it is recorded behavior only: a second rule at a priority the group already holds is refused HTTP 400 with Message '[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "<FirewallRuleGroupId>". Provide another priority. Trace Id: "<trace Root>"' — fixed text; the only request-derived variable is the rule group id (the priority itself is NOT echoed), plus the per-request trace Root.
Emulator: HTTP 400 with 'Priority <n> is already in use by another rule in rule group <group-id>' — self-authored prose, no RSLVR-02203, no trace clause, and it echoes a priority AWS never names.
Why: a client that retries with a new priority on RSLVR-02203 never recognises why it lost; exact-message negative tests fail.
Fix: the duplicate-priority check in CreateFirewallRule's handler, routed through the shared RSLVR formatter. Open issue 20261029T193214Z-route53resolver-dns-firewall-control-plane-10-net-new.md names the same RSLVR-02203 sentence from behavioral checks but does not carry this fixture step's replay key; this wire cause is filed here and may be closed by the same fix.
Traces: route53resolver-dns-firewall-rule-group-and-rules#task#17

=== Cause 5 of 7: AssociateResolverQueryLogConfig answers ResourceNotFoundException where AWS answers InvalidParameterException ===
AWS: Smithy: AssociateResolverQueryLogConfig(ResolverQueryLogConfigId required, ResourceId required) -> {ResolverQueryLogConfigAssociation}; errors AccessDenied | InternalServiceError | InvalidParameter | InvalidRequest | LimitExceeded | ResourceExists | ResourceNotFound | Throttling (all 400); InvalidParameterException carries a required Message plus FieldName ("the name of the parameter that's invalid"). Recorded behavior: associating a query-log config id that does not exist is refused HTTP 400 with __type 'InvalidParameterException' (Message asserted by shape) — the service treats the unknown id as an invalid parameter on this path, not as a missing resource.
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and a self-authored Message 'Resolver query log config with ID "<id>" does not exist.'
Why: a client that catches InvalidParameterException (and reads its FieldName to point at the offending input) does not catch ResourceNotFoundException, so the refusal escapes its handler.
Fix: the error classification in AssociateResolverQueryLogConfig's handler, which must raise InvalidParameterException with the service's wording and FieldName.
Traces: route53resolver-query-log-config-missing-resource-errors#task#5

=== Cause 6 of 7: AssociateResolverRule validates the rule before the VPC and answers the wrong class and sentence ===
AWS: Smithy: AssociateResolverRule(ResolverRuleId required, Name, VPCId required) -> {ResolverRuleAssociation}; errors InternalServiceError | InvalidParameter | InvalidRequest | LimitExceeded | ResourceExists | ResourceNotFound | ResourceUnavailable | Throttling (all 400). Recorded behavior: when both ids are absent, the VPC is reported first — HTTP 400, __type 'InvalidParameterException', Message "The vpc ID '<VPCId>' does not exist" — fixed text "The vpc ID '" + "' does not exist" with the vpc id from the request as the only variable (note: no trailing period).
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and '[RSLVR-00703] Resolver rule with ID "<rule id>" does not exist.' — it validates the rule first, so both the class and the subject of the refusal differ.
Why: a client that maps the refusal back to the offending input blames the rule instead of the VPC, and its InvalidParameterException handler never runs; the validation ORDER is itself part of the contract here.
Fix: the input-validation order and classification in AssociateResolverRule's handler — the VPCId existence check must run first and raise InvalidParameterException with the service's vpc sentence.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#13

=== Cause 7 of 7: GetResolverRuleAssociation's not-found Message is re-worded ===
AWS: Smithy: GetResolverRuleAssociation(ResolverRuleAssociationId required) -> {ResolverRuleAssociation}; errors InternalServiceError | InvalidParameter | ResourceNotFound | Throttling (400). Recorded behavior: an unknown association id is refused with Message 'ResolverRuleAssociation "<ResolverRuleAssociationId>" does not Exist.' — fixed text 'ResolverRuleAssociation "' + '" does not Exist.' (capital E in "Exist", resource type named in CamelCase); the only variable is the id from the request.
Emulator: 'Resolver rule association with ID "<id>" does not exist.' — a different sentence (spaced-out resource name, 'with ID', lowercase 'exist').
Why: exact-message assertions and log-based matching against AWS fail; the emulator reads as a different service for an error clients routinely pattern-match.
Fix: the not-found message for the resolver-rule-association read path, rendered from the shared message formatter with the service's exact wording.
Traces: route53resolver-vpc-config-and-association-missing-resource-errors#task#14

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). Seven mismatch clusters, none stub-blocked, none owned.

Cause 1 — ListFirewallConfigs, route53resolver-list-operations-invalid-token-errors#task#1, 400 -> 400:
  $.Message expected 'Value ( not-a-real-pagination-token ) for parameter NextToken is invalid. The token is invalid. (Service: AmazonEC2; Status Code: 400; Error Code: InvalidParameterValue; Request ID: 8b62ccec-d70c-44ed-97f7-1f271cf82f43; Proxy: null) Trace Id: "1-6a9e6884-704529537abc2b1a36142202"'
  got      'The NextToken you provided was not issued by this service for this query'   [2 affected steps]  (Request ID and Trace Id are per-run values; the envelope and token echo are the requirement)
Cause 2 — ListFirewallDomainLists, ...#task#2, 400 -> 400:
  $.Message expected '[RSLVR-00302] Invalid next token: "not-a-real-pagination-token", invalid version. Trace Id: "1-6a9e6885-5bfaed98743ecfba34247b84"'
  got      'The NextToken you provided was not issued by this service for this query'   [2 affected steps]
Cause 3 — CreateFirewallDomainList, route53resolver-dns-firewall-domain-list-lifecycle#task#11, 400 -> 400:
  $.message: missing (expected "1 validation error detected: Value 'not a valid name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!^[0-9]+$)([a-zA-Z0-9\\-_' ']+)", got None)
Cause 4 — CreateFirewallRule, route53resolver-dns-firewall-rule-group-and-rules#task#17, 400 -> 400:
  $.Message expected '[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "rslvr-frg-910a2dec89025cc1". Provide another priority. Trace Id: "1-6a76e073-3186793b770901975ae8ca24"'
  got      'Priority 100 is already in use by another rule in rule group rslvr-frg-910a2dec89025cc1'
Cause 5 — AssociateResolverQueryLogConfig, route53resolver-query-log-config-missing-resource-errors#task#5, 400 -> 400:
  $.__type: value_mismatch (expected 'InvalidParameterException', got 'ResourceNotFoundException')
  $.Message: expected '<shaped-not-asserted>', got 'Resolver query log config with ID "rqlc-0123456789abcdef0" does not exist.'
Cause 6 — AssociateResolverRule, route53resolver-vpc-config-and-association-missing-resource-errors#task#13, 400 -> 400:
  $.__type: value_mismatch (expected 'InvalidParameterException', got 'ResourceNotFoundException')
  $.Message: expected "The vpc ID 'vpc-0123456789abcdef0' does not exist", got '[RSLVR-00703] Resolver rule with ID "rslvr-rr-0123456789abcdef0" does not exist.'
Cause 7 — GetResolverRuleAssociation, ...#task#14, 400 -> 400:
  $.Message: expected 'ResolverRuleAssociation "rslvr-rrassoc-0123456789abcdef0" does not Exist.', got 'Resolver rule association with ID "rslvr-rrassoc-0123456789abcdef0" does not exist.'

## Steps to Reproduce
Replay fixtures route53resolver-list-operations-invalid-token-errors (task#1, task#2), route53resolver-dns-firewall-domain-list-lifecycle (task#11), route53resolver-dns-firewall-rule-group-and-rules (task#17), route53resolver-query-log-config-missing-resource-errors (task#5) and route53resolver-vpc-config-and-association-missing-resource-errors (task#13, task#14). Minimal reproductions:
1/2. ListFirewallConfigs(NextToken='not-a-real-pagination-token') and ListFirewallDomainLists(NextToken=same) -> the two Messages must be the service's distinct sentences (EC2-proxied InvalidParameterValue envelope vs '[RSLVR-00302] Invalid next token: "<token>", invalid version.'), each ending with the Trace Id clause.
3. CreateFirewallDomainList(CreatorRequestId=<id>, Name='not a valid name!') -> body must carry `message` with the '1 validation error detected: ...' constraint sentence quoting the pattern without ^/$ anchors.
4. Create a rule group + domain list, CreateFirewallRule(Priority=100), then a second CreateFirewallRule(Priority=100) -> Message must be the RSLVR-02203 sentence naming the rule group id.
5. AssociateResolverQueryLogConfig(ResolverQueryLogConfigId='rqlc-0123456789abcdef0', ResourceId=<vpc>) -> __type must be InvalidParameterException.
6. AssociateResolverRule(ResolverRuleId='rslvr-rr-0123456789abcdef0', VPCId='vpc-0123456789abcdef0') -> __type InvalidParameterException with "The vpc ID 'vpc-0123456789abcdef0' does not exist".
7. GetResolverRuleAssociation(ResolverRuleAssociationId='rslvr-rrassoc-0123456789abcdef0') -> Message must be 'ResolverRuleAssociation "<id>" does not Exist.'
