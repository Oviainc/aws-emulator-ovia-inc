Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-keyvaluestore-put-get-describe-etag-chain
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-keyvaluestore-put-get-describe-etag-chain cloudfront-keyvaluestore-put-key-empty-value-accepted-and-counted
Issue-Shape-Id: com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore
Issue-Key: cloudfront|CreateKeyValueStore|2xx->2xx|$headers.location
Issue-Commit: 326899db14d8647fedf8ea1301a88431f0a1aba6

# CloudFront CreateKeyValueStore Location header identifies the store by Name instead of its generated Id

> _Note: this issue was identified against the code base at commit `326899db14d8647fedf8ea1301a88431f0a1aba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeyValueStore

## Description
This issue carries ONE cause.

Cause 1 of 1: the Location header CreateKeyValueStore returns points at the store by the Name the caller supplied rather than by the store's generated Id.
AWS: Smithy: CreateKeyValueStore -> POST /2020-05-31/key-value-store, http code 201, output members KeyValueStore (httpPayload), ETag (httpHeader ETag) and Location (httpHeader Location); the created KeyValueStore structure carries both a required Name and a required unique Id, and the ARN is of the form arn:aws:cloudfront::<account>:key-value-store/<Id>. Recorded behavior: the Location header's resource path ends with that generated Id — the same opaque identifier that appears in the ARN and that the cloudfront-keyvaluestore data plane is keyed by — not with the request's Name. The host/scheme part of the header is environment-specific and is not the requirement; the identifier segment is.
Emulator: the response is 201 with the expected payload, but Location's last path segment is the requested store Name (observed: .../2020-05-31/key-value-store/sim2real-probe-kvs-etagchain-d04e42ff), so the shape assertion on the header fails.
Why: a client that follows Location to address the new resource — or that parses the Id out of it instead of out of the payload/ARN — builds a URL and an identifier AWS does not recognise, and the mismatch with the Id in the returned ARN makes the two identifiers disagree inside one response.
Fix: the CreateKeyValueStore handler's Location header construction in the cloudfront service crate, which must emit the store's generated Id (the same value it writes into KeyValueStore.Id and into the ARN) rather than echoing the input Name.
Traces: cloudfront-keyvaluestore-put-get-describe-etag-chain#setup#1 (cluster also covers cloudfront-keyvaluestore-put-key-empty-value-accepted-and-counted#setup#1).

## Evidence
Step cloudfront-keyvaluestore-put-get-describe-etag-chain#setup#1, CreateKeyValueStore: status 201 as expected, single divergence $headers.location: shape_mismatch — the recorded header's shape (a key-value-store resource path terminated by the store's generated Id, cf. the recorded KvsARN 'arn:aws:cloudfront::000000000000:key-value-store/e220a839-7b1d-cdaf-6e78-9e6aa1b965f4') was not matched; actual 'https://cloudfront.amazonaws.com/2020-05-31/key-value-store/sim2real-probe-kvs-etagchain-d04e42ff', whose final segment is the request's Name. Same divergence at cloudfront-keyvaluestore-put-key-empty-value-accepted-and-counted#setup#1.

## Steps to Reproduce
1. Call cloudfront CreateKeyValueStore(Name='<some-name>', Comment='...').
2. Read the response's Location header and the KeyValueStore.Id / KeyValueStore.ARN in the payload.
3. Expect Location's final path segment to equal KeyValueStore.Id (the same id embedded in the ARN); observe it is the Name that was sent instead.
4. Replay cloudfront-keyvaluestore-put-get-describe-etag-chain#setup#1 to confirm.
