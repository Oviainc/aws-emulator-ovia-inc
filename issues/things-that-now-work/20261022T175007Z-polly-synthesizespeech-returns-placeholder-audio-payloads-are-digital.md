Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.synthesize-speech-pcm-output-format polly.synthesize-speech-mulaw-alaw-output-formats polly.synthesize-speech-lexicon-pronunciation-changes-audio polly.synthesize-speech-multiple-lexicons-combined polly.synthesize-speech-ssml-prosody-roundtrip polly.synthesize-speech-output-format-content-types polly.synthesize-speech-viseme-marks-with-ssml
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.synthesize-speech-pcm-output-format|run#12|AssertionError|assert any(s != 0 for s in samples), "pcm payload is all-zero silence, not rendered speech"
Issue-Key: polly.synthesize-speech-mulaw-alaw-output-formats|run#8|AssertionError|assert len(set(mulaw_audio)) > 10, sorted(set(mulaw_audio))
Issue-Key: polly.synthesize-speech-lexicon-pronunciation-changes-audio|run#9|AssertionError|assert len(with_lexicon) > len(without) * 1.2, (
Issue-Key: polly.synthesize-speech-multiple-lexicons-combined|run#14|AssertionError|assert len(only_a) > len(baseline), (
Issue-Key: polly.synthesize-speech-ssml-prosody-roundtrip|run#7|AssertionError|assert len(slow) > len(plain) * 1.3, f"plain={len(plain)} slow={len(slow)}"
Issue-Key: polly|SynthesizeSpeech|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Polly SynthesizeSpeech returns placeholder audio: payloads are digital silence, lexicons and SSML are ignored, and ogg_vorbis / non-word speech marks answer 501 (5 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SynthesizeSpeech

## Description
SynthesizeSpeech answers with a fixed-size, content-independent placeholder payload rather than rendered speech, and two of its modeled OutputFormat/SpeechMarkTypes values are still 501 stubs. Five distinct root causes, all in the SynthesizeSpeech data path.

Cause 1 of 5: audio payloads are digital silence, not rendered speech.
AWS: Smithy: SynthesizeSpeech output AudioStream is the @httpPayload streaming blob and ContentType the Content-Type header; OutputFormat enum = [json, mp3, ogg_opus, ogg_vorbis, pcm, mulaw, alaw]. The model does not state sample content, so for the waveform this is recorded behavior only: real Polly returns raw signed 16-bit little-endian mono samples for pcm that carry a non-zero signal, and 8-bit companded telephony bytes for mulaw/alaw that take many distinct byte values.
Emulator: for OutputFormat=pcm every 16-bit sample decodes to 0 (the whole payload is zero bytes); for OutputFormat=mulaw the payload is a single repeated byte value 0xFF (set(payload) == {255}), i.e. mu-law-encoded silence. Length and ContentType look plausible (e.g. 51200 bytes, audio/pcm), the signal does not.
Why: a client that decodes, plays, measures loudness, transcribes or fingerprints the returned audio gets silence — every downstream audio assertion (duration-from-samples, VAD, waveform diffing) succeeds structurally and is wrong semantically, and mulaw vs alaw are indistinguishable.
Fix: the SynthesizeSpeech handler's audio rendering/encoding path (services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs) must emit a non-constant waveform per codec.
Traces: polly.synthesize-speech-pcm-output-format ("pcm payload is all-zero silence, not rendered speech"), polly.synthesize-speech-mulaw-alaw-output-formats (AssertionError: [255]).

Cause 2 of 5: stored lexicons are never applied to synthesis.
AWS: Smithy: SynthesizeSpeech.LexiconNames is a LexiconNameList with @length max 5 and the operation models LexiconNotFoundException, so the named lexicons are resolved and applied to the text. Recorded behavior: aliasing a short grapheme to a much longer phrase (AWS -> "Amazon Web Services") lengthens the rendered pcm stream, and applying two lexicons together lengthens it more than either alone.
Emulator: byte length is identical with and without any lexicon applied (without_lexicon=51200 with_lexicon=51200; baseline=66112 only_a=66112), so LexiconNames is accepted and discarded. PutLexicon stores the document (GetLexicon returns it) but synthesis does not consult it.
Why: a client validating that a pronunciation lexicon takes effect sees no effect at all; lexicon management appears to work while the feature it exists for does nothing.
Fix: the SynthesizeSpeech handler must look up each LexiconNames entry in the lexicon store and apply its lexeme substitutions before rendering (lexicon state + synthesize_speech.rs).
Traces: polly.synthesize-speech-lexicon-pronunciation-changes-audio, polly.synthesize-speech-multiple-lexicons-combined.

Cause 3 of 5: SSML input is not parsed or applied.
AWS: Smithy: SynthesizeSpeech.TextType enum includes ssml and the operation models InvalidSsmlException, so SSML markup is parsed and affects rendering. Recorded behavior: <prosody rate="x-slow"> materially lengthens the pcm stream for the same sentence.
Emulator: TextType=ssml with a prosody-slowed sentence returns exactly the same number of bytes as the plain-text rendering (plain=93856 slow=93856), so the markup changes nothing about the output.
Why: every SSML control a client relies on (rate, pauses, say-as, emphasis) is silently a no-op, so SSML regressions cannot be detected against this emulator.
Fix: SSML parsing/application in the SynthesizeSpeech handler's rendering path.
Traces: polly.synthesize-speech-ssml-prosody-roundtrip.

Cause 4 of 5: OutputFormat=ogg_vorbis answers HTTP 501 NotImplemented.
AWS: Smithy: OutputFormat is a closed enum that includes ogg_vorbis; the operation's modeled errors are EngineNotSupportedException, InvalidSampleRateException, InvalidSsmlException, LanguageNotSupportedException, LexiconNotFoundException, MarksNotSupportedForFormatException, SsmlMarksNotSupportedForTextTypeException, TextLengthExceededException (400) and ServiceFailureException (500) — NotImplemented is not among them. Recorded behavior: ogg_vorbis returns 200 with Content-Type audio/ogg and an Ogg container payload.
Emulator: HTTP 501, error code NotImplemented, message "operation SynthesizeSpeech with OutputFormat ogg_vorbis is scaffolded but not implemented in this emulator" (message quoted as an example).
Why: a client that requests a modeled, valid OutputFormat gets an unmodeled 501 its SDK treats as a server fault and may retry, instead of audio.
Fix: implement the ogg_vorbis branch of the OutputFormat dispatch in the SynthesizeSpeech handler.
Traces: polly.synthesize-speech-output-format-content-types.

Cause 5 of 5: SpeechMarkTypes other than sentence/word answer HTTP 501 NotImplemented.
AWS: Smithy: SpeechMarkType enum = [sentence, ssml, viseme, word] and SpeechMarkTypes is a list with @length max 4; a request for viseme marks with OutputFormat=json is valid input. Recorded behavior: 200 with Content-Type application/x-json-stream and one JSON mark per line.
Emulator: HTTP 501, error code NotImplemented, message "operation SynthesizeSpeech speech marks of type other than sentence and word is scaffolded but not implemented in this emulator" (example).
Why: viseme (lip-sync) and ssml mark consumers get an unmodeled 501 for input the model accepts; the mark-type dimension of the API is unusable.
Fix: the speech-mark generation branch of the SynthesizeSpeech handler must cover all four modeled mark types.
Traces: polly.synthesize-speech-viseme-marks-with-ssml.

## Evidence
Suite: polly 12/52 passed. Failing checks and the emulator's answers:
- polly.synthesize-speech-pcm-output-format: AssertionError "pcm payload is all-zero silence, not rendered speech" (ContentType/length fine, every sample 0).
- polly.synthesize-speech-mulaw-alaw-output-formats: AssertionError: [255] — the whole mulaw payload is one repeated byte.
- polly.synthesize-speech-lexicon-pronunciation-changes-audio: AssertionError "without_lexicon=51200 with_lexicon=51200"; expected the lexicon-applied pcm stream to be >1.2x longer.
- polly.synthesize-speech-multiple-lexicons-combined: AssertionError "lexicon 'sim2real886f0c249f17' alone must lengthen audio: baseline=66112 only_a=66112".
- polly.synthesize-speech-ssml-prosody-roundtrip: AssertionError "plain=93856 slow=93856"; expected x-slow prosody to be >1.3x longer.
- polly.synthesize-speech-output-format-content-types: ClientError (NotImplemented) "operation SynthesizeSpeech with OutputFormat ogg_vorbis is scaffolded but not implemented in this emulator"; expected 200 + Content-Type audio/ogg + an "OggS" signature.
- polly.synthesize-speech-viseme-marks-with-ssml: ClientError (NotImplemented) "operation SynthesizeSpeech speech marks of type other than sentence and word is scaffolded but not implemented in this emulator"; expected 200 + application/x-json-stream with ordered viseme marks ending in "sil".
Model facts used: smithy_query(operation="SynthesizeSpeech") — OutputFormat required enum [json, mp3, ogg_opus, ogg_vorbis, pcm, mulaw, alaw]; SpeechMarkTypes list of SpeechMarkType [sentence, ssml, viseme, word], @length max 4; LexiconNames @length max 5; AudioStream @httpPayload streaming blob; modeled errors are all 400/500 client/server errors, none named NotImplemented.

## Steps to Reproduce
Against the booted emulator with a botocore polly client:
1. Silence: resp = polly.synthesize_speech(Text="Testing raw pcm output from Amazon Polly.", OutputFormat="pcm", SampleRate="16000", VoiceId="Joanna", Engine="standard"); struct.unpack all 16-bit samples of resp["AudioStream"].read() -> every sample is 0. Repeat with OutputFormat="mulaw", SampleRate="8000" -> set(payload) == {255}.
2. Lexicons: put_lexicon(Name="lex1", Content=<PLS aliasing AWS -> Amazon Web Services>); synthesize_speech(Text="Please deploy AWS today.", OutputFormat="pcm", VoiceId="Joanna") with and without LexiconNames=["lex1"] -> identical byte lengths.
3. SSML: synthesize_speech(Text="This sentence is spoken at a different rate.", TextType="text", OutputFormat="pcm", VoiceId="Joanna") vs the same sentence wrapped in <speak><prosody rate="x-slow">...</prosody></speak> with TextType="ssml" -> identical byte lengths.
4. ogg: synthesize_speech(Text="Testing output format content types.", OutputFormat="ogg_vorbis", VoiceId="Joanna") -> HTTP 501 NotImplemented.
5. Marks: synthesize_speech(Text="<speak>...</speak>", TextType="ssml", OutputFormat="json", VoiceId="Joanna", SpeechMarkTypes=["viseme"]) -> HTTP 501 NotImplemented.
Replay: the seven check ids in trace_ids.
