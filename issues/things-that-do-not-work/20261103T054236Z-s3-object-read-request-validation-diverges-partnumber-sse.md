Issue-Source: trace-sweep-divergence
Issue-Trace: s3-csvcols-part-number-reads
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-csvcols-part-number-reads s3-rcsv-part-number-reads-on-single-part-csv s3-txt-part-number-download-on-a-single-part-text-file s3-csvcols-encryption-header-mistakes s3-rcsv-encryption-header-mistakes-on-sse-kms-read s3-txt-untyped-text-file-and-response-header-overrides s3-csvcols-version-addressing s3-rcsv-version-id-mistakes-on-versioned-csv s3-ovm-traj-version-id-argument-mistakes s3-txt-conditional-date-headers-and-precedence
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Code
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Message
Issue-Key: s3|GetObject|4xx->4xx|status
Issue-Key: s3|GetObject|4xx->2xx|$
Issue-Key: s3|GetObject|4xx->2xx|$headers.content-type
Issue-Key: s3|GetObject|4xx->2xx|status
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.Code
Issue-Key: s3|GetObjectAttributes|4xx->4xx|status
Issue-Key: s3|HeadObject|4xx->4xx|status
Issue-Related: 20261030T053639Z-s3-multipart-range-and-precondition-error-envelopes-drop.md
Issue-Related: 20261030T053639Z-s3-object-error-envelopes-omit-key-and-versionid.md
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Commit: 7e282e985d147868e376415a13aa29a1f3675b80

# s3 object-read request validation diverges: partNumber, SSE header, versionId and conditional-date refusals

