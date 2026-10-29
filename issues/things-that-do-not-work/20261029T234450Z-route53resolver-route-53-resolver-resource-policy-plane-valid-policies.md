Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-dns-firewall-rule-group-policy-and-tagging
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-dns-firewall-rule-group-policy-and-tagging route53resolver-resolver-rule-system-lifecycle route53resolver-query-log-config-missing-resource-errors
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|PutFirewallRuleGroupPolicy|2xx->4xx|$.ReturnValue
Issue-Key: route53resolver|PutFirewallRuleGroupPolicy|2xx->4xx|status
Issue-Key: route53resolver|GetFirewallRuleGroupPolicy|2xx->2xx|$.FirewallRuleGroupPolicy
Issue-Key: route53resolver|PutFirewallRuleGroupPolicy|4xx->4xx|$.Message
Issue-Key: route53resolver|PutResolverRulePolicy|4xx->2xx|$.Message
Issue-Key: route53resolver|PutResolverRulePolicy|4xx->2xx|$.__type
Issue-Key: route53resolver|PutResolverRulePolicy|4xx->2xx|status
Issue-Key: route53resolver|GetResolverRulePolicy|2xx->2xx|$.ResolverRulePolicy
Issue-Key: route53resolver|PutResolverQueryLogConfigPolicy|4xx->4xx|$.Message
Issue-Key: route53resolver|PutResolverQueryLogConfigPolicy|4xx->4xx|$.__type
Issue-Key: route53resolver|GetResolverQueryLogConfigPolicy|4xx->4xx|$.Message
Issue-Key: route53resolver|GetResolverQueryLogConfigPolicy|4xx->4xx|$.__type
Issue-Commit: c8db4453ab7530e781096e27f57c405b79639576

# Route 53 Resolver resource-policy plane: valid policies refused, policies read back empty, invalid documents accepted and not-found classes wrong (7 causes)

