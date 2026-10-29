Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.put-firewall-rule-group-policy-self-share-rejected route53resolver.firewall-rule-group-policy-lifecycle
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.put-firewall-rule-group-policy-self-share-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.firewall-rule-group-policy-lifecycle|run#12|AssertionError|assert message == _invalid_policy_message(self_share), message
Issue-Commit: fe80e3f2d2429a9c3f66590c6e5c47583e342d88

# PutFirewallRuleGroupPolicy's RSLVR-02035 ValidationException message is truncated: the remediation sentence "Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account." is missing

> _Note: this issue was identified against the code base at commit `fe80e3f2d2429a9c3f66590c6e5c47583e342d88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFirewallRuleGroupPolicy

## Description
Cause 1 of 1: the ValidationException Route 53 Resolver answers when it refuses a DNS Firewall rule group resource policy carries a TWO-sentence Message; the emulator emits only the first sentence, so every RSLVR-02035 refusal from PutFirewallRuleGroupPolicy is short by its remediation clause. Failing checks: route53resolver.put-firewall-rule-group-policy-self-share-rejected, route53resolver.firewall-rule-group-policy-lifecycle.

AWS: Smithy models PutFirewallRuleGroupPolicy(Arn: Arn required @length 1..255, FirewallRuleGroupPolicy: FirewallRuleGroupPolicy required @length 0..30000) -> {ReturnValue: Boolean}, errors AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400 (defaulted). ValidationException carries only {Message}; the model states NO message text, so the wording is recorded behavior only. The recorded Route 53 Resolver response for a policy document it will not attach (a document that shares the group with nobody but the group's own owner account, and a structurally incomplete document such as '{"Version":"2012-10-17"}' with no Statement) is HTTP 400, Error.Code `ValidationException`, Error.Message of the form:

    [RSLVR-02035] The specified IAM policy isn't valid: "<policy document>". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account.

Message anatomy for this error class: FIXED text is `[RSLVR-02035] The specified IAM policy isn't valid: "` … `". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account.` — note the ASCII apostrophe in `isn't`, the double quotes around the document, the period ending the first sentence, the single space, and the second sentence ending in its own period. The ONLY VARIABLE part is the policy document, echoed back byte-for-byte exactly as the request's `FirewallRuleGroupPolicy` member supplied it (no re-serialization, no whitespace normalisation). AWS then appends the per-request X-Ray ` Trace Id: "<Root>"` clause; both failing checks strip that suffix before asserting, so the trace clause is NOT what this issue is about — see open issue 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md, which owns the missing trace clause and does NOT cover this truncation (its assertion is on RSLVR-02025/RSLVR-02101 from Get/DeleteFirewallRuleGroup, and even with the trace clause added this message would still be missing a whole sentence).

Emulator: the refusal itself is right — HTTP 400, Error.Code `ValidationException`, the RSLVR-02035 prefix present, the submitted document echoed verbatim, and no policy persisted (the subsequent GetFirewallRuleGroupPolicy still returns `{}`) — but the Message stops at the first sentence's period. Replayed: expected `[RSLVR-02035] The specified IAM policy isn't valid: "<doc>". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account.`, got `[RSLVR-02035] The specified IAM policy isn't valid: "<doc>".` The defect is in the shared message builder, not in one handler: `invalid_iam_policy(policy)` composes the message through `rslvr_message(code, sentence)` (which hard-codes `format!("[{code}] {sentence}.")`, i.e. exactly one sentence) and so BOTH refusal paths in the handler — the grammar check and the shares-with-another-account check — emit the same truncated text.

Why: a client that matches, logs or asserts on the Resolver refusal gets a string no real Resolver response can produce. The dropped sentence is the only part of the message that states what a valid document must do (share the rule group with ANOTHER AWS account), so a caller debugging a rejected share loses the service's entire remediation hint, and operators' log-pattern and negative-test matchers written against AWS's wording fail against the emulator while ones written against the emulator fail in production. In this suite the truncation also aborts both checks at this assertion, so the rest of the rule-group policy contract they cover (the structurally-invalid-document rejection, the "a rejected Put persists nothing" invariant re-read, and the RSLVR-02025 ResourceNotFoundException for an unknown rule-group ARN) is never exercised at all.

Fix: the shared Route 53 Resolver message construction for this error class, not the operation's state or validation logic (the classification, status, code and the no-partial-state behaviour are already correct). The remediation sentence belongs in the builder that produces this exception body so both refusal paths of the handler get it, and the one-sentence assumption baked into the generic formatter must not force the second sentence to be dropped. File/line citations: services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs:1033-1040 (`invalid_iam_policy`, the truncated body), :503-505 (`rslvr_message`, the single-sentence formatter), and services/service-com-amazonaws-route53resolver-route53resolver/src/ops/put_firewall_rule_group_policy.rs:168-173 and :197-205 (the two refusal paths that call it).

Traces: route53resolver.put-firewall-rule-group-policy-self-share-rejected, route53resolver.firewall-rule-group-policy-lifecycle.

## Evidence
Cause 1 — two failing checks, same assertion on the same message.

[mismatch] route53resolver.put-firewall-rule-group-policy-self-share-rejected
  check_route53resolver_put_firewall_rule_group_policy_self_share_rejected.py, line 66, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message '[RSLVR-02035] The specified IAM policy isn\'t valid: "{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"route53resolver:GetFirewallRuleGroup","Resource":"arn:aws:route53resolver:<region>:<account>:firewall-rule-group/<group-id>"}]}". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account.', got '[RSLVR-02035] The specified IAM policy isn\'t valid: "{...same document...}".'
  Error.Code was ValidationException (correct) and the pre/post GetFirewallRuleGroupPolicy reads of `{}` passed — only the trailing sentence diverges. (The account id, region and rslvr-frg id above are this run's values filling the template, not part of the requirement.)

[mismatch] route53resolver.firewall-rule-group-policy-lifecycle
  check_route53resolver_firewall_rule_group_policy_lifecycle.py, line 59, in run
    assert message == _invalid_policy_message(self_share), message
  AssertionError: [RSLVR-02035] The specified IAM policy isn't valid: "{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["<account>"]},"Action":["route53resolver:GetFirewallRuleGroup","route53resolver:AssociateFirewallRuleGroup","route53resolver:DisassociateFirewallRuleGroup","route53resolver:ListFirewallRuleGroups"],"Resource":["arn:aws:route53resolver:<region>:<account>:firewall-rule-group/<group-id>"]}]}".
  The asserted-against value is the check's own helper:
    _invalid_policy_message(policy) = f'[RSLVR-02035] The specified IAM policy isn\'t valid: "{policy}". Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account.'
  so the emulator's actual Message above is the expected one minus its second sentence. Both checks route the message through a helper that first strips the ' Trace Id: "<Root>"' suffix, so the missing trace clause cannot account for this difference.

Model: PutFirewallRuleGroupPolicy errors = AccessDeniedException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException | ValidationException (all HTTP 400); ValidationException{Message} carries no modeled text, so the wording is recorded behavior only. (GetFirewallRuleGroupPolicy models the same error set and correctly returns `{}` for a group with no policy — that part passes.)

Code: state.rs `pub fn invalid_iam_policy(policy: &str) -> ValidationException { ValidationException { message: Some(rslvr_message(RSLVR_CODE_INVALID_IAM_POLICY, &format!("The specified IAM policy isn't valid: \"{policy}\""))) } }` and `pub fn rslvr_message(code: &str, sentence: &str) -> String { format!("[{code}] {sentence}.") }` — one sentence is all the formatter can emit, and no caller adds the remediation sentence. put_firewall_rule_group_policy.rs returns `ValidationException(crate::state::invalid_iam_policy(...))` from both its grammar check (line 168) and its `policy_shares_with_other_account` check (line 197), so both refusal classes are truncated identically.

## Steps to Reproduce
1. r53 = boto3 client("route53resolver"); account_id = sts.get_caller_identity()["Account"].
2. group = r53.create_firewall_rule_group(CreatorRequestId=<unique>, Name=<unique>)["FirewallRuleGroup"]; arn = group["Arn"].
3. r53.get_firewall_rule_group_policy(Arn=arn)["FirewallRuleGroupPolicy"] -> "{}" (correct today).
4. self_policy = '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::' + account_id + ':root"},"Action":"route53resolver:GetFirewallRuleGroup","Resource":"' + arn + '"}]}'  (a same-account share, which Resolver refuses).
5. r53.put_firewall_rule_group_policy(Arn=arn, FirewallRuleGroupPolicy=self_policy) — refused with HTTP 400 / ValidationException (correct). Inspect Error.Message with any trailing ' Trace Id: "..."' clause stripped: the emulator returns '[RSLVR-02035] The specified IAM policy isn\'t valid: "<self_policy>".' while AWS returns that sentence followed by ' Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS account.'
6. Same truncation on the grammar path: r53.put_firewall_rule_group_policy(Arn=arn, FirewallRuleGroupPolicy='{"Version":"2012-10-17"}') — again ValidationException with the first sentence only.
7. r53.get_firewall_rule_group_policy(Arn=arn) still returns "{}" (no partial state — that part is correct).

Replay: run_behavioral_suite and re-run route53resolver.put-firewall-rule-group-policy-self-share-rejected and route53resolver.firewall-rule-group-policy-lifecycle.
