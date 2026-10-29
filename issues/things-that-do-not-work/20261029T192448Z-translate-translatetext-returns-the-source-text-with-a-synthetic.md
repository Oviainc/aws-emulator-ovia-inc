Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: translate.translate-text-basic-round-trip translate.translate-text-formality-setting-applied translate.translate-text-profanity-masking-setting
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate.translate-text-basic-round-trip|run#9|AssertionError|assert source_text not in spanish, (
Issue-Key: translate.translate-text-formality-setting-applied|run#8|AssertionError|assert re.search(r"\bSie\b|\bIhnen\b", formal["TranslatedText"]), (
Issue-Key: translate.translate-text-profanity-masking-setting|run#5|AssertionError|assert _GRAWLIX in masked["TranslatedText"], (
Issue-Commit: 5b3f288c64c79f31818824eff80fa309a30ec0e8

# TranslateText returns the source text with a synthetic marker and never applies Settings (Formality/Profanity)

> _Note: this issue was identified against the code base at commit `5b3f288c64c79f31818824eff80fa309a30ec0e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TranslateText

## Description
Three net-new causes in Amazon Translate's TranslateText data plane. The shared fault is that the handler performs no translation at all: it returns the request's own `Text` (after optional custom-terminology substitution) prefixed with an internal marker, and it folds the requested `Settings` into that marker instead of letting them change the output text. A fixer must close all THREE before submitting.

Related open issue: `20261029T184304Z-translate-success-payloads-carry-wrong-terminology-arn-and.md` Cause 3 of 3 covers only the cosmetic `[mt:...]` prefix on `$.TranslatedText`. It does NOT cover the causes below — removing the marker would still leave the untranslated English source in `TranslatedText` and would still leave `Settings` with no effect on the output. These are behavioral requirements beyond that issue's wire-field comparison.

Cause 1 of 3 — TranslateText does not translate; the source text is passed through.
AWS: Smithy: TranslateText output `TranslatedText` is @required, targets `TranslatedTextString` (@length 0..20000, @pattern `^[\P{M}\p{M}]{0,20000}$`), documented as "The translated text." The operation is documented as "Translates input text from the source language to the target language". The contract is that for a supported pair the returned text is in the TARGET language, so the source-language text does not appear verbatim in the output, and translating the output back with the pair reversed recovers the source's content words. Status 200 both sides; only the body value diverges.
Emulator: returns the request's `Text` unchanged with a bracketed marker prepended — `[mt:en->es] Hello, how are you doing today?` for Text='Hello, how are you doing today?', SourceLanguageCode='en', TargetLanguageCode='es'. The handler builds `format!("{marker}{substituted}")` where `substituted` starts as `input.text` and is only rewritten where a named custom terminology matches; with no `TerminologyNames` the text is copied through byte for byte. Reversing the pair repeats the same pass-through, so an en->es->en round trip returns the original English wrapped in two markers rather than a translation in either direction.
Why: every caller that displays, stores, indexes, diffs or re-translates the result receives untranslated content that is also not valid output for the declared `TargetLanguageCode`. A client cannot distinguish "translated" from "not translated", and any pipeline that checks the output language, or that round-trips text through two TranslateText calls, silently produces wrong data instead of failing.
Emulator component to fix: the TranslateText handler's text-production path in the translate service crate (the `format!("{marker}{substituted}")` answer and the `marker()`/`SYNTHETIC_MARKER` helpers feeding it) — it must emit target-language text for the resolved pair rather than echoing the input.
Traces: translate.translate-text-basic-round-trip.

Cause 2 of 3 — `Settings.Formality` is echoed in `AppliedSettings` but does not change the translation.
AWS: Smithy: TranslateText input `Settings` targets `TranslationSettings`; its `Formality` member is documented as "You can specify the desired level of formality for translations to supported target languages. The formality setting controls the level of formal language usage (also known as register) in the translation output. You can set the value to informal or formal." For a formality-supporting target language (de), `Formality=FORMAL` and `Formality=INFORMAL` must therefore yield DIFFERENT `TranslatedText` for the same input, and the FORMAL rendering must use the formal German second-person register (`Sie`/`Ihnen`) while the INFORMAL rendering must not. `AppliedSettings` echoes the setting that was applied. Status 200 both sides.
Emulator: `AppliedSettings.Formality` is echoed correctly, but the setting only appears inside the marker: FORMAL returns `[mt:en->de:FORMAL] How are you today? I hope you can help me.` and INFORMAL returns the same sentence with `:INFORMAL` in the marker. The sentence body is the untranslated English input in both cases, so no formal pronoun is ever present and the two outputs differ only in the marker's literal setting token.
Why: a client using Formality to control register — the only reason to send the member — gets no register change at all. Any caller asserting that formal output carries the formal pronoun, or that the two settings produce materially different text, sees a difference that is only the echoed setting name.
Emulator component to fix: the TranslateText handler's translation step must consume `input.settings.formality` when producing the target-language text, instead of appending it to the marker detail string.
Traces: translate.translate-text-formality-setting-applied.

Cause 3 of 3 — `Settings.Profanity=MASK` does not mask profanity in the output.
AWS: Smithy: `TranslationSettings.Profanity` is documented as "You can enable the profanity setting if you want to mask profane words and phrases in your translation output. To mask profane words and phrases, Amazon Translate replaces them with the grawlix string “?$#@$“. This 5-character sequence is used for each profane word or phrase, regardless of the length or number of words." So with `Profanity=MASK` and a profane source term, the returned `TranslatedText` must contain the fixed 5-character grawlix `?$#@$` in place of each profane word, and without the setting it must NOT contain the grawlix. The grawlix is fixed text, not a per-run value; the surrounding translated words come from the request's text. Status 200 both sides.
Emulator: for Text='Esta mierda es inaceptable.', SourceLanguageCode='es', TargetLanguageCode='en', Settings={Profanity: MASK} it returns `[mt:es->en:MASK] Esta mierda es inaceptable.` — the profane source word is still present in full and no grawlix appears anywhere. `AppliedSettings.Profanity=MASK` is echoed, so the response claims a masking that was not performed.
Why: profanity masking is a content-safety control. A caller that enables it and then publishes the returned text believes the output is masked, and the emulator hands back the unmasked profanity while asserting in `AppliedSettings` that masking was applied — a silently wrong answer rather than a visible gap.
Emulator component to fix: the TranslateText handler's translation step — when `settings.profanity` is set to MASK it must substitute the grawlix `?$#@$` for each profane token in the produced output text, not record the setting in the marker.
Traces: translate.translate-text-profanity-masking-setting.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator (translate 6/20 passed; bootstrap_signal=false, so the emulator was serving).