> _Note: this issue was identified against the code base at commit `c8db4453ab7530e781096e27f57c405b79639576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFirewallRuleGroupPolicy

## Description
Seven net-new wire causes in the Route 53 Resolver resource-policy plane (firewall rule group, resolver rule and query-log config policies) of services/service-com-amazonaws-route53resolver-route53resolver. The shared fault: policy documents are not validated, not stored and not read back the way the service does, and the policy reads/writes pick the wrong not-found error class.

=== Cause 1 of 7: PutFirewallRuleGroupPolicy refuses a valid sharing policy with HTTP 400 ===
AWS: Smithy: PutFirewallRuleGroupPolicy(Arn required @length 1..255, FirewallRuleGroupPolicy required @length 0..30000) -> {ReturnValue: boolean}; errors AccessDenied | InternalServiceError | ResourceNotFound | Throttling | ValidationException (all 400). Recorded behavior: a well-formed IAM policy document naming the rule-group ARN as Resource and route53resolver:GetFirewallRuleGroup/ListFirewallRuleGroups as Action is ACCEPTED — HTTP 200 with ReturnValue true.
Emulator: HTTP 400 and no ReturnValue member (replay: "status: expected 200, got 400"; "$.ReturnValue: missing (expected True, got None)").
Why: the whole rule-group sharing workflow is unreachable — a client cannot attach a resource policy at all, so nothing downstream (GetFirewallRuleGroupPolicy, RAM-style sharing, cross-account reads) can be exercised.
Fix: the policy-document acceptance path in the PutFirewallRuleGroupPolicy handler (its validation is rejecting a document the service accepts) and the policy store in the crate's state module.
Traces: route53resolver-dns-firewall-rule-group-policy-and-tagging#task#2

=== Cause 2 of 7: GetFirewallRuleGroupPolicy answers '{}' instead of the attached policy ===
AWS: Smithy: GetFirewallRuleGroupPolicy(Arn required) -> {FirewallRuleGroupPolicy @length 0..30000}. Recorded behavior: the read returns the document that was last successfully Put, verbatim in the service's serialization (Version/Statement with the Sid, Effect, Principal, Action list and the rule-group ARN as Resource).
Emulator: HTTP 200 with FirewallRuleGroupPolicy '{}' — an empty document.
Why: a client that reads-modifies-writes the policy (the normal way to add a principal) loses the existing statements, and any policy audit sees an unshared rule group.
Fix: the policy read projection in GetFirewallRuleGroupPolicy must return the stored document; with cause 1 unfixed nothing is ever stored, but the read must also stop substituting an empty object for a missing policy.
Traces: route53resolver-dns-firewall-rule-group-policy-and-tagging#task#3

=== Cause 3 of 7: PutFirewallRuleGroupPolicy's RSLVR-02035 ValidationException Message is missing its trailing Trace Id clause ===
AWS: ValidationException carries only {Message} (target ExceptionMessage), so the text is recorded behavior only. Recorded behavior for a malformed policy document: HTTP 400, Message '[RSLVR-02035] The specified IAM policy isn\'t valid: "<the submitted document>". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account. Trace Id: "<Root of the request\'s X-Amzn-Trace-Id>"'. Fixed text: the RSLVR-02035 prefix, the two sentences and the ' Trace Id: "' framing; variable: the submitted policy string (from the request) and the trace Root (per request).
Emulator: the correct code and both sentences, but the Message stops at the final period — the whole Trace Id field is absent.
Why: a client or support workflow that parses the trace Root out of the Message to correlate a refusal with its request has nothing to parse, and exact-message assertions written against AWS fail.
Fix: the crate's RSLVR error-message formatter / error serializer must append the ' Trace Id: "<Root>"' clause from the request's X-Amzn-Trace-Id on every RSLVR-coded refusal. Open issue 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md describes the same omission for other operations but does not carry this step's replay key; this cause is filed here and may be closed by the same fix.
Traces: route53resolver-dns-firewall-rule-group-policy-and-tagging#task#9

=== Cause 4 of 7: PutResolverRulePolicy accepts an invalid policy document with HTTP 200 instead of InvalidPolicyDocument ===
AWS: Smithy: PutResolverRulePolicy(Arn required, ResolverRulePolicy required @length 0..30000) -> {ReturnValue}; errors AccessDenied | InternalServiceError | InvalidParameter | InvalidPolicyDocument | UnknownResource (all 400); InvalidPolicyDocument is documented "The specified Resolver rule policy is invalid." Recorded behavior: a policy document that is not a valid Resolver rule policy is refused HTTP 400 with __type 'InvalidPolicyDocument' and Message '[RSLVR-00312] Invalid Policy' (entirely fixed text).
Emulator: HTTP 200 with no error at all — the invalid document is accepted and stored.
Why: a client can attach a policy AWS would reject, so a sharing configuration that appears to succeed against the emulator fails in production; negative tests for policy validation cannot pass.
Fix: the policy-document validation in the PutResolverRulePolicy handler, raising InvalidPolicyDocument with the RSLVR-00312 sentence from the shared RSLVR formatter.
Traces: route53resolver-resolver-rule-system-lifecycle#task#6

=== Cause 5 of 7: GetResolverRulePolicy returns a policy the service never stored ===
AWS: Smithy: GetResolverRulePolicy(Arn required) -> {ResolverRulePolicy @length 0..30000}; errors AccessDenied | InternalServiceError | InvalidParameter | UnknownResource. Recorded behavior: after the Put of cause 4 was refused, the rule has no policy and the read answers HTTP 200 with ResolverRulePolicy '{}'.
Emulator: HTTP 200 returning the full document the refused Put should never have stored (a Version/Statement policy sharing the rule with another account).
Why: the read reports a share that does not exist on AWS; combined with cause 4 the emulator's policy state is reachable only through a call AWS rejects, so any assertion about who a rule is shared with is wrong in both directions.
Fix: follows cause 4 — the Put must not persist an invalid document; the read must project the absence of a policy as the empty document rather than the last attempted write.
Traces: route53resolver-resolver-rule-system-lifecycle#task#7

=== Cause 6 of 7: PutResolverQueryLogConfigPolicy answers UnknownResourceException where AWS answers ResourceNotFoundException ===
AWS: Smithy: PutResolverQueryLogConfigPolicy(Arn required, ResolverQueryLogConfigPolicy required) -> {ReturnValue}; the operation's modeled error set is AccessDenied | InternalServiceError | InvalidParameter | InvalidPolicyDocument | InvalidRequest | UnknownResource, and the recorded refusal for an ARN naming a query-log config that does not exist is HTTP 400 with __type 'ResourceNotFoundException' (the service's generic not-found class, whose Message the corpus asserts by shape).
Emulator: HTTP 400 with __type 'UnknownResourceException' and a self-authored Message 'Resolver query log config with ARN "<arn>" does not exist.' — wrong error class and emulator-authored prose with no RSLVR code.
Why: a client that catches ResourceNotFoundException (as the AWS SDK error shape for this path) does not catch UnknownResourceException, so the refusal escapes as an unmodeled error and retry/ignore-if-absent logic breaks.
Fix: the not-found classification in PutResolverQueryLogConfigPolicy's handler, and its message must come from the shared RSLVR formatter rather than local prose.
Traces: route53resolver-query-log-config-missing-resource-errors#task#9

=== Cause 7 of 7: GetResolverQueryLogConfigPolicy answers UnknownResourceException where AWS answers AccessDeniedException ===
AWS: Smithy: GetResolverQueryLogConfigPolicy(Arn required) -> {ResolverQueryLogConfigPolicy}; errors AccessDenied | InternalServiceError | InvalidParameter | InvalidRequest | UnknownResource. AccessDeniedException is documented "The current account doesn't have the IAM permissions required to perform the specified Resolver operation." Recorded behavior: reading the policy of a query-log config ARN the caller does not own/that does not exist is refused HTTP 400 with __type 'AccessDeniedException' (Message asserted by shape) — the service does not disclose existence here.
Emulator: HTTP 400 __type 'UnknownResourceException' with 'Resolver query log config with ARN "<arn>" does not exist.' — both the class and the disclosure differ.
Why: the emulator turns an authorization refusal into an existence refusal, so a client's permission-error handling never runs and the emulator leaks the existence/non-existence of resources AWS deliberately hides.
Fix: the authorization/not-found decision in GetResolverQueryLogConfigPolicy's handler — ownership failures must map to AccessDeniedException with the service's wording.
Traces: route53resolver-query-log-config-missing-resource-errors#task#10

## Evidence
Wire corpus simparity-wire-aws-v1, service route53resolver (bootstrapped). Seven mismatch clusters, none stub-blocked, none owned.

Cause 1 — PutFirewallRuleGroupPolicy, route53resolver-dns-firewall-rule-group-policy-and-tagging#task#2:
  status: expected 200, got 400; $.ReturnValue: missing (expected True, got None)
Cause 2 — GetFirewallRuleGroupPolicy, ...#task#3, 200 -> 200:
  $.FirewallRuleGroupPolicy: value_mismatch (expected '{"Version":"2012-10-17","Statement":[{"Sid":"ShareRuleGroup","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<acct>:root"},"Action":["route53resolver:GetFirewallRuleGroup","route53resolver:ListFirewallRuleGroups"],"Resource":"<rule-group arn>"}]}', got '{}')
Cause 3 — PutFirewallRuleGroupPolicy, ...#task#9, 400 -> 400:
  $.Message: expected '[RSLVR-02035] The specified IAM policy isn\'t valid: "{not-a-policy". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account. Trace Id: "1-6a76e074-570e69780dc8a349626ecf87"'
  got      '[RSLVR-02035] The specified IAM policy isn\'t valid: "{not-a-policy". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account.'  (the trace Root itself is per-request; the clause is the requirement)
Cause 4 — PutResolverRulePolicy, route53resolver-resolver-rule-system-lifecycle#task#6:
  status: expected 400, got 200; $.__type: missing (expected 'InvalidPolicyDocument', got None); $.Message: missing (expected '[RSLVR-00312] Invalid Policy', got None)
Cause 5 — GetResolverRulePolicy, ...#task#7, 200 -> 200:
  $.ResolverRulePolicy: value_mismatch (expected '{}', got '{"Version":"2012-10-17","Statement":[{"Sid":"ShareResolverRule",...,"Resource":"<resolver-rule arn>"}]}')
Cause 6 — PutResolverQueryLogConfigPolicy, route53resolver-query-log-config-missing-resource-errors#task#9, 400 -> 400:
  $.__type: value_mismatch (expected 'ResourceNotFoundException', got 'UnknownResourceException')
  $.Message: expected '<shaped-not-asserted>', got 'Resolver query log config with ARN "arn:aws:route53resolver:us-west-2:000000000000:resolver-query-log-config/rqlc-0123456789abcdef0" does not exist.'
Cause 7 — GetResolverQueryLogConfigPolicy, ...#task#10, 400 -> 400:
  $.__type: value_mismatch (expected 'AccessDeniedException', got 'UnknownResourceException')
  $.Message: expected '<shaped-not-asserted>', got 'Resolver query log config with ARN "...rqlc-0123456789abcdef0" does not exist.'

## Steps to Reproduce
Replay fixtures route53resolver-dns-firewall-rule-group-policy-and-tagging (task#2, task#3, task#9), route53resolver-resolver-rule-system-lifecycle (task#6, task#7) and route53resolver-query-log-config-missing-resource-errors (task#9, task#10). Minimal reproductions:
1/2. CreateFirewallRuleGroup, then PutFirewallRuleGroupPolicy(Arn=<group arn>, FirewallRuleGroupPolicy=<valid share policy>) -> must be 200 with ReturnValue true; then GetFirewallRuleGroupPolicy(Arn=<group arn>) -> must return that document, not '{}'.
3. PutFirewallRuleGroupPolicy with FirewallRuleGroupPolicy='{not-a-policy' and an X-Amzn-Trace-Id: Root=<root> header -> Message must end with ' Trace Id: "<root>"'.
4/5. CreateResolverRule, then PutResolverRulePolicy(Arn=<rule arn>, ResolverRulePolicy=<document the service rejects>) -> must be 400 InvalidPolicyDocument '[RSLVR-00312] Invalid Policy'; GetResolverRulePolicy afterwards must return '{}'.
6. PutResolverQueryLogConfigPolicy(Arn=<arn of a nonexistent rqlc>, ResolverQueryLogConfigPolicy=<any>) -> __type must be ResourceNotFoundException.
7. GetResolverQueryLogConfigPolicy(Arn=<arn of a nonexistent rqlc>) -> __type must be AccessDeniedException, with no existence disclosure in the Message.
