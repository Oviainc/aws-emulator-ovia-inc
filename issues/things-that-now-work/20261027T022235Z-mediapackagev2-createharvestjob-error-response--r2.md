Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# CreateHarvestJob words its own ServiceQuotaExceededException, naming the quota's value, instead of the crate's one quota refusal builder

_Filed: 2026-10-27T02:22:35Z_
_Operation: CreateHarvestJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The state module owns every modelled refusal body this service answers with, and documents the rule: a refusal's sentence depends only on what was refused, names no caller identifier and — for the quota refusal specifically — not the quota's value either, "which is a fact about the account rather than about the refusal". `crate::state::quota_exceeded(ResourceKind)` is that one builder, and `ResourceKind::HarvestJob` already exists (it is what `ResourceKind::HarvestJob.not_found()` uses for the 404 in the same handler).

The three sibling creates all go through it. `CreateHarvestJob` is the only create that constructs `ServiceQuotaExceededException` inline, with a sentence of its own that both breaks the family form ("MediaPackage can't create the resource you requested because you've reached the quota for …") and interpolates the emulator's internal ceiling `MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT` into the body. A client matching this service's 402 sentence sees one wording for channel groups, channels and origin endpoints and a different one for harvest jobs.

This is distinct from the already-fixed CreateHarvestJob error-response issue (20261025T040754Z-…-createharvestjob-error-response), which was about this handler's three `ValidationException` messages; those now route through `crate::state::validation_failed`, while this 402 path still does not route through `crate::state::quota_exceeded`.

## Evidence

The rule and the one builder — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:2100-2105:
```
// The rule they all follow: mediapackagev2 states a refusal in words that depend only
// on WHAT was refused — the kind of resource, the constraint that was violated — and
// never on the identifiers the caller sent. ... so they are chosen together, here, and
// no handler words a refusal of its own.
```
and src/state.rs:2232-2247:
```
/// The 402 a create answers when the collection it would grow is already at its ceiling.
///
/// `ServiceQuotaExceededException` declares no discriminator member, so the sentence is
/// the whole refusal: it names the KIND whose quota is reached and, like every refusal
/// here, neither the caller's names nor the quota's value — which is a fact about the
/// account rather than about the refusal.
pub fn quota_exceeded(kind: ResourceKind) -> ServiceQuotaExceededException {
    let plural = kind.plural();
    ServiceQuotaExceededException {
        message: Some(format!(
            "MediaPackage can't create the resource you requested because you've reached \
             the quota for {plural}. ..."
```

The handler that words its own — src/ops/create_harvest_job.rs:398-412:
```
        // The endpoint's job map never shrinks (a finished job stays readable), so its
        // size is bounded here rather than by how many requests arrive.
        if endpoint.harvest_jobs.len() >= crate::state::MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT {
            return Err(
                CreateHarvestJobErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "An origin endpoint holds at most {} harvest jobs",
                            crate::state::MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT
                        )),
                    },
                )
                .into(),
            );
        }
```

The three siblings that use the shared builder for the same refusal: src/ops/create_channel_group.rs:295-304 `crate::state::quota_exceeded(crate::state::ResourceKind::ChannelGroup)`, src/ops/create_channel.rs:454-459 `crate::state::quota_exceeded(crate::state::ResourceKind::Channel)`, src/ops/create_origin_endpoint.rs:429-436 `crate::state::quota_exceeded(crate::state::ResourceKind::OriginEndpoint)`.

The crate's own request-independence pin: tests/all/error_bodies.rs:14-17 ("The sentence depends only on WHAT was refused … So no refusal message contains a name from the request") and tests/all/refusal_text_and_entity_tags.rs:7-18. No test asserts the harvest-job 402 Message, so the divergence is untested.

## Steps to Reproduce

1. Create a channel group, a channel and an origin endpoint publishing one HLS manifest.
2. POST `/channelGroup/{g}/channel/{c}/originEndpoint/{e}/harvestJob` repeatedly with distinct `HarvestJobName`s until the create is refused 402 `ServiceQuotaExceededException`.
3. Read the body's `Message`: it is `"An origin endpoint holds at most <N> harvest jobs"`, naming the emulator's ceiling.
4. Compare with the 402 from CreateChannel past `MAX_CHANNELS_PER_GROUP` (or CreateOriginEndpoint past its ceiling): `"MediaPackage can't create the resource you requested because you've reached the quota for channels. Delete channels you no longer need, or request a quota increase, and then try again"` — the family form, with no number in it.
