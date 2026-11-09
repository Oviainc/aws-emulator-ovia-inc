Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-routes.calculate-isolines-distance-threshold geo-routes.calculate-isolines-distance-thresholds-success geo-routes.calculate-isolines-missing-origin-and-destination-rejected geo-routes.calculate-isolines-mixed-threshold-types-rejected
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes.calculate-isolines-distance-threshold|run#7|AssertionError|assert "TimeThreshold" not in isoline, f"unexpected TimeThreshold on a distance-mode isoline: {isoline.get('TimeThreshold')}"
Issue-Key: geo-routes.calculate-isolines-distance-thresholds-success|run#8|AssertionError|assert "TimeThreshold" not in isoline, f"expected no TimeThreshold on a distance-based isoline, got {isoline.get('TimeThreshold')}"
Issue-Key: geo-routes.calculate-isolines-missing-origin-and-destination-rejected|run#4|AssertionError|assert error["Message"] == (
Issue-Key: geo-routes.calculate-isolines-mixed-threshold-types-rejected|run#4|AssertionError|assert error["Message"] == (
Issue-Commit: 8de6682f074d8bd25eca3d1022f82b7ad69da0d3

# CalculateIsolines: inapplicable threshold member emitted as 0 instead of omitted, and two ValidationException messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `8de6682f074d8bd25eca3d1022f82b7ad69da0d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateIsolines

## Description
This issue bundles 3 net-new root causes in the geo-routes CalculateIsolines path (POST /v2/isolines): one in success-response synthesis/serialization and two in request validation. A fixer must close ALL THREE causes before submitting.

The open issue 20261103T145348Z-geo-routes-calculateisolines-response-synthesis-diverges-departuretime-omitted-origin-not.md covers only CalculateIsolines' DepartureTime omission, SnappedOrigin and the x-amz-geo-pricing-bucket header; none of the three causes below (the Isoline threshold members, and the two ValidationException messages) is in its cluster set or its Issue-Keys, so all three are net-new and not covered by it. The other open geo-routes issues cover CalculateRouteMatrix, route synthesis and IAM authorization, none of which is this.

