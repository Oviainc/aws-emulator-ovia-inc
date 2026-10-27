Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# CreateDomain applies the tag-count cap before the per-entry tag rule, so the same tags map answers 402 where TagResource answers 400

_Filed: 2026-10-26T23:22:23Z_
_Operation: CreateDomain | Class: error-response_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states the two `TagMap` rules once each in `src/service/tag_bounds.rs` — `violation` (the per-ENTRY key/value length + reserved-prefix rule, refused 400 `ValidationException`) and `merged_within_cap` (the per-RESOURCE count, refused 402 `ServiceQuotaExceededException`) — precisely so the two operations that accept a `TagMap` cannot enforce them differently. But the two handlers run them in OPPOSITE order:

* `CreateDomain` runs `merged_within_cap` FIRST and `violation` second.
* `TagResource` runs `violation` FIRST and `merged_within_cap` second.

So a `tags` map that breaks BOTH rules (more than `MAX_TAGS_PER_RESOURCE` entries, one of which has an unacceptable key/value — e.g. an empty key or an `aws:`-prefixed key) is refused with HTTP 402 `ServiceQuotaExceededException` by `CreateDomain` and with HTTP 400 `ValidationException` by `TagResource`. One byte-identical tag map therefore earns two different status codes and two different error codes depending on which operation carries it, which is exactly the divergence the shared helpers and the crate's own test suite say cannot happen. A client that branches on the error code to decide whether to drop tags (quota) or fix them (invalid entry) is told opposite things by the two operations for the same map.

## Evidence

`CreateDomain` counts first, validates entries second — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_domain.rs:276-289`:
```
        let tags = crate::service::tag_bounds::merged_within_cap(
            ::std::collections::BTreeMap::new(),
            input.tags.unwrap_or_default(),
        )
        .map_err(|count| {
            CreateDomainErrorModeled::ServiceQuotaExceededException(
```
and only then `create_domain.rs:296-298`:
```
        if let Some(violation) = crate::service::tag_bounds::violation(tags.iter()) {
            return Err(crate::scaffolded::Scaffolded::validation(violation));
        }
```

`TagResource` validates entries first, counts second — `src/ops/tag_resource.rs:225-227`:
```
        if let Some(violation) = crate::service::tag_bounds::violation(input.tags.iter()) {
            return Err(crate::scaffolded::Scaffolded::validation(violation));
        }
```
and only then `tag_resource.rs:235-249`:
```
        let merged = crate::service::tag_bounds::merged_within_cap(
            store.tags_for(&input.resource_arn),
            input.tags,
        )
        .map_err(|count| {
            TagResourceErrorModeled::ServiceQuotaExceededException(
```

The rule the crate states for itself, `src/service/tag_bounds.rs:7-9`: "//! Both halves live here: [`violation`] is the per-ENTRY rule and [`merged_within_cap`]\n//! the per-RESOURCE count, each stated once so no two operations that accept a `TagMap`\n//! can enforce different halves — or different readings — of one bound." And `src/ops/tag_resource.rs:220-223` claims the same parity for the entry check: "The SAME\n        // check `CreateDomain` applies to the tags a create carries, so neither can\n        // enforce half the rule".

The test suite states the same rule, `tests/all/tag_map_bounds.rs:10-12`: "//! The rule belongs to the `TagMap` shape, not to one operation, so every\n//! operation that accepts one — `CreateDomain`'s `tags` and `TagResource`'s `tags` —\n//! enforces it identically, and the two word the same violation the same way."

Test coverage: `tests/all/tag_map_bounds.rs` drives both operations with maps whose entry count is well inside the cap, and `tests/all/tag_count_cap.rs:94-269` drives both operations with over-cap maps whose every entry is valid (`tag_map(count, 8, 8)`), so no existing row carries a map that breaks BOTH rules at once and nothing pins the precedence — the divergence is unobserved by the suite.

## Steps to Reproduce

1. Start the emulator and build a `tags` map with `MAX_TAGS_PER_RESOURCE + 1` entries, one of which has an empty key (e.g. {"": "v", "k0": "v0", ... }).
2. POST /domain with {"name":"clinic","tags": <that map>} and observe HTTP 402 with error code `ServiceQuotaExceededException` (the count check at create_domain.rs:276 fires first).
3. POST /domain with {"name":"clinic"} alone, keep the returned `arn`.
4. POST /tags/<percent-encoded arn> with {"tags": <the same map>} and observe HTTP 400 with error code `ValidationException` (the entry check at tag_resource.rs:225 fires first).
5. Note that one identical tag map earned two different statuses and two different error codes from the two operations the crate documents as enforcing the same rules identically.
