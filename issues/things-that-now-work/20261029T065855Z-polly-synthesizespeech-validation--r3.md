Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 92ec46034f67b146e2b05eb49583adb1c148c1c8

# SampleRate is matched numerically, so spellings the model does not list ("+16000", "016000") are accepted

_Filed: 2026-10-29T06:58:55Z_
_Operation: SynthesizeSpeech | Class: validation_

> _Note: this issue was identified against the code base at commit `92ec46034f67b146e2b05eb49583adb1c148c1c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SampleRate` is a STRING member and the model enumerates its valid values as exact quoted strings per `OutputFormat`. The handler instead parses the caller's string with `parse::<u32>()` and compares the resulting number against the allowed set. Rust's integer parser accepts an optional leading sign and leading zeros, so `"+16000"`, `"016000"`, `"+8000"` and `"00022050"` all pass the check and are used as the stream's sample rate, while the crate's own documentation and test rule say a `SampleRate` is accepted "exactly when the chosen `OutputFormat` documents it" and `InvalidSampleRateException` otherwise. The emulator therefore answers 200 for a value the enumerated value set does not contain.

## Evidence

The value set, quoted verbatim by the crate itself — `src/ops/synthesize_speech.rs:280-295`:
```
/// The `SampleRate` values valid for `output_format`.
///
/// Verbatim from `SynthesizeSpeechInput$SampleRate`'s documentation: "The valid
/// values for mp3 and ogg_vorbis are 8000, 16000, 22050, 24000, 44100 and 48000 …
/// Valid values for pcm are 8000 and 16000 … Valid value for ogg_opus is 48000 …
/// Valid value for mu-law and a-law is 8000."
```
and the model sentence itself (smithy_query doc, `SynthesizeSpeechInput$SampleRate`, reproduced in the handler header at `src/ops/synthesize_speech.rs:129-135`): "The valid values for mp3 and ogg_vorbis are \"8000\", \"16000\", \"22050\", \"24000\", \"44100\" and \"48000\". … Valid values for pcm are \"8000\" and \"16000\" …" — i.e. the domain is a list of STRINGS.

What the handler does — `src/ops/synthesize_speech.rs:1236-1245`:
```
let valid = valid_sample_rates(output_format);
Some(match input.sample_rate.as_deref() {
    Some(requested) => {
        let parsed = requested.parse::<u32>().ok().filter(|r| valid.contains(r));
        parsed.ok_or_else(|| {
            SynthesizeSpeechErrorModeled::InvalidSampleRateException(
                crate::service::errors::invalid_sample_rate(),
            )
        })?
    }
```
`"+16000".parse::<u32>()` and `"016000".parse::<u32>()` both yield `Ok(16000)`, so `valid.contains(&16000)` holds and the request succeeds.

The crate's own rule — `tests/all/lexicons_and_speech.rs:712-713, 717-722`:
```
/// RULE: a `SampleRate` is accepted exactly when the chosen `OutputFormat` documents
/// it, and refused with `InvalidSampleRateException` otherwise.
...
    // Verbatim from `SynthesizeSpeechInput$SampleRate`'s documentation.
    let table: [(&str, &[&str], &[&str]); 5] = [
        (
            "mp3",
            &["8000", "16000", "22050", "24000", "44100", "48000"],
            &["11025", "0", "96000", "not-a-number"],
        ),
```
The invalid column covers only out-of-set numbers and non-numbers, never a non-canonical spelling of an in-set value, so the hole is untested.

## Steps to Reproduce

1. POST /v1/speech with body {"Text":"hello","VoiceId":"Joanna","OutputFormat":"pcm","SampleRate":"16000"} — HTTP 200, as documented.
2. POST /v1/speech with body {"Text":"hello","VoiceId":"Joanna","OutputFormat":"pcm","SampleRate":"+16000"} — expected HTTP 400 InvalidSampleRateException ("+16000" is not one of the documented values "8000"/"16000"), actual HTTP 200 with a 16 kHz stream.
3. POST /v1/speech with body {"Text":"hello","VoiceId":"Joanna","OutputFormat":"mp3","SampleRate":"022050"} — expected HTTP 400 InvalidSampleRateException, actual HTTP 200.
4. Compare with the refusal path the same handler takes for "11025" (HTTP 400), which the test at tests/all/lexicons_and_speech.rs:750-764 pins.
