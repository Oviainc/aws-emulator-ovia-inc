Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8d31dcdeed45f0913eecb26aedcf59e0f3c47424

# The spent-idempotency-token decision (mismatch -> conflict, resource gone -> free the token) is written out four times, once per create handler

_Filed: 2026-10-28T15:46:06Z_
_Operation: CreateAppBundle | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8d31dcdeed45f0913eecb26aedcf59e0f3c47424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four `@idempotencyToken` create handlers in this crate open-code the same three-step decision over `store.client_tokens`:

1. build the key with `crate::state::client_token_key(<signed region>, Self::NAME, token)`;
2. if the key is spent and `spent.fingerprint != fingerprint`, return `crate::state::idempotent_parameter_mismatch(token, "<ResourceType>")` as the modelled `ConflictException`;
3. otherwise try to resolve `spent.arn` back to a stored record — replay it when found, and `store.client_tokens.remove(&key)` when it is gone so the call creates afresh.

Only step 3's resolver differs between the four (which level of the chain the ARN is looked up in, and whether the response projection takes a clock). Steps 1 and 2 and the "token spent on nothing -> remove the key" rule are byte-for-byte the same decision in four places, so the copies can drift: a change to how a mismatch is classified, or to when a stale token is freed, has to be made four times and a missed copy is silently inconsistent. state.rs already owns both halves of the vocabulary (`client_token_key`, `idempotent_parameter_mismatch`, `remember_client_token`), so the decision itself belongs beside them — e.g. one `state.rs` helper that takes the key and the fingerprint and answers Replay(arn) / Mismatch / Fresh, leaving each handler only its own ARN-to-record resolver.

The fix is response-preserving: every status, error member and replayed body stays exactly what it is today, and the existing idempotency suites keep passing unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_app_bundle.rs:176-193:
```
        if let Some(token) = input.client_token.as_deref() {
            let key = crate::state::client_token_key(&region, Self::NAME, token);
            if let Some(spent) = store.client_tokens.get(&key).cloned() {
                if spent.fingerprint != fingerprint {
                    return Err(CreateAppBundleErrorModeled::ConflictException(
                        crate::state::idempotent_parameter_mismatch(token, "AppBundle"),
                    )
                    .into());
                }
                if let Some(bundle) = store.resolve_bundle(&spent.arn) {
                    return Ok(CreateAppBundleOutput { app_bundle: bundle.to_app_bundle() });
                }
                store.client_tokens.remove(&key);
            }
        }
```

Copy 2 — src/ops/create_app_authorization.rs:294-314:
```
        if let Some(token) = input.client_token.as_deref() {
            let key = crate::state::client_token_key(&signed_region, Self::NAME, token);
            if let Some(spent) = store.client_tokens.get(&key).cloned() {
                if spent.fingerprint != fingerprint {
                    return Err(CreateAppAuthorizationErrorModeled::ConflictException(
                        crate::state::idempotent_parameter_mismatch(token, "AppAuthorization"),
                    )
                    .into());
                }
                ...
                store.client_tokens.remove(&key);
            }
        }
```

Copy 3 — src/ops/create_ingestion.rs:269-293:
```
        if let Some(token) = input.client_token.as_deref() {
            let key = crate::state::client_token_key(&signed_region, Self::NAME, token);
            if let Some(spent) = store.client_tokens.get(&key).cloned() {
                if spent.fingerprint != fingerprint {
                    return Err(CreateIngestionErrorModeled::ConflictException(
                        crate::state::idempotent_parameter_mismatch(token, "Ingestion"),
                    )
                    .into());
                }
                ...
                store.client_tokens.remove(&key);
            }
        }
```

Copy 4 — src/ops/create_ingestion_destination.rs:274-297:
```
        if let Some(token) = input.client_token.as_deref() {
            let key = crate::state::client_token_key(&signed_region, Self::NAME, token);
            if let Some(spent) = store.client_tokens.get(&key).cloned() {
                if spent.fingerprint != fingerprint {
                    return Err(CreateIngestionDestinationErrorModeled::ConflictException(
                        crate::state::idempotent_parameter_mismatch(token, "IngestionDestination"),
                    )
                    .into());
                }
                ...
                store.client_tokens.remove(&key);
            }
        }
```

The vocabulary the decision already shares lives in state.rs:1284-1290 (`client_token_key`), state.rs:1881-1892 (`idempotent_parameter_mismatch`) and state.rs:865-885 (`remember_client_token`).

Test coverage that must stay green and unchanged: tests/all/idempotent_replay_before_preconditions.rs (`RULE (the same one, across every create this service models): a spent token …`, line 370), tests/all/client_token_region_scope.rs (lines 101, 188, 267), tests/all/create_app_authorization_credential_idempotency.rs:142, tests/all/created_resource_tag_set.rs:345, tests/all/app_bundle_region_quota.rs:315, tests/all/ingestion_destination_lifecycle.rs:965, tests/all/app_bundle_ingestion_lifecycle.rs:1060 and :1252.

## Steps to Reproduce

1. Open src/ops/create_app_bundle.rs:176-193, src/ops/create_app_authorization.rs:294-314, src/ops/create_ingestion.rs:269-293 and src/ops/create_ingestion_destination.rs:274-297 side by side
2. Observe the identical three-step spent-token decision (key from client_token_key, fingerprint mismatch -> idempotent_parameter_mismatch ConflictException, resource gone -> client_tokens.remove(&key)) in all four
3. Change any part of that rule (for example which error a mismatch is reported as) and note that four edits are needed, with no compiler error for a missed copy
