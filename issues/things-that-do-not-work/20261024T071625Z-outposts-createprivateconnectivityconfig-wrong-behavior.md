Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# The private connectivity configuration is returned without RoleArn or ProvisioningRoleArn

_Filed: 2026-10-24T07:16:24Z_
_Operation: CreatePrivateConnectivityConfig | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePrivateConnectivityConfig` persists the caller's `VpcInformationList` and sets `PrivateConnectivityStatus` to `ENABLED`, but hardcodes `role_arn: None` and `provisioning_role_arn: None`. Every later `GetPrivateConnectivityConfig` therefore reports a configuration with the two role members absent.

In the real service these two members are the whole point of the response: `ProvisioningRoleArn` is the role AWS Outposts assumes to provision the service-link resources in the customer's VPC, and `RoleArn` is the role it assumes to run it; a client reads them to attach/verify permissions. A response that omits them cannot be used for that.

Expected: the created (and later read) `PrivateConnectivityConfig` carries both role ARNs in the modelled ARN form for the request's account and partition. Actual: both members are omitted from the JSON body on create and on every read.

## Evidence

Spec: the model's `PrivateConnectivityConfig` structure declares `RoleArn` and `ProvisioningRoleArn` alongside `VpcInformationList` and `PrivateConnectivityStatus`, and `CreatePrivateConnectivityConfig`/`GetPrivateConnectivityConfig` are the only operations that report that shape — the members exist to be reported for a configuration that has been created.

Handler: `src/ops/create_private_connectivity_config.rs` lines ~213-227 construct the config with `role_arn: None, provisioning_role_arn: None`; `src/ops/get_private_connectivity_config.rs` line ~185 returns the stored clone verbatim, so no read path can fill them in.

Tests: `tests/all/private_connectivity.rs::create_persists_the_vpc_information_and_both_identifier_forms_read_it_back` asserts the VPC information and the status round-trip; no test asserts anything about the two role members, so the omission is uncovered.

## Steps to Reproduce

1. CreateSite, then CreateOutpost against it; keep the OutpostId.
2. POST /outposts/{OutpostId}/privateConnectivity with {"VpcInformationList":[{"VpcId":"vpc-0123456789abcdef0"}]} — observe the returned PrivateConnectivityConfig has no RoleArn and no ProvisioningRoleArn.
3. GET /outposts/{OutpostId}/privateConnectivity — the same configuration is reported, still without either role ARN.
