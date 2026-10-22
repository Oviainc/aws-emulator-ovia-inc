Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-firewall.rule-group-capacity-tracked-on-create network-firewall.resource-policy-missing-required-action-rejected
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall.rule-group-capacity-tracked-on-create|run#6|AssertionError|assert code == "InvalidRequestException", f"expected a capacity-rejection error, got {code}"
Issue-Key: network-firewall.resource-policy-missing-required-action-rejected|run#10|AssertionError|assert code == "InvalidResourcePolicyException", f"expected InvalidResourcePolicyException, got {code}"
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Network Firewall rule-group capacity accounting/DryRun and resource-policy statement validation are not served (2 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRuleGroup

## Description
Two distinct Network Firewall behaviors are missing. The service IS standing up (CreateFirewallPolicy in the same checks succeeds), but these two request paths answer the scaffold's NotImplemented instead of the modeled semantics. NOTE: separate scaffold-stub issues exist for merely standing up `CreateRuleGroup` (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/create_rule_group.rs) and `PutResourcePolicy` (.../src/ops/put_resource_policy.rs). Those cover existence only; the causes below are the request-validation and state-accounting semantics those handlers must carry once they exist, and they are what these two checks assert.

Cause 1 of 2: CreateRuleGroup does not validate or track rule-group capacity, and does not honour DryRun.
AWS: Smithy (com-amazonaws-networkfirewall-networkfirewall-20201112) models CreateRuleGroup with required `Capacity` (RuleCapacity, integer) and required `Type` (RuleGroupType), an optional `DryRun` boolean with default false, and the client error `InvalidRequestException` (httpError 400). A rule group's capacity is fixed at creation and, per AWS documentation, a STATEFUL rule group needs at least one capacity unit per individual rule, so a `Capacity` below the number of rules in `RuleGroup.RulesSource.StatefulRules` is rejected with `InvalidRequestException` (HTTP 400) — the recording shows the message `StatefulRules capacity exceeded, parameter: [2], context: RulesSource.StatefulRules` as an example; the requirement is the status/code plus the fact that the rejection names the exceeded capacity and the offending context. With a sufficient `Capacity`, `DryRun=True` must succeed and echo the requested `Capacity` in `RuleGroupResponse.Capacity` while persisting nothing — a following DescribeRuleGroup for that name must fail `ResourceNotFoundException` (modeled on DescribeRuleGroup, 400). A real (non-dry-run) create must then be describable with `RuleGroupResponse.ConsumedCapacity` (Smithy doc: "The number of capacity units currently consumed by the rule group rules") equal to the rule count and never greater than the declared `Capacity`, and `RuleGroup.RulesSource.StatefulRules` must round-trip both rules.
Emulator: every CreateRuleGroup call — including the DryRun one — is answered with error code `NotImplemented` ("operation CreateRuleGroup is scaffolded but not implemented in this emulator"), so no capacity rejection, no DryRun no-op, and no ConsumedCapacity accounting exists.
Why: a client cannot size a rule group. The capacity pre-flight (`DryRun=True`) that callers use to compute required units returns an error instead of an estimate, an under-capacity group is never refused so a caller believes an impossible group was accepted, and ConsumedCapacity — the value used to decide whether more rules fit — is unavailable. Any Network Firewall provisioning flow stops at step one.
Fix: the CreateRuleGroup handler plus its request validation (capacity-vs-rule-count check, DryRun short-circuit before any state mutation) and the rule-group state store that must record Capacity/ConsumedCapacity for DescribeRuleGroup to report.
Traces: network-firewall.rule-group-capacity-tracked-on-create.

Cause 2 of 2: PutResourcePolicy does not validate the policy statement, so an out-of-set action is accepted-path-errored instead of rejected with InvalidResourcePolicyException, and DescribeResourcePolicy cannot report policy absence.
AWS: Smithy models PutResourcePolicy (required `ResourceArn` pattern ^arn:aws, required `Policy` PolicyString) with the client error `InvalidResourcePolicyException` (httpError 400, Smithy doc: "The policy statement failed validation."). For a firewall-policy resource AWS permits only the fixed action set `network-firewall:AssociateFirewallPolicy` and `network-firewall:ListFirewallPolicies`; a statement naming any other action (here `network-firewall:DeleteFirewallPolicy`) is rejected with `InvalidResourcePolicyException` (HTTP 400) and nothing is attached — the recorded message `The supplied policy does not match RAM managed permissions.` is an example of that rejection. With nothing attached, DescribeResourcePolicy for that ARN must fail its modeled `ResourceNotFoundException` (400; recorded message example "No object with the provided name or resource identifier was found"). A statement limited to the allowed action set must succeed, and DescribeResourcePolicy must then return that document in `Policy`.
Emulator: PutResourcePolicy answers error code `NotImplemented` for both the invalid and the valid document, so the fixed-action-set validation never runs and no resource policy can ever be attached or described.
Why: a caller sharing a firewall policy cross-account gets the wrong verdict in both directions — an over-permissive policy document is not refused with the code the caller branches on (`InvalidResourcePolicyException`), and a correct document cannot be stored, so every resource-policy read-back and the RAM-style sharing flow built on it fails.
Fix: the PutResourcePolicy handler, its policy-document validation (allowed-action set per resource type, emitting InvalidResourcePolicyException), and the resource-policy state that DescribeResourcePolicy/DeleteResourcePolicy read, including the not-found path when no policy is attached.
Traces: network-firewall.resource-policy-missing-required-action-rejected.

## Evidence
Cause 1 — check network-firewall.rule-group-capacity-tracked-on-create:
  expected: CreateRuleGroup(RuleGroupName=..., Type=STATEFUL, Capacity=1, RuleGroup={2 StatefulRules}, DryRun=True) -> ClientError code `InvalidRequestException` (HTTP 400), e.g. message "StatefulRules capacity exceeded, parameter: [2], context: RulesSource.StatefulRules"; then Capacity=2 DryRun=True -> RuleGroupResponse.Capacity == 2 and DescribeRuleGroup(name) -> `ResourceNotFoundException`; then real create -> DescribeRuleGroup gives ConsumedCapacity == 2 <= Capacity and 2 StatefulRules round-tripped.
  actual: AssertionError: expected a capacity-rejection error, got NotImplemented
          (botocore error code `NotImplemented`: "operation CreateRuleGroup is scaffolded but not implemented in this emulator")

Cause 2 — check network-firewall.resource-policy-missing-required-action-rejected:
  setup CreateFirewallPolicy SUCCEEDS, so the service is serving.
  expected: PutResourcePolicy(ResourceArn=<firewall policy arn>, Policy={Action:["network-firewall:DeleteFirewallPolicy"]}) -> ClientError code `InvalidResourcePolicyException` (HTTP 400), e.g. message "The supplied policy does not match RAM managed permissions."; then DescribeResourcePolicy -> `ResourceNotFoundException`; then PutResourcePolicy with Action [AssociateFirewallPolicy, ListFirewallPolicies] -> success and DescribeResourcePolicy returns that document.
  actual: AssertionError: expected InvalidResourcePolicyException, got NotImplemented
          (botocore error code `NotImplemented` from put_resource_policy)

Smithy consulted: CreateRuleGroup (required Capacity/Type, DryRun default false, errors InvalidRequestException/LimitExceededException/InsufficientCapacityException); DescribeRuleGroup (errors ResourceNotFoundException); RuleGroupResponse.ConsumedCapacity doc "The number of capacity units currently consumed by the rule group rules"; PutResourcePolicy (errors InvalidResourcePolicyException/ResourceNotFoundException/InvalidRequestException); InvalidResourcePolicyException doc "The policy statement failed validation."

## Steps to Reproduce
Cause 1:
1. aws network-firewall create-rule-group --rule-group-name rg-under --type STATEFUL --capacity 1 --dry-run --rule-group '{"RulesSource":{"StatefulRules":[{"Action":"DROP","Header":{"Protocol":"TCP","Source":"ANY","SourcePort":"ANY","Direction":"FORWARD","Destination":"192.0.2.0/24","DestinationPort":"445"},"RuleOptions":[{"Keyword":"sid:1"}]},{"Action":"ALERT","Header":{"Protocol":"UDP","Source":"ANY","SourcePort":"ANY","Direction":"ANY","Destination":"198.51.100.0/24","DestinationPort":"53"},"RuleOptions":[{"Keyword":"sid:2"}]}]}}'
   -> observe NotImplemented; AWS answers InvalidRequestException (400) for capacity 1 < 2 rules.
2. Repeat with --capacity 2 --dry-run: AWS returns RuleGroupResponse.Capacity=2 and persists nothing (DescribeRuleGroup -> ResourceNotFoundException).
3. Repeat with --capacity 2 (no --dry-run), then `aws network-firewall describe-rule-group --rule-group-arn <arn>`: AWS reports ConsumedCapacity=2 <= Capacity=2 and both StatefulRules.

Cause 2:
1. aws network-firewall create-firewall-policy --firewall-policy-name p1 --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"]}'  (succeeds)
2. aws network-firewall put-resource-policy --resource-arn <policy arn> --policy '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"<account>"},"Action":["network-firewall:DeleteFirewallPolicy"],"Resource":"<policy arn>"}]}'
   -> observe NotImplemented; AWS answers InvalidResourcePolicyException (400) and attaches nothing.
3. aws network-firewall describe-resource-policy --resource-arn <policy arn> -> AWS answers ResourceNotFoundException (400).
4. Repeat step 2 with Action ["network-firewall:AssociateFirewallPolicy","network-firewall:ListFirewallPolicies"] -> AWS succeeds and step 3 then returns the document.
