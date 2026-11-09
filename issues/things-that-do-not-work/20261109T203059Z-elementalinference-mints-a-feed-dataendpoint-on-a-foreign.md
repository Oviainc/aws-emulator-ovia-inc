Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.feed-media-data-plane-not-reachable
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference.feed-media-data-plane-not-reachable|run#9|PublishedUrlRejected|data_endpoint = ctx.endpoint_url(got["dataEndpoints"][0])
Issue-Commit: 622e0f603a4886f3f4d7e4980155df7d3d956f5b

# ElementalInference mints a feed dataEndpoint on a foreign AWS host, so the advertised data plane cannot be reached from the emulator (1 cause)

> _Note: this issue was identified against the code base at commit `622e0f603a4886f3f4d7e4980155df7d3d956f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFeed

## Description
One net-new root cause in the elementalinference crate, grounded in a failing behavioral check whose replay key is not carried by any open issue.

Cause 1 of 1 — the feed `dataEndpoints` value the control plane advertises names a host that is not the emulator, so a client that follows the endpoint it was just handed is routed off the emulator entirely and the data plane can never be exercised (check: elementalinference.feed-media-data-plane-not-reachable).

AWS: Smithy: `CreateFeed` http POST /v1/feed code 202, `GetFeed` http GET /v1/feed/{id} code 200; both responses carry `dataEndpoints` targeting `StringList` and **required**. The member is documented as the endpoint at which the feed receives source media (PutMedia) and returns metadata (GetMetadata) — i.e. an endpoint the responding service actually answers on, which a client concatenates a path onto. The media data plane itself is not in the Smithy model, so its request/response contract is recorded behavior only: the recording signs `POST https://<dataEndpoint>/v1/feed/<id>/input/<n>/metadata` with signing name `elemental-inference` and gets a validated answer — HTTP 200 with a metadata document containing an `items` list, or a client error (400/404) whose JSON body carries `message`/`Message`. The requirement is that the advertised authority is one the responder serves; the opaque host label and the domain seen in any one recording are environment-specific and are NOT the requirement.

