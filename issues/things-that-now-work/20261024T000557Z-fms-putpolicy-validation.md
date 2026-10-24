Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# PutPolicy accepts policy documents the service rejects: ResourceType incompatible with the security-service Type, unknown ResourceSetIds, and IncludeMap together with ExcludeMap

_Filed: 2026-10-24T00:05:56Z_
_Operation: PutPolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler performs no semantic validation of the Policy document at all — it strips PolicyId/PolicyUpdateToken, defaults PolicyStatus, and stores whatever was sent. Three classes of input the service refuses are accepted with 200 and persisted, so a later GetPolicy/ListPolicies reports a policy AWS would never have created:

* Cause 1 — ResourceType (or an entry of ResourceTypeList) that is not valid for `SecurityServicePolicyData.Type`. The model enumerates the valid resource types per policy type (e.g. Shield Advanced: load balancers / EIP / CloudFront; network ACL: AWS::EC2::Subnet; DNS Firewall, Network Firewall and third-party firewall: AWS::EC2::VPC). A policy with Type=SHIELD_ADVANCED and ResourceType=AWS::EC2::Subnet, or Type=NETWORK_FIREWALL and ResourceType=AWS::EC2::Instance, must be refused with a 400 client error (the model declares `InvalidTypeException` — 'The value of the Type parameter is invalid' — and `InvalidInputException` for this operation); the emulator stores it.
* Cause 2 — `Policy$ResourceSetIds` naming resource sets that do not exist. The member is documented as 'the unique identifiers of the resource sets used by the policy'; an id that resolves to nothing in `store.resource_sets` is invalid input, but the handler never looks at the member.
* Cause 3 — both `IncludeMap` and `ExcludeMap` supplied. Both members' own documentation states 'You can specify inclusions or exclusions, but not both.' The handler stores both and `ListMemberAccounts` then reads accounts out of each.

Expected: a 400 modeled client error (InvalidTypeException / InvalidInputException) and nothing written. Actual: 200 with the document persisted verbatim.

## Evidence

Model (smithy_query shape com.amazonaws.fms#Policy): `ResourceType` doc lists the valid resource types for each Firewall Manager policy type; `ResourceSetIds` doc 'The unique identifiers of the resource sets used by the policy'; `IncludeMap`/`ExcludeMap` doc 'You can specify inclusions or exclusions, but not both'; PutPolicy declares InvalidInputException and InvalidTypeException (OPERATIONS.md, PutPolicy section). Handler: `src/ops/put_policy.rs` handle (line 197) — the only checks are the update-token match, the MAX_POLICIES bound and the PolicyStatus default; neither `resource_type`, `resource_type_list`, `resource_set_ids`, `include_map` nor `exclude_map` is examined, and `InvalidTypeException` is constructed nowhere in the crate. Test coverage: `tests/all/policy_lifecycle.rs` only drives well-formed documents (and one case deliberately combining ResourceType="ResourceTypeList" with a ResourceTypeList); no test asserts a refusal for an incompatible type, an unknown resource-set id, or both scope maps.

## Steps to Reproduce

1. Boot the emulator and call AssociateAdminAccount.
2. POST PutPolicy with {"Policy":{"PolicyName":"bad-type","SecurityServicePolicyData":{"Type":"SHIELD_ADVANCED"},"ResourceType":"AWS::EC2::Subnet","ExcludeResourceTags":false,"RemediationEnabled":false}}. Observed: 200 and the policy is readable with GetPolicy. Expected: 400 InvalidTypeException/InvalidInputException and no policy created.
3. POST PutPolicy with a valid WAFV2 policy plus "ResourceSetIds":["0123456789AbCdEfGhIjKl"] (no such resource set). Observed: 200. Expected: 400 InvalidInputException.
4. POST PutPolicy with a valid policy carrying BOTH "IncludeMap":{"ACCOUNT":["111111111111"]} and "ExcludeMap":{"ACCOUNT":["222222222222"]}. Observed: 200 and both maps are echoed by GetPolicy. Expected: 400 InvalidInputException.
