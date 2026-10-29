Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3ee3ad8755f7cbca1472e043fe3407ec62e0eff1

# parse_page_cursor honours sequence spellings page_cursor never writes ("+1", "001"), so a NextToken this listing never handed out is served as a page

_Filed: 2026-10-29T11:37:29Z_
_Operation: ListLanguageModels | Class: pagination_

> _Note: this issue was identified against the code base at commit `3ee3ad8755f7cbca1472e043fe3407ec62e0eff1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every sequence-paged listing of this crate (ListLanguageModels, ListTranscriptionJobs, ListMedicalTranscriptionJobs, ListMedicalScribeJobs, ListCallAnalyticsJobs, ListVocabularies, ListMedicalVocabularies, ListVocabularyFilters) reads its `NextToken` through the single-homed codec `crate::state::resume_after_sequence` -> `crate::state::parse_page_cursor`.

The codec MINTS the position with `format!("{sequence}...")` over a `u64`, so the only spelling it ever writes is the canonical decimal one: no sign, no leading zeros. It READS the position back with `sequence.parse::<u64>()`, and Rust's `FromStr` for unsigned integers accepts an optional leading `+` and any number of leading zeros. So `ListLanguageModels|us-east-1|+1|model-a` and `ListLanguageModels|us-east-1|0000001|model-a` both decode to `Some(1)` and are served as a resume point, although `page_cursor` can never have produced either — which is exactly what the codec's own documentation says it refuses.

The effect is a silent mis-paging rather than the modelled refusal: a cursor the service never issued is accepted, the page resumes at whatever position the forged spelling happens to decode to, and the caller is never told its token is invalid. The scope prefix and the mintedness range (added when the earlier structural hole was closed) do not catch it, because `+1` / `0000001` are in-scope and in-range; only the SPELLING is one the encoder never writes.

The same codec is the one every other sequence-paged listing of this service reads, so the single fix closes all eight. `ListLanguageModels` is named here because the shared helper sits in `state.rs`; its handler is the one quoted below.

## Evidence

What the codec WRITES — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:3294-3308:
"/// The `NextToken` the listing `scope` hands out to resume after the record with this\n/// creation sequence and name.\n///\n/// The token is opaque to clients: this service writes\n/// `{listing}|{region}|{sequence}|{name}` and reads back the sequence\n/// ([`parse_page_cursor`]) in that same scope alone."
"pub fn page_cursor(scope: CursorScope<'_>, sequence: u64, name: &str) -> String {\n    format!(\n        \"{prefix}{sequence}{CURSOR_SEPARATOR}{name}\",\n        prefix = scope.prefix(),\n    )\n}"
(`sequence` is a `u64`, so `Display` writes the canonical decimal form only — never `+1`, never `0000001`.)

The rule the codec states for itself — state.rs:3310-3325:
"/// The creation sequence a `NextToken` resumes after, or `None` for a token the listing\n/// `scope` never handed out — which that listing refuses rather than silently\n/// restarting from the top or answering an empty page.\n///\n/// A token this listing wrote carries the WHOLE of what [`page_cursor`] hands out: the\n/// scope that listing mints in, a creation sequence, the separator, and the non-empty\n/// name the cursor was minted with"

What it ACCEPTS — state.rs:3326-3341:
"pub fn parse_page_cursor(\n    token: &str,\n    scope: CursorScope<'_>,\n    minted_through: u64,\n) -> Option<u64> {\n    let (sequence, name) = token\n        .strip_prefix(scope.prefix().as_str())?\n        .split_once(CURSOR_SEPARATOR)?;\n    if name.is_empty() {\n        return None;\n    }\n    sequence\n        .parse::<u64>()\n        .ok()\n        .filter(|sequence| (1..=minted_through).contains(sequence))\n}"
Nothing compares the decoded sequence's SPELLING against the one `page_cursor` would have written, so `"+1"` and `"0000001"` pass `parse::<u64>()` and then pass the `1..=minted_through` range check.

The handler that reaches it — src/ops/list_language_models.rs:194-212:
"        // A page cursor: the creation sequence of the last model of the previous page,\n        // read through this service's own token codec\n        // (`state::resume_after_sequence`), so every listing pages the same way and\n        // refuses a cursor it never handed out in the same words."
"        let resume_below = crate::state::resume_after_sequence(\n            input.next_token.as_deref(),\n            scope,\n            store.minted_sequences(FAMILY),"
and the decoded value goes straight to the paging rule at list_language_models.rs:222-226 (`list_page(records, scope, resume_below, page_size, ..)`).

Test coverage leaves the spelling open — tests/all/list_cursor_rule.rs:87-154, the table of "Every form of token `listing` never handed out": its rows are a bare sequence ("1", `minted`, "999999"), the position without the scope, an in-scope cursor one past / far past `minted`, an in-scope cursor of 0, "-1|{stored}", a 26-digit overflow, an empty name, an empty sequence, the scope alone, "|", "not-a-cursor" and "name|{stored}". Every one of them either fails `parse::<u64>()` or falls outside `1..=minted`; no row sends an in-range sequence written the other way (`+1`, `01`), so `every_sequence_paged_listing_refuses_a_cursor_it_never_minted` (list_cursor_rule.rs:158-181) passes with the hole open and no existing expectation has to change.

## Steps to Reproduce

1. Stand the emulator up with transcribe mounted, as tests/all/language_model_listing.rs does
2. CreateLanguageModel twice (e.g. ModelName "model-a" then "model-b", LanguageCode "en-US", BaseModelName "WideBand", InputDataConfig naming an S3 prefix) so the LanguageModels family has minted sequences 1 and 2
3. POST / with X-Amz-Target: Transcribe.ListLanguageModels and body {"MaxResults":1} — the answer carries NextToken "ListLanguageModels|us-east-1|2|model-b" (the canonical spelling page_cursor minted)
4. Repeat with {"NextToken":"ListLanguageModels|us-east-1|+2|model-b"} — accepted: parse_page_cursor's `"+2".parse::<u64>()` is Ok(2), the range check passes, and the request is served as a page
5. Repeat with {"NextToken":"ListLanguageModels|us-east-1|000002|model-b"} — accepted the same way
6. Expected per state.rs:3310-3317 ("a token this listing wrote carries the WHOLE of what page_cursor hands out"): 400 BadRequestException "The NextToken in your request is not valid. Use the NextToken returned by your previous ListLanguageModels request.", the answer every row of tests/all/list_cursor_rule.rs:89-154 already gets
7. The same two spellings are accepted by the other seven sequence-paged listings, since all of them read the one codec (ListTranscriptionJobs, ListMedicalTranscriptionJobs, ListMedicalScribeJobs, ListCallAnalyticsJobs, ListVocabularies, ListMedicalVocabularies, ListVocabularyFilters)