Emulator: `GetFeed` (and `CreateFeed`/`UpdateFeed`, which share the same minting helper) returns a hard-coded public-AWS-domain URL of the form `https://<10-hex-label>.elemental-inference-data.<region>.api.aws/`. That authority is neither the address the control-plane request reached nor any listener in this build. The harness refuses to route to it before any request is sent: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<label>.elemental-inference-data.<region>.api.aws/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>`. The crate states the gap outright in its own doc comment: "no listener backs them — the field reports where the feed would be reached".

Why: a client following the documented flow (CreateFeed → AssociateFeed → poll GetFeed → read `dataEndpoints[0]` → push media / query metadata) sends its signed data-plane request to a public AWS domain or to a name that does not resolve, instead of to the emulator it is talking to. The failure surfaces as a connection/resolution error rather than a modeled client error, so it is not retryable and cannot be handled; the emulated feature cannot be exercised end to end even though every control-plane call succeeds. A value whose host the responder does not own is also unusable to any recorded-flow replay, since it can never be normalized back to the responder.

Fix: the data-endpoint minting in this crate's state — the helper that formats the URL plus the domain-suffix/label constants — must root the advertised endpoint at the authority the emulator is actually reached on (path shape preserved, opaque label free to differ per environment), and the corresponding feed input/metadata data-plane route must be mounted in this crate's routing so the advertised endpoint is served and validates the signed request against stored feed state. The `GetFeed`/`CreateFeed` handlers are correct in reading/storing the record; the stored value and the missing route are what is wrong, so records already minted need migrating too.

Related but NOT covering this check: open issue 20261029T183811Z-elementalinference-archived-feed-association-refusal-searchfixtures-7-day.md Cause 3 carries this check's OLD replay key (`assert status in (200, 400, 404)`, 501 NotImplemented from a routed data-plane POST) and describes the missing data-plane ROUTE; the check now fails strictly earlier, at `ctx.endpoint_route()`, because the published host is foreign, so that key no longer matches and the suite reports this failure as unowned. Open wire issue 20261109T140415Z-elemental-inference-elementalinference-advertises-a-foreign-aws-data-plane-host.md reports the same `host_not_own` defect from the wire corpus but carries only wire replay keys (CreateFeed/GetFeed/UpdateFeed `$.dataEndpoints[*]`), not this behavioral check. Both conditions must hold: the minted endpoint must name the emulator, AND that endpoint must be served.

Traces: check elementalinference.feed-media-data-plane-not-reachable. Code: services/service-com-amazonaws-elementalinference-elementalinference/src/state.rs lines 2283-2325 (DATA_ENDPOINT_TLD = "api.aws", DATA_ENDPOINT_LABEL_LEN, feed_data_endpoints, data_endpoint_label), with the admission in the doc comment at lines 2306-2309; stored at src/ops/create_feed.rs around line 416.

## Evidence
Suite: simparity-behavioral-aws-v1, 27/30 passed, 3 mismatched (two of them OWNED by an existing issue and skipped).

Failing check elementalinference.feed-media-data-plane-not-reachable, verdict mismatch:

    File "context.py", line 572, in endpoint_route
      return self._published_target(url, "endpoint_url"), {}
    File "context.py", line 586, in _published_target
      raise PublishedUrlRejected(...)
    simparity.published.PublishedUrlRejected: endpoint_url was given
    https://a3fef9610a.elemental-inference-data.us-west-2.api.aws/ (published by the emulator or
    built by the check): its host is not one of the emulator's own hosts; a customer of the
    emulator needs http(s)://127.0.0.1:44023/<resource path>, or http(s)://<own host>:<port>/<path>
    on a per-resource listener

The host label `a3fef9610a` and the port `44023` are per-run values and are not part of the requirement; what diverges is the authority class: the emulator answers the control plane on its own address while handing clients `*.elemental-inference-data.<region>.api.aws`.

Expected (recorded AWS): a sigv4-signed POST to `<dataEndpoints[0]>/v1/feed/<feed-id>/input/0/metadata` with body {"outputName":"sub1","timeSpecification":{"ptsBased":{"startPts":0,"endPts":1001,"timescale":1000}}} reaches a live endpoint and answers HTTP 200 with a body containing an `items` list, or HTTP 400/404 with a body containing `message`/`Message`.

Actual (emulator): the advertised endpoint's host is not the emulator's, so the request is never delivered — the check fails at the routing boundary and no status is produced at all. In an earlier build where the harness did route it, the same endpoint answered HTTP 501 with {"__type":"NotImplemented","message":"operation POST //v1/feed/<id>/input/0/metadata is not implemented in this emulator"}; neither 501 nor the error code NotImplemented is modeled for this service.

Model citations: GetFeedResponse$dataEndpoints target StringList, required; CreateFeedResponse$dataEndpoints target StringList, required; CreateFeed POST /v1/feed code 202; GetFeed GET /v1/feed/{id} code 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. CreateFeed with a unique name and one subtitling output ([{"name":"sub1","outputConfig":{"subtitling":{"language":"eng"}},"status":"ENABLED"}]); note the returned id.
3. AssociateFeed(id, associatedResourceName=<unique name>, outputs=[]), then poll GetFeed(id) until status == ACTIVE.
4. Read dataEndpoints[0] from the GetFeed response and compare its authority with the address the control-plane call was sent to: the emulator returns https://<10-hex>.elemental-inference-data.<region>.api.aws/ while it is reached on its own host:port, so the two differ.
5. Sigv4-sign (signing name from the service model, elemental-inference) and send POST <dataEndpoints[0]>/v1/feed/<feed-id>/input/0/metadata with body {"outputName":"sub1","timeSpecification":{"ptsBased":{"startPts":0,"endPts":1001,"timescale":1000}}}. The request leaves the emulator / is refused at the routing boundary instead of returning 200 with an `items` list or a 400/404 whose body carries a `message`.
6. Expected: dataEndpoints[0]'s authority is the emulator's own, that path is mounted, and the signed POST answers 200/400/404 with a well-formed JSON body.
