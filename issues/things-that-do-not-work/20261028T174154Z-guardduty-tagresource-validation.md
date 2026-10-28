Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 145e568e7ef9b3115be9abe08039cd724717eead

# The per-resource tag ceiling is 200, on a comment that claims the model states no figure — the model states "a limit of 50 tags per resource", and the crate quotes that sentence itself

_Filed: 2026-10-28T17:41:54Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `145e568e7ef9b3115be9abe08039cd724717eead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` refuses an add only once a resource would exceed `crate::state::TAGS_PER_RESOURCE` = 200 tags, so tags 51..200 on one GuardDuty resource (detector, filter, IPSet, threat intel set, trusted/threat entity set, publishing destination) are accepted and persisted.

The bound is defended by a comment in `state.rs` that states, as its whole justification, that the service model carries no per-resource tag figure and that 200 therefore has to be the emulator's own choice, taken from `TagMap`'s `@length(1..200)`. That justification is false: the model's `ListTagsForResource` operation documentation states the figure outright — "Tagging is currently supported for detectors, finding filters, IP sets, threat intel sets, and publishing destination, with a limit of 50 tags per resource." The crate even reproduces that sentence verbatim in its own handler header for `ListTagsForResource`, so the model text and the comment denying it sit in the same crate.

`TagMap`'s `@length(1..200)` bounds how many pairs ONE request's map may carry, which is a different quantity from how many tags one resource may hold; the operation documentation is what states the latter. So the handler currently accepts a resource state the model documents as impossible, and the only thing standing behind it is a comment the model contradicts.

## Evidence

THE MODEL SENTENCE — `smithy_query(query="doc", shape="ListTagsForResource")`:
```
"<p>Lists tags for a resource. Tagging is currently supported for detectors, finding filters, IP sets, threat intel sets, and publishing destination, with a limit of 50 tags per resource. When invoked, this operation returns all assigned tags for a given resource.</p>"
```

THE SAME SENTENCE, QUOTED INSIDE THIS CRATE — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/list_tags_for_resource.rs:60-63:
```
//! Lists tags for a resource. Tagging is currently supported for detectors, finding
//! filters, IP sets, threat intel sets, and publishing destination, with a limit of 50 tags
//! per resource. When invoked, this operation returns all assigned tags for a given
//! resource.
```

THE CRATE'S CONTRADICTING CLAIM AND THE CONSTANT — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:8137-8152:
```
/// The most tags one resource carries.
///
/// Neither the model nor this service's documentation carries a tags-per-resource
/// figure, so — like [`PUBLISHING_DESTINATIONS_PER_DETECTOR`] — this is an
/// EMULATOR-CHOSEN bound rather than a published quota: a 201st tag is refused
/// here, where the real service may refuse it sooner. The number is taken from the
/// map that both carries tags in and reports them (`TagMap` is `len 1..200` —
/// `smithy_query(shape="TagMap")`), because a resource holding more than that
/// could not be reported by the `Tags` member of its own read at all.
...
pub const TAGS_PER_RESOURCE: usize = 200;
```

THE REFUSAL MESSAGE BUILT FROM IT — src/state.rs:8160-8164:
```
pub fn tag_limit_message() -> String {
    format!(
        "The request is rejected because the resource has reached the limit of {TAGS_PER_RESOURCE} tags."
    )
}
```

THE ENFORCEMENT SITE — src/ops/tag_resource.rs:174-183:
```
        let added = input
            .tags
            .keys()
            .filter(|key| !tags.contains_key(key.as_str()))
            .count();
        if tags.len() + added > crate::state::TAGS_PER_RESOURCE {
            return Err(crate::scaffolded::Scaffolded::invalid_input(
                crate::state::tag_limit_message(),
            ));
        }
```

TEST COVERAGE: tests/all/resource_tag_addition.rs, tests/all/resource_tag_listing.rs, tests/all/resource_tag_removal.rs and tests/all/tag_map_projection.rs drive the tag surface against every taggable resource class, but none of them adds more than a handful of tags, so no test pins 200 (or 50) and no test states a reason for the divergence.

## Steps to Reproduce

1. `POST /detector` to create a detector in the request Region and note the `detectorId`.
2. `POST /tags/arn%3Aaws%3Aguardduty%3A<region>%3A<account>%3Adetector%2F<detectorId>` with a `tags` map of 50 distinct keys — accepted (HTTP 204).
3. Repeat with 50 further distinct keys (`k51`..`k100`). Expected per the model sentence quoted in list_tags_for_resource.rs:60-63 ("a limit of 50 tags per resource"): refused. Actual: HTTP 204, and the resource now carries 100 tags.
4. `GET /tags/<same ARN>` — `ListTagsForResource` reports all 100 tags, a resource state the operation's own modelled documentation says cannot exist.
5. Keep going to 200 keys: still accepted. Only the 201st distinct key is refused, with `tag_limit_message()` naming 200 — the number state.rs:8137-8152 says it had to invent because "neither the model nor this service's documentation carries a tags-per-resource figure".
