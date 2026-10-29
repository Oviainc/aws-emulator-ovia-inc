Issue-Source: trace-sweep-divergence
Issue-Trace: translate-terminology-import-creates-resource-with-expected-properties
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: translate-terminology-import-creates-resource-with-expected-properties translate-terminology-with-translation translate-translate-text-basic
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate|ImportTerminology|2xx->2xx|$.TerminologyProperties.Arn
Issue-Key: translate|GetTerminology|2xx->2xx|$.TerminologyProperties.Arn
Issue-Key: translate|TranslateText|2xx->2xx|$.TranslatedText
Issue-Commit: 96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9

# Translate success payloads carry wrong terminology ARN and translated text

> _Note: this issue was identified against the code base at commit `96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportTerminology

## Description
Three net-new causes in Amazon Translate's 200-response payloads. The shared fault is that the emulator synthesizes response field VALUES instead of producing the values AWS produces: a terminology ARN's version segment is a random token rather than AWS's fixed segment, and TranslateText's output text is decorated with an internal engine marker. All statuses are 200 on both sides; only the body values diverge. A fixer must close all THREE before submitting.

Cause 1 of 3 - ImportTerminology returns a TerminologyProperties.Arn whose last segment is a random value instead of the fixed version segment.
AWS: Smithy: ImportTerminology output TerminologyProperties.Arn targets TerminologyArn (@length 1..512), documented as 'The Amazon Resource Name (ARN) of the custom terminology.' The ARN's exact layout is recorded behavior only: `arn:aws:translate:<region>:<accountId>:terminology/<Name>/LATEST`. Fixed text: the `arn:aws:translate:` prefix, the `:terminology/` resource type and the trailing `/LATEST` version segment; variable parts: region, account id and the `Name` from the request. The trailing segment is the literal string `LATEST` for the current version of a terminology, not a per-resource identifier.
Emulator: emits `arn:aws:translate:<region>:<accountId>:terminology/<Name>/<random-hex>` — a generated 16-hex-digit token in the version position (e.g. `.../af2a4cf9964a8cbe`).
Why: a client that stores the returned ARN and then constructs or compares the canonical current-version ARN (or passes it to IAM policy/resource matching, where `terminology/<name>/LATEST` is the documented resource form) gets a value AWS would never return, and the ARN differs on every call so it is not even stable across a re-import.
Fix: the ARN construction in the translate terminology state/handler - the version segment must be the fixed `LATEST` rather than a generated id.
Traces: translate-terminology-import-creates-resource-with-expected-properties#task#1.

Cause 2 of 3 - GetTerminology returns the same malformed ARN, so the defect is in the stored/serialized ARN and not only in the import response.
AWS: Smithy: GetTerminology output TerminologyProperties.Arn, same TerminologyArn shape; recorded layout `arn:aws:translate:<region>:<accountId>:terminology/<Name>/LATEST`, and it must be IDENTICAL to the ARN the preceding ImportTerminology returned for that name.
Emulator: returns the random-hex version segment again.
Why: read-after-write on a terminology does not round-trip the ARN AWS would hand back, so any caller keying resources by ARN sees an identifier that is wrong and that cannot be re-derived from the terminology name.
Fix: same ARN construction/storage path, exercised through the GetTerminology serializer.
Traces: translate-terminology-import-creates-resource-with-expected-properties#task#2.

Cause 3 of 3 - TranslateText prefixes TranslatedText with an internal engine marker.
AWS: Smithy: TranslateText output TranslatedText is @required, targets TranslatedTextString (@length 0..20000, @pattern `^[\P{M}\p{M}]{0,20000}$`), documented as the translated text. The response carries ONLY the translated text; AWS reports the language pair in the separate SourceLanguageCode/TargetLanguageCode members, never inside the text. The recording shows TranslatedText = the translation alone.
Emulator: returns the translation prefixed with a bracketed marker of the form `[mt:<source>-><target>] ` (observed as `[mt:en->fr] ` ahead of the translation).
Why: every caller that displays, stores, hashes or re-translates the returned text receives corrupted content, and the duplicated language-pair information is in a field whose contract is translated text only; round-tripping the output back through TranslateText would re-prefix it again.
Fix: the TranslateText handler/serializer - the debug marker must not be written into the TranslatedText member (the language pair is already carried by SourceLanguageCode/TargetLanguageCode).
Traces: translate-terminology-with-translation#task#4 (cluster also covers translate-translate-text-basic#task#1 and translate-translate-text-basic#task#2).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false).

Cause 1 - translate-terminology-import-creates-resource-with-expected-properties#task#1, ImportTerminology, status 200 both sides; $.TerminologyProperties.Arn expected 'arn:aws:translate:us-west-2:000000000000:terminology/<name>/LATEST', got 'arn:aws:translate:us-west-2:000000000000:terminology/sim2real-term-16640011-a6cd-4e01-b881-2cdd701dc209/af2a4cf9964a8cbe' - i.e. the trailing /LATEST replaced by a generated hex token. (The terminology name and account id are per-run values; the /LATEST segment is the requirement.)

Cause 2 - translate-terminology-import-creates-resource-with-expected-properties#task#2, GetTerminology, status 200 both sides; $.TerminologyProperties.Arn expected 'arn:aws:translate:us-west-2:000000000000:terminology/sim2real-term-16640011-a6cd-4e01-b881-2cdd701dc209/LATEST', got '.../af2a4cf9964a8cbe'.

Cause 3 - translate-terminology-with-translation#task#4, TranslateText, status 200 both sides; $.TranslatedText expected 'Bonjour', got '[mt:en->fr] Bonjour'. Same cluster: translate-translate-text-basic#task#1, translate-translate-text-basic#task#2.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture translate-terminology-import-creates-resource-with-expected-properties, step task#1 - ImportTerminology, then inspect TerminologyProperties.Arn; its last segment must be the literal LATEST.
4. Cause 2: same fixture, step task#2 - GetTerminology for the name just imported and confirm the Arn is byte-identical to the one returned by ImportTerminology and ends in /LATEST.
5. Cause 3: fixture translate-terminology-with-translation, step task#4 (or translate-translate-text-basic, steps task#1/task#2) - TranslateText en->fr and confirm TranslatedText contains the translation only, with no '[mt:...]' prefix.
