Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# Tag key/value character check accepts characters the modelled TagKey pattern refuses

_Filed: 2026-10-28T15:22:36Z_
_Operation: Tag | Class: validation_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `TagKey` and `TagValue` both carry the pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, i.e. Unicode LETTERS (`\p{L}`), separators (`\p{Z}`), numbers (`\p{N}`) and eight punctuation characters. The crate re-implements that character class itself in `crate::state::is_tag_character`, because — as the crate's own comment states — the request boundary does not walk map KEY constraints, so this function is the ONLY enforcement for tag keys on every tagging path (`Tag`, `Untag`, `CreateGroup`).

The re-implementation uses `char::is_alphanumeric()` for `\p{L}`/`\p{N}`. `char::is_alphanumeric()` is `is_alphabetic() || is_numeric()`, and `is_alphabetic()` is the Unicode *Alphabetic* property, which is strictly larger than general category L: it additionally contains every `Other_Alphabetic` character, i.e. the combining vowel signs and marks in categories Mn/Mc (e.g. THAI CHARACTER SARA I U+0E34, DEVANAGARI VOWEL SIGN AA U+093E, HEBREW POINT HIRIQ U+05B4). Those characters are in none of `\p{L}`, `\p{Z}` or `\p{N}`, so the modelled pattern refuses them while this service accepts and stores them.

Consequence: a tag key such as `"ก" + U+0E34` (ordinary Thai text) is refused by the modelled pattern but is accepted by `Tag` (and by `CreateGroup`'s `Tags`, and removable by `Untag`), and then reported back by `GetTags`. The code comment under the function asserts the equivalence that does not hold ("Unicode letters and numbers (which is what `char::is_alphanumeric` answers)").

## Evidence

Model (smithy_query query="shape" shape="TagKey,TagValue"):
  "TagKey": {"type": "string", "length": {"min": 1, "max": 128}, "pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$"}
  "Tags": {"type": "map", "key": {"target": "TagKey", "pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$"}, ...}

Crate side — this function is the only enforcement for map keys:
services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:292-294
  "A tag key is a map KEY, whose constraints the request boundary does not walk, so this service decides them itself — in [`check_tag`], the one place it decides them."

services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:304-315
  "/// The modelled `TagKey` and `TagValue` share one pattern,
   /// `^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$`: Unicode letters and numbers (which is what
   /// [`char::is_alphanumeric`] answers), the separator categories, and the eight
   /// punctuation characters it lists.
   fn is_tag_character(c: char) -> bool {
       c.is_alphanumeric()
           || (c.is_whitespace() && !c.is_control())
           || matches!(c, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
   }"

Caller that relies on it (handler side):
services/service-com-amazonaws-resourcegroups-ardi/src/ops/tag.rs:184-190 — `crate::state::check_tags(input.tags.iter()...).map_err(bad_request)?;`

Test coverage: services/service-com-amazonaws-resourcegroups-ardi/tests/all/group_tags.rs and tests/all/group_untag.rs exercise the character rule, but only with precomposed letters — tests/all/group_untag.rs:89 uses `("Équipe", "Ωμέγα 42")` and tests/all/group_untag.rs:71 quotes the pattern — so no existing test pins acceptance of a combining mark; nothing in the crate justifies the divergence.

## Steps to Reproduce

1. CreateGroup Name=chars (200).
2. Call Tag on the group's ARN with Tags={"\u0e01\u0e34": "v"} (THAI CHARACTER KO KAI followed by THAI CHARACTER SARA I, U+0E34, general category Mn / Other_Alphabetic).
3. Observe HTTP 200: `is_tag_character` admits U+0E34 through `char::is_alphanumeric()`, so `check_tag_key` passes and the pair is stored.
4. Call GetTags on the same ARN: the key is reported back, even though the modelled `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` matches none of its characters against U+0E34.
