Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-resource-policy-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|DescribeResourcePolicy|2xx->2xx|$.Policy
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# PutResourcePolicy stores the IAM policy document verbatim instead of canonicalizing Principal, so DescribeResourcePolicy returns a bare account id where AWS returns the root ARN

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeResourcePolicy

## Description
ONE net-new root cause, one service (network-firewall). The service IS serving — PutResourcePolicy and DescribeResourcePolicy both answer HTTP 200 in this same fixture, and 34 of 38 replayed steps pass — so this is a response-fidelity gap, not a bootstrap gap.

Cause 1 of 1: the resource policy document a client PUTs is persisted and echoed back byte-for-byte, so the IAM Principal canonicalization AWS performs on store never happens.

AWS: Smithy (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json) models PutResourcePolicy with required members ResourceArn (ResourceArn, len 1..256, pattern ^arn:aws) and Policy (PolicyString, len 1..395000, pattern \S), documented as "The IAM policy statement that lists the accounts that you want to share your Network Firewall resources with and the operations that you want the accounts to be able to perform", and DescribeResourcePolicy returning Policy (PolicyString) — "The IAM policy statement that lists the accounts that you want to share your Network Firewall resources with". The model does not describe the document's internal grammar, so the normalization rule is recorded behavior only, and it is the standard IAM resource-policy rule: a Statement[*].Principal.AWS value given as a bare 12-digit account id is canonicalized on store to that account's root principal ARN, arn:aws:iam::<accountId>:root (partition taken from the resource's partition). DescribeResourcePolicy therefore returns the CANONICALIZED document, not the literal bytes that were PUT: the Principal.AWS value of a statement whose principal was supplied as an account id reads back as arn:aws:iam::<accountId>:root. Everything else in the document (Version, Effect, Action list ordering, Resource) is returned as supplied. In the recorded step the PUT document named the principal by account id and the readback carried "Principal":{"AWS":"arn:aws:iam::<accountId>:root"}; the account id 000000000000, the region us-west-2, the rule-group name and the stateful-rulegroup ARN in that recording are environment-specific values that fill the template and are NOT part of the requirement.

Emulator: PutResourcePolicy inserts the request's Policy string unchanged into the resource-policy store (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/put_resource_policy.rs: state.resource_policies.insert(input.resource_arn, input.policy)) and DescribeResourcePolicy clones it straight back out (src/ops/describe_resource_policy.rs: Ok(DescribeResourcePolicyOutput { policy: Some(policy.clone()) })). No parse, no canonicalization. The replayed readback answers HTTP 200 — the correct status — but $.Policy differs from AWS: the emulator returns "Principal":{"AWS":"000000000000"} where AWS returns "Principal":{"AWS":"arn:aws:iam::000000000000:root"}. The divergence is confined to that one member of the JSON document; Version, Statement[0].Effect, the Action list and Resource all match.

Why: a client that shares a Network Firewall rule group or firewall policy and then reads the policy back to confirm or to amend it gets a document that is not the one AWS would have stored. Code that compares the readback against a canonical expected document (IaC drift detection, policy-diff tooling, idempotency checks that re-PUT only on change) sees a permanent spurious difference against the emulator and either loops re-PUTting or reports drift that does not exist. Code that parses the readback and keys on Principal.AWS being an ARN — splitting out the account id, matching it against a principal allow-list, or re-emitting the statement into a merged document — mis-handles the bare numeric form, since on AWS that form never appears on the way out even though it is accepted on the way in. The asymmetry (accepted on write, canonical on read) is exactly the part the emulator drops.

Fix: the resource-policy store and its write path — PutResourcePolicy's handler must parse the submitted document and canonicalize each Statement[*].Principal.AWS entry (bare 12-digit account id -> arn:aws:iam::<accountId>:root, in the partition of the target ResourceArn; already-ARN principals, "*", and service principals pass through unchanged) BEFORE inserting it, so DescribeResourcePolicy's verbatim echo is correct for every reader. Equivalently the canonicalization may live in the state module alongside shared_resource_kind / resource_policy_matches_permissions, which already parse the document for action validation and so already hold the parsed form. Note that the existing open issues "Network Firewall does not track policy->rule-group associations, and the DisassociateSubnets / SubnetChangeProtection / UpdateLoggingConfiguration cont..." and "A READY firewall's SyncStates endpoints are not registered as EC2 VPC endpoints..." do NOT cover this: neither mentions PutResourcePolicy, DescribeResourcePolicy, or the resource-policy document, so this cause must be closed on its own.

Traces: network-firewall-resource-policy-lifecycle#task#4 (DescribeResourcePolicy, 200 vs 200, $.Policy value_mismatch on Statement[0].Principal.AWS).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up true, /__admin/health 200; 38 steps total, 34 passed, 4 mismatched, score 89.5%; bootstrap_signal false).

Divergence cluster (verdict mismatch, divergence_key "service=network-firewall|DescribeResourcePolicy|status=200->200|keys=$.Policy", affected_count 1, stub_path empty, owned_by empty):
  fixture_id: network-firewall-resource-policy-lifecycle
  step_key:   task#4
  operation:  DescribeResourcePolicy
  expected_status 200, actual_status 200 — the status and the error channel are correct; only the body member $.Policy diverges.

  Expected $.Policy (AWS):
    {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["network-firewall:CreateFirewallPolicy","network-firewall:ListRuleGroups"],"Resource":"arn:aws:network-firewall:us-west-2:000000000000:stateful-rulegroup/sim2real-probe-network-firewall-nfw-respolicy-rg-03373758"}]}

  Actual $.Policy (emulator):
    {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"000000000000"},"Action":["network-firewall:CreateFirewallPolicy","network-firewall:ListRuleGroups"],"Resource":"arn:aws:network-firewall:us-west-2:000000000000:stateful-rulegroup/sim2real-probe-network-firewall-nfw-respolicy-rg-03373758"}]}

  Fixed text of the requirement: Principal.AWS on readback is the root-principal ARN form "arn:aws:iam::<accountId>:root". Variable parts taken from the request/state and NOT part of the requirement: the account id (000000000000 here), the partition and region, and the generated rule-group name / stateful-rulegroup ARN (sim2real-probe-network-firewall-nfw-respolicy-rg-03373758). The Action list and Resource match AWS exactly and are not at issue.

