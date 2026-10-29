Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.disassociate-feed-archives-feed-irreversibly elementalinference.search-fixtures-date-window-exceeds-seven-days-rejected elementalinference.feed-media-data-plane-not-reachable
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference.disassociate-feed-archives-feed-irreversibly|run#13|AssertionError|assert message == (
Issue-Key: elementalinference.search-fixtures-date-window-exceeds-seven-days-rejected|run#7|AssertionError|assert message == (
Issue-Key: elementalinference.feed-media-data-plane-not-reachable|run#14|AssertionError|assert status in (200, 400, 404), (
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# ElementalInference: ARCHIVED-feed association refusal, SearchFixtures 7-day-window refusal and the advertised feed dataEndpoint all diverge (3 causes)

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateFeed

## Description
Three distinct root causes in the elementalinference crate, all grounded in failing behavioral checks. Each must be closed before submitting.

Cause 1 of 3 — AssociateFeed on an ARCHIVED feed returns the generic "status doesn't allow an association" sentence instead of AWS's ARCHIVED-specific message (check: elementalinference.disassociate-feed-archives-feed-irreversibly).
AWS: a feed that DisassociateFeed moved to ARCHIVED (FeedStatus enum includes ARCHIVED; the transition is one-way) refuses a further AssociateFeed with the modeled ValidationException — Smithy: AssociateFeed errors include ValidationException @httpError 400, ValidationException.message required. Recorded behavior: HTTP 400, code "ValidationException", message exactly "You can't update a feed that has ARCHIVED status. If you need this feed, you should create it again." The whole sentence is FIXED text for this error class; the only variable part is the status token AWS names (ARCHIVED, from the stored feed's status) — no feed id, name or resource name is interpolated.
Emulator: returns ValidationException (400) with the crate's status-independent sentence "You can't associate this feed because its current status doesn't allow an association." — i.e. crate::state::FEED_NOT_ASSOCIABLE_MESSAGE, which deliberately omits the status, so every non-associable status (DELETING, ARCHIVED, …) is reported with one message and the ARCHIVED-specific wording is never produced.
Why: a client that matches on the refusal text (or surfaces it to an operator) cannot tell "this feed is permanently archived, create a new one" from a transient non-associable status, so retry/recreate logic that AWS's message drives takes the wrong branch. The status itself is also lost from the diagnostic.
Fix: the status-precondition arm of the AssociateFeed handler and the shared message constant it uses — the ARCHIVED (terminal) status needs its own refusal sentence naming the status, distinct from the generic non-associable sentence; the effect (feed stays ARCHIVED) is already correct.
Traces: services/service-com-amazonaws-elementalinference-elementalinference/src/ops/associate_feed.rs lines 349-359 (the `_ =>` arm returning crate::state::FEED_NOT_ASSOCIABLE_MESSAGE); check elementalinference.disassociate-feed-archives-feed-irreversibly.

Cause 2 of 3 — SearchFixtures rejects an over-wide date window with a paraphrase of the model doc instead of AWS's error message (check: elementalinference.search-fixtures-date-window-exceeds-seven-days-rejected).
AWS: Smithy: SearchFixtures errors include ValidationException @httpError 400; SearchFixturesRequest$endDate documents "The window from startDate through endDate must not exceed seven days." The recorded refusal of an 11-day window is HTTP 400, code "ValidationException", message exactly "The search window must not span more than 7 days. Narrow the range between startDate and endDate." That sentence is entirely fixed text for this error class (the bound, 7 days, is the service's documented limit; no request value — neither startDate, endDate nor the computed span — is interpolated).
Emulator: correct code and status, but the message is "The window from startDate through endDate must not exceed 7 days." — the handler formats the Smithy doc sentence with crate::state::MAX_SEARCH_WINDOW_DAYS instead of the message AWS actually returns, and it omits the remediation clause.
Why: the refusal is a validation error clients parse and display; a message that differs from AWS's wording breaks text-matching assertions, log-based alerting and any UI that echoes the service's remediation guidance, and makes the emulator unusable as a fidelity reference for this negative class.
Fix: the window-span validation in the SearchFixtures handler — emit AWS's message text for this error class rather than a format of the documentation sentence.
Traces: services/service-com-amazonaws-elementalinference-elementalinference/src/ops/search_fixtures.rs lines 322-330; check elementalinference.search-fixtures-date-window-exceeds-seven-days-rejected.

Cause 3 of 3 — the dataEndpoint advertised by GetFeed/CreateFeed is not served: a sigv4-signed data-plane request to it answers 501 NotImplemented (check: elementalinference.feed-media-data-plane-not-reachable).
AWS: recorded behavior only (no boto3/Smithy operation models this REST data plane; GetFeed$dataEndpoints is a required StringList of real endpoints, and the user guide's "Deliver the source media"/"Query the metadata" steps sign `POST https://<data-endpoint>/v1/feed/<id>/input/<n>/metadata` with signing name elemental-inference). The recording shows that endpoint is live and VALIDATES the request: it answers 200 with a metadata document containing an `items` list, or a client error (400/404) whose body carries a `message`/`Message` field. It never answers 5xx for a well-formed signed request.
Emulator: the advertised host/path is routed to the emulator but has no handler — HTTP 501 with body {"__type":"NotImplemented","message":"operation POST //v1/feed/<id>/input/0/metadata is not implemented in this emulator"} (note also the doubled leading slash in the reported path, i.e. the data-plane URI is not being normalized/matched at all). Status 501 and error code NotImplemented are both unmodelled for this service.
Why: the control plane hands clients an endpoint that is advertised as usable but is dead, so any workload that follows the documented flow — push media to the feed, then query metadata — fails with a non-retryable 5xx/NotImplemented instead of a validated response. The emulator passes control-plane smoke tests while the feature it exists to emulate cannot be exercised end to end.
Fix: the data-plane route for feed input/metadata must exist in this crate's service routing (alongside the control-plane ops) and validate the signed request against stored feed state, answering a well-formed metadata document or a modeled client error; the dataEndpoints value minted when a feed is created must point at a route that is actually mounted.
Traces: check elementalinference.feed-media-data-plane-not-reachable (dataEndpoints[0] from GetFeed after AssociateFeed; POST /v1/feed/{id}/input/0/metadata).

## Evidence
Cause 1: expected (AWS) HTTP 400 ValidationException message "You can't update a feed that has ARCHIVED status. If you need this feed, you should create it again."; actual (emulator) HTTP 400 ValidationException message "You can't associate this feed because its current status doesn't allow an association." Assertion: `assert message == (...)` at check_elementalinference_disassociate_feed_archives_feed_irreversibly.py line 64. Error code and status matched; only the message diverged. The feed did correctly reach and stay in ARCHIVED.

Cause 2: expected (AWS) HTTP 400 ValidationException message "The search window must not span more than 7 days. Narrow the range between startDate and endDate."; actual (emulator) "The window from startDate through endDate must not exceed 7 days." Assertion at check_elementalinference_search_fixtures_date_window_exceeds_seven_days_rejected.py line 37, for a startDate..endDate window of 11 days.

Cause 3: expected HTTP 200 (metadata document with an `items` list) or 400/404 with a `message` body; actual HTTP 501 with body b'{"__type":"NotImplemented","message":"operation POST //v1/feed/<feed-id>/input/0/metadata is not implemented in this emulator"}' for a sigv4-signed POST to the feed's advertised dataEndpoint. Assertion `assert status in (200, 400, 404)` at check_elementalinference_feed_media_data_plane_not_reachable.py line 101.

## Steps to Reproduce
Cause 1: CreateFeed with one subtitling output -> AssociateFeed(id, associatedResourceName=R, outputs=[]) -> poll GetFeed until status ACTIVE -> DisassociateFeed(id, associatedResourceName=R) -> poll GetFeed until status ARCHIVED -> call AssociateFeed(id, associatedResourceName=R, outputs=[]) again and inspect the ValidationException message.

Cause 2: SearchFixtures(sport="basketball", startDate=today-10d, endDate=today) and inspect the ValidationException message; then SearchFixtures with startDate=endDate=today as a positive control.

Cause 3: CreateFeed -> AssociateFeed -> poll GetFeed until ACTIVE -> take dataEndpoints[0] -> sigv4-sign (signing name from the service model) and send POST https://<dataEndpoint>/v1/feed/<feed-id>/input/0/metadata with body {"outputName":"sub1","timeSpecification":{"ptsBased":{"startPts":0,"endPts":1001,"timescale":1000}}} and inspect the HTTP status and body.
