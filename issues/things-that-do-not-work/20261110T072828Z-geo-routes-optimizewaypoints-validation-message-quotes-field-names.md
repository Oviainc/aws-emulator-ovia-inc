Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-routes.optimize-waypoints-origin-only-minimal-success geo-routes.snap-to-roads-not-matched-notice
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes.optimize-waypoints-origin-only-minimal-success|run#4|AssertionError|assert error["Message"] == "At least one of the following fields must be set: Destination, Waypoints.", (
Issue-Key: geo-routes.snap-to-roads-not-matched-notice|run#6|AssertionError|assert resp["Notices"] == [
Issue-Commit: db7313a8939a25ce751a06463082be3acfaf223f

# geo-routes: OptimizeWaypoints validation message quotes field names, and SnapToRoads mis-codes the off-road trace-point notice

> _Note: this issue was identified against the code base at commit `db7313a8939a25ce751a06463082be3acfaf223f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OptimizeWaypoints, SnapToRoads

## Description
Two net-new root causes in the geo-routes data plane. Both must be closed before submitting.

Cause 1 of 2: OptimizeWaypoints — ValidationException message wraps the member names in single quotes.
AWS: OptimizeWaypoints requires Origin only (Smithy: OptimizeWaypointsRequest.Origin @required; Destination and Waypoints both optional), but an Origin-only request with neither Destination nor Waypoints is refused with HTTP 400 ValidationException (Smithy: OptimizeWaypoints errors include ValidationException, errorKind client, httpError 400). The recorded message is exactly: "At least one of the following fields must be set: Destination, Waypoints." Fixed text: "At least one of the following fields must be set: " followed by the names of the unset alternative members, comma-and-space separated, BARE (no quoting of any kind), terminated by a period. The variable part is the member-name list, which comes from the operation's own set of mutually-alternative members — here Destination, Waypoints.
Emulator: returns the right code and status, but the message is "At least one of the following fields must be set: 'Destination', 'Waypoints'." — each member name wrapped in ASCII single quotes.
Why: clients and SDK-level error handling that match or display this message (and parity/assertion harnesses that compare it) see text AWS never emits; the quoting also leaks an emulator-internal formatting convention into a customer-visible string, so any client keying on the exact AWS wording mis-handles the rejection.
Why (general): wherever this "at least one of" validator formats a member-name list it must emit bare names, not just for this one operation/member pair.
Fix: the OptimizeWaypoints input validation in the geo-routes service crate — specifically the helper that renders the missing-alternative-members message; drop the quoting when joining member names.
Traces: geo-routes.optimize-waypoints-origin-only-minimal-success.

Cause 2 of 2: SnapToRoads — a trace point with no nearby road is reported with the wrong notice Code, and the notice carries an invented Title.
AWS: recorded behavior only for the classification. For SnapToRoads with a trace-point list where one position lies far from any mapped road, real AWS still snaps every point (SnappedTracePoints has one entry per input TracePoint) and reports exactly one notice whose Code is TracePointsMovedByLargeDistance, with TracePointIndexes listing the 0-based indexes of the offending input trace points (Smithy: RoadSnapNoticeCode enum includes both TracePointsMovedByLargeDistance and TracePointsNotMatched; RoadSnapNotice has members Code, Title, TracePointIndexes). The recorded response's notice carries only Code and TracePointIndexes — no Title key is present on the wire, even though RoadSnapNotice.Title is modeled @required; the recorded response is authoritative for what the emulator must serialize. Variable part: the index list, taken from the request's TracePoints positions; fixed part: the Code enum value.
Emulator: returns one notice with Code "TracePointsNotMatched" and an extra Title member whose text is "1 trace points had no road of the network at all: the position is outside the mapped territory the network covers" — prose that is not AWS's and is assembled by the emulator (it even mis-agrees in number). So the emulator both mis-classifies the condition (NotMatched rather than MovedByLargeDistance, contradicting its own response, which DID snap the point) and emits a member AWS omits here.
Why: a client that branches on notice Code to decide whether a snapped position is usable sees TracePointsNotMatched — which means the point was not matched at all — while the emulator simultaneously hands back a snapped position for it, so the two parts of the response disagree and the client discards usable results (or trusts a point AWS would have flagged as moved). Clients that compare the notice structure against AWS see an extra Title field and a different code.
Fix: the SnapToRoads handler's notice construction/serialization in the geo-routes service crate: classify a point that was snapped but displaced far from its input position as TracePointsMovedByLargeDistance, and emit the notice with the member set AWS puts on the wire for this case (Code + TracePointIndexes) rather than synthesizing Title prose.
Traces: geo-routes.snap-to-roads-not-matched-notice.

## Evidence
Cause 1 — geo-routes.optimize-waypoints-origin-only-minimal-success:
  call: optimize_waypoints(Origin=[-122.3410, 47.6089])
  expected (AWS): Code="ValidationException", Message="At least one of the following fields must be set: Destination, Waypoints."
  actual (emulator): Code="ValidationException", Message="At least one of the following fields must be set: 'Destination', 'Waypoints'."
  assertion: AssertionError: unexpected message: "At least one of the following fields must be set: 'Destination', 'Waypoints'."

Cause 2 — geo-routes.snap-to-roads-not-matched-notice:
  call: snap_to_roads(TracePoints=[{Position:[-122.3421,47.6101]},{Position:[-122.3428,47.6110]},{Position:[-122.3433,47.6120]},{Position:[-140.0,40.0]}], TravelMode="Truck", TravelModeOptions={Truck:{GrossWeight:12000,Height:400,Length:1200,Width:250}})
  expected (AWS): Notices == [{"Code": "TracePointsMovedByLargeDistance", "TracePointIndexes": [3]}]
  actual (emulator): Notices == [{"Code": "TracePointsNotMatched", "Title": "1 trace points had no road of the network at all: the position is outside the mapped territory the network covers", "TracePointIndexes": [3]}]
  assertion: AssertionError: expected a single TracePointsMovedByLargeDistance notice for index 3, got [{'Code': 'TracePointsNotMatched', 'Title': '...', 'TracePointIndexes': [3]}]
  (SnappedTracePoints was correct: 4 entries, each with 2-element SnappedPosition and Confidence in [0,1].)

## Steps to Reproduce
Cause 1:
1. Point a geo-routes client at the emulator.
2. Call OptimizeWaypoints with Origin=[-122.3410, 47.6089] and no Destination and no Waypoints.
3. Observe HTTP 400 ValidationException; compare Message to "At least one of the following fields must be set: Destination, Waypoints." — the emulator single-quotes each member name.

Cause 2:
1. Call SnapToRoads with four TracePoints: three close-together downtown-Seattle road fixes and one position far offshore ([-140.0, 40.0]), TravelMode="Truck" with Truck options (GrossWeight 12000, Height 400, Length 1200, Width 250).
2. Inspect the response's Notices: AWS returns a single notice with Code TracePointsMovedByLargeDistance and TracePointIndexes [3] and no Title on the wire; the emulator returns Code TracePointsNotMatched plus a synthesized Title.

Replay: run_behavioral_suite and re-check geo-routes.optimize-waypoints-origin-only-minimal-success and geo-routes.snap-to-roads-not-matched-notice.
