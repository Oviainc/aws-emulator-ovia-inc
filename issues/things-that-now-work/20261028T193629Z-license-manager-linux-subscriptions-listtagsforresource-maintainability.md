Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 160eac1be171dea85ae8441ad5f20b2dc0c6ebbb

# The ListTagsForResource reply→tag-set projection is implemented five times across this crate's suites

_Filed: 2026-10-28T19:36:29Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `160eac1be171dea85ae8441ad5f20b2dc0c6ebbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The projection "read the `tags` map out of a `ListTagsForResource` reply and return it as a sorted `BTreeMap<String, String>`, panicking with the case label when the member is missing or an entry is not a string" is written out FIVE times in this crate's own test tree, even though `tests/all/harness.rs` is the module that owns this vocabulary and already owns the `list_tags` call every copy is built on.

The copies have ALREADY drifted: four of them state one panic message ("no tags map in …") while `resource_tags.rs` splits the same failure into two different ones ("no tags member in …" / "tags is not a map in …"), so the same malformed reply is reported differently depending on which suite observed it. Two of the copies (`tag_removal.rs` and `tag_selection_limit.rs`) are byte-identical bodies under different doc comments, and `shared_tag_merge.rs` carries the same body again under a third name (`reported_set`).

The fix is additive and response-neutral: state the projection ONCE on `tests/all/harness.rs` — beside `list_tags`, in the sync form over `&Reply` and the async form over an ARN the suites use — and delete the five private copies. No handler, no response body and no existing assertion changes; the suites keep asserting exactly the sets they assert today.

Precedent in this same crate: the already-fixed issue `20261027T050231Z-license-manager-linux-subscriptions-tagresource-maintainability--r2` ("Collapse the two private copies of the test harness onto tests/all/harness.rs") collapsed the same class of per-suite copy, and `tests/all/shared_harness.rs` is the suite that pins the result — yet that very file carries one of the five copies of this projection.

## Evidence

The rule the harness module states for itself, `tests/all/harness.rs:3-4`:
```
//! The shared wire harness for this crate's suites: one emulator, one request builder,
//! and one reply assertion vocabulary, so no suite grows its own.
```

Copy 1 — `tests/all/tag_removal.rs:96-115`:
```
/// The tag set a `ListTagsForResource` body reports, as a sorted map so two sets compare
/// independently of iteration order.
async fn reported_tags(h: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let reply = list_tags(h, arn).await;
    let body = reply.ok(case);
    body.get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {body:?}"))
        .iter()
        .map(|(key, value)| { ( key.clone(), value.as_str().unwrap_or_else(|| panic!("case {case:?}: tag {key:?} is not a string")).to_string(), ) })
        .collect()
}
```

Copy 2 — `tests/all/tag_selection_limit.rs:183-202`, the same body verbatim:
```
/// The tag set `arn` reports, as a sorted map so two sets compare independently of
/// iteration order.
async fn reported_tags(h: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let reply = list_tags(h, arn).await;
    let body = reply.ok(case);
    body.get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {body:?}"))
```

Copy 3 — `tests/all/shared_tag_merge.rs:157-177`, same logic under a third name:
```
/// The tag set the resource `arn` carries, read back over the wire through
/// `ListTagsForResource`.
async fn reported_set(h: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let reply = list_tags(h, arn).await;
    reply.ok(case).get("tags").and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {:?}", reply.body))
```

Copy 4 — `tests/all/shared_harness.rs:131-137` (in the very suite that pins "the ONE wire harness of this crate"):
```
/// The tag set a `ListTagsForResource` body reports, as the same sorted map.
fn reported_tags(reply: &Reply, case: &str) -> BTreeMap<String, String> {
    reply.ok(case).get("tags").and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {:?}", reply.body))
```

Copy 5 — `tests/all/resource_tags.rs:141-149`, the copy that has already DRIFTED (two panic messages instead of one):
```
/// The tag set a `ListTagsForResource` body reports, as a sorted map so two sets compare
/// independently of iteration order.
fn reported_tags(reply: &Reply, case: &str) -> BTreeMap<String, String> {
    let body = reply.ok(case);
    let tags = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: no tags member in {body:?}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: tags is not a map in {body:?}"))
```

The handler these copies read is unchanged by the fix — `src/ops/list_tags_for_resource.rs:168-177` returns the stored set and nothing here touches it.

Tests that must stay green, unchanged, and that already cover every copy: `tests/all/tag_removal.rs` (`a_removal_deletes_exactly_the_named_keys_and_nothing_else`, `a_removed_key_can_be_written_again_and_the_record_is_otherwise_untouched`, `a_refused_removal_leaves_every_stored_set_as_it_was`), `tests/all/tag_selection_limit.rs` (`a_tag_selection_past_the_modelled_maximum_is_refused_by_the_tag_surface`, `a_tag_selection_within_the_modelled_maximum_is_accepted`), `tests/all/shared_tag_merge.rs` (`both_tag_merge_paths_obey_the_one_resultant_set_bound`), `tests/all/resource_tags.rs` and `tests/all/shared_harness.rs`.

## Steps to Reproduce

1. grep -n 'fn reported_tags\\|fn reported_set' services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/tests/all/*.rs — five definitions are reported (tag_removal.rs:98, tag_selection_limit.rs:185, shared_tag_merge.rs:159, resource_tags.rs:143, shared_harness.rs:132).
2. Compare the five bodies: all read `tags` out of a `ListTagsForResource` reply into a sorted `BTreeMap<String, String>` with the same per-entry panic; `resource_tags.rs` already words the missing/non-map failure with two different sentences while the other four share one.
3. Read tests/all/harness.rs:3-4, which states that the crate has 'one reply assertion vocabulary, so no suite grows its own', and note that `list_tags` (harness.rs:302) — the call every copy wraps — already lives there while the projection does not.
4. Change the shape of the `tags` member assertion in one copy (e.g. the panic text) and observe that the other four keep their own wording: the rule is not stated in one place, so the copies can and already do drift.
