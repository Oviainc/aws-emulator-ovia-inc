Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ae9fe3612b346aad8c571a3d654fb37d429bcc9f

# The store-wide profile retained-byte ceiling is charged only by PublishDataTransformationProfile, so creates blow past it and then every publish is refused for a quota no publish consumed

_Filed: 2026-10-29T16:00:47Z_
_Operation: CreateDataTransformationProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `ae9fe3612b346aad8c571a3d654fb37d429bcc9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` declares `MAX_PROFILE_RETAINED_BYTES_TOTAL` as the bytes of version content EVERY data transformation profile retains together, and gives it one stated purpose: so `MAX_PROFILES` profiles at their own budget "cannot multiply into an unbounded heap". The crate also provides the helper that measures it, `ComAmazonawsHealthlakeHealthlakeState::profile_retained_bytes_total()`, documented as "what [`MAX_PROFILE_RETAINED_BYTES_TOTAL`] bounds".

Only ONE handler calls that helper: `PublishDataTransformationProfile`. `CreateDataTransformationProfile` — which MINTS a whole new DRAFT whose content `validate_profile_mapping` admits up to `MAX_PROFILE_MAPPING_BYTES` (1 MiB) — charges only the profile COUNT ceiling (`MAX_PROFILES`, 100) and never consults `profile_retained_bytes_total()`. `UpdateDataTransformationProfile`, which replaces the DRAFT with up to another 1 MiB of content, charges neither total either. So 33 creates of a ~1 MiB mapping retain ~33 MiB and 100 creates retain ~100 MiB — more than 3x the 32 MiB the constant declares — with no refusal, although `CreateDataTransformationProfile` already models (and already raises) `ServiceQuotaExceededException` for its other ceiling.

The divergence is caller-visible, not just a heap concern: because the total IS charged in `PublishDataTransformationProfile`, once creates have pushed the store past 32 MiB every subsequent publish of ANY profile is refused 400 `ServiceQuotaExceededException` ("Publishing this version would pass the 33554432 bytes of profile content every profile retains together") — the ceiling is enforced on the one operation that did not cause the overshoot, and the operation that did cause it is answered 201.

This is not a new feature: the helper exists and is already wired into a sibling handler, and the crate's own sibling resource does exactly the check this path skips — `CreateFHIRDatastore` charges `store.fhir_resource_bytes_total()` against `MAX_FHIR_RESOURCE_BYTES_TOTAL` when it mints a preloaded data store, and `fhir_rest::admits` charges the same total on every FHIR write. The profile ceiling's own doc names that FHIR budgeting as the pattern it follows.

## Evidence

THE RULE, in this crate's own state module — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:495-498:
```
/// The most BYTES of version content EVERY data transformation profile retains
/// together, so [`MAX_PROFILES`] profiles at their own budget cannot multiply into
/// an unbounded heap.
pub const MAX_PROFILE_RETAINED_BYTES_TOTAL: usize = 32 << 20;
```
and src/state.rs:469-476, which states that the per-version ceiling is NOT the bound and names the pattern to follow:
```
/// It bounds ONE version; a profile holds up to
/// `1 + MAX_PROFILE_VERSION` of them once `PublishDataTransformationProfile`
/// snapshots the DRAFT into published versions, so what one profile retains is
/// bounded by [`MAX_PROFILE_RETAINED_BYTES`] and what every profile retains
/// together by [`MAX_PROFILE_RETAINED_BYTES_TOTAL`] — the way the FHIR data plane
/// budgets [`MAX_FHIR_RESOURCE_BYTES`], [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`]
/// and [`MAX_FHIR_RESOURCE_BYTES_TOTAL`] — so no product of caller-chosen factors
/// is the bound.
```

THE HELPER THAT MEASURES IT — src/state.rs:1783-1795:
```
    /// How many BYTES of profile content every data transformation profile
    /// retains together — what [`MAX_PROFILE_RETAINED_BYTES_TOTAL`] bounds.
    ...
    pub fn profile_retained_bytes_total(&self) -> usize {
        self.profiles.values().fold(0usize, |total, profile| {
            total.saturating_add(profile.retained_bytes())
        })
    }
