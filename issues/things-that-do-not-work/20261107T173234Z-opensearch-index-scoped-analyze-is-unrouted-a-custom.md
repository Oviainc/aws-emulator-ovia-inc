Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.e2e-package-association-enables-index-analyzer opensearch.e2e-package-association-search-flow opensearch.index-update-schema-semantics opensearch.visibility-timeout-ttl-dry-run-progress-expiry
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.e2e-package-association-enables-index-analyzer|_analyze#16|AssertionError|raise AssertionError(f"could not run _analyze against {endpoint!r}: {last}")
Issue-Key: opensearch.e2e-package-association-search-flow|_assert_package_content_drives_search#27|AssertionError|assert _hits("pkg_text", _KEEP_WORD) == 1, (
Issue-Key: opensearch.index-update-schema-semantics|_poll_until#9|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: opensearch.visibility-timeout-ttl-dry-run-progress-expiry|run#12|AssertionError|assert terminal_status in ("succeeded", "failed"), "dry run never reached a terminal status"
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# opensearch: index-scoped _analyze is unrouted, a custom analyzer matches nothing, GetIndex's IndexSchema is not keyed by index name, and a dry run never reaches a terminal DryRunStatus (4 causes)

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIndex

## Description
This issue bundles 4 net-new root causes, all in the opensearch service. Each cause must be closed before submitting.