Cause 1 — [mismatch] translate.translate-text-basic-round-trip:
  AssertionError: expected genuine Spanish output, got the English source smuggled through: '[mt:en->es] Hello, how are you doing today?'
  Request: TranslateText(Text='Hello, how are you doing today?', SourceLanguageCode='en', TargetLanguageCode='es'). Expected: `TranslatedText` is Spanish and does not contain the English source; reversing the pair recovers the content words 'how' and 'today'. Got: the English source verbatim behind a `[mt:en->es] ` marker.

Cause 2 — [mismatch] translate.translate-text-formality-setting-applied:
  AssertionError: expected a formal German pronoun (Sie/Ihnen), got '[mt:en->de:FORMAL] How are you today? I hope you can help me.'
  Request: TranslateText(Text='How are you today? I hope you can help me.', SourceLanguageCode='en', TargetLanguageCode='de', Settings={Formality: FORMAL}) and the same call with INFORMAL. Expected: FORMAL output matches /\bSie\b|\bIhnen\b/, INFORMAL output does not, and the two texts differ. Got: the untranslated English sentence in both, differing only in the `:FORMAL` / `:INFORMAL` marker token. `AppliedSettings.Formality` was correct in both.

Cause 3 — [mismatch] translate.translate-text-profanity-masking-setting:
  AssertionError: expected grawlix '?$#@$' in masked translation, got '[mt:es->en:MASK] Esta mierda es inaceptable.'
  Request: TranslateText(Text='Esta mierda es inaceptable.', SourceLanguageCode='es', TargetLanguageCode='en', Settings={Profanity: MASK}). Expected: `TranslatedText` contains the fixed grawlix `?$#@$`. Got: the unmasked Spanish source text behind the marker, with `AppliedSettings.Profanity=MASK` echoed.

Source of the behavior in the checkout: services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/ops/translate_text.rs — `SYNTHETIC_MARKER` (line 109), `marker()` (lines 213-234, which appends the Formality and Profanity tokens to the marker detail), and the answer `let translated_text = format!("{marker}{substituted}")` (line 470) where `substituted` is initialised from `input.text` (line 435).

(The specific sentences above are the checks' inputs; the requirement is the general behavior — output in the target language, register varying with Formality, the fixed grawlix appearing under Profanity=MASK.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay behavioral suite simparity-behavioral-aws-v1 (run_behavioral_suite) and re-run the three checks in trace_ids.
3. Cause 1: translate.translate-text-basic-round-trip — call TranslateText(Text='Hello, how are you doing today?', SourceLanguageCode='en', TargetLanguageCode='es'); assert the source string is NOT a substring of TranslatedText, then feed TranslatedText back with SourceLanguageCode='es', TargetLanguageCode='en' and assert the round trip recovers 'how' and 'today'.
4. Cause 2: translate.translate-text-formality-setting-applied — call TranslateText twice for en->de with Settings={Formality: FORMAL} and Settings={Formality: INFORMAL}; assert the FORMAL TranslatedText matches /\bSie\b|\bIhnen\b/, the INFORMAL one does not, and the two texts differ by more than the setting token.
5. Cause 3: translate.translate-text-profanity-masking-setting — call TranslateText for es->en with a profane source word and Settings={Profanity: MASK}; assert '?$#@$' appears in TranslatedText, then call it without Settings and assert '?$#@$' does not appear.
