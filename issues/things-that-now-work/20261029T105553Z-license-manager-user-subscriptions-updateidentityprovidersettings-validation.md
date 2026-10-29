Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6bea01b02671f5bc5cdfd807295cdb4bfa0eb328

# A security-group-only update is refused for a registration this crate itself stored with no subnets

_Filed: 2026-10-29T10:55:53Z_
_Operation: UpdateIdentityProviderSettings | Class: validation_

> _Note: this issue was identified against the code base at commit `6bea01b02671f5bc5cdfd807295cdb4bfa0eb328`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegisterIdentityProvider` deliberately accepts a request that omits the optional `Settings` member and stores the EMPTY configuration — no subnet, no security group. `UpdateIdentityProviderSettings` then refuses every payload that does not add a subnet to such a registration, on the grounds that the result would carry no subnet and "Settings.Subnets requires at least one" — even when the payload removes nothing at all.

`UpdateSettings` declares `AddSubnets` and `RemoveSubnets` both `@required`, so the minimal payload a client can send is `{"AddSubnets": [], "RemoveSubnets": []}` plus an optional `SecurityGroupId`. Against a registration that holds two subnets, that exact payload is a supported and tested update (`a security group only`). Against a registration the SAME crate created with no `Settings`, the identical payload is answered HTTP 400 `ValidationException` with "UpdateSettings would leave the registration with no subnet" — so the security group of a `Settings`-less registration can never be set, and the no-op update the `@idempotent` trait asks for is refused too.

The two sides of the crate disagree about whether an empty `Settings.Subnets` is a state this service holds: the registration writer stores it and every summary reports it, while the update validator treats it as impossible.

## Evidence

The crate stores the empty configuration on purpose — `src/ops/register_identity_provider.rs:180-184`:
```
// `Settings` is optional on the wire, so an omitted one is a registration that
// provisions no activation-server VPC endpoints: it is stored as the empty
// configuration (no subnet, no security group) rather than refused, and nothing
// is invented to fill it in. Whatever the caller DID supply is stored verbatim.
let settings = input.settings.clone().unwrap_or_default();
```
and `src/state.rs:481-486` documents the stored record the same way: "The input member is OPTIONAL, so a registration that provisions no activation-server VPC endpoints supplies none; the record then carries no subnet and no security group (an empty [`Settings`]) and every summary reports that empty configuration rather than values nobody asked for."

The update validator refuses exactly that state — `src/state.rs:2590-2595`:
```
if subnets.is_empty() {
    return Err(StateError::Validation(
        "UpdateSettings would leave the registration with no subnet, and \
         Settings.Subnets requires at least one"
            .to_string(),
```
reached from `src/ops/update_identity_provider_settings.rs:207-213` (`crate::state::apply_update_settings(&settings, &input.update_settings.add_subnets, &input.update_settings.remove_subnets, input.update_settings.security_group_id.as_deref()).map_err(into_error)?`), whose `into_error` renders every state refusal as the modelled `ValidationException` (`src/ops/update_identity_provider_settings.rs:270-283`).

The same payload is pinned as APPLICABLE for a registration that has subnets — `tests/all/resource_resolution_refusals.rs:1873-1882`:
```
(
    "a security group only",
    json!({
        "AddSubnets": [],
        "RemoveSubnets": [],
        "SecurityGroupId": "sg-0123456789abcdef0",
    }),
    &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
    "sg-0123456789abcdef0",
),
```
and the model documents that combination: `UpdateSettings` — "You can update any combination of settings in a single operation such as the: Subnets which you want to add to provision VPC endpoints. Subnets which you want to remove the VPC endpoints from. Security group ID which permits traffic to the VPC endpoints." (`smithy_query(query="shape", shape="UpdateSettings")`, also quoted in `src/ops/update_identity_provider_settings.rs:73-77`).

No test covers an update of a `Settings`-less registration: `tests/all/optional_settings_registration.rs` registers three of them and only drives `CreateLicenseServerEndpoint` on them, and `tests/all/resource_resolution_refusals.rs:1944-2008` exercises the no-subnet refusal only against a registration that started with two subnets.

## Steps to Reproduce

1. POST /identity-provider/RegisterIdentityProvider with `{"IdentityProvider":{"ActiveDirectoryIdentityProvider":{"DirectoryId":"d-0123456789"}},"Product":"REMOTE_DESKTOP_SERVICES"}` — no `Settings` member.
2. Observe HTTP 200 and `IdentityProviderSummary.Settings == {"Subnets": [], "SecurityGroupId": ""}` (the stored empty configuration).
3. POST /identity-provider/UpdateIdentityProviderSettings with `{"IdentityProviderArn":"<the minted ARN>","UpdateSettings":{"AddSubnets":[],"RemoveSubnets":[],"SecurityGroupId":"sg-0123abcd"}}`.
4. Observe HTTP 400 `ValidationException` "UpdateSettings would leave the registration with no subnet, and Settings.Subnets requires at least one", although the payload removes no subnet and the same payload succeeds against a registration that has subnets.
5. Repeat with `{"AddSubnets":[],"RemoveSubnets":[]}` and observe the same refusal for a payload that changes nothing.