```

THE ONE PATH THAT CALLS IT — src/ops/publish_data_transformation_profile.rs:210-214 and 271-277:
```
        let mut store = state.lock();
        // What the whole store retains, read before the profile this call writes is
        // borrowed from it, so the ceiling below is charged against every profile
        // together and not just this one.
        let retained_total = store.profile_retained_bytes_total();
...
        if retained_total.saturating_add(bytes) > crate::state::MAX_PROFILE_RETAINED_BYTES_TOTAL {
            return Err(quota_exceeded(format!(
                "Publishing this version would pass the {} bytes of profile content every \
                 profile retains together",
                crate::state::MAX_PROFILE_RETAINED_BYTES_TOTAL
            )));
        }
```

THE MINT THAT SKIPS IT — src/ops/create_data_transformation_profile.rs:264-282 (only the COUNT ceiling is charged; `profile_retained_bytes_total` is never called in this file), and the profile is then inserted at :319:
```
        let mapping = initial_mapping(&store, &input)?;
        crate::state::validate_profile_mapping(&mapping).map_err(validation)?;

        // The store holds a bounded number of profiles; past the ceiling this
        // operation answers with the quota refusal it models.
        if store.profiles.len() >= crate::state::MAX_PROFILES {
            return Err(
                CreateDataTransformationProfileErrorModeled::ServiceQuotaExceededException(
...
        store.profiles.insert(id.clone(), profile);
```
where `validate_profile_mapping` bounds ONE mapping only — src/state.rs:1685-1691 against `MAX_PROFILE_MAPPING_BYTES` (`1 << 20`, src/state.rs:480).

THE SECOND PATH THAT SKIPS IT — src/ops/update_data_transformation_profile.rs:179-202 (per-version bytes validated, then the DRAFT is rewritten; neither total is charged):
```
        crate::state::validate_profile_mapping(&mapping).map_err(validation)?;

        let mut store = state.lock();
        let Some(profile) = store.profiles.get_mut(&input.profile_id) else {
...
        profile.draft.mapping = mapping;
```

THE SIBLING RESOURCE DOES CHARGE ITS TOTAL ON A MINT — src/ops/create_fhir_datastore.rs:342-357:
```
        if store
            .fhir_resource_bytes_total()
            .saturating_add(fhir_resource_bytes)
            > crate::state::MAX_FHIR_RESOURCE_BYTES_TOTAL
        {
            return Err(CreateFHIRDatastoreErrorModeled::ThrottlingException(
```
and on every data-plane write — src/service/fhir_rest.rs:820-835 (`admits`, "The three ceilings this emulator holds the FHIR data plane to, in one place so a create and an update charge them identically", fhir_rest.rs:780-785).

TEST COVERAGE: the suites pin the per-MAPPING ceiling (tests/all/data_transformation_profile.rs:494-496, tests/all/update_data_transformation_profile.rs:494-497) and the per-PROFILE ceiling on publish (tests/all/publish_data_transformation_profile.rs:720-781, `a_profile_runs_out_of_retained_bytes`). No test exercises `MAX_PROFILE_RETAINED_BYTES_TOTAL`, and every existing test creates only small profiles, so charging the total on create/update leaves them unchanged.

## Steps to Reproduce

1. Call CreateDataTransformationProfile 40 times, each with SourceFormat=CCDA and Source.ProfileMapping carrying ~1 MiB of content (e.g. 11 entries whose values are 95 000 characters each — inside the modelled ProfileMapping bounds and inside the crate's 1 MiB per-mapping ceiling).
2. Observe all 40 creates answer 201: the store now retains ~40 MiB of profile content, past the 32 MiB `MAX_PROFILE_RETAINED_BYTES_TOTAL` the crate declares, and `profile_retained_bytes_total()` is never consulted on this path.
3. Pick any one of those profiles and call PublishDataTransformationProfile with its ProfileId and SourceFormat=CCDA.
4. Observe 400 ServiceQuotaExceededException "Publishing this version would pass the 33554432 bytes of profile content every profile retains together" — the store-wide ceiling is charged against the publish, although only the creates consumed it, and no create was ever refused.
5. For the second path: create one profile with a small mapping, then call UpdateDataTransformationProfile with a ~1 MiB ProfileMapping repeatedly across many profiles; each update is answered 200 and grows what the store retains without either the per-profile or the store-wide total being charged.
