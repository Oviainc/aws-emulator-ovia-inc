Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# Re-fold at write time can store a Settings whose subnets span two VPCs, the invariant the resolution enforces

_Filed: 2026-10-29T09:19:01Z_
_Operation: UpdateIdentityProviderSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateIdentityProviderSettings` resolves the network its payload would store BEFORE the EC2 hop, against a snapshot of the registration's settings read under a first lock, and then WRITES by re-folding the same payload onto whatever the store holds under a second lock. The value actually stored is therefore never the value that was resolved, and nothing re-checks the one-VPC agreement rule on it.

The crate enforces that rule itself: `agreed_vpc` refuses a subnet set spanning two VPCs ("All subnets must belong to the same VPC") and `resolve_settings_network` refuses a security group of another VPC than the subnets. Because the write recomputes `current_settings + AddSubnets - RemoveSubnets` from the post-hop value, an update that was valid against the pre-hop snapshot can compose with another update accepted inside the hop window into a stored configuration that the crate's own resolver would refuse — subnets in two VPCs, and/or a security group in neither.

The handler's own comment claims the opposite ("The resulting settings are resolved as a whole, so the invariant holds for what is stored however the payload spelled it"), and `update_live_settings`'s doc narrows that claim to the payload's ids only ("The fold is of the PAYLOAD's own ids, which the resolution covered") while still concluding "the configuration stored here is one the account's network carries" — which does not follow for the composed set. The hop window is real and already exercised: `tests/all/settings_update_writes_at_write_time.rs` drives exactly this interleaving ("The table below drives the WRITE STEP (`update_live_settings`, the one the handler runs after its hop) with another update already accepted over the wire in between, which is exactly the window the hop opens"), but all of its rows stay inside one VPC, so the cross-VPC composition is uncovered. The result is persisted and then reported by `ListIdentityProviders`/`DeregisterIdentityProvider`, so a later read serves a configuration no single request could have created.

## Evidence

Handler claims the invariant holds for what is STORED — services/.../src/ops/update_identity_provider_settings.rs:215-222:
```
// What the update would STORE has to be a configuration the account's network can
// actually carry: the subnets License Manager creates the VPC endpoints in, and the
// security group in front of them, are resources of one VPC the account has. The
// resulting settings are resolved as a whole, so the invariant holds for what is
// stored however the payload spelled it.
crate::state::ec2_fleet::resolve_settings_network(ctx, &would_store)
```
...but `would_store` was folded onto the PRE-HOP snapshot (same file:188-213: `let (arn, settings) = { let mut state = state.lock(); ... record.settings.clone() ... }` then `apply_update_settings(&settings, ...)`), while the value actually written is re-folded onto the POST-HOP value (same file:233-244):
```
let mut state = state.lock();
...
let identity_provider_summary = state
    .update_live_settings(&arn, &input.update_settings.add_subnets, ...)
```
and `update_live_settings` writes that recomputed value with no network check — services/.../src/state.rs:1988-1994:
```
let updated = apply_update_settings(
    &record.settings,
    add_subnets,
    remove_subnets,
    security_group_id,
)?;
record.settings = updated;
```
Its own doc admits the coverage gap one sentence before claiming the conclusion — state.rs:1968-1970:
```
/// The fold is of the PAYLOAD's own ids, which the resolution covered; what it folds
/// them onto is settings some accepted write already resolved, so the configuration
/// stored here is one the account's network carries.
```
The rule that is bypassed is enforced in this crate — services/.../src/state/ec2_fleet.rs:386-399:
```
fn agreed_vpc(vpcs: &[Option<String>]) -> Result<Option<String>, crate::state::StateError> {
...
                return Err(crate::state::StateError::Validation(format!(
                    "All subnets must belong to the same VPC: '{expected}'"
                )));
```
Test coverage: `tests/all/settings_network_resolution.rs:196-209` pins that a mixed-VPC Settings is refused (`subnets: &[ACCOUNT_SUBNETS[0], OTHER_VPC_SUBNET], expected: Expected::SubnetsDisagree(ACCOUNT_VPC)`), and `tests/all/settings_update_writes_at_write_time.rs:23-28` already drives the write step with "another update already accepted over the wire in between" — but only with single-VPC rows, so the composition above is untested.

## Steps to Reproduce

1. Build the harness of tests/all/settings_network_resolution.rs, whose account holds ACCOUNT_VPC (ACCOUNT_SUBNETS/ACCOUNT_SECURITY_GROUPS) and OTHER_VPC (OTHER_VPC_SUBNET/OTHER_VPC_SECURITY_GROUP).
2. RegisterIdentityProvider with Settings = { Subnets: [ACCOUNT_SUBNETS[0]], SecurityGroupId: ACCOUNT_SECURITY_GROUPS[0] } and note the IdentityProviderArn.
3. Issue update B: UpdateIdentityProviderSettings with UpdateSettings = { AddSubnets: [ACCOUNT_SUBNETS[1]], RemoveSubnets: [] }. Its pre-hop resolution sees [ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1]] — one VPC — and is accepted.
4. Inside B's EC2 hop window, let update A be accepted: UpdateSettings = { AddSubnets: [OTHER_VPC_SUBNET], RemoveSubnets: [ACCOUNT_SUBNETS[0]], SecurityGroupId: OTHER_VPC_SECURITY_GROUP }. Its own would_store is entirely in OTHER_VPC, so it resolves and is stored. (Deterministically: call the state seam `update_live_settings` for A between B's two locks, the way settings_update_writes_at_write_time.rs drives the write step.)
5. B's write then folds its payload onto the post-A value: stored Settings.Subnets = [OTHER_VPC_SUBNET, ACCOUNT_SUBNETS[1]] with SecurityGroupId = OTHER_VPC_SECURITY_GROUP.
6. ListIdentityProviders (or the UpdateIdentityProviderSettings response itself) reports that Settings. Expected: no accepted write leaves a registration whose subnets span two VPCs — the configuration the handler's comment says is "resolved as a whole". Actual: the stored configuration is one `resolve_settings_network` refuses with "All subnets must belong to the same VPC", and re-sending it verbatim on a fresh register/update is rejected.
