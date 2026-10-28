Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# The ARN-to-tag-set resolution rule (search jobs first, then export jobs) is implemented twice in state.rs

_Filed: 2026-10-27T13:42:51Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that decides WHICH record a tag operation's `ResourceArn` names — scan the held search jobs for a record whose `arn` equals it, and only if none matches scan the held export jobs — is written out twice in this crate: once in `tags_by_arn` (the read path, used by `ListTagsForResource`) and once again in `tags_by_arn_mut` (the write path, used by `TagResource` and `UntagResource`). Both copies encode the same two-step order and the same "no record matched" outcome that becomes the tag APIs' not-found refusal, so the two can drift: a change to the resolution (a third taggable record type, or a change in which record wins when two carry the same ARN) has to be made in both places, and a change made in only one leaves the read path resolving an ARN the write path refuses, or the reverse. Collapsing the two onto one resolution step leaves every response and every existing test byte-identical.

## Evidence

Copy 1 — read path (src/state.rs:1213-1224):
```
pub fn tags_by_arn(&self, arn: &str) -> Option<&HashMap<String, String>> {
    self.search_jobs
        .values()
        .find(|job| job.arn == arn)
        .map(|job| &job.tags)
        .or_else(|| {
            self.export_jobs
                .values()
                .find(|job| job.arn == arn)
                .map(|job| &job.tags)
        })
}
```
Copy 2 — write path, same two-step rule restated (src/state.rs:1229-1251):
```
pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
    if let Some(identifier) = self.search_jobs.values().find(|job| job.arn == arn).map(|job| job.identifier.clone()) {
        return self.search_jobs.get_mut(&identifier).map(|job| &mut job.tags);
    }
    let identifier = self.export_jobs.values().find(|job| job.arn == arn).map(|job| job.identifier.clone())?;
    self.export_jobs.get_mut(&identifier).map(|job| &mut job.tags)
}
```
Its own doc comment states the two are meant to be the same rule (src/state.rs:1226-1228): "The tag set stored on the resource whose minted ARN is `arn`, mutably — the same resolution [`tags_by_arn`](Self::tags_by_arn) performs, for `TagResource` and `UntagResource`."
Call sites: src/ops/list_tags_for_resource.rs:157 (`state.tags_by_arn(...)`), src/ops/tag_resource.rs:167 and src/ops/untag_resource.rs:169 (`state.tags_by_arn_mut(...)`).
Test coverage that must stay unchanged: tests/all/resource_tags.rs:310 `the_tags_of_every_resource_this_service_mints_are_read_back_added_to_and_removed`, :377 `a_tag_write_touches_only_the_resource_its_arn_names`, :461 `an_arn_no_held_resource_matches_is_not_found_on_every_tag_operation`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs and read `tags_by_arn` (lines 1213-1224) and `tags_by_arn_mut` (lines 1229-1251).
2. Observe both spell out the same resolution order: match an `arn` against `search_jobs` first, then against `export_jobs`, then `None`.
3. Change the rule in one copy only (e.g. add a third record type, or swap the order) and note that the read path and the write path of the tag APIs then disagree about which ARNs resolve, with no test or type forcing them back together.
