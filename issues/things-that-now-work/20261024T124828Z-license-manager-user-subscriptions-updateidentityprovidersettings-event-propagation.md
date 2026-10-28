Issue-Severity: LOW
Issue-Gap-Class: event-propagation
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# UpdateSettings.AddSubnets/SecurityGroupId are applied without resolving them against the account's EC2 store

_Filed: 2026-10-24T12:48:28Z_
_Operation: UpdateIdentityProviderSettings | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateIdentityProviderSettings` changes the subnets License Manager provisions VPC endpoints in, and the security group attached to them. The handler applies `AddSubnets`/`RemoveSubnets`/`SecurityGroupId` purely as set arithmetic over the stored strings (`crate::state::apply_update_settings`) and never asks EC2 whether the account holds the subnets being added or the security group being set, so an update that names non-existent network resources answers 200 and the registration then REPORTS those ids as its live settings in every later ListIdentityProviders.

Expected: adding a subnet the account does not hold (or setting a security group that does not exist) cannot succeed silently — this operation models ValidationException (400) for exactly a parameter that is not usable, and the registration must not end up reporting settings that provision nothing.

Actual: the only refusals are structural (a subnet in both Add and Remove, an update that would leave zero subnets, more than the per-update cap). EC2's store is readable in the shared engine — `pub subnets: BTreeMap<String, Subnet>` / `pub security_groups: BTreeMap<String, SecurityGroup>` in `services/service-com-amazonaws-ec2-amazonec2/src/state.rs`, reached the way `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state/ec2_resources.rs` reaches it — so the check is available and simply not wired. Same root cause as the registration-time gap, but a distinct operation and a distinct observable: here it also MUTATES a previously valid registration into one whose reported settings name nothing.

## Evidence

Spec: `UpdateSettings$AddSubnets` (@required) — "The ID of one or more subnets in which License Manager will create a VPC endpoint for products that require connectivity to activation servers"; `UpdateSettings$SecurityGroupId` (pattern ^sg-...) — "A security group ID that allows inbound TCP port 1688 communication between resources in your VPC and the VPC endpoints for activation servers"; the operation declares ValidationException (400) (smithy_query shape=UpdateSettings, operation=UpdateIdentityProviderSettings). Handler: `src/ops/update_identity_provider_settings.rs` calls `crate::state::apply_update_settings(&settings, &add, &remove, sg)` and writes the result onto the record; `apply_update_settings` in `src/state.rs` only bounds list length, rejects add/remove overlap and an empty result set — there is no existence check and the crate has no EC2 dependency. Tests: `tests/all/resource_resolution_refusals.rs::updating_settings_applies_the_payload_and_persists_it` and `a_payload_that_cannot_be_applied_is_refused` assert only the structural rules.

## Steps to Reproduce

1. RegisterIdentityProvider with Settings whose Subnets are real EC2 subnets of the account.
2. Call UpdateIdentityProviderSettings for that registration with UpdateSettings={AddSubnets:["subnet-0ffffffffffffffff"], RemoveSubnets:[], SecurityGroupId:"sg-0ffffffffffffffff"} — neither id exists in the account's EC2 store.
3. Observe HTTP 200 and an IdentityProviderSummary whose Settings now list the non-existent subnet and security group; ListIdentityProviders keeps reporting them. Expected: ValidationException naming the unusable subnet/security group, leaving the stored settings untouched.
