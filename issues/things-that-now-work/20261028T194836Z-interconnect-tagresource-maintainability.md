Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# The "merge a tag write, never replace" rule is implemented twice, once per tag writer

_Filed: 2026-10-28T19:48:36Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately centralises the JUDGEMENT half of a tag write in one helper (`crate::state::tag_write_violation`, whose doc says "one rule decides what a stored tag set can be however it was reached"), but the APPLICATION half — merge the incoming entries over the held ones, and leave the `None` representation of "no tags" alone when the request carries an empty map — is written out twice, once in `TagResource::handle` and once in `AcceptConnectionProposal::handle`. The two copies are character-for-character the same computation on the same field (`stored.connection.tags`), so a change to the merge semantics or to the empty-map guard in one writer silently leaves the other behind — which is precisely the drift the crate already treats as a defect for the bandwidth rule (fixed issue 20261024T233253Z-interconnect-createconnection-maintainability) and for the attach-point verdict mapping (fixed issue 20261028T013820Z-interconnect-acceptconnectionproposal-maintainability). Extracting one `merge_tags(&mut Option<HashMap<..>>, &HashMap<..>)` helper beside `tag_write_violation` leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-interconnect-interconnect/src/ops/tag_resource.rs:241-252:
```
        if !input.tags.is_empty() {
            stored
                .connection
                .tags
                .get_or_insert_with(::std::collections::HashMap::new)
                .extend(
                    input
                        .tags
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
        }
```
Copy 2 — src/ops/accept_connection_proposal.rs:426-435:
```
        if let Some(tags) = input.tags.as_ref().filter(|tags| !tags.is_empty()) {
            stored
                .connection
                .tags
                .get_or_insert_with(::std::collections::HashMap::new)
                .extend(
                    tags.iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
        }
```
Both sites restate the same rule in their comments — tag_resource.rs:234-240 ("Merge, never replace ... \"No tags\" has ONE representation in the store (`None` ...), so a request carrying no tag leaves that representation alone instead of turning it into an empty map.") and accept_connection_proposal.rs:422-425 ("Merge, never replace, the way every tag writer of this service does: each key the request carries is written over whatever the connection held for it, and \"no tags\" keeps its one representation in the store (`None`) when the request carries none.").

The single-rule principle the crate already applies to the other half of the same write — src/state.rs:988-993:
```
/// Both of this service's tag writers (tag-on-create and the tagging operation) judge
/// their input here, so one rule decides what a stored tag set can be however it was
/// reached
```

Test coverage that pins the behaviour both copies must keep producing: tests/all/tag_resource.rs and tests/all/resource_tags.rs (merge-over-held keys, empty request leaves the set alone), and tests/all/accept_connection_proposal.rs:103-108 and :349-353, which assert "the acceptance's tags merged onto the proposal's" via `merged_tags()`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-interconnect-interconnect/src/ops/tag_resource.rs at lines 234-252 and src/ops/accept_connection_proposal.rs at lines 422-435 side by side.
2. Observe the identical `get_or_insert_with(HashMap::new).extend(..)` merge guarded by the same "incoming map is non-empty" condition, restating the same rule in both comments.
3. Change the merge semantics in one site only (for example, make the empty-map guard unconditional) and observe that the other writer keeps the old behaviour: tests/all/tag_resource.rs and tests/all/accept_connection_proposal.rs exercise the two copies independently, so neither suite can catch the divergence introduced in the other handler.
4. Compare with src/state.rs:988-993, where the judgement half of the same rule is already shared by both writers.
