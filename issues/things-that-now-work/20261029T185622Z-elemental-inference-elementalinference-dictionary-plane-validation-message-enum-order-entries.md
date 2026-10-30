Issue-Source: trace-sweep-divergence
Issue-Trace: elementalinference-dictionary-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elementalinference-dictionary-error-paths elementalinference-dictionary-lifecycle elementalinference-list-dictionaries-and-feeds-pagination
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elemental-inference|CreateDictionary|4xx->4xx|$.message
Issue-Key: elemental-inference|ExportDictionaryEntries|2xx->2xx|$.entries
Issue-Key: elemental-inference|CreateDictionary|2xx->2xx|$.id
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ElementalInference dictionary plane: validation-message enum order, entries payload not canonicalized, and minted id shape (3 causes)

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDictionary

## Description
Three distinct root causes in the elementalinference crate's dictionary plane, each grounded in a replayed wire fixture step of simparity-wire-aws-v1. All three must be closed before submitting. None is covered by the open issue 20261029T183811Z-elementalinference-archived-feed-association-refusal-searchfixtures-7-day, which is about the feed/fixtures plane.

Cause 1 of 3 - the enum-constraint ValidationException lists the DictionaryLanguage value set in the wrong order.
AWS: Smithy: CreateDictionaryRequest$language is @required and targets the enum DictionaryLanguage; CreateDictionary declares ValidationException @httpError 400 with a required `message`. The recorded refusal of a language outside the enum is HTTP 400, error code ValidationException, with the AWS framework's constraint message: the fixed text is "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]"; `<value>` is the rejected value from the request, `<member>` the input member's name ('language'), and `<values>` the service's own ordering of that enum's value set, recorded as "spa, por, deu, fra, ita, eng". Nothing in the request or in state varies the value list - for this enum it is fixed text.
Emulator: same status, same code and the same template, but the value list is the model's declaration order: "[eng, fra, ita, deu, spa, por]" (DictionaryLanguage values in the on-disk model, which is also crate::ops::types::DICTIONARY_LANGUAGE_VALUES). Expected message "...enum value set: [spa, por, deu, fra, ita, eng]", actual "...enum value set: [eng, fra, ita, deu, spa, por]".
Why: the whole message is a requirement for this error class, and clients, tests and log-based alerting match on it verbatim; a differently ordered value set makes the emulator's refusal text unusable as a fidelity reference and breaks any assertion on the message AWS returns.
Fix: the boundary/enum validation path that renders the "Member must satisfy enum value set" message for this service - the ordering of the rendered value set must be the one the service emits for the enum, not the model's member order (crate::state::invalid_language_message carries the crate's own alternative sentence and is not the text being emitted here, so the framework renderer is the component to change).
Traces: elementalinference-dictionary-error-paths#task#2 (CreateDictionary, language 'klingon', 400/400, only $.message diverges).

Cause 2 of 3 - the exported entries payload is the client's bytes rather than the service's canonical form.
AWS: Smithy: ExportDictionaryEntries is GET /v1/dictionary/{id}/entries/export -> 200 with output member `entries` (DictionaryEntriesPayload, @length max 40960); the model says nothing about the payload's internal form, so recorded behavior only. The recording shows the service does NOT echo the submitted bytes: it reports the entry list in a canonical compact JSON form - no whitespace after ':' or ',' - e.g. [{"content":"COLOUR","sounds_like":["color"]},...], one object per entry with its content and its sounds_like list, in the order the entries were supplied. The compactness and the member names are the fixed part; the entries themselves come from what the caller stored.
Emulator: returns the payload byte-for-byte as it arrived, so the request's whitespace survives into the response: '[{"content": "COLOUR", "sounds_like": ["color"]}, ...]'. src/ops/export_dictionary_entries.rs hands back dictionary.entries.clone() unchanged, and CreateDictionary/UpdateDictionary store input.entries verbatim.
Why: the export is the documented way to read a dictionary back; a client that diffs, hashes, caches or byte-compares the exported payload against the canonical form AWS returns (or that re-imports it expecting a stable round-trip) sees a payload that differs from AWS's for the same dictionary, and the difference depends on how the client happened to format its request.
Fix: parse and canonicalize the entries payload where a dictionary is stored (crate::state::Dictionary.entries, written by the CreateDictionary and UpdateDictionary handlers) or render it canonically in the ExportDictionaryEntries handler, so the exported document is the service's form regardless of the submitted formatting.
Traces: elementalinference-dictionary-lifecycle#task#5 (ExportDictionaryEntries, 200/200, only $.entries diverges).

