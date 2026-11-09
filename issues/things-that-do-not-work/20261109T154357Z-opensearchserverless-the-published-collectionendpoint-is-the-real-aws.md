Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving opensearchserverless.server-side-network-policy-enforces-vpc-only-access
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name|run#11|AssertionError|assert not (p := ctx.published_url_mismatch(endpoint, labels=("aoss",))), p
Issue-Key: opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing|run#15|AssertionError|assert not (p := ctx.published_url_mismatch(endpoint, labels=("aoss",))), p
Issue-Key: opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving|run#12|AssertionError|assert not (p := ctx.published_url_mismatch(endpoint, labels=("aoss",))), p
Issue-Key: opensearchserverless.server-side-network-policy-enforces-vpc-only-access|run#41|PublishedUrlRejected|url, route = ctx.endpoint_route(host)
Issue-Commit: 44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07

# opensearchserverless: the published collectionEndpoint is the real AWS host <id>.<region>.aoss.amazonaws.com, an address the emulator does not serve, so every collection data-plane consumer is refused at egress before any request is sent

> _Note: this issue was identified against the code base at commit `44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCollection

## Description
ONE net-new root cause, grounded in four failing behavioral checks listed under the suite heading `### opensearchserverless`.

Overlap note (not a duplicate): open issue 20261104T072704Z-opensearchserverless-the-published-collectionendpoint-serves-the-aws-json.md and open issue 20261108T223024Z-opensearchserverless-3-net-new-causes-on-the-collection.md describe LATER walls on the same endpoint (the endpoint routing to the AWS-JSON x-amz-target dispatcher, and the authorization layer behind that front door not evaluating data-access/network policies). Both of those presuppose that the published endpoint can be reached at all. In this replay it cannot: the host published as collectionEndpoint is no longer an emulator address, so none of those checks get as far as sending a request. The suite marked none of these four checks OWNED, so the new cause is filed here; it must be fixed before either of those issues can be re-observed.

=== Cause 1 of 1: collectionEndpoint is serialized as the production AWS hostname instead of a host:port the emulator serves ===
AWS: Smithy (com.amazonaws.opensearchserverless) documents CollectionDetail.collectionEndpoint as the "Collection-specific endpoint used to submit index, search, and data upload requests to an OpenSearch Serverless collection"; the protocol spoken there (the OpenSearch REST API) is not Smithy-modeled, so the data plane is recorded behavior only. On real AWS the published value is a host that RESOLVES to the service and SERVES that REST API, so a client can take collectionEndpoint out of BatchGetCollection/CreateCollection and immediately address it. For an emulator the derived requirement is that the value be an address the emulator itself answers on; the harness states the contract verbatim: "a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The URL template is the requirement; the collection id, the region label and the port in any one run are per-run values that fill it.
Emulator: BatchGetCollection publishes collectionDetails[0].collectionEndpoint = https://<24-char collection id>.<region>.aoss.amazonaws.com — the production AWS hostname, which the emulator neither serves nor resolves to. Any consumer that routes to it is refused at egress before a single byte is sent, with simparity.published.PublishedUrlRejected / AssertionError: "the emulator published https://<id>.<region>.aoss.amazonaws.com: its host is not one of the emulator's own hosts" (and the endpoint_route variant: "endpoint_url was given https://<id>.<region>.aoss.amazonaws.com (published by the emulator or built by the check)"). No status code, no error code and no body are produced, because no request is ever made.
Why: a client that reads collectionEndpoint and addresses it — every OpenSearch client, opensearch-py, curl, any SigV4 'aoss' signer — is pointed off the emulator entirely. Against a sandboxed/offline estate the call fails as a DNS/connect error; against a network with egress it would be aimed at real AWS. Either way the whole collection data plane (index create/head, document index/get, _search, _mapping) is unreachable, and every authorization decision that only shows up on that plane (data-access-policy permissions, network-policy SourceVPCEs, SigV4 scope) is unobservable.
Fix: the collection projection/serializer in the opensearchserverless service crate that fills CollectionDetail.collectionEndpoint (and the sibling dashboardsEndpoint), together with the emulator's host/listener registration — the published value must name a host:port the emulator actually listens on (its own host and port, or a per-resource listener), and that listener must be the one routed to the collection data plane.
Traces: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name, opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing, opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving, opensearchserverless.server-side-network-policy-enforces-vpc-only-access.

## Evidence
Expected (AWS/emulator contract): BatchGetCollection on an ACTIVE collection returns a collectionEndpoint whose host the serving endpoint answers on, so that a SigV4 'aoss'-signed request to https://<collectionEndpoint>/<index>... is delivered and served (or refused with a service status).

Actual (emulator), all four checks:
- opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name [mismatch]: `AssertionError: the emulator published https://gr01kk12rgu95uiz72j75i07.us-west-2.aoss.amazonaws.com: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:35859/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener` (raised by ctx.published_url_mismatch(endpoint, labels=("aoss",)) at check line 436).
- opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing [mismatch]: same assertion, published https://aq2qog9w8tpxtdtipbny3sam.us-west-2.aoss.amazonaws.com, emulator reachable at 127.0.0.1:46143.
- opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving [mismatch]: same assertion, published https://twy02nmi8soungjzgiv58u5w.us-west-2.aoss.amazonaws.com, emulator reachable at 127.0.0.1:35355.
- opensearchserverless.server-side-network-policy-enforces-vpc-only-access [mismatch]: `simparity.published.PublishedUrlRejected: endpoint_url was given https://otp7oi0nyxmwvflaooet21sz.us-west-2.aoss.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts ...` raised from context.py endpoint_route/_published_target.

The 24-character collection ids, the `us-west-2` label and the 127.0.0.1 ports are this run's values; the divergence is the SHAPE of the published host (a production aoss.amazonaws.com name) versus an address the emulator serves.

## Steps to Reproduce
1. opensearchserverless.create_security_policy(type='encryption', policy matching collection/<name>, aWSOwnedKey=true).
2. opensearchserverless.create_collection(name=<name>, type='SEARCH').
3. Poll opensearchserverless.batch_get_collection(ids=[<id>]) until collectionDetails[0].status == 'ACTIVE' and read collectionDetails[0].collectionEndpoint.
   -> EXPECTED: a URL whose host:port the emulator serves (e.g. http://127.0.0.1:<the emulator's port>/..., or a per-resource listener host).
   -> ACTUAL: https://<collection-id>.<region>.aoss.amazonaws.com — a production AWS host the emulator does not serve.
4. Address that URL with any client (SigV4-signed with service name 'aoss', x-amz-content-sha256 on body-bearing requests): the request is never delivered to the emulator; the harness refuses the published URL with PublishedUrlRejected, and a plain client fails to resolve/connect.
