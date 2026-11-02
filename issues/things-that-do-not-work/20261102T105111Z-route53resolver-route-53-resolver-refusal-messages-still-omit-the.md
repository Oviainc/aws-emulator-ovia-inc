Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.concurrent-associate-firewall-rule-group-priority-conflict route53resolver.get-firewall-rule-group-policy-not-found
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.concurrent-associate-firewall-rule-group-priority-conflict|run#18|AssertionError|assert message.startswith(expected_prefix), f"expected message starting {expected_prefix!r}, got {message!r}"
Issue-Key: route53resolver.get-firewall-rule-group-policy-not-found|run#15|AssertionError|assert message.startswith(expected_prefix), (
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# Route 53 Resolver refusal Messages still omit the trailing `Trace Id: "<Root>"` clause on AssociateFirewallRuleGroup (RSLVR-02303) and GetFirewallRuleGroupPolicy (RSLVR-02025) (2 causes)

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateFirewallRuleGroup

## Description
Two net-new causes, both in the Route 53 Resolver DNS Firewall control plane: the emulator's RSLVR-coded error Message ends at the sentence-final period, where real Route 53 Resolver appends a final ` Trace Id: "<Root>"` clause. Related open issues describe the same shared-formatter omission for OTHER operations and do NOT carry these two checks' replay keys: 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md (GetFirewallRuleGroup / DeleteFirewallRuleGroup), 20261029T234450Z-route53resolver-route-53-resolver-not-found-refusal-messages-fail.md (8 not-found causes) and 20261030T052215Z-route53resolver-route-53-resolver-deleteresolverquerylogconfig-not-found-message-omits.md (DeleteResolverQueryLogConfig). The two operations below are additional, still-diverging surfaces of that same defect and are not covered by any of those issues. Note also 20261028T183627Z-route53resolver-6-net-new-causes-dns-firewall-duplicate.md, which covers the RSLVR-02303 duplicate-priority refusal CLASS and WORDING: that part now matches (code, sentence and the winner's association id are all correct) — what remains missing on this path is only the trailing trace clause, which is Cause 1 here.

Cause 1 of 2: AssociateFirewallRuleGroup — the duplicate-priority ValidationException message stops before the trace clause.
AWS: Smithy models AssociateFirewallRuleGroup(CreatorRequestId, FirewallRuleGroupId, VpcId, Priority, Name required) with errors AccessDeniedException | ConflictException | InternalServiceErrorException | LimitExceededException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400; no Message text is modeled, so the wording is recorded behavior only. The recorded refusal when a second association targets a VPC/Priority pair already taken is HTTP 400, Error.Code `ValidationException`, Error.Message of the form:
    [RSLVR-02303] This priority is the same with another association: "<winning FirewallRuleGroupAssociation.Id>" . Try again with another priority number. Trace Id: "<Root>"
Message anatomy: FIXED text is `[RSLVR-02303] This priority is the same with another association: "` … `" . Try again with another priority number. Trace Id: "` … `"` (note the space before the period after the quoted id, exactly as the service emits it). VARIABLE parts are (a) the Id of the association that WON the race, read from emulator state, and (b) the X-Ray trace Root, taken from the request's `X-Amzn-Trace-Id` header (`Root=<root>;Sampled=0`) and minted by the service only when the caller sends none.
Emulator: HTTP 400 and `ValidationException` are right, the RSLVR-02303 sentence is right and the winner's association id is right, but the Message ends at `... Try again with another priority number.` — the ` Trace Id: "<Root>"` clause is absent entirely. Replay: expected a message starting '[RSLVR-02303] This priority is the same with another association: "<assoc-id>" . Try again with another priority number. Trace Id: "', got '[RSLVR-02303] This priority is the same with another association: "<assoc-id>" . Try again with another priority number.' (the assoc-id is this run's value filling the template, not part of the requirement).
Why: a client that matches or parses Resolver's Message loses the correlation key it uses to tie a refusal to its X-Ray trace and to a support case. Negative tests written against AWS's exact message (which ends in `Trace Id: "..."`) fail against the emulator, and tests written against the emulator's short message fail in production.
Fix: the shared RSLVR message formatter in the route53resolver crate's state module (`rslvr_message` and the duplicate-priority/validation message builders layered on it) plus the plumbing that passes the request's trace Root from the handler's OperationContext into it; the AssociateFirewallRuleGroup handler must stop discarding `ctx`.
Traces: route53resolver.concurrent-associate-firewall-rule-group-priority-conflict.

Cause 2 of 2: GetFirewallRuleGroupPolicy — the not-found ResourceNotFoundException message stops before the trace clause.
AWS: Smithy models GetFirewallRuleGroupPolicy(Arn required, length 1..255) -> FirewallRuleGroupPolicy, errors AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400; ResourceNotFoundException{Message, ResourceType} models no text, so the wording is recorded behavior only. For a well-formed `arn:aws:route53resolver:<region>:<account>:firewall-rule-group/<id>` naming a rule group that does not exist, the recorded response is HTTP 400, Error.Code `ResourceNotFoundException`, Error.Message of the form:
    [RSLVR-02025] Can’t find the resource with ID "<rule group id from the ARN>". Trace Id: "<Root>"
Message anatomy: FIXED text is `[RSLVR-02025] Can’t find the resource with ID "` … `". Trace Id: "` … `"` (with the service's U+2019 right single quote in `Can’t`). VARIABLE parts are the rule-group id parsed out of the request's Arn and the X-Ray trace Root as above.
Emulator: HTTP 400, `ResourceNotFoundException` and the RSLVR-02025 sentence naming only the id (the fix recorded in 20261029T092806Z-route53resolver-getfirewallrulegrouppolicy-resourcenotfoundexception-message-echoes-the-whole-rule-group.md now holds — the id, not the whole ARN, is echoed) are all right, but the Message ends at the period: '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-0000000000000000".' with no ` Trace Id: "..."` clause. This is not a reopen of that fixed issue: its refusal class and id echo are correct, and the call is refused exactly as AWS refuses it.
Why: same as Cause 1 — the per-request correlation key AWS puts at the end of every Resolver refusal message is unavailable, so error-triage tooling and exact-message assertions diverge. In this check the omission aborts the run before the final assertion that the real rule group still reads back as `{}`, so that per-resource independence is never exercised.
Fix: the same shared formatter and trace plumbing, applied to the resource-not-found builder used by the GetFirewallRuleGroupPolicy handler.
Traces: route53resolver.get-firewall-rule-group-policy-not-found.

Both causes share one root cause in the emulator — the RSLVR message formatter has no trace-id parameter and no Resolver handler reads the request's `X-Amzn-Trace-Id` — so a single formatter change plus threading the Root through these two handlers closes both, but BOTH checks must be re-replayed before submitting.

## Evidence
Cause 1 — route53resolver.concurrent-associate-firewall-rule-group-priority-conflict [mismatch]
Two concurrent AssociateFirewallRuleGroup calls for the same VpcId and Priority=733: exactly one succeeded and one was refused (correct), with Error.Code = ValidationException (correct).
Expected (AWS) Error.Message prefix: '[RSLVR-02303] This priority is the same with another association: "<winner assoc id>" . Try again with another priority number. Trace Id: "'
Actual (emulator) Error.Message:            '[RSLVR-02303] This priority is the same with another association: "<winner assoc id>" . Try again with another priority number.'
Replay excerpt:
  File "check_route53resolver_concurrent_associate_firewall_rule_group_priority_conflict.py", line 67, in run
    assert message.startswith(expected_prefix), f"expected message starting {expected_prefix!r}, got {message!r}"
  AssertionError: expected message starting '[RSLVR-02303] This priority is the same with another association: "rslvr-frgassoc-1d0b14e4db018fed" . Try again with another priority number. Trace Id: "', got '[RSLVR-02303] This priority is the same with another association: "rslvr-frgassoc-1d0b14e4db018fed" . Try again with another priority number.'
  (rslvr-frgassoc-1d0b14e4db018fed is this run's winning association id filling the template; the check asserts only the PRESENCE of the opening `Trace Id: "` of the clause, not any particular Root, so no per-run value is being required.)
Model: AssociateFirewallRuleGroup errors include ValidationException, httpError 400 defaulted; no message text modeled.

Cause 2 — route53resolver.get-firewall-rule-group-policy-not-found [mismatch]
GetFirewallRuleGroupPolicy(Arn='arn:aws:route53resolver:<region>:<account>:firewall-rule-group/rslvr-frg-0000000000000000') on an id that cannot exist.
Expected (AWS): HTTP 400, Error.Code = ResourceNotFoundException, Error.Message starting '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-0000000000000000". Trace Id: "'
Actual (emulator): HTTP 400, Error.Code = ResourceNotFoundException (correct), Error.Message = '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-0000000000000000".' — the trailing clause is absent.
Replay excerpt:
  File "check_route53resolver_get_firewall_rule_group_policy_not_found.py", line 49, in run
    assert message.startswith(expected_prefix)
  AssertionError: expected message starting '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-0000000000000000". Trace Id: "', got '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-0000000000000000".'
The earlier half of the same check passed: GetFirewallRuleGroupPolicy on a freshly created rule group with no attached policy returned FirewallRuleGroupPolicy == "{}" as AWS does.
Model: GetFirewallRuleGroupPolicy(Arn: Arn required, length 1..255) errors AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400; ResourceNotFoundException{Message: String, ResourceType: String} models no text.

## Steps to Reproduce
Cause 1 (AssociateFirewallRuleGroup, RSLVR-02303):
1. ec2.create_vpc(CidrBlock='10.83.0.0/16') -> vpc_id.
2. Create two DNS Firewall rule groups: route53resolver.create_firewall_rule_group(CreatorRequestId=<unique>, Name=<unique>) twice -> group_a, group_b.
3. From two threads at once, call route53resolver.associate_firewall_rule_group(CreatorRequestId=<unique per call>, FirewallRuleGroupId=group_a|group_b, VpcId=vpc_id, Priority=733, Name=<unique per call>).
4. Exactly one call succeeds; capture the winner's FirewallRuleGroupAssociation.Id. Inspect the other call's ClientError: Code is ValidationException (correct) and the Message is '[RSLVR-02303] This priority is the same with another association: "<winner id>" . Try again with another priority number.' — AWS continues with ' Trace Id: "<Root>"'.
5. Optional, to pin the Root byte-exactly: register a boto3 before-sign hook that sets a fixed 'X-Amzn-Trace-Id: Root=1-<8 hex>-<24 hex>;Sampled=0' on every route53resolver request; AWS echoes that Root in the clause, the emulator emits no clause at all.

Cause 2 (GetFirewallRuleGroupPolicy, RSLVR-02025):
1. group = route53resolver.create_firewall_rule_group(CreatorRequestId=<unique>, Name=<unique>)['FirewallRuleGroup'].
2. route53resolver.get_firewall_rule_group_policy(Arn=group['Arn']) -> FirewallRuleGroupPolicy == '{}' (already correct).
3. route53resolver.get_firewall_rule_group_policy(Arn=f'arn:aws:route53resolver:{region}:{account}:firewall-rule-group/rslvr-frg-0000000000000000') with the caller's own region/account -> refused HTTP 400 ResourceNotFoundException (correct). Inspect Error.Message: the emulator returns '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-0000000000000000".' while AWS returns that sentence followed by ' Trace Id: "<Root>"'.

Replay: run_behavioral_suite and re-run BOTH route53resolver.concurrent-associate-firewall-rule-group-priority-conflict and route53resolver.get-firewall-rule-group-policy-not-found.
