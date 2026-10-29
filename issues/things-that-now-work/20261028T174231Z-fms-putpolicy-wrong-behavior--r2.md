Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 145e568e7ef9b3115be9abe08039cd724717eead

# A policy written without ResourceTagLogicalOperator is stored and reported with none, although the member's documentation states its default is AND

_Filed: 2026-10-28T17:42:31Z_
_Operation: PutPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `145e568e7ef9b3115be9abe08039cd724717eead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Policy$ResourceTagLogicalOperator` is documented with an explicit default: "Default: `AND`". `PutPolicy` clones the caller's document verbatim, strips only the three service-owned members (`PolicyId`, `PolicyUpdateToken`, `PolicyStatus`), and never materialises that default, so the stored `Policy` keeps `resource_tag_logical_operator: None`.

`PolicyRecord::policy()` — the one projection every reader of a policy goes through (`PutPolicy`'s own response, `GetPolicy`) — overlays exactly those same three members and nothing else, so a policy created without the member is reported with no `ResourceTagLogicalOperator` at all on every subsequent read, rather than with the `AND` the model says applies. A client that reads the member back to learn how the policy combines its `ResourceTags` gets "unknown" where the service documents "AND".

## Evidence

Smithy, `smithy_query(query="doc", shape="com.amazonaws.fms#Policy", member="ResourceTagLogicalOperator")`:

  "Specifies whether to combine multiple resource tags with AND, so that a resource must have all tags to be included or excluded, or OR, so that a resource must have at least one tag.
   Default: `AND`"

The handler stores the document verbatim and removes only the service-owned members — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_policy.rs:514-517:

```
        let mut policy = input.policy.clone();
        let requested_id = policy.policy_id.take();
        let requested_token = policy.policy_update_token.take();
        policy.policy_status = None;
```

The single reader projection overlays only those three, so nothing fills the member in later — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:356-362:

```
    pub fn policy(&self, policy_status: &str) -> Policy {
        let mut policy = self.policy.clone();
        policy.policy_id = Some(self.policy_id.clone());
        policy.policy_update_token = Some(self.update_token.clone());
        policy.policy_status = Some(policy_status.to_string());
        policy
    }
```

The member is nullable in the generated type, so the absent value survives — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/types/shared_types.rs:128:

```
    pub resource_tag_logical_operator: Option<String>,
```

Test coverage: services/service-com-amazonaws-fms-awsfms-20180101/tests/all/policy_lifecycle.rs:137-151 is the "minimal-wafv2" case, which sends no `ResourceTagLogicalOperator`; policy_lifecycle.rs:339-350 asserts only that "member {member} was not stored as sent" for the members the case DID send, so no existing test pins the member's absence — the only row that exercises it at all sends it explicitly (`"ResourceTagLogicalOperator": "OR"`, policy_lifecycle.rs:169).

## Steps to Reproduce

1. Call `AssociateAdminAccount` so the administrator gate passes.
2. Call `PutPolicy` with a minimal policy that omits `ResourceTagLogicalOperator`, e.g. `{"Policy": {"PolicyName": "demo", "SecurityServicePolicyData": {"Type": "WAFV2"}, "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer", "ExcludeResourceTags": false, "RemediationEnabled": false}}`.
3. Read the response's `Policy`, then call `GetPolicy` with the minted `PolicyId`.
4. Observe that `ResourceTagLogicalOperator` is absent from both, although the member's own documentation states "Default: `AND`".
