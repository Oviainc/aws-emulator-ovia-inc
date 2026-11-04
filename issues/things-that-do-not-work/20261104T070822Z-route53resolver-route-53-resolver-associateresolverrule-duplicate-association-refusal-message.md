Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.associate-resolver-rule-duplicate-vpc-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.associate-resolver-rule-duplicate-vpc-rejected|run#11|AssertionError|assert err["Message"] == (
Issue-Commit: 7bce35011e06a27791111295606361de5e667ce7

# Route 53 Resolver AssociateResolverRule duplicate-association refusal Message is emulator-authored: no [RSLVR-00704] code, an extra `by association "..."` clause and no terminating period

> _Note: this issue was identified against the code base at commit `7bce35011e06a27791111295606361de5e667ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResolverRule

## Description
One net-new cause in service route53resolver (crate services/service-com-amazonaws-route53resolver-route53resolver).

Cause 1 of 1: AssociateResolverRule, called again for a (ResolverRuleId, VPCId) pair that is already associated, is refused with the correct error code but a self-authored Message.

AWS: Smithy: AssociateResolverRule(ResolverRuleId required @length 1..64, VPCId required @length 1..64, Name) -> {ResolverRuleAssociation}; modeled errors include ResourceExistsException (httpError 400, defaulted). That a second association of the same rule+VPC pair is the condition refused is not modellable, so the wording is recorded behavior only: HTTP 400, error code `ResourceExistsException`, Message
  [RSLVR-00704] Resolver rule "<ResolverRuleId>" is already associated with VPC "<VPCId>".
Fixed text: '[RSLVR-00704] Resolver rule "', '" is already associated with VPC "', '".' — the RSLVR-00704 code prefix, the two quoted-id clauses and the terminating period. Variable parts: the resolver rule id and the VPC id, both taken verbatim from the request. The message names NO third identifier: the id of the pre-existing association is not part of it.

Emulator: HTTP 400 with the right Code (`ResourceExistsException`) but Message
  Resolver rule "<rule id>" is already associated with VPC "<vpc id>" by association "<existing association id>"
It drops the `[RSLVR-00704] ` prefix, appends an ` by association "<id>"` clause AWS never emits, and omits the trailing period. (In the replay the ids were the per-run rslvr-rr-…/vpc-…/rslvr-rrassoc-… values of that run; the ids themselves are not the requirement, the template is.)

Why: clients and operators key duplicate-association handling on the RSLVR-00704 code embedded in the Message (the wire Code is the generic ResourceExistsException, shared with several unrelated conflicts on this service), so a client cannot tell this conflict apart from any other ResourceExists refusal. Exact-message negative tests and log matching written against AWS fail, and the extra association id leaks emulator state into a sentence AWS keeps request-derived.

Fix: the duplicate-association conflict path in AssociateResolverRule's handler and the message it renders — it must go through the shared RSLVR message formatter with the RSLVR-00704 sentence built from the request's ResolverRuleId and VPCId only, instead of a locally formatted string that interpolates the existing association id. Note the related open issues 20261029T234450Z-route53resolver-route-53-resolver-refusal-classes-and-messages-are.md (AssociateResolverRule's NOT-FOUND class/sentence) and 20261102T105111Z-route53resolver-route-53-resolver-refusal-messages-still-omit-the.md (the trailing Trace Id clause on other RSLVR messages): neither covers this duplicate-association sentence, which is net-new here.

Traces: behavioral check route53resolver.associate-resolver-rule-duplicate-vpc-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service route53resolver (bootstrapped).

[mismatch] route53resolver.associate-resolver-rule-duplicate-vpc-rejected
  check_route53resolver_associate_resolver_rule_duplicate_vpc_rejected.py line 41:
    assert err["Message"] == (f'[RSLVR-00704] Resolver rule "{rule_id}" is already associated with VPC "{vpc_id}".')
  AssertionError: {'Message': 'Resolver rule "rslvr-rr-910a2dec89025cc1" is already associated with VPC "vpc-00000000000000026" by association "rslvr-rrassoc-975835de1c9756ce"', 'Code': 'ResourceExistsException'}

Expected (AWS):  '[RSLVR-00704] Resolver rule "<rule id>" is already associated with VPC "<vpc id>".'
Actual (emulator): 'Resolver rule "<rule id>" is already associated with VPC "<vpc id>" by association "<assoc id>"'
Error Code matches on both sides (ResourceExistsException, HTTP 400); the divergence is the Message only: missing '[RSLVR-00704] ' prefix, extra ' by association "<id>"' clause, missing trailing '.'. The rslvr-rr-/vpc-/rslvr-rrassoc- values above are that run's ids, not part of the requirement.

The check then goes on to assert that exactly one association survives and that the identical call succeeds after DisassociateResolverRule; it aborts at the Message assertion, so those effects are untested here and may hold.

## Steps to Reproduce
1. Pick a usable VPC and create a resolver rule (RuleType=SYSTEM) via route53resolver CreateResolverRule.
2. AssociateResolverRule(ResolverRuleId=<rule id>, VPCId=<vpc id>) and poll GetResolverRuleAssociation until Status is COMPLETE/OVERRIDDEN.
3. Call AssociateResolverRule again with the SAME ResolverRuleId and VPCId.
4. Observe the refusal: HTTP 400 / Code ResourceExistsException is correct, but Message is 'Resolver rule "<rule id>" is already associated with VPC "<vpc id>" by association "<assoc id>"'. AWS answers '[RSLVR-00704] Resolver rule "<rule id>" is already associated with VPC "<vpc id>".'

Replay: behavioral check route53resolver.associate-resolver-rule-duplicate-vpc-rejected.