Cause 1 of 4: the domain REST data plane has no index-scoped `_analyze` route — `POST /<index>/_analyze` is unrouted.
AWS: recorded behavior only (the data plane is the engine's REST API, not the Smithy control-plane model). An OpenSearch domain answers `POST /<index>/_analyze` with `{"analyzer": "<name>", "text": "<term>"}` with HTTP 200 and a `tokens` array for an analyzer defined in that index's `settings.analysis`, including a `synonym` filter whose `synonyms_path` is `analyzers/<PackageID>` of an associated package.
Emulator: the request is refused HTTP 400 with the engine-shaped body `{"error":"no handler found for uri [/<index>/_analyze] and method [POST]"}` — the fixed text is `no handler found for uri [<uri>] and method [<method>]`, with the uri and method taken from the request. The route exists in no index-scoped form, so the whole analyzer-introspection capability is unreachable.
Why: a client cannot inspect how a field will be tokenized, and cannot verify that an associated analysis package was mounted; code that calls `_analyze` gets a 400 that reads like a malformed request rather than a missing capability. Open issue issues/things-that-do-not-work/20261101T140814Z-opensearch-the-domain-rest-data-plane-serves-only.md records a narrow served route set that names `_analyze`, but it does not carry this check's replay key and the index-scoped POST form is still unrouted.
Fix: the domain REST data-plane router — add the `POST|GET /<index>/_analyze` route and its handler.
Traces: opensearch.e2e-package-association-enables-index-analyzer.

Cause 2 of 4: a `text` field mapped to a custom analyzer matches nothing, and an associated TXT-DICTIONARY package mounted at `analyzers/<PackageID>` drives no analysis at all.
AWS: recorded behavior only. `PUT /<index>` with `settings.analysis.filter.<f> = {"type":"stop","stopwords_path":"analyzers/<PackageID>"}`, `settings.analysis.analyzer.<a> = {"type":"custom","tokenizer":"standard","filter":["lowercase","<f>"]}` and a `text` field using `<a>` is accepted 200; indexing a document (201) and `POST /<index>/_refresh` (200) then make a `match` query on that field return `hits.total.value == 1` for an ordinary term in the text, and `0` for a term the associated package lists as a stopword, while the same text on a sibling `standard`-analyzed control field still returns 1 for that stopword.
Emulator: the ordinary (non-stopword) term returns 0 hits through the custom-analyzed field, so the field matches nothing that was written to it — the custom analyzer chain is not applied when indexing or querying, and the package's content is never loaded.
Why: any index that defines its own analyzer silently stops being searchable: writes report success, refresh reports success, and every query returns an empty result set with HTTP 200, which a client cannot distinguish from "no such document". Custom dictionaries/synonyms attached through AssociatePackage have no effect on search.
Fix: the data plane's index-analysis pipeline — honour `settings.analysis` (custom analyzers, tokenizer and token filters) on both the index and query paths, and mount an associated package's content at `analyzers/<PackageID>` so `stopwords_path`/`synonyms_path` resolve.
Traces: opensearch.e2e-package-association-search-flow.

Cause 3 of 4: GetIndex returns `IndexSchema` as a bare `{"mappings": …, "settings": …}` object instead of a document keyed by the index name.
AWS: Smithy: GetIndex (GET /2021-01-01/opensearch/domain/{DomainName}/index/{IndexName}) returns a required `IndexSchema` of type `document`. Recorded behavior: that document mirrors the engine's mapping API and is keyed by the index name — `{"<IndexName>": {"mappings": {"properties": …}, "settings": {"index": {…}}}}` — which is NOT the flat shape CreateIndex/UpdateIndex accept as input.
Emulator: the response carries the flat form, e.g. `{'mappings': {'properties': {'title': {'type': 'keyword'}}}, 'settings': {'index': {'number_of_shards': '1', 'number_of_replicas': '0'}}}` with no `<IndexName>` key.
Why: a client reading `IndexSchema[<IndexName>]["mappings"]["properties"]` — the documented path — gets a KeyError or an endless readiness poll instead of the mapping, so neither a CreateIndex nor an UpdateIndex result can be read back at all. This is the SAME root cause as Cause 2 of open issue issues/things-that-do-not-work/20261029T182036Z-opensearch-index-reads-writes-and-data-source-attachment.md, which does not carry this check's replay key; closing it there should close this.
Fix: GetIndex's response serializer must nest the stored schema under the index name.
Traces: opensearch.index-update-schema-semantics.

Cause 4 of 4: DescribeDryRunProgress never reports a terminal `DryRunStatus`, so a dry-run config change never completes.
AWS: Smithy: UpdateDomainConfig(DryRun=true, DryRunMode="Verbose") returns `DryRunProgressStatus` with a required 36-char GUID `DryRunId` and a required `DryRunStatus` string; DescribeDryRunProgress (GET /2021-01-01/opensearch/domain/{DomainName}/dryRun?dryRunId=…) echoes that status. Recorded behavior: the status converges to the terminal lowercase value `succeeded` (or `failed`) within seconds, after which a real UpdateDomainConfig overwrites the dry run and the stale id is refused 409 ResourceNotFoundException with `No Dry Run found for the provided DryRunId: <dryRunId>` (fixed text; the id comes from the request).
Emulator: DescribeDryRunProgress keeps answering 200 with a non-terminal `DryRunStatus` for the whole 120 s poll — it never enters `succeeded`/`failed`, so the subsequent overwrite-and-refuse half of the lifecycle is never reached. Open issue issues/things-that-do-not-work/20261030T073207Z-opensearch-3-net-new-root-causes-a-verbose.md records the emulator answering the non-AWS value `COMPLETED` here; it carries none of this check's replay key.
Why: a client that polls a dry run to completion before applying a change loops forever, and never learns whether the proposed configuration validated.
Fix: the dry-run state machine behind UpdateDomainConfig/DescribeDryRunProgress — settle the dry run and report AWS's terminal `DryRunStatus` vocabulary.
Traces: opensearch.visibility-timeout-ttl-dry-run-progress-expiry.

## Evidence
Cause 1 — opensearch.e2e-package-association-enables-index-analyzer: expected 200 with a `tokens` array from `POST /<index>/_analyze`; actual `AssertionError: could not run _analyze against '<host>/_aws/es/<domain>': HTTP 400: {"error":"no handler found for uri [/<index>/_analyze] and method [POST]"}`.
Cause 2 — opensearch.e2e-package-association-search-flow: after a 200 index create, a 201 document write and a 200 `_refresh`, a `match` query on the custom-analyzed field for an ordinary term returns 0 hits — `AssertionError: a non-stopword term must still be found through the package-driven field` (expected `hits.total.value == 1`).
Cause 3 — opensearch.index-update-schema-semantics: expected `IndexSchema[<IndexName>]["mappings"]["properties"]["title"]["type"] == "keyword"`; actual, polled 90 s, `{'mappings': {'properties': {'title': {'type': 'keyword'}}}, 'settings': {'index': {'number_of_shards': '1', 'number_of_replicas': '0'}}}` — no index-name key.
Cause 4 — opensearch.visibility-timeout-ttl-dry-run-progress-expiry: expected `DryRunProgressStatus.DryRunStatus` in ('succeeded','failed') within 120 s; actual `AssertionError: dry run never reached a terminal status` after repeated DescribeDryRunProgress calls that each answered 200 with a non-terminal status.

## Steps to Reproduce
Cause 1: create a domain, CreatePackage(PackageType=TXT-DICTIONARY) from S3, wait AVAILABLE, AssociatePackages, wait DomainPackageStatus=ACTIVE, CreateIndex with `settings.analysis.filter.<f>.synonyms_path = analyzers/<PackageID>` and a custom analyzer using it, then send a SigV4-signed (`es`) `POST https://<Endpoint>/<index>/_analyze` with `{"analyzer":"<a>","text":"quick"}` -> 400 `no handler found for uri [/<index>/_analyze] and method [POST]`.
Cause 2: same setup with a `stop` filter whose `stopwords_path` is `analyzers/<PackageID>`; `PUT /<index>` with a `text` field using that custom analyzer and a `standard`-analyzed control field, `PUT /<index>/_doc/1` with text containing the package's stopwords plus an ordinary term, `POST /<index>/_refresh`, then `POST /<index>/_search` with `{"query":{"match":{"<customField>":"<ordinaryTerm>"}}}` -> `hits.total.value` is 0, expected 1.
Cause 3: CreateIndex(DomainName=<d>, IndexName=<i>, IndexSchema={"mappings":{"properties":{"title":{"type":"keyword"}}},"settings":{"index":{"number_of_shards":"1","number_of_replicas":"0"}}}) then GetIndex(DomainName=<d>, IndexName=<i>) and read `response["IndexSchema"][<i>]` -> the key is absent.
Cause 4: CreateDomain, wait active, UpdateDomainConfig(ClusterConfig={InstanceCount:2}, DryRun=True, DryRunMode="Verbose"), then poll DescribeDryRunProgress(DomainName=<d>, DryRunId=<id>) -> DryRunStatus never becomes `succeeded`/`failed`.
