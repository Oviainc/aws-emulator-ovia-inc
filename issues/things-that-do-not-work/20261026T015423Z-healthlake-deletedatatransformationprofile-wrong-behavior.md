Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# DeleteDataTransformationProfile drops the profile but keeps its ClientToken record, so a later create with that token is refused for a profile that no longer exists

_Filed: 2026-10-26T01:54:23Z_
_Operation: DeleteDataTransformationProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteDataTransformationProfile` removes the record from `store.profiles` and touches nothing else. The `profile_client_tokens` entry that `CreateDataTransformationProfile` wrote for that profile survives the deletion and keeps pointing at a `ProfileId` the store no longer holds.

That breaks the invariant this crate states for every other idempotency token it keeps. The data-store side has a helper for exactly this (`forget_client_tokens_for`, "so a token never outlives the data store it names"), called from both reapers, so a data store's token dies with the data store's record; an export/transformation job keeps its token ON the job record for the same stated reason, so the token "cannot resolve to a record that is gone". The profile path is the one place where the token outlives its resource — and, unlike the data-store case, indefinitely, since nothing ever drops a `profile_client_tokens` entry except the `MAX_PROFILE_CLIENT_TOKENS` eviction queue.

It is observable: after Create(ClientToken=T) -> Delete(profile), a Create carrying the same token T with a DIFFERENT request body is answered 400 `ValidationException` "ClientToken was already used for a different CreateDataTransformationProfile request", even though the profile that token minted is gone and no repeat can resolve to it. The same sequence against a data store stops being refused as soon as the deleted record is dropped. The stale entry also keeps consuming one of the bounded token slots, so a create/delete loop evicts live profiles' tokens with records for profiles that no longer exist.

## Evidence

The delete writes only to `profiles` — services/service-com-amazonaws-healthlake-healthlake/src/ops/delete_data_transformation_profile.rs:165-182:
```
        let mut store = state.lock();
        // "Deletes a data transformation profile and all its versions, including the
        // DRAFT and all published versions": every version lives on the one record,
        // so removing it drops them together — and the content bytes and the profile
        // slot it held are given back by the record being dropped.
        let Some(profile) = store.profiles.remove(&input.profile_id) else {
            return Err(not_found(crate::state::PROFILE_NOT_FOUND_MESSAGE));
        };
```
No `profile_client_tokens` entry is removed, and the state module offers no `forget_profile_client_tokens_for` to call.

The invariant the crate states for the OTHER token stores — src/state.rs:1855-1863:
```
    /// Drop every idempotency token that minted `datastore_id`, so a token never
    /// outlives the data store it names.
    fn forget_client_tokens_for(&mut self, datastore_id: &str) {
        self.client_tokens
            .retain(|_, record| record.datastore_id != datastore_id);
```
called from both data-store reapers (src/state.rs:1626 and src/state.rs:1695), and src/state.rs:923-928 on `ExportJob::client_token`:
```
    /// Held ON the job rather than in a second map beside the registry, so a
    /// token is dropped with the job it started (and so with the data store the
    /// job belonged to) and cannot resolve to a record that is gone.
```

The refusal the stale entry produces — src/ops/create_data_transformation_profile.rs:231-242:
```
        if let Some(recorded) = input
            .client_token
            .as_deref()
            .and_then(|token| store.profile_client_tokens.get(token))
            .cloned()
        {
            if recorded.fingerprint != fingerprint {
                return Err(validation(
                    "ClientToken was already used for a different \
                     CreateDataTransformationProfile request",
                ));
            }
```
(the following arm, `if let Some(existing) = store.profiles.get(&recorded.profile_id)`, is the only place the handler notices the profile is gone — and it is reached only when the fingerprints match).

The map and its bound — src/state.rs:1519-1526 ("`CreateDataTransformationProfile` idempotency: `ClientToken` -> what the first call with that token minted. Bounded by [`MAX_PROFILE_CLIENT_TOKENS`]") and src/state.rs:1916-1949 (`record_profile_client_token`, which has no inverse).

Test coverage: tests/all/delete_data_transformation_profile.rs and tests/all/data_transformation_profile.rs exercise delete and the token replay, but no test pins the post-delete behaviour of a reused token, so dropping the entry on delete changes no existing assertion.

## Steps to Reproduce

1. CreateDataTransformationProfile with ClientToken "tok-1", SourceFormat CCDA, ProfileName "p", Source.ProfileMapping {"a.json": "{}"} -> 201, note the ProfileId
2. DeleteDataTransformationProfile with that ProfileId -> 200 (the record is removed from state.profiles)
3. CreateDataTransformationProfile again with the SAME ClientToken "tok-1" but a different body (e.g. ProfileName "q") -> 400 ValidationException "ClientToken was already used for a different CreateDataTransformationProfile request", although the profile that token minted no longer exists
4. Compare the data-store path: CreateFHIRDatastore with ClientToken "tok-2", DeleteFHIRDatastore, wait out DELETED_RETENTION so the record is reaped (src/state.rs:1617-1630 calls forget_client_tokens_for), then CreateFHIRDatastore with "tok-2" and a different body -> accepted, because the token did not outlive its record
