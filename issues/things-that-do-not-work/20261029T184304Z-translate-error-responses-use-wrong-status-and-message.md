Issue-Source: trace-sweep-divergence
Issue-Trace: translate-terminology-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: translate-terminology-errors translate-terminology-with-translation translate-list-languages translate-parallel-data-errors
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate|GetTerminology|4xx->4xx|$.Message
Issue-Key: translate|GetTerminology|4xx->4xx|status
Issue-Key: translate|DeleteTerminology|4xx->4xx|$.Message
Issue-Key: translate|DeleteTerminology|4xx->4xx|status
Issue-Key: translate|TranslateText|4xx->4xx|$.Message
Issue-Key: translate|TranslateText|4xx->4xx|status
Issue-Key: translate|ListLanguages|4xx->4xx|$.message
Issue-Key: translate|CreateParallelData|4xx->4xx|$.message
Issue-Commit: 96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9

# Translate error responses use wrong status and message text

> _Note: this issue was identified against the code base at commit `96695958bae92fbaa0e7b1c1c8b9876d2b2e4ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTerminology

## Description
Five net-new causes in the Amazon Translate error path. All five are the same shared fault: the emulator does not reproduce AWS's error identity for Translate — it picks the wrong modeled error (and therefore the wrong HTTP status) for a missing terminology resource, and it renders validation messages from the Smithy model's own text instead of the text AWS emits on the wire. A fixer must close all FIVE before submitting.

Cause 1 of 5 - GetTerminology on a terminology name that does not exist.
AWS: recorded behavior only (the model binds both InvalidParameterValueException/400 and ResourceNotFoundException/404 to GetTerminology, so the model alone does not pick one; Smithy: GetTerminology errors = InternalServerException 500, InvalidParameterValueException 400, ResourceNotFoundException 404, TooManyRequestsException 429). The recording shows AWS answering HTTP 400 with body member `Message` set to the template `The Terminology for <accountId> <name> was not found`. Fixed text: `The Terminology for `, ` was not found`; variable parts: the caller's 12-digit account id and the `Name` from the request, separated by a single space.
Emulator: answers HTTP 404 with the generic ResourceNotFoundException prose `The resource you are looking for has not been found. Review the resource you're looking for and see if a different resource will accomplish your needs before retrying the revised request.` — i.e. the Smithy shape documentation, with no account id and no resource name.
Why: a client that branches on the status or on the error shape sees a 404/ResourceNotFoundException where AWS raises a 400 client-validation error, so retry/branching logic and error-code assertions take the wrong path, and the message names no resource so a caller cannot tell WHICH terminology was missing.
Fix: the GetTerminology handler's not-found path in the translate service crate — it must raise the 400 error class AWS uses for a missing terminology and format the message from the account id and requested name rather than emitting the generic 404 shape doc.
Traces: translate-terminology-errors#task#1 (also affects translate-terminology-with-translation#teardown#6).

Cause 2 of 5 - DeleteTerminology on a terminology name that does not exist.
AWS: recorded behavior only (Smithy: DeleteTerminology errors = InternalServerException 500, InvalidParameterValueException 400, ResourceNotFoundException 404, TooManyRequestsException 429). The recording shows HTTP 400 with `Message` = `The TerminologyMetadata for <accountId> <name> was not found` — note the resource word is `TerminologyMetadata` here, NOT `Terminology` as in Cause 1. Fixed text: `The TerminologyMetadata for `, ` was not found`; variable parts: account id and the request's `Name`.
Emulator: HTTP 404 with the same generic ResourceNotFoundException prose as Cause 1.
Why: same wrong-status/wrong-error-class break as Cause 1; additionally a client (or test) matching AWS's per-operation resource wording never matches, because the emulator emits one generic sentence for every operation.
Fix: the DeleteTerminology handler's not-found path — distinct message template from GetTerminology.
Traces: translate-terminology-errors#task#2.

Cause 3 of 5 - TranslateText naming a terminology in TerminologyNames that does not exist.
AWS: recorded behavior only (Smithy: TranslateText errors include InvalidRequestException 400, ResourceNotFoundException 404, UnsupportedLanguagePairException 400, TextSizeLimitExceededException 400). The recording shows HTTP 400 with `Message` = `The Terminology for <accountId> <name> was not found`, the same template as Cause 1, resolved against the entry in `TerminologyNames`.
Emulator: HTTP 404 with the generic ResourceNotFoundException prose.
Why: a client supplying a custom terminology cannot distinguish a bad terminology reference (a 400 request error on AWS) from a missing translation resource, and gets no name back to correct.
Fix: the terminology-resolution step inside the TranslateText handler, which must raise the same 400 error class and template as GetTerminology when a named terminology is absent from state.
Traces: translate-terminology-errors#task#3.

