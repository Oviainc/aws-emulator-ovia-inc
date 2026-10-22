Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the networkfirewall emulator

_Filed: 2026-10-21T22:08:47Z_
_Operation: service-com-amazonaws-networkfirewall-networkfirewall-20201112 | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 85 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds zero state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the firewall-policy + firewall lifecycle, which is the service's primary resource and the create -> use -> read -> list -> delete loop every Network Firewall client begins with.

Wire protocol (from OPERATIONS.md): awsJson1_0, `POST /` with `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, JSON body using modeled member names.

Happy path, in order (6 operations):

1. **CreateFirewallPolicy** — in: `FirewallPolicyName` (required, 1..128, `^[a-zA-Z0-9-]+$`), `FirewallPolicy` (required), `Description`, `Tags`, `DryRun` (default false), `EncryptionConfiguration`. Out: `UpdateToken` (required, must match `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`) and `FirewallPolicyResponse` (required) carrying the required `FirewallPolicyName`, `FirewallPolicyArn` (`arn:aws:network-firewall:<region>:<account>:firewall-policy/<name>`), `FirewallPolicyId` (36-char UUID form), plus `Description`, `Tags`, `FirewallPolicyStatus` (ACTIVE), `NumberOfAssociations` (0 at creation), `LastModifiedTime`. Must store the policy; a second create with the same name must not silently overwrite-and-return a fresh ARN. `DryRun=true` must NOT persist anything.

2. **CreateFirewall** — in: `FirewallName` (required), `FirewallPolicyArn` (required, `^arn:aws`), `VpcId`, `SubnetMappings`, `Description`, `Tags`, `DeleteProtection`/`SubnetChangeProtection`/`FirewallPolicyChangeProtection` (booleans, default false). Out: `Firewall` and `FirewallStatus`. The returned `Firewall` must be built from the stored record: `FirewallName`, `FirewallArn` (`arn:aws:network-firewall:<region>:<account>:firewall/<name>`), required `FirewallPolicyArn`, required `VpcId`, required `SubnetMappings`, required `FirewallId` (UUID), the three protection booleans, `Description`, `Tags`. `FirewallStatus` must carry required `Status` (FirewallStatusValue: PROVISIONING then READY) and required `ConfigurationSyncStateSummary` (ConfigurationSyncState: IN_SYNC), plus one `SyncStates` entry per availability zone derived from `SubnetMappings`. Referencing a `FirewallPolicyArn` that was never created must fail with `InvalidRequestException` (400), not succeed; creating the policy must bump that policy's `NumberOfAssociations`.

3. **DescribeFirewall** — in: `FirewallName` OR `FirewallArn` (both optional in the model; resolve by either, and reject a request that supplies neither with `InvalidRequestException`). Out: `UpdateToken`, `Firewall`, `FirewallStatus` — all read back from the stored record and byte-for-byte consistent with what CreateFirewall returned (same `FirewallId`, `FirewallArn`, `Description`, `Tags`, protection flags). Unknown name/ARN must return `ResourceNotFoundException` (400).

4. **UpdateFirewallDescription** — in: `UpdateToken`, `FirewallArn`/`FirewallName`, `Description`. Out: `FirewallArn`, `FirewallName`, `Description`, `UpdateToken`. Must mutate the stored firewall so a following DescribeFirewall shows the NEW description, and must return a NEW `UpdateToken`. A stale/mismatched `UpdateToken` must fail with `InvalidTokenException` (400); an unknown firewall with `ResourceNotFoundException`.

5. **ListFirewalls** — `@paginated(inputToken=NextToken, outputToken=NextToken, items=Firewalls)`; in: `NextToken` (1..4096, `^[0-9A-Za-z:\/+=]+$`), `MaxResults` (1..100), `VpcIds`. Out: `Firewalls` (list<FirewallMetadata> of `FirewallName` + `FirewallArn`) and `NextToken`. Must enumerate every created-and-not-deleted firewall, honour `MaxResults` as a real page bound, return a `NextToken` only when more remain, resume exactly from it, and filter by `VpcIds` when supplied.

6. **DeleteFirewall** then **DeleteFirewallPolicy** — DeleteFirewall in: `FirewallName`/`FirewallArn`; out: `Firewall` + `FirewallStatus` with `Status=DELETING`. It must remove the firewall from state so DescribeFirewall afterwards returns `ResourceNotFoundException` and ListFirewalls no longer lists it, and must decrement the policy's `NumberOfAssociations`. A firewall with `DeleteProtection=true` must fail with `InvalidOperationException` (400). DeleteFirewallPolicy out: `FirewallPolicyResponse` (required) with `FirewallPolicyStatus=DELETING`; deleting a policy that still has associations must fail with `InvalidOperationException` (400), and after success DescribeFirewallPolicy/ListFirewallPolicies must no longer see it.

Every one of these six MUST compute its response from persisted state. No canned bodies, no echoing the input back, and no generating an ARN/ID that is never stored — a create followed by a describe must return exactly what was created, and a delete must be observable by the next reader. Server-generated values (`FirewallId`, `FirewallPolicyId`, `UpdateToken`) must satisfy the modeled patterns and must be STABLE across reads of the same resource (the UpdateToken changes only on mutation).

## State Model

Service state lives in `ComAmazonawsNetworkfirewallNetworkfirewall20201112State` (`src/state.rs`), reached via `crate::state::service_state(&*self.engine)` and mutated under its lock. Concrete structures:

- `firewalls: HashMap<String /* FirewallName */, FirewallRecord>` where
  `FirewallRecord { firewall_name: String, firewall_arn: String, firewall_id: String /* uuid */, firewall_policy_arn: String, vpc_id: String, subnet_mappings: Vec<SubnetMapping>, delete_protection: bool, subnet_change_protection: bool, firewall_policy_change_protection: bool, description: Option<String>, tags: Vec<Tag>, encryption_configuration: Option<EncryptionConfiguration>, enabled_analysis_types: Vec<EnabledAnalysisType>, status: FirewallStatusValue, config_sync_state: ConfigurationSyncState, sync_states: HashMap<String /* AZ */, SyncState>, update_token: String /* uuid */, created_at: SystemTime }`
- `firewall_arn_index: HashMap<String /* FirewallArn */, String /* FirewallName */>` so DescribeFirewall/DeleteFirewall/UpdateFirewallDescription resolve by ARN or name.
- `firewall_policies: HashMap<String /* FirewallPolicyName */, FirewallPolicyRecord>` where
  `FirewallPolicyRecord { policy_name: String, policy_arn: String, policy_id: String /* uuid */, policy: FirewallPolicy /* the stored rule-group references and default actions */, description: Option<String>, tags: Vec<Tag>, encryption_configuration: Option<EncryptionConfiguration>, status: ResourceStatus, number_of_associations: u32, consumed_stateless_rule_capacity: i32, consumed_stateful_rule_capacity: i32, update_token: String, last_modified_time: SystemTime }`
- `firewall_policy_arn_index: HashMap<String /* FirewallPolicyArn */, String /* name */>`.
- ARN construction helper using the request's region/account from `OperationContext` so ARNs are consistent across operations and round-trip through the `^arn:aws` pattern.

Everything else (rule groups, TLS inspection configurations, proxies, VPC endpoint associations, flow operations, analysis reports, tagging, resource policies, transit-gateway attachments) stays scaffolded at 501 for now and will be filed as individual gap issues later.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/` matches all 85 operation files, including `create_firewall.rs`, `create_firewall_policy.rs`, `describe_firewall.rs`, `update_firewall_description.rs`, `list_firewalls.rs`, `delete_firewall.rs` and `delete_firewall_policy.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501 and persists nothing.

Contract fields and errors above come from the crate's generated `OPERATIONS.md` (CreateFirewall lines 63-67, CreateFirewallPolicy 69-73, DeleteFirewall 123-127, DeleteFirewallPolicy 129-133, DescribeFirewall 195-199, DescribeFirewallPolicy 207-211, ListFirewallPolicies 324-329, ListFirewalls 331-336, UpdateFirewallDescription 467-471) and from the Smithy model shapes `Firewall`, `FirewallStatus`, `FirewallPolicyResponse`, `FirewallMetadata`, `FirewallPolicyMetadata` (required members `FirewallPolicyArn`, `VpcId`, `SubnetMappings`, `FirewallId`; `FirewallStatus.Status` + `ConfigurationSyncStateSummary` required; `FirewallPolicyResponse.NumberOfAssociations` documented as 'the number of firewalls that are associated with this firewall policy'). Wire protocol (awsJson1_0, `X-Amz-Target: NetworkFirewall_20201112.*`) from OPERATIONS.md lines 8-15 and README.md.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI network-firewall client at it.
2. CreateFirewallPolicy with FirewallPolicyName=test-policy and a minimal FirewallPolicy (StatelessDefaultActions=[aws:forward_to_sfe], StatelessFragmentDefaultActions=[aws:forward_to_sfe]) -> expect 200 with a FirewallPolicyResponse containing an ARN ending in firewall-policy/test-policy, a 36-char FirewallPolicyId, NumberOfAssociations=0, and an UpdateToken matching the modeled UUID pattern (currently 501).
3. CreateFirewall with FirewallName=test-fw, FirewallPolicyArn from step 2, VpcId=vpc-0123abcd, SubnetMappings=[{SubnetId: subnet-0123abcd}] -> expect 200 with Firewall{FirewallArn, FirewallId, FirewallPolicyArn, VpcId, SubnetMappings} and FirewallStatus{Status, ConfigurationSyncStateSummary}.
4. CreateFirewall with a FirewallPolicyArn that was never created -> expect InvalidRequestException (HTTP 400), not a success.
5. DescribeFirewall with FirewallName=test-fw -> expect the SAME FirewallId/FirewallArn/VpcId/SubnetMappings returned in step 3, plus an UpdateToken.
6. DescribeFirewall with FirewallArn from step 3 (instead of the name) -> expect the identical record.
7. UpdateFirewallDescription with FirewallName=test-fw, the UpdateToken from step 5 and Description='edited' -> expect 200 and a new UpdateToken; re-run DescribeFirewall and expect Description='edited'.
8. UpdateFirewallDescription again replaying the OLD UpdateToken -> expect InvalidTokenException (HTTP 400).
9. Create two more firewalls, then ListFirewalls with MaxResults=2 -> expect exactly 2 FirewallMetadata entries plus a NextToken; call ListFirewalls again with that NextToken -> expect the remaining firewall and no overlap.
10. ListFirewalls with VpcIds=[vpc-0123abcd] -> expect only firewalls in that VPC.
11. DeleteFirewallPolicy for test-policy while test-fw is still associated -> expect InvalidOperationException (HTTP 400).
12. DeleteFirewall for test-fw -> expect 200 with FirewallStatus.Status=DELETING; then DescribeFirewall test-fw -> expect ResourceNotFoundException (HTTP 400), and ListFirewalls must not list it.
13. DeleteFirewallPolicy for test-policy now that associations are gone -> expect 200 with FirewallPolicyResponse.FirewallPolicyStatus=DELETING; ListFirewallPolicies must no longer include it.
14. Set DeleteProtection=true on a new firewall at create time and call DeleteFirewall -> expect InvalidOperationException (HTTP 400) and the firewall still present in DescribeFirewall.
