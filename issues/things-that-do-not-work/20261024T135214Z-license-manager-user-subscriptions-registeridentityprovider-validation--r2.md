Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Settings.Subnets / SecurityGroupId are stored without ever being resolved against the account's VPC resources

_Filed: 2026-10-24T13:52:14Z_
_Operation: RegisterIdentityProvider | Class: validation_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `Settings` is "the registered identity provider's product related configuration settings such as the subnets to provision VPC endpoints" and `SecurityGroupId` is "A security group ID that allows inbound TCP port 1688 communication between resources in your VPC and the VPC endpoint for activation servers" — License Manager actually creates VPC endpoints in those subnets with that security group, so a subnet or security group the account does not hold cannot be registered: the real service refuses the request (and a subnet set spanning unrelated VPCs cannot be satisfied either).

Actual: the handler stores whatever the caller supplied verbatim (`let settings = input.settings.clone().unwrap_or_default();` … `settings` written into `IdentityProviderRecord`) and resolves nothing: any string matching `^subnet-[a-z0-9]{8,17}` and `^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$` is accepted, including `subnet-00000000` / `sg-00000000` that no VPC holds, and the registration then reports them back in every `IdentityProviderSummary`. The same hole exists on `UpdateIdentityProviderSettings`: `apply_update_settings` only checks the add/remove overlap, the per-payload count and that at least one subnet remains (`src/state.rs:1792-1846`), never whether `AddSubnets` names subnets that exist.

Since the repo's ec2 emulator keeps real subnet and security-group stores (and EVS already resolves network resources against them), this gate can be implemented the same way.

## Evidence

Spec: `Settings$Subnets` (`@required`, `@length(min:1)`, member pattern `^subnet-[a-z0-9]{8,17}`) and `Settings$SecurityGroupId` (`@required`, pattern `^sg-...`) per `smithy_query(query="shape", shape="Settings")`, with the documentation quoted above; `RegisterIdentityProvider` models `ValidationException` (400) for "A parameter is not valid".

Handler: `src/ops/register_identity_provider.rs:175` and `:266-278` store `settings` with no existence check — the only pre-store checks are `validate_product`, `validate_tags` and `resolve_directory`. `src/ops/update_identity_provider_settings.rs:186-201` + `src/state.rs:1792-1846` for the update path.

Cross-service precedent: `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state/ec2_resources.rs:66-105` resolves `vpcId`/subnet ids against `service_com_amazonaws_ec2_amazonec2::state`.

Test coverage: `tests/all/optional_settings_registration.rs` and the registration suites only assert that supplied settings are echoed back; no test asserts that a non-existent subnet or security group is refused.

## Steps to Reproduce

1. POST /identity-provider/RegisterIdentityProvider with `{"IdentityProvider":{"ActiveDirectoryIdentityProvider":{"DirectoryId":"d-9f8e7d6c5b"}},"Product":"OFFICE_PROFESSIONAL_PLUS","Settings":{"Subnets":["subnet-00000000"],"SecurityGroupId":"sg-00000000"}}` without creating any VPC/subnet/security group.
2. Observe HTTP 200 and an `IdentityProviderSummary` echoing the non-existent subnet and security group, instead of a `ValidationException` naming the unresolvable network resource.
3. Call POST /identity-provider/UpdateIdentityProviderSettings with `UpdateSettings.AddSubnets=["subnet-11111111"]` and observe the same silent acceptance.