Cause 4 of 5 - ListLanguages rejecting an out-of-enum DisplayLanguageCode: enum value set is rendered in the wrong order.
AWS: Smithy: ListLanguages.DisplayLanguageCode targets enum DisplayLanguageCode; errors InvalidParameterValueException 400, UnsupportedDisplayLanguageCodeException 400. Status and message come from the recording: HTTP 400, `message` = `1 validation error detected: Value '<value>' at 'displayLanguageCode' failed to satisfy constraint: Member must satisfy enum value set: [de, zh-TW, pt, ko, en, it, fr, zh, es, ja]`. Fixed text: the `1 validation error detected: Value '` / `' at 'displayLanguageCode' failed to satisfy constraint: Member must satisfy enum value set: [` / `]` frame and the member path; variable part: the rejected value from the request. The bracketed value list is a FIXED part of this message — AWS emits those ten codes in that exact, non-alphabetical order, which is not the model's declaration order.
Emulator: HTTP 400 with the right frame but the list sorted alphabetically: `[de, en, es, fr, it, ja, ko, pt, zh, zh-TW]`.
Why: the status and error class are right, but a client or conformance test asserting on AWS's validation message text never matches, and tooling that parses the accepted-value list out of the message sees a list AWS never emits.
Fix: the ListLanguages input validation / validation-message serializer — the enum value set must be emitted in AWS's fixed order for this message rather than sorted.
Traces: translate-list-languages#task#3.

Cause 5 of 5 - CreateParallelData rejecting a non-S3 ParallelDataConfig.S3Uri: the quoted regular expression carries anchors AWS strips.
AWS: Smithy: CreateParallelData.ParallelDataConfig.S3Uri @pattern `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`; error InvalidParameterValueException 400. Status and message from the recording: HTTP 400, `message` = `1 validation error detected: Value '<value>' at 'parallelDataConfig.s3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?` — AWS quotes the pattern WITHOUT the leading `^` and trailing `$`. Fixed text: the validation frame, the member path `parallelDataConfig.s3Uri` and the unanchored pattern; variable part: the rejected URI from the request.
Emulator: HTTP 400 with the same frame but the pattern echoed straight from the model including anchors: `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`.
Why: same class of break as Cause 4 — an AWS-faithful client or test asserting the constraint-violation message text diverges on every pattern-constrained member of this service, not just this one, because the emulator's shared validation serializer echoes the model pattern verbatim.
Fix: the constraint-violation message serializer used by the translate input validation — strip the anchors from a @pattern before interpolating it into the `Member must satisfy regular expression pattern:` message.
Traces: translate-parallel-data-errors#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false).

Cause 1 - translate-terminology-errors#task#1, GetTerminology: expected status 400, got 404; $.Message expected 'The Terminology for 000000000000 <name> was not found', got "The resource you are looking for has not been found. Review the resource you're looking for and see if a different resource will accomplish your needs before retrying the revised request." Cluster also contains translate-terminology-with-translation#teardown#6.

Cause 2 - translate-terminology-errors#task#2, DeleteTerminology: expected status 400, got 404; $.Message expected 'The TerminologyMetadata for 000000000000 <name> was not found', got the same generic ResourceNotFoundException sentence.

Cause 3 - translate-terminology-errors#task#3, TranslateText: expected status 400, got 404; $.Message expected 'The Terminology for 000000000000 <name> was not found', got the same generic ResourceNotFoundException sentence.

Cause 4 - translate-list-languages#task#3, ListLanguages: status 400 both sides; $.message expected "1 validation error detected: Value 'xx' at 'displayLanguageCode' failed to satisfy constraint: Member must satisfy enum value set: [de, zh-TW, pt, ko, en, it, fr, zh, es, ja]", got "... enum value set: [de, en, es, fr, it, ja, ko, pt, zh, zh-TW]".

Cause 5 - translate-parallel-data-errors#task#5, CreateParallelData: status 400 both sides; $.message expected "... at 'parallelDataConfig.s3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?", got the same message with the pattern anchored as ^...$.

(The account id 000000000000 and the generated terminology names in the recorded messages are environment values that fill the template; the template frame and the resource wording are the requirement.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture translate-terminology-errors, step task#1 - GetTerminology on a terminology name that was never imported; observe 404 + generic message instead of 400 + 'The Terminology for <accountId> <name> was not found'.
4. Cause 2: fixture translate-terminology-errors, step task#2 - DeleteTerminology on the same missing name; expect 400 + 'The TerminologyMetadata for <accountId> <name> was not found'.
5. Cause 3: fixture translate-terminology-errors, step task#3 - TranslateText with TerminologyNames naming the missing terminology; expect 400 + 'The Terminology for <accountId> <name> was not found'.
6. Cause 4: fixture translate-list-languages, step task#3 - ListLanguages with DisplayLanguageCode='xx'; compare the bracketed enum value set in the 400 message against AWS's order [de, zh-TW, pt, ko, en, it, fr, zh, es, ja].
7. Cause 5: fixture translate-parallel-data-errors, step task#5 - CreateParallelData with ParallelDataConfig.S3Uri='https://example.com/corpus.tsv'; confirm the quoted regular expression in the 400 message has no ^/$ anchors.
