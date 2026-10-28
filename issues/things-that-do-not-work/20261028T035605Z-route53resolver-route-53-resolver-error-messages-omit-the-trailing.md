Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.delete-firewall-rule-group-with-vpc-association-conflict
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.delete-firewall-rule-group-with-vpc-association-conflict|run#8|AssertionError|assert control_deleted_message == expected, f"expected message {expected!r}, got {control_deleted_message!r}"
Issue-Commit: 2afdbd6f9cc461f20a1852f9ae0fa205a10ef33d

# Route 53 Resolver error messages omit the trailing X-Ray `Trace Id: "<Root>"` clause, so every RSLVR-coded Message is short by its last field (GetFirewallRuleGroup ResourceNotFoundException, and the whole rslvr_message class)

> _Note: this issue was identified against the code base at commit `2afdbd6f9cc461f20a1852f9ae0fa205a10ef33d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFirewallRuleGroup

## Description
Cause 1 of 1: every Route 53 Resolver error Message the emulator builds ends after the RSLVR-coded sentence; real Route 53 Resolver appends a ` Trace Id: "<Root>"` clause carrying the X-Ray trace Root of the REQUEST that was refused.

AWS: Smithy for this service states no message text at all — ResourceNotFoundException{Message: String, ResourceType: String}, httpError 400, modeled on GetFirewallRuleGroup (and on DeleteFirewallRuleGroup together with ConflictException{Message: ExceptionMessage}), so the Message text is recorded behavior only. The recorded Route 53 Resolver response for a FirewallRuleGroupId that does not exist is HTTP 400, Error.Code `ResourceNotFoundException`, and Error.Message of the form:

    [RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupId>". Trace Id: "<Root>"

Message anatomy for this error class: FIXED text is `[RSLVR-02025] Can’t find the resource with ID "` … `". Trace Id: "` … `"` (note the service's U+2019 right-single-quote in `Can’t`, the double quotes around both values, and the period that ends the sentence BEFORE the Trace Id clause). VARIABLE parts are (a) the resource id taken verbatim from the request (`FirewallRuleGroupId` here) and (b) the X-Ray trace Root. The Root is NOT a value the service invents per run: it is taken from the caller's `X-Amzn-Trace-Id` request header (`Root=<root>;Sampled=0`), so a caller that pins that header gets a byte-exact message, which is exactly how the failing check pins it. Only when the caller sends no X-Amzn-Trace-Id does the service mint a Root itself. The same suffix is part of the ConflictException the same check expects later from DeleteFirewallRuleGroup on a VPC-associated group: `[RSLVR-02101] The DNS Firewall rule group is associated to a VPC: "<FirewallRuleGroupId>". Delete the association and try again later. Trace Id: "<Root>"`.

Emulator: the refusal is otherwise right — HTTP 400, Code `ResourceNotFoundException`, ResourceType set, the RSLVR-02025 prefix and the id present — but the message stops at the sentence's period: got `[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-910a2dec89025cc1".` where AWS returns `[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-910a2dec89025cc1". Trace Id: "1-6ae16e88-ff4aa3322da8497b848feb98"`. The defect is not local to one handler: the message is assembled by one shared formatter that has no notion of the trace clause and no handler passes the request's trace Root into it, so EVERY Resolver error of this family (resource-not-found for endpoints, rules, query-log configs, firewall domain lists and rule groups; the conflict/validation/invalid-parameter sentences; the ConflictException above) is short by the same final clause. Note: open issue 20261026T053226Z-route53resolver-2-net-new-causes-listfirewallconfigs-omits-the asserts in passing that the `Trace Id` suffix is per-request and therefore not a requirement — that is the judgement this issue corrects, because the Root is a request input the caller controls, not a per-run value the service generates; fixed issues 20261022T084101Z-route53resolver-resolver-endpoint-not-found-error-omits-rslvr-code and 20261025T225734Z-route53resolver-route-53-resolver-rule-error-messages-omit-the added the RSLVR code prefix only and neither touched the trailing clause, and neither left the call refused (the refusal here is the expected one), so this is a net-new message defect rather than a reopen.

Why: a client that matches, logs or asserts on Resolver's Message gets a string that no real Resolver response can produce. Concretely, the trace Root is how a caller correlates a Resolver refusal with its X-Ray trace and with a support case; against the emulator that correlation key is simply absent, so error-triage tooling silently loses it. Negative tests and SDK/CLI wrappers written against AWS's exact message (the pattern ends in `Trace Id: "..."`) fail against the emulator, and tests written against the emulator's short message fail in production. In this suite the omission aborts the check at its FIRST assertion, so the whole DNS-Firewall in-use-delete contract that follows (ConflictException RSLVR-02101, the group surviving the rejected delete, and the delete succeeding after disassociation) is never exercised at all.

Fix: the shared message formatter and the plumbing of the request's trace id into it, not the individual operation. `rslvr_message(code, sentence)` currently returns exactly `format!("[{code}] {sentence}.")`, and `resource_not_found_message`/`firewall_rule_group_not_found` build on it, so the Root must be threaded from the request (the `X-Amzn-Trace-Id` header, available on the handler's OperationContext request parts) through to that formatter — with a service-minted Root as the fallback when the header is absent — and appended as ` Trace Id: "<Root>"` after the sentence-ending period for every Resolver exception body that carries a Message. Handlers such as GetFirewallRuleGroup, which today call `crate::state::firewall_rule_group_not_found(&input.firewall_rule_group_id)` with nothing from `ctx` (it explicitly does `let _ = ctx;`), must pass the context in.

Traces: route53resolver.delete-firewall-rule-group-with-vpc-association-conflict — asserts the message on GetFirewallRuleGroup after a clean delete (fails there), and would then assert the same clause on DeleteFirewallRuleGroup's ConflictException and on the final post-delete GetFirewallRuleGroup. File/line citations: services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs:486-504 (`rslvr_message`, `resource_not_found_message`), :1195-1200 (`firewall_rule_group_not_found`), and services/service-com-amazonaws-route53resolver-route53resolver/src/ops/get_firewall_rule_group.rs:140-176 (handler discards `ctx`).

## Evidence
Cause 1 — route53resolver.delete-firewall-rule-group-with-vpc-association-conflict [mismatch]

The check registers a before-sign hook that sends a fixed `X-Amzn-Trace-Id: Root=<root>;Sampled=0` on every route53resolver call, so the expected message is byte-exact (check source: `_traced(client)` — "Route 53 Resolver ends a DNS Firewall refusal message with the caller's trace Root, so a fixed Root makes the whole message exact").

Expected (AWS), on GetFirewallRuleGroup for an id just deleted:
  HTTP 400, Error.Code = "ResourceNotFoundException"
  Error.Message = '[RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupId>". Trace Id: "<Root from the request's X-Amzn-Trace-Id>"'

Actual (emulator):
  HTTP 400, Error.Code = "ResourceNotFoundException" (correct), Error.Message = '[RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupId>".' — the ` Trace Id: "..."` clause is absent.

Replay excerpt:
  File "check_route53resolver_delete_firewall_rule_group_with_vpc_association_conflict.py", line 218, in run
    assert control_deleted_message == expected, f"expected message {expected!r}, got {control_deleted_message!r}"
  AssertionError: expected message '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-910a2dec89025cc1". Trace Id: "1-6ae16e88-ff4aa3322da8497b848feb98"', got '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-910a2dec89025cc1".'
  (the id and the Root above are this run's values filling the template, not part of the requirement)

Model: GetFirewallRuleGroup(FirewallRuleGroupId: ResourceId required, len 1..64) errors AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException, all HTTP 400; ResourceNotFoundException{Message: String, ResourceType: String} — no message text is modeled, so the wording is recorded behavior only. DeleteFirewallRuleGroup additionally models ConflictException{Message: ExceptionMessage} (HTTP 400), the refusal whose RSLVR-02101 message the same check asserts with the identical trailing clause.

Code: state.rs `pub fn rslvr_message(code: &str, sentence: &str) -> String { format!("[{code}] {sentence}.") }` — the formatter has no trace-id parameter; `resource_not_found_message(resource_id)` wraps it; `firewall_rule_group_not_found(id)` wraps that; `get_firewall_rule_group.rs` handler begins its body with `let _ = ctx;`, so the request's X-Amzn-Trace-Id header is never read by any Resolver handler.

## Steps to Reproduce
1. Create a boto3 route53resolver client and register a before-sign hook that sets a FIXED header on every request: `X-Amzn-Trace-Id: Root=1-<8 hex>-<24 hex>;Sampled=0`. Keep that Root value.
2. group_id = route53resolver.create_firewall_rule_group(CreatorRequestId=<unique>, Name=<unique>)["FirewallRuleGroup"]["Id"].
3. route53resolver.delete_firewall_rule_group(FirewallRuleGroupId=group_id) — succeeds (the group has no VPC association).
4. route53resolver.get_firewall_rule_group(FirewallRuleGroupId=group_id) — refused with HTTP 400 / ResourceNotFoundException (correct). Inspect Error.Message: the emulator returns '[RSLVR-02025] Can’t find the resource with ID "<group_id>".' while AWS returns that same sentence followed by ' Trace Id: "<the Root sent in step 1>"'.
5. Same omission on the conflict path the check reaches next: recreate a group, create a VPC, associate_firewall_rule_group(FirewallRuleGroupId=..., VpcId=..., Priority=101), poll get_firewall_rule_group_association until Status == COMPLETE, then delete_firewall_rule_group on the associated group — AWS answers ConflictException with '[RSLVR-02101] The DNS Firewall rule group is associated to a VPC: "<group_id>". Delete the association and try again later. Trace Id: "<Root>"'.
6. Also reproducible with no X-Amzn-Trace-Id header at all: AWS still ends the message with a ` Trace Id: "<service-minted Root>"` clause; the emulator never emits the clause.

Replay: run_behavioral_suite and re-run route53resolver.delete-firewall-rule-group-with-vpc-association-conflict.
