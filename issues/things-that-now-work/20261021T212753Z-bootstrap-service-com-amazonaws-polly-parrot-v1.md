Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Polly emulator

_Filed: 2026-10-21T21:27:53Z_
_Operation: service-com-amazonaws-polly-parrot-v1 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler in `services/service-com-amazonaws-polly-parrot-v1/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding zero state. No client workflow works.

Implement a SMALL first slice: the lexicon lifecycle plus the flagship synthesis call, all computed from persisted state. In order:

1. **PutLexicon** — `PUT /v1/lexicons/{Name}` (restJson1). Input: `Name` (@httpLabel, required, pattern `^[0-9A-Za-z]{1,20}$`), `Content` (required, PLS XML). Output has no members (200, empty body). Must INSERT/REPLACE the lexicon in state, deriving and storing `LexiconAttributes`: `Alphabet` (from the PLS `alphabet` attribute), `LanguageCode` (from PLS `xml:lang`), `LastModified` (now), `LexiconArn` (`arn:aws:polly:<region>:<account>:lexicon/<Name>`), `LexemesCount` (count of `<lexeme>` elements), `Size` (byte length of `Content`). Re-PUT of the same name must overwrite and bump `LastModified`, not duplicate.
2. **GetLexicon** — `GET /v1/lexicons/{Name}`. Output: `Lexicon` (`{Name, Content}` — the EXACT content stored by PutLexicon, byte-for-byte) and `LexiconAttributes` (the stored attributes). Unknown name → `LexiconNotFoundException` (HTTP 404).
3. **ListLexicons** — `GET /v1/lexicons`. Output: `Lexicons` as a list of `LexiconDescription{Name, Attributes}` built from state (every lexicon previously put, and no others), plus `NextToken` (omit when there is no further page; an unparseable token → `InvalidNextTokenException`, 400).
4. **DeleteLexicon** — `DELETE /v1/lexicons/{Name}`. Removes the entry from state; 200 empty output. Deleting an absent name → `LexiconNotFoundException` (404). After delete, GetLexicon must 404 and ListLexicons must no longer list it.
5. **SynthesizeSpeech** — `POST /v1/speech`. Input: `Text` (required), `VoiceId` (required enum), `OutputFormat` (required enum: mp3 | ogg_vorbis | ogg_opus | pcm | alaw | mulaw | json), optional `Engine`, `LanguageCode`, `SampleRate`, `TextType`, `LexiconNames` (0..5). Output: `AudioStream` (@httpPayload blob — synthesize deterministic non-empty bytes derived from the request, e.g. a length-proportional silent buffer with the right container header; must NOT be empty), `ContentType` (@httpHeader `Content-Type`, derived from `OutputFormat`: `audio/mpeg`, `audio/ogg`, `audio/pcm`, `application/x-json-stream` for `json`, etc.), and `RequestCharacters` (@httpHeader `x-amzn-RequestCharacters`, the actual character count of `Text`). Any name in `LexiconNames` that is not in state → `LexiconNotFoundException` (404), which is what proves synthesis reads the lexicon store rather than ignoring it.
6. **DescribeVoices** — `GET /v1/voices`. Input: `Engine`, `LanguageCode`, `IncludeAdditionalLanguageCodes` (default false), `NextToken`, all @httpQuery. Output: `Voices` as `Voice{Id, Name, Gender, LanguageCode, LanguageName, AdditionalLanguageCodes, SupportedEngines}` served from a seeded voice catalog in state, FILTERED by the `Engine` and `LanguageCode` query params (filtering must actually narrow the list; bilingual voices appear for a secondary language only when `IncludeAdditionalLanguageCodes=true`).

No canned, echoed or hardcoded responses: each of the six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (never a freshly constructed engine). A random ID or an echoed input without storage is a failure. Operations outside this slice (StartSpeechSynthesisTask, GetSpeechSynthesisTask, ListSpeechSynthesisTasks, StartSpeechSynthesisStream) stay 501 and are filed as individual gap issues later.

## State Model

In `src/state.rs` (`ComAmazonawsPollyParrotV1State`):

- `lexicons: BTreeMap<String /* Name */, StoredLexicon>` — ordered by name so ListLexicons is deterministic and paginable.
  - `StoredLexicon { name: String, content: String, alphabet: Option<String>, language_code: Option<LanguageCode>, last_modified: SystemTime, lexicon_arn: String, lexemes_count: i32, size: i32 }`
- `voices: Vec<StoredVoice>` — seeded catalog, immutable at runtime.
  - `StoredVoice { id: VoiceId, name: String, gender: Gender, language_code: LanguageCode, language_name: String, additional_language_codes: Vec<LanguageCode>, supported_engines: Vec<Engine> }`
- `synthesis_requests: u64` (optional counter) — reserved for the later task slice; not required by this one.

Pagination tokens for ListLexicons/DescribeVoices should be an opaque encoding of the last key/index returned, validated on input so a garbage token yields `InvalidNextTokenException` rather than a panic or a silent reset.

Done = an integration test runs PutLexicon → GetLexicon (content and attributes match what was put) → ListLexicons (contains it) → SynthesizeSpeech referencing that lexicon (non-empty audio, correct `Content-Type` and `x-amzn-RequestCharacters`) → SynthesizeSpeech with an unknown lexicon (404) → DeleteLexicon → GetLexicon (404) → ListLexicons (absent), plus DescribeVoices filtered by `LanguageCode=en-US`.

## Evidence

`services/service-com-amazonaws-polly-parrot-v1/OPERATIONS.md` lists all 10 modeled operations with routes, input/output members and errors (PutLexicon `PUT /v1/lexicons/{Name}` with required `Name`+`Content`; GetLexicon returning `Lexicon`+`LexiconAttributes`; ListLexicons returning `Lexicons`+`NextToken`; DeleteLexicon with `LexiconNotFoundException` 404; SynthesizeSpeech with required `Text`/`VoiceId`/`OutputFormat` and output `AudioStream`/`ContentType`/`RequestCharacters`; DescribeVoices with `Engine`/`LanguageCode`/`IncludeAdditionalLanguageCodes`/`NextToken` query params). Shape members cited from the Smithy model via smithy_query for `Lexicon`, `LexiconAttributes`, `LexiconDescription`, `Voice`. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 9 wired handler files (delete_lexicon.rs, describe_voices.rs, get_lexicon.rs, get_speech_synthesis_task.rs, list_lexicons.rs, list_speech_synthesis_tasks.rs, put_lexicon.rs, start_speech_synthesis_task.rs, synthesize_speech.rs), so no operation has any behavior today. No tests assert any of this (only the generated `tests/state_roundtrip.rs` example).

## Steps to Reproduce

1. Point an AWS SDK/CLI `polly` client at the emulator endpoint.
2. `aws polly put-lexicon --name TestLex --content file://lexicon.pls` → expect 200; today returns HTTP 501 NotImplemented.
3. `aws polly get-lexicon --name TestLex` → expect the exact PLS content back plus LexiconAttributes (LexiconArn, LastModified, LexemesCount, Size, LanguageCode, Alphabet).
4. `aws polly list-lexicons` → expect TestLex present with its attributes.
5. `aws polly synthesize-speech --text 'hello world' --voice-id Joanna --output-format mp3 --lexicon-names TestLex out.mp3` → expect non-empty audio bytes, `Content-Type: audio/mpeg`, and `x-amzn-RequestCharacters: 11`.
6. `aws polly synthesize-speech --text hi --voice-id Joanna --output-format mp3 --lexicon-names NoSuchLex out.mp3` → expect LexiconNotFoundException (HTTP 404).
7. `aws polly delete-lexicon --name TestLex` → expect 200, then `get-lexicon --name TestLex` → LexiconNotFoundException (404) and `list-lexicons` no longer shows it.
8. `aws polly describe-voices --language-code en-US` → expect only en-US voices, each with Id/Name/Gender/LanguageName/SupportedEngines.
