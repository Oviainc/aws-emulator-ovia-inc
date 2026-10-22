Issue-Source: trace-sweep-divergence
Issue-Trace: elementalinference-fixtures-search-and-get
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elementalinference-fixtures-search-and-get
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elemental-inference|SearchFixtures|2xx->2xx|$.fixtures[*]
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# SearchFixtures returns a seeded fixture catalog regardless of the request

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchFixtures

## Description
Cause 1 of 1 - SearchFixtures answers from a built-in catalog instead of from the search criteria.
AWS: Smithy: SearchFixtures (POST /v1/fixtures, 200) takes a required `sport` (DataSourceSport enum), a required `startDate` and optional `endDate` (FixtureDate, @pattern ^\\d{4}-\\d{2}-\\d{2}$), optional `filters` (@length max 10) and `maxResults` (range 1-100), is @paginated over `fixtures`, and returns a required `fixtures` list of FixtureSummary. The model does not describe matching semantics, so recorded behavior only: the recorded response for the probed sport/date window returns a `fixtures` list with no entries for a window the service has no events in - the list is populated only from fixtures that satisfy the request.
Emulator: it returns five hard-coded entries that are plainly emulator seed data and are unrelated to the request - fixtureIds basketball-20240101-01..basketball-20240102-01, fixtureGroup "Emulator Basketball League", scheduledStart values in the recorded window's past, status Completed, with competitor names from a fixed roster. Replay reports them as `$.fixtures[0]..[4]: extra (expected None, got {...})`, i.e. the response carries rows the request does not select.
Why: a client paginating SearchFixtures to discover what it can clip gets fixtures that do not match its sport/date/filters, then feeds those fixtureIds into GetFixture or a clipping output; and a client that treats a non-empty `fixtures` list as "events exist in this window" branches the wrong way. It also makes maxResults/nextToken pagination meaningless, since page content does not derive from the query.
Emulator seed rows are also emitted with scheduledStart as an epoch number; GetFixtureResponse.scheduledStart is a Timestamp with timestampFormat date-time, so the seeded data would not serialize correctly either - noted as context, the divergence filed here is the unfiltered catalog itself.
Fix: the SearchFixtures handler - select from fixture state using the request's sport, startDate/endDate window and filters (and honor maxResults/nextToken over that selection) instead of returning a constant list.
Traces: elementalinference-fixtures-search-and-get#task#1.

## Evidence
Replay of simparity-wire-aws-v1, step elementalinference-fixtures-search-and-get#task#1 (SearchFixtures), expected 200 / actual 200: `$.fixtures[0]: extra (expected None, got {'fixtureId': 'basketball-20240101-01', 'name': 'Dunmore Dynamos at Ironwood Icons', 'fixtureGroup': 'Emulator Basketball League', 'scheduledStart': 1704128400, 'status': 'Completed', 'competitors': [...]})` and the same for `$.fixtures[1]`..`$.fixtures[4]` (basketball-20240101-02/-03/-04, basketball-20240102-01). The recorded response selected none of these for the request's criteria.

## Steps to Reproduce
1. Replay fixture elementalinference-fixtures-search-and-get, step task#1: POST /v1/fixtures with the fixture's sport and start/end date window.
2. The 200 body contains five 'Emulator Basketball League' entries that the request's criteria do not select; the recorded response's `fixtures` list has none of them.
3. Re-issue the same call with a different startDate/endDate or filters - the same five rows come back, showing the response is independent of the request.
