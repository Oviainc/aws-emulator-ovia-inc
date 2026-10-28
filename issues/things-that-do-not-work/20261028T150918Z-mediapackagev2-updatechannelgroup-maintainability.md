Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# The commit rule (bump version, restamp ModifiedAt, re-derive the ETag) is written out four times, once per persisted record

_Filed: 2026-10-28T15:09:18Z_
_Operation: UpdateChannelGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`VersionedRecord` exists precisely so the "did this write change the record?" question is decided in ONE place, and its `commit_version` doc states the commit rule once: "Commit a change: bump the version, stamp `at` on `ModifiedAt`, re-derive the tag." The rule itself is then implemented four separate times — one `commit` body per persisted record (`ChannelGroup`, `Channel`, `OriginEndpoint`, `HarvestJobRecord`) — plus four identical `commit_version` bodies that do nothing but forward to them. The four copies are byte-identical except for which tag derivation they call, so they can drift: a fifth record, or an edit to one copy (forgetting `saturating_add`, or restamping `ModifiedAt` before bumping the version), would leave two records committing by different rules while every conditional writer in the crate assumes they commit by one.

The rule has exactly one variable part — how that record's entity tag is derived — which each type already owns (`recompute_etag` on three of them, `origin_endpoint_etag` for the endpoint). Moving the three-step body onto the trait as `commit_version`'s default, with the tag derivation as the per-type hook, leaves every response and every existing test unchanged: the computed version, `ModifiedAt` and ETag are identical, and the four `commit_version` forwarders disappear with it.

## Evidence

The rule, stated once on the trait — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:840-841:
```
    /// Commit a change: bump the version, stamp `at` on `ModifiedAt`, re-derive the tag.
    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime);
```

Copy 1 — src/state.rs:961-966:
```
    /// Commit a change: bump the version, stamp `modified_at`, and derive the new ETag.
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }
```

Copy 2 — src/state.rs:1057-1062:
```
    /// Commit a change. See [`ChannelGroup::commit`].
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }
```

Copy 3 — src/state.rs:1148-1157:
```
    /// Commit a change. See [`ChannelGroup::commit`].
    ///
    /// The tag is derived by [`origin_endpoint_etag`], the ONE place an endpoint's ETag
    /// comes from, ...
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.etag = origin_endpoint_etag(self);
    }
```

Copy 4 — src/state.rs:1218-1223:
```
    /// Commit a change. See [`ChannelGroup::commit`].
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }
```

And the four identical forwarders that exist only to connect them to the trait — src/state.rs:886-888, 908-910, 926-928, 941-943, each:
```
    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
```

Callers that depend on all four committing by the same rule: src/ops/update_channel_group.rs:233 `group.commit_if_changed(&before, crate::state::now());`, src/ops/update_channel.rs:392, src/ops/update_origin_endpoint.rs:535, src/state.rs:664/673/688/707 (`with_tags_mut`, reached from TagResource and UntagResource) and src/ops/cancel_harvest_job.rs:237 `job.commit(at);`.

Test coverage that pins the behaviour the consolidation must not change: tests/all/no_op_write_commits_nothing.rs ("a write commits a new version of the resource it addresses — restamping `ModifiedAt` and rotating the entity tag — only when it CHANGED that resource's stored content", rows over UpdateChannelGroup, UpdateChannel, UpdateOriginEndpoint, TagResource, UntagResource) and tests/all/shared_entity_tag_precondition.rs (all four conditional writers, including CancelHarvestJob, plus the clock-driven harvest-job commit in `settle_job`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs and read the `VersionedRecord::commit_version` declaration at line 840-841: the commit rule is stated once.
2. Read lines 961-966, 1057-1062, 1148-1157 and 1218-1223: four `commit` bodies, each performing the same three steps, differing only in the tag-derivation call.
3. Read lines 886-888, 908-910, 926-928 and 941-943: four identical `commit_version` bodies that only forward to those.
4. Move the three-step body into `commit_version`'s default implementation on the trait, with the per-record tag derivation as the trait's only required hook, and delete the four forwarders.
5. Run the crate's tests (tests/all/no_op_write_commits_nothing.rs and tests/all/shared_entity_tag_precondition.rs in particular): every version, ModifiedAt and ETag must be unchanged.
