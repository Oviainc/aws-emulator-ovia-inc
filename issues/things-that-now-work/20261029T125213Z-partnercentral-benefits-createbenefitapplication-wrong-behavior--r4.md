Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# insert_application writes the ARN index and the spent ClientToken before its catalog guard, contradicting its own "nothing is stored" contract and the store's one-entry-per-application bound

_Filed: 2026-10-29T12:52:13Z_
_Operation: CreateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::insert_application` is the single write path a create takes. Its doc states the contract "Nothing is stored when the catalog does not exist", and the store struct's own doc claims "`arns` and `client_tokens` carry at most one entry per stored application" as part of what makes the store BOUNDED. The body breaks both: only the third insert (the application itself) is guarded by `if let Some(catalog) = self.catalogs.get_mut(&application.catalog)`, while `self.arns.insert(..)` and `self.client_tokens.insert(..)` run unconditionally ahead of it.

So for a catalog the store does not hold, the function stores two of the three things it says it stores nothing of, and leaves them permanently orphaned: an `arns` entry pointing at a (catalog, id) pair with no application behind it, and a `client_tokens` entry whose `application_id` names nothing. The second is the damaging one, because `replayed_create` reads that index first: its own doc says `Ok(None)` means "a token no create holds, so the request is a new one", but for an orphaned token the fingerprint comparison runs BEFORE the application lookup, so any later create that reuses that `ClientToken` with different members is answered `ConflictException` (idempotency-parameter-mismatch) for an application that was never created — the token is poisoned for the life of the process — while a create repeating the orphaning request exactly falls through to `Ok(None)` and creates a second application under the same token, which is precisely the state the index exists to prevent.

The guard is in the wrong place: all three inserts belong behind the catalog lookup (or the function should resolve the catalog once, up front, and return without touching any index when it is absent). Severity is LOW because the only wire caller refuses an unknown catalog under the same lock before it reaches here, so the bad state is not reachable from the API today — but the helper publishes a contract it does not keep, and the next caller of it (or a reordering of the create's checks) inherits a silently corrupted token index rather than a no-op.

## Evidence

The contract and the code that breaks it — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:1842-1867:
```
    /// Nothing is stored when the catalog does not exist, which a handler has already
    /// refused.
    pub fn insert_application(
        &mut self,
        client_token: String,
        request_fingerprint: String,
        application: BenefitApplication,
    ) {
        self.arns.insert(
            application.arn.clone(),
            (application.catalog.clone(), application.id.clone()),
        );
        self.client_tokens.insert(
            client_token,
            ClientTokenRecord {
                catalog: application.catalog.clone(),
                application_id: application.id.clone(),
                request_fingerprint,
            },
        );
        if let Some(catalog) = self.catalogs.get_mut(&application.catalog) {
            catalog
                .applications
                .insert(application.id.clone(), application);
        }
    }
```
Two unconditional inserts, one guarded insert — the opposite of "nothing is stored".

The bound the orphans break — src/state.rs:1515-1520:
```
/// BOUNDED: `CreateBenefitApplication` refuses a create that would take a catalog past
/// [`MAX_APPLICATIONS_PER_CATALOG`], the catalog set is fixed by [`SEEDED_CATALOGS`],
/// `arns` and `client_tokens` carry at most one entry per stored application, one
/// application records at most [`MAX_APPLIED_WRITE_HISTORY`] applied writes, and a catalog holds
/// at most [`MAX_ALLOCATIONS_PER_CATALOG`] allocations — so no caller can grow this store
/// without limit.
```

The reader that misinterprets an orphaned token — src/state.rs:1745-1763:
```
    /// `Ok(None)` is a token no create holds, so the request is a new one. The index the
    /// token is looked up in is the whole store's ([`Self::client_tokens`]) ...
    pub fn replayed_create(
        &self,
        client_token: &str,
        request_fingerprint: &str,
    ) -> Result<Option<&BenefitApplication>, crate::ops::types::ConflictException> {
        let Some(spent) = self.client_tokens.get(client_token) else {
            return Ok(None);
        };
        if spent.request_fingerprint != request_fingerprint {
            return Err(idempotency_conflict());
        }
        Ok(self
            .catalogs
            .get(&spent.catalog)
            .and_then(|stored| stored.applications.get(&spent.application_id)))
```
The `Err(idempotency_conflict())` arm fires on the orphan, so a token with no application behind it refuses every subsequent differing create; the final arm collapses the orphan to `Ok(None)`, which the create reads as "a new request".

The call site, and the check that currently keeps the hole unreachable from the wire — src/ops/create_benefit_application.rs:273-280 and :491:
```
        let catalog = match store.catalog(&input.catalog) {
            Ok(catalog) => catalog,
            Err(missing) => {
                return Err(
                    CreateBenefitApplicationErrorModeled::ResourceNotFoundException(missing).into(),
                )
            }
        };
...
        store.insert_application(input.client_token, fingerprint, application);
```

Test coverage: no test calls `insert_application` with an absent catalog (`grep -n insert_application tests/` returns nothing), and tests/all/client_token_idempotency.rs and tests/all/idempotent_parameter_mismatch.rs exercise the token index only through creates into the seeded catalogs `AWS`/`Sandbox` (src/state.rs:524 `pub const SEEDED_CATALOGS: &[&str] = &["AWS", "Sandbox"];`), so nothing pins the current ordering or states a reason for it.

## Steps to Reproduce

1. Read src/state.rs:1842-1867 and observe that the doc sentence immediately above the function states "Nothing is stored when the catalog does not exist" while `self.arns.insert(..)` and `self.client_tokens.insert(..)` execute before the `if let Some(catalog) = self.catalogs.get_mut(..)` guard.
2. In an in-process test holding the shared engine (as tests/all/benefit_allocations.rs:98 does for `grant_allocation`), call `state.lock().insert_application("tok-orphan".to_string(), <any fingerprint>, BenefitApplication { catalog: "NoSuchCatalog".into(), .. })`.
3. Observe that `catalogs` gained nothing (correct) but `client_tokens` now holds "tok-orphan" and `arns` holds the application's ARN — the two entries the doc says are not stored, and the two the struct doc at src/state.rs:1517 bounds at "one entry per stored application".
4. Call `replayed_create("tok-orphan", <a different fingerprint>)` and observe `Err(ConflictException)` carrying IDEMPOTENT_PARAMETER_MISMATCH — a 409 for an application that does not exist; any later CreateBenefitApplication reusing that client token is refused for the life of the process.
5. Call `replayed_create("tok-orphan", <the same fingerprint>)` and observe `Ok(None)` — the same token now reads as "a new request", so repeating the create mints a second application under an already-spent token.