Cause 3 of 3 - a minted DictionaryId does not have the shape the recorded service's ids have.
AWS: Smithy: CreateDictionaryResponse$id is @required, targets DictionaryId (@length 1..19, @pattern ^[a-zA-Z0-9]+$) and is documented as "A unique ID that Elemental Inference assigns to the dictionary" - a service-generated value, which is why the fixture asserts its SHAPE and not a literal (the recorded value is held as the placeholder '<shaped-not-asserted>'). The shape the recorded service produces is what the assertion carries; the emulator's value does not satisfy it. Hypothesis: the recorded ids, like this service's FeedId (@pattern ^[a-z0-9]{19}$), are lower-case alphanumeric, while the emulator mixes case.
Emulator: minted ids are drawn from a 62-symbol mixed-case alphabet - crate::state::DICTIONARY_ID_ALPHABET = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ" over DICTIONARY_ID_LEN = 19 (mint_dictionary_id) - so it serves ids such as a 19-character value containing upper-case letters, and the step's $.id assertion fails. The emulated value satisfies the model's pattern but not the shape the recording asserts.
Why: the id is the handle every later dictionary call and every dictionary ARN carries (the tagging resourceArn pattern admits ...:(feed|dictionary)/[a-zA-Z0-9]{1,19}); a client, fixture or regex that was written against the id shape AWS actually issues rejects or mis-parses the emulator's ids, and no recorded id of this service is reproduced.
Fix: the id minting in crate::state (mint_dictionary_id and the alphabet/length constants it draws from) must mint ids of the shape the recorded service issues.
Traces: elementalinference-list-dictionaries-and-feeds-pagination#setup#1 (CreateDictionary, 202/202, only $.id diverges; same cluster also #task#2).

## Evidence
From replay_wire_corpus on trace set simparity-wire-aws-v1 (emulator up, 54 steps, 41 passed, 13 mismatched, 0 blocked/errored, score 75.9%). None of the three clusters carries a stub_path or an owned_by.

Cause 1 - divergence key service=elemental-inference|CreateDictionary|status=400->400|keys=$.message, fixture elementalinference-dictionary-error-paths step task#2: expected message "1 validation error detected: Value 'klingon' at 'language' failed to satisfy constraint: Member must satisfy enum value set: [spa, por, deu, fra, ita, eng]"; actual "1 validation error detected: Value 'klingon' at 'language' failed to satisfy constraint: Member must satisfy enum value set: [eng, fra, ita, deu, spa, por]". Status 400 and code ValidationException match; the only divergence is the order of the rendered value set. The fixed text and the request-derived parts ('klingon', 'language') are reproduced correctly. Model: DictionaryLanguage values are declared [eng, fra, ita, deu, spa, por].

Cause 2 - divergence key service=elemental-inference|ExportDictionaryEntries|status=200->200|keys=$.entries, fixture elementalinference-dictionary-lifecycle step task#5: expected '[{"content":"COLOUR","sounds_like":["color"]},{"content":"FAVOURITE","sounds_like":["favorite"]},{"content":"REALISE","sounds_like":["realize"]}]'; actual the same three entries in the same order but with a space after every ':' and ',' - i.e. the submitted formatting echoed back. Emulator code: src/ops/export_dictionary_entries.rs lines 183-185 (entries: Some(dictionary.entries.clone())); src/ops/create_dictionary.rs lines 228 and 287 store input.entries unchanged.

Cause 3 - divergence key service=elemental-inference|CreateDictionary|status=202->202|keys=$.id, fixture elementalinference-list-dictionaries-and-feeds-pagination step setup#1 (affected: also task#2): expected '<shaped-not-asserted>' (the recorded id is a generated value asserted by shape), actual a 19-character mixed-case alphanumeric id. Emulator code: src/state.rs lines 93-101 (DICTIONARY_ID_LEN/DICTIONARY_ID_ALPHABET) and lines 1090-1103 (mint_dictionary_id); compare lines 86-91, where FeedId is correctly minted over the lower-case [a-z0-9] alphabet its pattern admits.

## Steps to Reproduce
Replay the elemental-inference fixtures of simparity-wire-aws-v1 (replay_wire_corpus), or by hand against the emulator:

Cause 1: POST /v1/dictionary with {"name":"<valid-name>","language":"klingon"} and compare the 400 body's message with AWS's - same template, different order of the enum value set.

Cause 2: POST /v1/dictionary with an entries payload written with whitespace between JSON tokens (e.g. Python json.dumps defaults), then GET /v1/dictionary/{id}/entries/export and compare the returned `entries` string byte-for-byte with the compact canonical document AWS returns.

Cause 3: POST /v1/dictionary with a valid name/language and inspect `id` in the 202 body across several creates - the emulator's ids contain upper-case characters and so do not match the shape the recorded CreateDictionary responses are asserted against.
