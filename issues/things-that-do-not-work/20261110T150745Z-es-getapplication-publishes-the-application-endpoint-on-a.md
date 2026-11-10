Issue-Source: trace-sweep-divergence
Issue-Trace: esdef-application-dead-id-probe
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: esdef-application-dead-id-probe
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|GetApplication|2xx->2xx|$.endpoint
Issue-Commit: 6c9eb5ccb540e6cc94c4b43537edc00f8c6eb6bb

# es GetApplication publishes the application endpoint on a real-AWS opensearch.amazonaws.com host the emulator does not serve

> _Note: this issue was identified against the code base at commit `6c9eb5ccb540e6cc94c4b43537edc00f8c6eb6bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
One net-new cause in the es (OpenSearch Service) application read path; all other divergences this replay reported are already owned by open issues.

Cause 1 of 1: GetApplication's `endpoint` is minted on a real AWS production hostname instead of the address the emulator is reached on.

AWS: Smithy (com-amazonaws-opensearch-amazonopensearchservice): GetApplication, GET /2021-01-01/opensearch/application/{id}, success code 200, output member `endpoint` of type String (no pattern or length constraint in the model); errors include ResourceNotFoundException (httpError 409) and ValidationException (400). What the member means is recorded behavior only: it is the HTTPS URL a client follows to reach that application's OpenSearch/Dashboards data plane, and it must be a host the serving endpoint actually answers on. The recorded step does not assert a literal URL (expected value is '<shaped-not-asserted>'), because the host, id and region are environment-specific; what it asserts is host ownership — the endpoint an emulator publishes must be the emulator's own address.

Emulator: the 200 body carries `endpoint` built from an AWS production template, `https://<application-id>.<region>.opensearch.amazonaws.com` (the replay observed one such value for the probed application id), and the replay check fails as `$.endpoint: host_not_own`. Status, error class and the rest of the envelope match; only the announced host diverges.

Why: a client that reads an application and then follows `endpoint` to query the application's search/Dashboards plane is sent off the emulator entirely. Nothing in the emulator listens on *.opensearch.amazonaws.com, so the request either resolves to real AWS, is refused by egress, or fails name resolution — the advertised data plane is unreachable, and any flow that chains GetApplication into a dashboards/search request cannot be exercised at all even though the control-plane call succeeded.

Fix: the application projection used by the es/opensearch GetApplication handler (and whatever mints/stores the application endpoint when the application is created, so the stored value is not already an AWS host) must derive the endpoint's scheme and host from the emulator's own serving endpoint/base URL, keeping only the per-application id/label part as a path or sub-label. The host must never be a hard-coded *.opensearch.amazonaws.com template.

Traces: esdef-application-dead-id-probe#task#2 (GetApplication, HTTP 200, $.endpoint host_not_own).

## Evidence
Replay cluster (trace set simparity-wire-aws-v1, service=es, GetApplication, status 200 -> 200):
  fixture_id: esdef-application-dead-id-probe
  step_key:   task#2
  detail:     $.endpoint: host_not_own (expected '<shaped-not-asserted>', got 'https://sfi9sv1baaaa.us-west-2.opensearch.amazonaws.com')
  divergence_key: service=es|GetApplication|status=200->200|keys=$.endpoint
  affected_count: 1 (esdef-application-dead-id-probe#task#2)
  owned_by: [] (no open issue carries this replay key); stub_path: "" (operation is implemented)

Expected: `endpoint` is an absolute HTTPS URL whose host is the endpoint the emulator serves (the host the GetApplication request itself was made to), with the application-specific id carried in the path or as a sub-label.
Actual: `endpoint` is an absolute HTTPS URL on the real AWS host template <application-id>.<region>.opensearch.amazonaws.com, which the emulator does not serve. The application id and region in the recorded value are per-run values filling that template, not requirements.

Related but NOT covering this: open issue 20261105T004752Z-es-createapplication-still-mints-a-constant-application-id.md concerns the minted application id and its derived ARN, not the published endpoint host.

## Steps to Reproduce
1. Boot the emulator from this checkout and note the host:port the data listener is reached on.
2. Replay fixture `esdef-application-dead-id-probe`, step `task#2`: CreateApplication for the probe application, then GET /2021-01-01/opensearch/application/{id} against the emulator's es endpoint.
3. Read `endpoint` in the 200 body. Observed: an https://<application-id>.<region>.opensearch.amazonaws.com URL (a real AWS host). Expected: a URL whose host is the emulator's own serving endpoint.
4. Attempt to connect to the returned `endpoint`: nothing in the emulator answers it, confirming the advertised application data plane is unreachable.
