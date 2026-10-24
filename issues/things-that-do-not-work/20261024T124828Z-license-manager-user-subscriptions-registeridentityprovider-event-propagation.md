Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# Settings.Subnets and SecurityGroupId are never resolved against the account's EC2 store, so a registration for non-existent network resources settles REGISTERED

_Filed: 2026-10-24T12:48:28Z_
_Operation: RegisterIdentityProvider | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Settings` is the registration's "product related configuration settings such as the subnets to provision VPC endpoints", i.e. `Subnets` are EC2 subnets of the caller's account and `SecurityGroupId` is an EC2 security group that must permit inbound TCP 1688 to those VPC endpoints. The handler stores whatever the caller sent verbatim and never asks EC2 whether the account holds them, so `RegisterIdentityProvider` with `Subnets=[subnet-00000000000000000]` and `SecurityGroupId=sg-00000000000000000` (well-formed but belonging to nothing) returns 200 and the registration then settles into a healthy `REGISTERED` status with `FailureMessage` null.

Expected: a registration whose VPC endpoints cannot be provisioned is not a healthy registration — AWS either refuses the request outright (the operation models ValidationException/ResourceNotFoundException) or lets the registration settle into a failed status carrying `IdentityProviderSummary.FailureMessage`. Either way, a client cannot be told REGISTERED for subnets and a security group the account does not have.

Actual: the ids are a free-form string list to this emulator. Nothing in the crate ever reads EC2's store (no dependency on the EC2 service crate), and `IdentityProviderRecord.failure_message` is written `None` by the only writer and never set by any later path, so no registration can ever report a failure.

The repo already does exactly this kind of cross-service resolution: `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state/ec2_resources.rs` reads EC2's own store through `service_com_amazonaws_ec2_amazonec2::state::service_state(engine)` to judge a `vpcId`/`subnetId` a request names (and treats an unreadable store as "no failure to report"). EC2's state exposes `pub subnets: BTreeMap<String, Subnet>` and `pub security_groups: BTreeMap<String, SecurityGroup>` (`services/service-com-amazonaws-ec2-amazonec2/src/state.rs`), and `ec2:CreateSubnet`/`CreateSecurityGroup`/`RunInstances` are implemented, so a client that builds its network in the emulator and then registers gets no consistency between the two services.

## Evidence

Spec: `Settings$Subnets` (@required, @length min 1) — "The subnets defined for the registered identity provider"; `Settings$SecurityGroupId` (@required, pattern ^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$) — "A security group ID that allows inbound TCP port 1688 communication between resources in your VPC and the VPC endpoint for activation servers"; `RegisterIdentityProvider.Settings` — "the subnets to provision VPC endpoints"; `IdentityProviderSummary$FailureMessage` — "The failure message associated with an identity provider" (smithy_query shape=Settings/IdentityProviderSummary). Handler: `src/ops/register_identity_provider.rs` takes `let settings = input.settings.clone().unwrap_or_default();` and stores it unchecked — the only resolution it performs is `resolve_directory` against this crate's OWN `account_directories` fixture; `failure_message: None` is hard-coded in the record it inserts and `grep failure_message src/state.rs` shows no writer. Precedent for the wiring: `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state/ec2_resources.rs` lines 66-74 + `network_failures`. Tests: `tests/all/identity_provider_subscriptions.rs::values_outside_the_modelled_constraints_are_refused` only exercises the modelled pattern/length constraints; no test asserts a subnet or security group is resolved against EC2.

## Steps to Reproduce

1. Start the emulator with both the EC2 and license-manager-user-subscriptions services mounted.
2. Do NOT create any subnet or security group in EC2.
3. Call RegisterIdentityProvider with IdentityProvider.ActiveDirectoryIdentityProvider.DirectoryId of an existing directory, Product=VISUAL_STUDIO_ENTERPRISE and Settings={Subnets:["subnet-00000000000000000"], SecurityGroupId:"sg-00000000000000000"} — the ids are well-formed but name nothing in the account.
4. Observe HTTP 200 and an IdentityProviderSummary echoing those settings; poll ListIdentityProviders and observe Status settle to REGISTERED with FailureMessage null. Expected: the request is refused for the unknown subnet/security group, or the registration settles into a failed status carrying a FailureMessage.
5. Conversely, create a real subnet/security group via ec2:CreateSubnet / ec2:CreateSecurityGroup and register with those ids: the emulator cannot tell the two cases apart, so the real ones get no better treatment than the invented ones.
