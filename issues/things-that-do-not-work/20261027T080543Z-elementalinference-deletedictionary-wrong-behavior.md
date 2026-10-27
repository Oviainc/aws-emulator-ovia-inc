Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 01d4c565076fad2daa34afee2942eef023c65029

# An accepted dictionary deletion completes even after a feed comes to reference the dictionary, leaving the feed with a dangling reference

_Filed: 2026-10-27T08:05:43Z_
_Operation: DeleteDictionary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `01d4c565076fad2daa34afee2942eef023c65029`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteDictionary enforces "you cannot delete a dictionary that is referenced by a feed" ONCE, at the instant the deletion is accepted, and the record is then removed unconditionally when the 400 ms transition falls due. Nothing stops a feed from acquiring a reference to the dictionary inside that window: the reference resolver only asks whether the record EXISTS, and a dictionary in DELETING is still in the store, so CreateFeed / UpdateFeed / AssociateFeed accept and persist an output whose subtitling config names it. When the deletion then falls due the dictionary is removed with no re-check of references, and the stored feed is left naming a dictionary that no longer exists.

This breaks the documented delete rule (the deletion completes while a feed references the dictionary) and it also breaks the invariant this crate states for itself in two places: that an output reference which resolves to no record "is refused rather than stored" and that "no feed is minted carrying a reference to nothing". The crate elsewhere treats a DELETING record as on its way out and refuses writes that involve it (UpdateDictionary/TagResource/UntagResource refuse a mutation of a record being deleted; feeds_blocking_delete skips a DELETING feed because it "is on its way out"), but the reference scan makes no such distinction — so the one write that is allowed to involve a DELETING dictionary is exactly the one that creates a permanent dangling reference.

After the window, GetFeed reports an output pointing at an id for which GetDictionary answers 404, and GetDictionary$references/DeleteDictionary's own refusal can never be used to clean it up because the dictionary record is gone.

## Evidence

Smithy (smithy_query query="doc" shape="DeleteDictionary"): "Deletes the specified dictionary. You cannot delete a dictionary that is referenced by a feed. You must first remove the dictionary reference from the feed's subtitling configuration."

The check is taken only at accept time — `src/ops/delete_dictionary.rs:184`: `let blocking = state.feeds_blocking_delete(&region, &input.id);` and `:210-215`: `if !blocking.is_empty() { return Err(DeleteDictionaryErrorModeled::ConflictException(...dictionary_referenced_message(blocking.len())...)) }`.

The removal re-checks nothing — `src/state.rs:1419`: `DictionaryStatus::Deleting => return false,` inside `settle_due_transitions`.

The reference scan admits a DELETING record — `src/state.rs:1358-1362`: `if let Some(dictionary_id) = dictionary_reference(config).filter(|dictionary_id| self.dictionary(region, dictionary_id).is_none()) { return Some(cannot_find_dictionary_message(dictionary_id)); }`, over `src/state.rs:1059-1063`: `pub fn dictionary(&self, region: &str, id: &str) -> Option<&Dictionary> { self.dictionaries.get(id).filter(|dictionary| dictionary.region == region) }` — existence and Region only, no status.

The invariant the crate says it upholds — `src/state.rs:1341-1346`: "so an id that resolves to no record describes a reference the service's integrity rules do not admit, and the request is refused rather than stored. Every operation that writes outputs into a feed goes through this ONE scan (`CreateFeed`, `UpdateFeed`, `AssociateFeed`), so none of them can disagree about which references a stored feed may carry." and `src/ops/create_feed.rs:23-25`: "an output that names another resource — a dictionary, or a fixture of the data source — is stored only when that id resolves, so no feed is minted carrying a reference to nothing".

Sibling handling of a DELETING record — `src/ops/update_dictionary.rs:216-221`: `if dictionary.status == crate::state::DictionaryStatus::Deleting { return Err(...ConflictException { message: DICTIONARY_BEING_DELETED_MESSAGE... }) }`, and `src/state.rs:1320-1324`: "A feed whose deletion has been ACCEPTED (`DELETING`) is on its way out and so holds nothing back".

Test coverage: no test exercises a reference acquired during the deletion window. `tests/all/dictionary_mutation.rs:347-395` pins only DELETING being reported and `:761` the 409 for a dictionary already referenced at accept time; `tests/all/refusals_from_state.rs:318-348` likewise creates the reference BEFORE the delete ("The recorded flow waits for the dictionary to settle before deleting it."). `tests/all/feed_output_validation.rs:19-25` pins only ids that resolve to no record at all.

## Steps to Reproduce

1. POST /v1/dictionary with {"name":"dangling","language":"eng"} and note the returned id D; wait past crate::state::TRANSITION_SPAN so D reports AVAILABLE
2. DELETE /v1/dictionary/D — observe HTTP 202 with status DELETING (no feed references it yet, so feeds_blocking_delete is empty and the deletion is accepted)
3. Immediately, inside the 400 ms TRANSITION_SPAN, POST /v1/feed with one output whose outputConfig is {"subtitling":{"language":"eng","dictionary":"D"}} — observe HTTP 202 and a stored feed, because unresolved_output_reference resolves D (the record is still in the store)
4. Wait past TRANSITION_SPAN, then GET /v1/dictionary/D — observe HTTP 404: the deletion completed even though a feed references D
5. GET /v1/feed/{feedId} — observe the output still naming dictionary D, i.e. a stored feed carrying a reference to nothing, the state the crate says it never mints
6. Expected: either the create/update/associate that would add the reference is refused (ValidationException with cannot_find_dictionary_message, as for any unresolvable reference), or the dictionary's deletion does not complete while a feed references it
