Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b7068a3aaca27128a9512527e96d46285251fa2e

# DescribeVoices omits voices the model declares and emits an empty AdditionalLanguageCodes array

_Filed: 2026-10-24T03:33:51Z_
_Operation: DescribeVoices | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b7068a3aaca27128a9512527e96d46285251fa2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two symptoms of the same class in the DescribeVoices response body.

1. The served catalog is a hand-seeded subset: src/state.rs VOICE_CATALOG carries 98 rows while the model's VoiceId enum declares 115 voices, so around 17 real voices are reported by neither an unfiltered nor a filtered query — Ambre, Andres, Arjun, Beatrice, Carlos, Carolina, Curtis, Florian, Hyunduk, Kenneth, Kiara, Lennart, Leo, Lorenzo, Nate, Sabrina, Tiffany, Tina. A client that discovers voices through DescribeVoices (e.g. "which generative voices exist for fr-FR / es-MX / ko-KR") gets a different answer than AWS, and SynthesizeSpeech happily accepts those same VoiceIds (the handler deliberately does not narrow a voice the catalog lacks), so the two views of the catalog disagree.

2. Every voice without extra languages carries `"AdditionalLanguageCodes": []` in the body, because the handler always builds `additional_language_codes: Some(...)`. Amazon Polly omits the key entirely for a voice that speaks only its default language and includes it only for bilingual voices (e.g. Aditi -> ["hi-IN"]), so the response document differs field-for-field from a recorded one.

Expected: the voice list reports every voice the service offers (the model's VoiceId enum is the enumeration of them) with its real gender/language/supported engines, and AdditionalLanguageCodes present only where a voice has additional languages.
Actual: 98 of 115 modeled voices, and an empty-array AdditionalLanguageCodes on every monolingual voice.

## Evidence

Smithy model: smithy_query(query="shape", shape="VoiceId") returns 115 enum values; DescribeVoices is documented as returning "the list of voices that are available for use when requesting speech synthesis". Handler/state: src/state.rs VOICE_CATALOG (lines 301-411) is 98 rows and its own doc comment concedes "This is a subset of Amazon Polly's full voice list"; src/ops/describe_voices.rs lines 273-293 always set `additional_language_codes: Some(... .collect())`, producing `[]` for a voice whose slice is empty. Tests: the crate's own state test only asserts every seeded row uses modeled enum values (src/state.rs::every_seeded_voice_uses_modeled_enum_values); no test asserts catalog completeness and none asserts the absence of AdditionalLanguageCodes, so neither symptom is covered.

## Steps to Reproduce

1. Boot the emulator and page through GET /v1/voices following NextToken; collect every Id
2. Observe that modeled VoiceId values such as Arjun, Ambre, Beatrice, Carlos, Carolina, Curtis, Sabrina, Tiffany and Tina never appear, while real Polly reports them
3. Call polly.synthesize_speech(Text="hi", OutputFormat="mp3", VoiceId="Arjun") and observe it is accepted even though DescribeVoices never reported that voice
4. Inspect any monolingual voice in the response (e.g. Joanna) and observe the `"AdditionalLanguageCodes": []` key, which real Polly omits
