Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# AssociateUser's ProviderNotRegistered arm carries the other plane's sentence, contradicting its own comment

_Filed: 2026-10-28T13:37:24Z_
_Operation: AssociateUser | Class: error-response_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateUser::into_error` maps `StateError::ProviderNotRegistered` onto `ResourceNotFoundException` carrying `IDENTITY_PROVIDER_NOT_VALID` ("Identity provider is not valid"). The comment directly above that arm says it exists so the two paths "cannot word one refusal two ways" — but the handler's own inline refusal for exactly that fact, 130 lines earlier, carries `IDENTITY_PROVIDER_NOT_REGISTERED` ("the identity provider is not registered for user-based subscriptions"), and so does the identically-commented arm in the sibling `ListUserAssociations`. `state.rs` documents the rule explicitly: the user-association plane answers `ResourceNotFoundException` carrying `IDENTITY_PROVIDER_NOT_REGISTERED`; `IDENTITY_PROVIDER_NOT_VALID` belongs to the registration plane. So this arm wires in the wrong plane's sentence while claiming to prevent exactly that. The arm should carry `IDENTITY_PROVIDER_NOT_REGISTERED`, as its sibling does.

## Evidence

The arm and the comment that denies what it does — `src/ops/associate_user.rs:338-349`:
```
        // This handler resolves the registration ITSELF (the inline branch above), and
        // answers the modelled `ResourceNotFoundException` for one it cannot resolve — the
        // contract this operation already landed with, which the
        // `resource_resolution_refusals` suite pins and nothing in this change moves. The
        // lookup therefore cannot report this here; the arm keeps that same class so the
        // two paths cannot word one refusal two ways.
        crate::state::StateError::ProviderNotRegistered => {
            AssociateUserErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::IDENTITY_PROVIDER_NOT_VALID.to_string()),
            })
            .into()
        }
```
The same handler's inline refusal for the same fact — `src/ops/associate_user.rs:209-218`:
```
            return Err(AssociateUserErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // The one sentence this service's user-association plane states this
                    // refusal with, so this handler and `ListUserAssociations` — which
                    // resolves the same directory the same way — cannot word one refusal
                    // two ways.
                    message: Some(crate::state::IDENTITY_PROVIDER_NOT_REGISTERED.to_string()),
                },
            )
```
The sibling arm, with the same comment, uses the other constant — `src/ops/list_user_associations.rs:283-295`:
```
        // This handler resolves the directory's registration ITSELF (the inline branch
        // above) and answers the modelled `ResourceNotFoundException` for one it cannot
        // resolve, as the rest of the user-association plane does; the lookups it calls
        // therefore cannot report this here, and the arm keeps that same class so the two
        // paths cannot word one refusal two ways.
        crate::state::StateError::ProviderNotRegistered => {
            ListUserAssociationsErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::IDENTITY_PROVIDER_NOT_REGISTERED.to_string()),
                },
            )
```
And the state module states which sentence this plane owns — `src/state.rs:393-398`:
```
    /// The USER-ASSOCIATION plane (`AssociateUser`, `ListUserAssociations`) is neither: it
    /// resolves the directory's registration itself and answers its own modelled
    /// `ResourceNotFoundException` carrying [`IDENTITY_PROVIDER_NOT_REGISTERED`], because
```
plus `src/state.rs:353-362`, which declares `IDENTITY_PROVIDER_NOT_REGISTERED` as "One sentence in one place ... `AssociateUser` creates an association through the registration of the directory it names and `ListUserAssociations` reports the associations of that same directory, so both resolve it with ... `provider_key_is_registered` and both answer their modelled `ResourceNotFoundException` carrying this."

No test covers this arm: `tests/all/resource_resolution_refusals.rs` and `tests/all/user_associations.rs` exercise the inline branch (`IDENTITY_PROVIDER_NOT_REGISTERED`) only. The arm is currently unreachable — every `into_error` call site in `src/ops/associate_user.rs` (`provider_key` at line 207, `validate_tags` at line 226, `merge_tags` at line 300) can only yield `StateError::Validation` (`src/state.rs:2266-2287` returns only `StateError::Validation`) — so the defect is a latent wrong sentence rather than an observable one today.

## Steps to Reproduce

1. Read `src/ops/associate_user.rs:338-349` and note the arm's comment promising the two paths "cannot word one refusal two ways".
2. Compare the message constant it uses (`IDENTITY_PROVIDER_NOT_VALID`) with the one the handler's own inline refusal uses at `src/ops/associate_user.rs:215` (`IDENTITY_PROVIDER_NOT_REGISTERED`).
3. Compare with the sibling arm at `src/ops/list_user_associations.rs:288-293`, which carries the same comment and uses `IDENTITY_PROVIDER_NOT_REGISTERED`.
4. Compare with the rule stated in `src/state.rs:393-398`: the user-association plane answers `ResourceNotFoundException` carrying `IDENTITY_PROVIDER_NOT_REGISTERED`.
5. Have any lookup `AssociateUser` calls return `StateError::ProviderNotRegistered` (e.g. if `provider_key` or a future helper gains that arm) and the operation answers the registration plane's sentence through the user-association plane's error.