Cause 1 of 3: a distance-mode isoline carries TimeThreshold (and, symmetrically, a time-mode isoline would carry DistanceThreshold) as 0 instead of omitting the member that does not apply.
AWS: Smithy: CalculateIsolinesResponse.Isolines targets IsolineList of Isoline. Isoline.DistanceThreshold (DistanceMeters, long, @default 0, @range 0..4294967295, @sensitive) is documented as "The travel distance in meters used to calculate this isoline, if distance-based thresholds were specified in the request"; Isoline.TimeThreshold (DurationSeconds, long, @default 0, @range 0..4294967295, @sensitive) is documented as "The travel time in seconds used to calculate this isoline, if time-based thresholds were specified in the request". Both are non-required members whose value equals their @default when they do not apply, so the serializer omits them: the threshold member that matches the request's threshold type is present and the other is ABSENT from the isoline object, not present with the value 0. Recorded behavior agrees: a request whose Thresholds carries only Distance comes back with isolines that have DistanceThreshold and no TimeThreshold key.
Emulator: for a Thresholds={"Distance": [...]} request the emulator returns each isoline with the correct DistanceThreshold AND an extra "TimeThreshold": 0. The replayed assertion reads: AssertionError: unexpected TimeThreshold on a distance-mode isoline: 0, and in the two-threshold variant: expected no TimeThreshold on a distance-based isoline, got 0.
Why: presence of the threshold member is how a client tells a distance isoline from a time isoline, because the two modes are mutually exclusive in one request. With TimeThreshold always present, a client that branches on key presence (`if "TimeThreshold" in isoline`) or that maps isolines back to the thresholds it asked for treats every distance isoline as a 0-second time isoline — a reachable area of zero travel time — and renders or buckets it against the wrong axis. The reverse holds for time-mode requests whose DistanceThreshold would read as a 0-metre reach.
Fix: the CalculateIsolines response serializer / Isoline builder — omit a threshold member whose value equals its modeled @default (equivalently: populate only the member matching the request's threshold type) rather than always writing both longs.
Traces: geo-routes.calculate-isolines-distance-threshold, geo-routes.calculate-isolines-distance-thresholds-success.

Cause 2 of 3: the ValidationException message for a request with neither Origin nor Destination does not quote the field names as AWS does.
AWS: Smithy: CalculateIsolinesRequest has Origin and Destination both optional (only Thresholds is @required), so the "at least one of" rule is not expressible in the model — recorded behavior only: the call is refused with HTTP 400, error code ValidationException and the message "At least one of the following fields must be set: 'Origin', 'Destination'." Fixed text of this message class: "At least one of the following fields must be set: " followed by the applicable field names each wrapped in single quotes, comma-separated, then a period. The variable part is the list of field names taken from the operation's input members (here Origin and Destination); the quoting, separators and trailing period are fixed text and not per-request values.
Emulator: refuses with the right code but the message "At least one of the following fields must be set: Origin, Destination." — the single quotes around each field name are missing. Status and error code match; only the message text diverges.
Why: this message is part of the API contract. Clients, test suites and support tooling match these strings (and humans read them) to distinguish "you omitted a required combination" from other 400s; an emulator-only spelling makes any assertion or log-based alert on the real AWS text fail against the emulator and pass bad code through.
Fix: the CalculateIsolines request-validation component that raises the "at least one of" ValidationException — the message template must quote each field name.
Traces: geo-routes.calculate-isolines-missing-origin-and-destination-rejected.

Cause 3 of 3: the ValidationException message for mixed Thresholds types is a different message class from AWS's.
AWS: Smithy: IsolineThresholds is a structure with optional Distance (DistanceThresholdList, @length 1..5) and Time (TimeThresholdList, @length 1..5), documented as "You can provide up to five thresholds for a single type" — all thresholds in one request must be the same type, a rule the model cannot express, so: recorded behavior only. A request setting both Thresholds.Distance and Thresholds.Time is refused with HTTP 400, code ValidationException and the message "Only one of the following fields may be set within 'Thresholds': 'Consumption', 'Distance', 'Time'." Fixed text: "Only one of the following fields may be set within " + the quoted container member name + ": " + the quoted, comma-separated list of the mutually exclusive member names + "." — AWS names the full exclusive set for the container (Consumption, Distance, Time), not just the two the caller happened to send, so the list is a property of the shape and not of the request.
Emulator: refuses with the right code but reports "'Thresholds.Time' must not be set when 'Thresholds.Distance' is set." — a different message class altogether: it names only the two members present, uses dotted paths instead of a container plus member names, and phrases the rule as a conditional rather than as a mutual-exclusion set.
Why: same contract concern as Cause 2, with an extra loss: AWS's message tells the client the complete exclusive set, so a client discovering the constraint from the error learns Consumption is in it too. The emulator's wording hides that and breaks any matching on the real text.
Fix: the CalculateIsolines request-validation component for IsolineThresholds — replace the pairwise "must not be set when" check/message with the mutual-exclusion message template over the container's full exclusive member set.
Traces: geo-routes.calculate-isolines-mixed-threshold-types-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator: 56/62 passed (90.3%), 6 mismatched, 0 errored, 0 skipped. All 6 failures are under service heading `### geo-routes`; 2 of them (calculate-routes-missing-permission-rejected, iam-deny-calculate-routes-access-denied) are marked OWNED by 20261029T183444Z-geo-routes-performs-no-iam-authorization-calculateroutes-succeeds.md and are excluded here. The 4 clustered below are not marked ALREADY FILED or OWNED.

Cause 1 — [mismatch] geo-routes.calculate-isolines-distance-threshold:
  request: CalculateIsolines(Origin=<fixed position>, Thresholds={"Distance": [2000]}, TravelMode="Car", IsolineGeometryFormat="Simple")
  response: HTTP 200; Isolines[0].DistanceThreshold correct, but the isoline object also contains TimeThreshold.
  assertion: `assert "TimeThreshold" not in isoline` -> AssertionError: unexpected TimeThreshold on a distance-mode isoline: 0

Cause 1 — [mismatch] geo-routes.calculate-isolines-distance-thresholds-success:
  request: CalculateIsolines(Origin=<fixed position>, TravelMode="Pedestrian", Thresholds={"Distance": [1000, 2000]})
  response: HTTP 200 with 2 isolines; both carry TimeThreshold.
  assertion: `assert "TimeThreshold" not in isoline` -> AssertionError: expected no TimeThreshold on a distance-based isoline, got 0
  (this check's later assertions on geometry format and on distinct geometry per threshold were never reached, so nothing is claimed about them here)

Cause 2 — [mismatch] geo-routes.calculate-isolines-missing-origin-and-destination-rejected:
  request: CalculateIsolines(Thresholds={"Time": [600]}, TravelMode="Car") — no Origin, no Destination.
  expected: Error.Code == "ValidationException" (HTTP 400) and Error.Message == "At least one of the following fields must be set: 'Origin', 'Destination'."
  actual:   Error.Code == "ValidationException" (matches); Error.Message == "At least one of the following fields must be set: Origin, Destination."
  diff: the field names are unquoted. Fixed text = "At least one of the following fields must be set: " ... "."; variable part = the field-name list (Origin, Destination), each of which must be wrapped in single quotes.
  assertion: AssertionError: unexpected message: 'At least one of the following fields must be set: Origin, Destination.'

Cause 3 — [mismatch] geo-routes.calculate-isolines-mixed-threshold-types-rejected:
  request: CalculateIsolines(Origin=<fixed position>, Thresholds={"Distance": [2000], "Time": [600]})
  expected: Error.Code == "ValidationException" (HTTP 400) and Error.Message == "Only one of the following fields may be set within 'Thresholds': 'Consumption', 'Distance', 'Time'."
  actual:   Error.Code == "ValidationException" (matches); Error.Message == "'Thresholds.Time' must not be set when 'Thresholds.Distance' is set."
  diff: wrong message class. Fixed text = "Only one of the following fields may be set within " + '<container>' + ": " + quoted member list + "."; variable parts = the container member name ('Thresholds') and the exclusive member set of that shape ('Consumption', 'Distance', 'Time') — not the subset the request happened to populate.
  assertion: AssertionError: unexpected message: "'Thresholds.Time' must not be set when 'Thresholds.Distance' is set."

The emulator was up and serving throughout (the same checks' positive controls and 56 other checks returned 200s), so none of these is a transport, tooling or per-run-value artefact: Causes 2 and 3 compare only fixed message text and Cause 1 compares only key presence.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1 against it.
2. Cause 1: run check geo-routes.calculate-isolines-distance-threshold — CalculateIsolines with Thresholds={"Distance": [2000]}, TravelMode="Car", IsolineGeometryFormat="Simple". Inspect Isolines[0]: the key "TimeThreshold" is present with value 0; it must be ABSENT because no time threshold was requested. Repeat with geo-routes.calculate-isolines-distance-thresholds-success (Thresholds={"Distance": [1000, 2000]}) — both isolines must carry DistanceThreshold only. Also check the symmetric direction: a Thresholds={"Time": [600]} request must return isolines with TimeThreshold and NO DistanceThreshold key.
3. Cause 2: run check geo-routes.calculate-isolines-missing-origin-and-destination-rejected — CalculateIsolines(Thresholds={"Time": [600]}, TravelMode="Car") with neither Origin nor Destination. The call is correctly refused with HTTP 400 / ValidationException, but compare Error.Message to "At least one of the following fields must be set: 'Origin', 'Destination'." — the emulator omits the single quotes around each field name.
4. Cause 3: run check geo-routes.calculate-isolines-mixed-threshold-types-rejected — CalculateIsolines(Origin=<any valid position>, Thresholds={"Distance": [2000], "Time": [600]}). The call is correctly refused with HTTP 400 / ValidationException, but compare Error.Message to "Only one of the following fields may be set within 'Thresholds': 'Consumption', 'Distance', 'Time'." — the emulator instead returns the pairwise form "'Thresholds.Time' must not be set when 'Thresholds.Distance' is set."
5. After fixing, re-replay exactly these 4 checks: geo-routes.calculate-isolines-distance-threshold, geo-routes.calculate-isolines-distance-thresholds-success, geo-routes.calculate-isolines-missing-origin-and-destination-rejected, geo-routes.calculate-isolines-mixed-threshold-types-rejected. All four must pass (note checks 1 and 4 also assert their positive controls still return exactly 1 isoline, and check 2 goes on to assert default FlexiblePolyline geometry and distinct geometry per threshold once the TimeThreshold assertion stops failing).
