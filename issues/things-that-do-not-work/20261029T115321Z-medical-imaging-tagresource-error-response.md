Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: da4330c5500325c8e5ff90f3211b2833b72817eb

# The two tag-map rules are applied in opposite order on the two write paths, so one bad map is refused for different reasons

_Filed: 2026-10-29T11:53:21Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `da4330c5500325c8e5ff90f3211b2833b72817eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDatastore` and `TagResource` enforce the same two tag-map rules — the per-resource tag count (`MAX_TAGS_PER_RESOURCE`) and the `TagKey` constraints (`tag_map_violation`) — and `CreateDatastore` says so explicitly, adding WHY it checks them in that order: "The ENTRY COUNT first, as the boundary validator checks a collection's size before its elements". `TagResource` applies them the other way round: it runs the KEY check first (before any state is touched) and the count check only after the resource has been resolved.

So for one and the same tag map that breaks both rules — say 51 entries, one of them keyed `aws:owner` — `CreateDatastore` answers the count clause ("this emulator holds at most 50 tags per resource; this request would leave 51") while `TagResource` answers the key clause ("...failed to satisfy constraint: Member must satisfy regular expression pattern: (?!aws:)[a-zA-Z+-=._:/]+"). A client cannot rely on which rule a tagging write reports, even though the crate states the two paths apply the same two rules.

The count check's position inside `TagResource` is inconsistent in a second way: the KEY rule is stated before the store is reached (so an illegal key is a 400 whatever the ARN names), while the COUNT rule is stated after `writable_resource_tags_mut`, so an over-large map aimed at a resource this emulator does not hold answers 404 and the same map aimed at an existing resource answers 400. `CreateDatastore` states both before anything is read.

## Evidence

`CreateDatastore` — count first, with the stated rationale and the claim that these are the same two rules the tag path applies:

- `src/ops/create_datastore.rs:157-186`: "Both rules below belong to the TAG MAP rather than to this operation, so they are the same two `TagResource` applies — a map one write path refuses cannot be installed through the other. // // The ENTRY COUNT first, as the boundary validator checks a collection's size before its elements: one call may not install more tags than a resource holds" -> `if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE { return Err(... too_many_tags_message(tags.len()) ...) }` then `if let Some(violation) = crate::state::tag_map_violation("tags", tags) { ... }`

`TagResource` — keys first, count after the resource lookup:

- `src/ops/tag_resource.rs:177-187`: "The KEYS, against every constraint the model puts on a `TagKey` — the same check `CreateDatastore` applies, because what makes a tag map legal belongs to the map and not to the operation installing it." -> `if let Some(violation) = crate::state::tag_map_violation("tags", &input.tags) { return Err(...ValidationException...) }`
- `src/ops/tag_resource.rs:206-226`: `let tags = store.writable_resource_tags_mut(&region, &arn).map_err(|missing| ...ResourceNotFoundException...)?;` ... `if tags.len() + added > crate::state::MAX_TAGS_PER_RESOURCE { return Err(...too_many_tags_message(tags.len() + added)...) }`

One bound, one wording, for both paths — which is what makes the differing ORDER the only divergence: `src/state.rs:624-636` (`too_many_tags_message`): "One sentence for both write paths: the create path and the tag path enforce one bound, so they state it with one text."

Test coverage: `tests/all/tag_map_validation.rs:236` (`an_illegal_tag_key_is_refused_by_create_datastore_and_creates_nothing`), `:289` (`an_illegal_tag_key_is_refused_by_the_tagging_routes_and_changes_nothing`) and `:403` (`no_write_path_installs_more_tags_than_a_resource_holds`) drive each rule on its own — `distinct_tags(n)` builds only LEGAL keys for the count rows — so no case drives a map that breaks both rules and nothing pins today's precedence on either path.

## Steps to Reproduce

1. CreateDatastore with a `tags` map of 51 entries, one of them keyed `aws:owner`: observe HTTP 400 ValidationException whose message is the COUNT clause ("this emulator holds at most 50 tags per resource; this request would leave 51").
2. Create a data store with no tags, let it settle into ACTIVE, and read its ARN with GetDatastore.
3. Call TagResource against that ARN with the identical 51-entry map: observe HTTP 400 ValidationException whose message is the KEY clause about the `aws:` prefix instead — the same map, the same two broken rules, a different reported cause.
4. Call TagResource with that 51-entry map against an ARN whose datastore id this emulator never minted: observe HTTP 404 ResourceNotFoundException, while the illegal KEY in the same map would have answered 400 before the lookup.
