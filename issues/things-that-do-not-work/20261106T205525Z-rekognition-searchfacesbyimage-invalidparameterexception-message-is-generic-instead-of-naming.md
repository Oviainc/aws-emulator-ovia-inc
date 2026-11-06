Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-index-search-and-delete-faces
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-index-search-and-delete-faces
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|SearchFacesByImage|4xx->4xx|$.Message
Issue-Commit: 35b42028f3202d783cfd444fbd332c2960d24a7a

# SearchFacesByImage InvalidParameterException message is generic instead of naming the no-face condition

> _Note: this issue was identified against the code base at commit `35b42028f3202d783cfd444fbd332c2960d24a7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchFacesByImage

## Description
Cause 1 of 1: SearchFacesByImage rejects an image that contains no detectable face with the right error type and status, but with a placeholder message instead of the message AWS words for that specific condition.

AWS: Smithy (com.amazonaws.rekognition#SearchFacesByImage) models InvalidParameterException (errorKind client, httpError 400, members Message/Code/Logref) with the doc "Input parameter violated a constraint. Validate your parameter before calling the API operation again." SearchFacesByImage takes a required Image and searches the single largest face in it; when the supplied image contains no face there is nothing to search by, and the recorded AWS response for that condition is HTTP 400 InvalidParameterException with Message "There are no faces in the image. Should be at least 1." The model does not spell that sentence out, so for the exact wording this is recorded behavior only. The message is entirely fixed text for this error class - it has no variable parts taken from the request or from state (no collection id, face id, count or threshold is interpolated), so it must be reproduced verbatim whenever the input image yields zero faces.

Emulator: answers HTTP 400 (status matches) but serialises $.Message as "Request has invalid parameters" - a single generic string apparently used for every InvalidParameterException raised by this operation, with no branch for the no-face case.

Why: clients and SDK-based code distinguish Rekognition's InvalidParameterException sub-conditions by the Message text, because the error code is the same for a bad CollectionId, a bad threshold and a face-less image. Retry/branching logic that keys on "There are no faces in the image" (for example to fall back to IndexFaces or to skip the frame) never matches against the emulator, so the no-face path is silently mis-handled; assertions and log scrapes on the AWS wording also fail.

Fix: the SearchFacesByImage handler's input/image validation in the rekognition service crate - where the zero-detected-faces condition is turned into InvalidParameterException - must emit the condition-specific message rather than falling back to the operation's generic InvalidParameterException text; the error serializer already places Message correctly.

Traces: rekognition-index-search-and-delete-faces#task#5 (expected $.Message 'There are no faces in the image. Should be at least 1.', got 'Request has invalid parameters').

Note: the other two rekognition divergences in this replay are already owned by open issues 20261106T074501Z-rekognition-recognizecelebrities-... and 20261102T103806Z-rekognition-detectlabels-... and are not re-filed here.

## Evidence
Fixture rekognition-index-search-and-delete-faces, step task#5, operation SearchFacesByImage.
Expected (recorded AWS): HTTP 400, InvalidParameterException, $.Message = "There are no faces in the image. Should be at least 1."
Actual (emulator): HTTP 400, $.Message = "Request has invalid parameters"
Verdict: mismatch on $.Message (value_mismatch); status matched 400->400.
Fixed text of the AWS message: the whole sentence "There are no faces in the image. Should be at least 1." Variable parts: none.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture rekognition-index-search-and-delete-faces: create a collection with CreateCollection, then call SearchFacesByImage against it with an image that contains no detectable face (step task#5).
3. Inspect the 400 response body: $.Message is "Request has invalid parameters" instead of "There are no faces in the image. Should be at least 1."