> _Note: this issue was identified against the code base at commit `7e282e985d147868e376415a13aa29a1f3675b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
The emulator's object-read path (GetObject / HeadObject / GetObjectAttributes) does not implement S3's pre-read request validation. Malformed or mutually exclusive read parameters are either served successfully, mapped to the wrong refusal class, or rejected by the framework's generic coercion layer instead of by the operation. 8 net-new causes, all s3, all on the object read plane.

Cause 1 of 8 — GetObject partNumber out of range (partNumber=0)
AWS: Smithy: GetObject.PartNumber is httpQuery "partNumber" targeting com.amazonaws.s3#PartNumber (integer; the model carries no @range, so the bound is service-side). Recorded AWS response: HTTP 400, Error.Code "InvalidArgument", Error.Message "Part number must be an integer between 1 and 10000, inclusive" (that whole sentence is fixed text, with no variable parts), plus the contextual members Error.ArgumentName = "partNumber" (fixed, the member's wire name) and Error.ArgumentValue = the rejected partNumber exactly as the request sent it.
Emulator: HTTP 416, Error.Code "InvalidPartNumber", Error.Message "The requested partnumber is not satisfiable", and no ArgumentName / ArgumentValue members at all.
Why: a client gets a range-not-satisfiable condition for a request-validation fault. SDK retry/classification logic treats 416 and 400 differently, and tooling that reads ArgumentName/ArgumentValue to report which parameter was bad has nothing to read.
Fix: GetObject's input validation — reject partNumber outside 1..10000 before part/range resolution, and have the refusal serializer emit the InvalidArgument envelope with ArgumentName/ArgumentValue.
Traces: s3-csvcols-part-number-reads#task#6 (also s3-rcsv-part-number-reads-on-single-part-csv#task#7, s3-txt-part-number-download-on-a-single-part-text-file#task#6).

Cause 2 of 8 — GetObject non-numeric partNumber (partNumber=one)
AWS: same modelled member as Cause 1. Recorded AWS response: HTTP 400, Error.Code "InvalidArgument", Error.Message "Part number must be an integer between 1 and 10000, inclusive" — i.e. a non-numeric value is the SAME refusal as an out-of-range one — with Error.ArgumentName "partNumber" and Error.ArgumentValue carrying the raw string as sent.
Emulator: HTTP 400 but Error.Code "ValidationException" and Error.Message "invalid i32 `one`: invalid digit found in string" — the deserialization framework's coercion text, naming a Rust integer type, reaches the wire. No ArgumentName / ArgumentValue.
Why: the error code is not an S3 error code at all, so a client branching on InvalidArgument never matches, and the message exposes emulator internals rather than the AWS rule.
Fix: GetObject's query-parameter parsing must treat an unparseable partNumber as the operation's own InvalidArgument refusal rather than letting the generic scalar coercion layer answer. The open issue "s3 scalar parameter refusals leak Smithy coercion wording" (20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md) reports the same class of leak but does not carry this step's replay key.
Traces: s3-csvcols-part-number-reads#task#7.

Cause 3 of 8 — GetObject with both a Range header and partNumber
AWS: Smithy: GetObject.Range is httpHeader "Range" and GetObject.PartNumber is httpQuery "partNumber"; the two are mutually exclusive. Recorded AWS response: HTTP 400, Error.Code "InvalidRequest", Error.Message "Cannot specify both Range header and partNumber query parameter" (entirely fixed text), content-type application/xml.
Emulator: HTTP 206 with the object's bytes and content-type text/csv — the combination is accepted and a partial-content read is served.
Why: a client that mistakenly sends both gets data instead of a refusal, so the bug in the caller is silently masked and the bytes returned are whichever of the two selectors the emulator happened to honour.
Fix: GetObject's input validation — add the Range/partNumber mutual-exclusion check ahead of body resolution.
Traces: s3-csvcols-part-number-reads#task#8 (also s3-rcsv-part-number-reads-on-single-part-csv#task#8, s3-txt-part-number-download-on-a-single-part-text-file#task#7).

Cause 4 of 8 — GetObject carrying x-amz-server-side-encryption
AWS: Smithy: GetObject's input binds no x-amz-server-side-encryption header (only the SSE-C members x-amz-server-side-encryption-customer-algorithm/-key/-key-MD5); x-amz-server-side-encryption is an output header only. Recorded AWS response: HTTP 400, Error.Code "InvalidArgument", Error.Message "x-amz-server-side-encryption header is not supported for this operation." (fixed text; the header name in it is the offending header), with Error.ArgumentName = "x-amz-server-side-encryption" and Error.ArgumentValue = the header value as sent. content-type application/xml.
Emulator: HTTP 200 with the full object body and content-type text/csv — the unsupported header is ignored.
Why: a caller that wrongly sets the write-side encryption header on a read believes the read honoured it; the request is accepted where AWS refuses, so encryption-handling bugs surface only in production.
Fix: GetObject's input validation — refuse requests that carry a write-only SSE header.
Traces: s3-csvcols-encryption-header-mistakes#task#4 (also s3-rcsv-encryption-header-mistakes-on-sse-kms-read#task#4, s3-txt-untyped-text-file-and-response-header-overrides#task#7 and #task#8).

Cause 5 of 8 — GetObject with an empty versionId
AWS: Smithy: GetObject.VersionId is httpQuery "versionId" targeting ObjectVersionId (string). Recorded AWS response: HTTP 400, Error.Code "InvalidArgument" with Error.Message "Version id cannot be the empty string" — an empty value has its OWN fixed message, distinct from the message for a malformed one; no variable parts.
Emulator: HTTP 400 and the right code, but Error.Message "Invalid version id specified" — the generic malformed-version message is reused for the empty case.
Why: the refusal no longer tells the caller that the parameter was empty rather than wrong, and message-asserting clients and docs tests diverge.
Fix: GetObject's versionId validation — branch the empty string onto its own message.
Traces: s3-csvcols-version-addressing#task#5 (also s3-rcsv-version-id-mistakes-on-versioned-csv#task#5).

Cause 6 of 8 — GetObjectAttributes with a malformed versionId
AWS: Smithy: GetObjectAttributes.VersionId is httpQuery "versionId" targeting ObjectVersionId; the operation models NoSuchKey (404) but a version id that is not a well-formed S3 version-id token is refused before any lookup. Recorded AWS response: HTTP 400, Error.Code "InvalidArgument", Error.Message "Invalid version id specified" (fixed text), with Error.ArgumentName = "versionId" and Error.ArgumentValue = the version id as sent.
Emulator: HTTP 404, Error.Code "NoSuchVersion", Error.Message "The specified version does not exist.", no ArgumentName/ArgumentValue — every unrecognised version id is treated as an absent version.
Why: a malformed-input fault is reported as absence, so a client cannot distinguish "you sent a bad version id" from "that version was deleted" and will retry or fall back instead of fixing the request.
Fix: GetObjectAttributes' input validation — validate the versionId token shape before state lookup, and emit the contextual error members.
Traces: s3-ovm-traj-version-id-argument-mistakes#task#9.

Cause 7 of 8 — GetObject conditional HTTP-date headers are parsed as RFC-3339, so preconditions never evaluate
AWS: Smithy: GetObject.IfUnmodifiedSince / IfModifiedSince are httpHeader-bound timestamps (If-Unmodified-Since / If-Modified-Since); HTTP header timestamps are http-date (RFC 7231 IMF-fixdate, e.g. "Sat, 01 Jan 2000 00:00:00 GMT"). Recorded AWS response when the condition does not hold: HTTP 412, Error.Code "PreconditionFailed", Error.Message "At least one of the pre-conditions you specified did not hold" (fixed text) plus Error.Condition naming the header that failed — here "If-Unmodified-Since", taken from the request.
Emulator: HTTP 400, Error.Code "ValidationException", Error.Message "invalid timestamp `<the header value>`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times", and no Error.Condition. The header is being parsed with the date-time (RFC-3339) parser instead of the http-date parser, so a perfectly valid IMF-fixdate is rejected and the precondition is never evaluated at all.
Why: every conditional read using a date header is unusable — well-formed requests are refused, the 412/200 precedence contract cannot be exercised, and the message leaks the emulator's parser internals with an error code S3 does not define.
Fix: the object-read request deserializer's timestamp binding for header-bound timestamps (http-date), then GetObject's precondition evaluation must produce the PreconditionFailed envelope including Error.Condition.
Traces: s3-txt-conditional-date-headers-and-precedence#task#5 (also #task#10).

Cause 8 of 8 — HeadObject conditional date header: 400 instead of 412
AWS: Smithy: HeadObject.IfUnmodifiedSince / IfModifiedSince are the same http-date header-bound timestamps. Recorded AWS response when the condition fails: HTTP 412 with no body (HEAD carries none).
Emulator: HTTP 400 — the same header-parse failure as Cause 7, surfacing on the HEAD path.
Why: HEAD-based conditional probing (the usual cheap freshness check) reports a client-request fault instead of "precondition failed", so cache-revalidation and if-unmodified-since write guards behave wrongly.
Fix: same deserializer/precondition path, applied to HeadObject.
Traces: s3-txt-conditional-date-headers-and-precedence#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (285 fixtures, 3177 steps). Per cluster, expected (recorded AWS) vs actual (emulator):

1) s3-csvcols-part-number-reads#task#6 GetObject — expected 400 InvalidArgument "Part number must be an integer between 1 and 10000, inclusive" + ArgumentName "partNumber", ArgumentValue "0"; actual 416 InvalidPartNumber "The requested partnumber is not satisfiable", ArgumentName/ArgumentValue missing. 3 steps affected.
2) s3-csvcols-part-number-reads#task#7 GetObject — expected 400 InvalidArgument "Part number must be an integer between 1 and 10000, inclusive" + ArgumentName "partNumber", ArgumentValue "one"; actual 400 ValidationException "invalid i32 `one`: invalid digit found in string".
3) s3-csvcols-part-number-reads#task#8 GetObject — expected 400 InvalidRequest "Cannot specify both Range header and partNumber query parameter" with content-type application/xml; actual 206 with the CSV object body, content-type text/csv. 3 steps affected.
4) s3-csvcols-encryption-header-mistakes#task#4 GetObject — expected 400 InvalidArgument "x-amz-server-side-encryption header is not supported for this operation." + ArgumentName "x-amz-server-side-encryption", ArgumentValue "AES256"; actual 200 with the object body, content-type text/csv. 4 steps affected.
5) s3-csvcols-version-addressing#task#5 GetObject — expected Error.Message "Version id cannot be the empty string"; actual "Invalid version id specified". 2 steps affected.
6) s3-ovm-traj-version-id-argument-mistakes#task#9 GetObjectAttributes — expected 400 InvalidArgument "Invalid version id specified" + ArgumentName "versionId", ArgumentValue = the sent version id; actual 404 NoSuchVersion "The specified version does not exist.".
7) s3-txt-conditional-date-headers-and-precedence#task#5 GetObject — expected 412 PreconditionFailed "At least one of the pre-conditions you specified did not hold" + Error.Condition "If-Unmodified-Since"; actual 400 ValidationException "invalid timestamp `Sat, 01 Jan 2000 00:00:00 GMT`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times", Condition absent. 2 steps affected.
8) s3-txt-conditional-date-headers-and-precedence#task#11 HeadObject — expected 412; actual 400.

(The RequestId value differences reported alongside these clusters are the separately filed request-id token-shape defect, not part of these causes.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: GET /<bucket>/<key>?partNumber=0 — replay s3-csvcols-part-number-reads step task#6.
3. Cause 2: GET /<bucket>/<key>?partNumber=one — replay s3-csvcols-part-number-reads step task#7.
4. Cause 3: GET /<bucket>/<key>?partNumber=1 with a Range header — replay s3-csvcols-part-number-reads step task#8.
5. Cause 4: GET /<bucket>/<key> with header x-amz-server-side-encryption: AES256 — replay s3-csvcols-encryption-header-mistakes step task#4.
6. Cause 5: GET /<bucket>/<key>?versionId= (empty) — replay s3-csvcols-version-addressing step task#5.
7. Cause 6: GET /<bucket>/<key>?attributes&versionId=<malformed id> with x-amz-object-attributes — replay s3-ovm-traj-version-id-argument-mistakes step task#9.
8. Cause 7: GET /<bucket>/<key> with If-Unmodified-Since: Sat, 01 Jan 2000 00:00:00 GMT — replay s3-txt-conditional-date-headers-and-precedence steps task#5 and task#10.
9. Cause 8: the same header on HEAD /<bucket>/<key> — replay s3-txt-conditional-date-headers-and-precedence step task#11.
10. Compare status, Error.Code, Error.Message and the contextual Error members against each recorded response.
