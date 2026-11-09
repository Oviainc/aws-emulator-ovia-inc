Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-optimize-waypoints-origin-only-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-optimize-waypoints-origin-only-rejected
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|OptimizeWaypoints|4xx->4xx|$.fieldList[*].message
Issue-Key: geo-routes|OptimizeWaypoints|4xx->4xx|$.message
Issue-Commit: b3a669b6381e9a6af7dec70573ce410127db568e

# OptimizeWaypoints ValidationException message quotes the member names, diverging from AWS wording

> _Note: this issue was identified against the code base at commit `b3a669b6381e9a6af7dec70573ce410127db568e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OptimizeWaypoints

## Description
Cause 1 of 1: the ValidationException the emulator raises when an OptimizeWaypoints request supplies only Origin carries a message whose fixed text does not match AWS's, because the emulator wraps the member names it interpolates in single quotes.

AWS: Smithy georoutes#OptimizeWaypoints models Origin as @required and Destination / Waypoints as optional members, and binds ValidationException (errorKind client, httpError 400) with required members Message (jsonName "message"), Reason (jsonName "reason", enum ValidationExceptionReason incl. "Missing") and FieldList (jsonName "fieldList", list of ValidationExceptionField {name, message}). The model does not state the message text, so for the wording this is recorded behavior only: the recorded 400 response carries the message, and the same message repeated in fieldList[0].message, in the form "At least one of the following fields must be set: <Member>, <Member>." — i.e. the fixed text is "At least one of the following fields must be set: ", then the bare member names in model order separated by ", ", then a terminating ".". The member names are the variable part, derived from which of the mutually-required members the request omitted (here Destination and Waypoints); nothing else in the message varies.

Emulator: returns the correct status (400) and the correct error class, and populates fieldList, but renders each interpolated member name inside single quotes: "At least one of the following fields must be set: 'Destination', 'Waypoints'." — in both $.message and $.fieldList[0].message. Every other field of the response matches.

Why: the error message is part of the contract for this error class. A client that surfaces, logs or asserts on the message sees text AWS never emits, and any consumer that parses the member names out of the message (a common pattern for turning a mutually-required-field violation into a form/field-level error) extracts names decorated with quote characters that are not part of the identifier, so lookups against the request shape or a field map fail. Because the same wrong text is duplicated into fieldList[0].message, a client using the structured field list is misled too.

Fix: the OptimizeWaypoints request validation path that detects the "none of Destination/Waypoints supplied" condition and builds the ValidationException — specifically the message template it formats the omitted member names into. The names must be interpolated bare, with no surrounding quote characters, and the same string must be used for both the top-level message and the fieldList entry's message.

Traces: geo-routes-optimize-waypoints-origin-only-rejected#task#1 (the only recorded step exercising this error class; the fixture's task#2 diverges for an unrelated, already-filed reason).

## Evidence
Fixture geo-routes-optimize-waypoints-origin-only-rejected, step task#1, POST /v2/optimize-waypoints with Origin only.

Status: expected 400, actual 400 (matches). Error class ValidationException (matches).

Message text diverges in two places:
  $.message            expected: At least one of the following fields must be set: Destination, Waypoints.
                       actual:   At least one of the following fields must be set: 'Destination', 'Waypoints'.
  $.fieldList[0].message expected: At least one of the following fields must be set: Destination, Waypoints.
                         actual:   At least one of the following fields must be set: 'Destination', 'Waypoints'.

Fixed text of this error class: "At least one of the following fields must be set: " ... ".". Variable parts: the omitted mutually-required member names (Destination, Waypoints), taken from the request. The emulator adds a single-quote character before and after each variable member name; the recorded AWS response has none.

Smithy evidence: georoutes#OptimizeWaypoints input — Origin @required, Destination and Waypoints optional; errors include ValidationException (client, httpError 400). georoutes#ValidationException members Message/Reason/FieldList (jsonName message/reason/fieldList, all required); georoutes#ValidationExceptionField members Name/Message (jsonName name/message).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or issue the request directly.
2. POST to the geo-routes endpoint path /v2/optimize-waypoints with a body that sets only Origin (a 2-element Position) and omits both Destination and Waypoints — exactly fixture geo-routes-optimize-waypoints-origin-only-rejected, step task#1.
3. Observe the 400 ValidationException body.
4. Compare $.message and $.fieldList[0].message against the recorded response: the emulator wraps each omitted member name in single quotes ("'Destination', 'Waypoints'") where AWS emits the bare names ("Destination, Waypoints").