Code read (this checkout):
  services/.../src/ops/put_resource_policy.rs:260-262 — state.resource_policies.insert(input.resource_arn, input.policy): the raw request string is stored.
  services/.../src/ops/describe_resource_policy.rs:171-173 — Ok(DescribeResourcePolicyOutput { policy: Some(policy.clone()) }): the raw stored string is returned.

Smithy consulted: PutResourcePolicy (required ResourceArn pattern ^arn:aws len 1..256, required Policy -> PolicyString len 1..395000 pattern \S; errors InternalServerError 500, InvalidRequestException/InvalidResourcePolicyException/ResourceNotFoundException/ThrottlingException 400), DescribeResourcePolicy (output Policy -> PolicyString; errors InternalServerError 500, InvalidRequestException/ResourceNotFoundException/ThrottlingException 400).

## Steps to Reproduce
1. Create a stateful rule group so there is a shareable resource:
   aws network-firewall create-rule-group --rule-group-name rg1 --type STATEFUL --capacity 10 --rule-group '{"RulesSource":{"StatefulRules":[{"Action":"ALERT","Header":{"Protocol":"TCP","Source":"ANY","SourcePort":"ANY","Direction":"ANY","Destination":"ANY","DestinationPort":"ANY"},"RuleOptions":[{"Keyword":"sid:1"}]}]}}'  -> note RuleGroupArn.
2. Attach a resource policy whose Principal.AWS is a BARE 12-digit account id (not an ARN):
   aws network-firewall put-resource-policy --resource-arn <RuleGroupArn> --policy '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"<accountId>"},"Action":["network-firewall:CreateFirewallPolicy","network-firewall:ListRuleGroups"],"Resource":"<RuleGroupArn>"}]}'  -> 200, empty body.
3. Read it back:
   aws network-firewall describe-resource-policy --resource-arn <RuleGroupArn>
   Emulator: Policy contains "Principal":{"AWS":"<accountId>"} — the literal bytes submitted.
   AWS: Policy contains "Principal":{"AWS":"arn:aws:iam::<accountId>:root"}; Version, Effect, the Action list and Resource are returned as submitted.
4. Control that must keep passing after the fix: re-run step 2 with Principal.AWS already given as "arn:aws:iam::<accountId>:root" and confirm the readback is byte-identical to the submitted document (an already-canonical principal must not be rewritten), and that "*" as a principal is likewise passed through unchanged.
5. Re-replay the wire fixture to confirm: fixture network-firewall-resource-policy-lifecycle, step task#4.
