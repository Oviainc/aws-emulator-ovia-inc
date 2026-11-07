Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisvideo.list-fragments-reflects-ingestion-transition-from-empty kinesisvideo.putmedia-fragments-round-trip-through-get-media
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo.list-fragments-reflects-ingestion-transition-from-empty|_persisted_fragment_numbers#6|AssertionError|assert status == 200, ("PutMedia answered HTTP %s (x-amzn-ErrorType %r): %r"
Issue-Key: kinesisvideo.putmedia-fragments-round-trip-through-get-media|run#12|AssertionError|assert status == 200, f"PutMedia expected HTTP 200, got {status!r} (tail {tail[:200]!r})"
Issue-Commit: 7846fec8a86647e77da10e6415de80579a2805a4

# PutMedia ingest plane still unserved: POST /putMedia answers 404 (x-amzn-ErrorType UnknownService / empty body) instead of a 200 acknowledgement stream, so no media can be produced and GetMedia/ListFragments can never see a fragment

> _Note: this issue was identified against the code base at commit `7846fec8a86647e77da10e6415de80579a2805a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMedia

## Description
One root cause, grounded in the two kinesis-video behavioral checks the suite did NOT mark owned. NOTE: open issue 20261029T174024Z-kinesis-video-4-net-new-root-causes-the.md describes this same ingest gap as its "Cause 2 of 4", but its Issue-Key replay keys do not carry these two checks (their keys are `kinesis-video.list-fragments-reflects-ingestion-transition-from-empty|_persisted_fragment_numbers#2|AssertionError|assert status == 200` and `kinesis-video.putmedia-fragments-round-trip-through-get-media|run#?|AssertionError|assert status == 200, f"PutMedia expected HTTP 200, ..."`), so neither check is gated by that issue and both are filed here. These two checks reach the ingest call through a path the other 22 do not: they first exercise the read side on a created-but-EMPTY stream (that part now succeeds — GetMedia on an empty ACTIVE stream answers 200 video/webm with zero bytes, and ListFragments answers an empty success), and then fail on the first PutMedia.

=== Cause 1 of 1: there is no /putMedia route or handler on any host, so every ingest attempt is refused 404 instead of being acknowledged ===
AWS: recorded behavior only — PutMedia is in no SDK model (com.amazonaws.kinesisvideo#KinesisVideo_20170930 binds 32 operations: CreateStream, DescribeStream, GetDataEndpoint, ... UpdateStreamStorageConfiguration; none of them is PutMedia; com.amazonaws.kinesisvideomedia binds GetMedia only). The recording shows the ingest front end as: POST /putMedia to the host GetDataEndpoint(APIName=PUT_MEDIA) minted for the stream, SigV4-signed over the literal string UNSIGNED-PAYLOAD with x-amz-content-sha256 present and signed, request headers x-amzn-stream-name (or x-amzn-stream-arn), x-amzn-fragment-timecode-type (RELATIVE|ABSOLUTE) and x-amzn-producer-start-timestamp, and a Matroska body in which ONE Cluster is ONE fragment. The response is HTTP 200 whose body is a stream of JSON Acknowledgement objects; for an accepted container the EventType values observed are BUFFERING, RECEIVED and PERSISTED, with one PERSISTED acknowledgement per accepted cluster carrying the service-minted FragmentNumber and the cluster's FragmentTimecode, and no ERROR acknowledgement. Fixed text is the JSON member names and the EventType tokens; the variable parts are the stream name from the request headers, the FragmentNumber the service mints and the FragmentTimecode read from the submitted cluster. Those fragments are then readable by the archive/read plane: ListFragments on the same stream, which answered an empty Fragments list before ingestion, must afterwards report exactly the ingested fragments (one per Cluster, with the span implied by the SimpleBlock spacing), and GetMedia must return a Matroska stream carrying the exact frame bytes that were written.
Emulator: no /putMedia route exists anywhere. The ingest POST is answered HTTP 404 by the front door: on the control-plane-shaped hosts with header x-amzn-ErrorType: UnknownService and body {"__type":"UnknownService","message":"no emulated service is registered for 'kinesisvideo'"}, and on the discovered data host with the same 404 and an empty body. Replay: `AssertionError: PutMedia answered HTTP 404 (x-amzn-ErrorType 'UnknownService'): b'{"__type":"UnknownService","message":"no emulated service is registered for \'kinesisvideo\'"}'` and `AssertionError: PutMedia expected HTTP 200, got 404 (tail b'')`. Because nothing can write the fragment log, the read side is permanently empty: the empty-stream half of both checks passes and the post-ingestion half is never reached.
Why: the entire producer half of Kinesis Video Streams is unavailable. A client gets UnknownService — an error code Kinesis Video Streams never returns — rather than either the 200 acknowledgement stream or a modeled refusal; no fragment can ever exist, so ListFragments can only ever report an empty archive and GetMedia can only ever serve zero bytes, and the create -> ingest -> read round trip that producer SDKs (KVS C/Java producer, GStreamer kvssink) perform cannot complete. Every producer-side validation contract is also silently absent rather than enforced.
Fix: the ingest data plane needs an implementation — a /putMedia route bound on the PUT_MEDIA data host with a handler that parses the Matroska container cluster by cluster, appends each cluster to the stream's fragment log in the registry the archive/media read paths read from, and streams back BUFFERING/RECEIVED/PERSISTED acknowledgements with the minted FragmentNumber and the cluster FragmentTimecode; it belongs with the kinesisvideo/kinesisvideomedia state that today only the internal put_fragment helper writes.
Traces: kinesis-video.list-fragments-reflects-ingestion-transition-from-empty, kinesis-video.putmedia-fragments-round-trip-through-get-media

## Evidence
Suite: kinesis-video 0/24 passed (0%), 24 mismatched. 22 of the 24 were marked OWNED by 20261029T174024Z-kinesis-video-4-net-new-root-causes-the.md and skipped; the two below carry no owning replay key.

[mismatch] kinesis-video.list-fragments-reflects-ingestion-transition-from-empty
  check_kinesis_video_list_fragments_reflects_ingestion_transition_from_empty.py:286 -> :192 _persisted_fragment_numbers
  expected: PutMedia -> HTTP 200 with PERSISTED acknowledgements, after which the same ListFragments call that returned an empty list reports the ingested fragments.
  actual:   AssertionError: PutMedia answered HTTP 404 (x-amzn-ErrorType 'UnknownService'): b'{"__type":"UnknownService","message":"no emulated service is registered for \'kinesisvideo\'"}'

[mismatch] kinesis-video.putmedia-fragments-round-trip-through-get-media
  check_kinesis_video_putmedia_fragments_round_trip_through_get_media.py:58 (run)
  expected: three hand-built MKV Clusters -> HTTP 200, acknowledgement EventTypes {BUFFERING, RECEIVED, PERSISTED}, no ERROR ack, then GetMedia returns a Matroska stream with the exact frame bytes.
  actual:   AssertionError: PutMedia expected HTTP 200, got 404 (tail b'')
  (the empty-stream preamble of this check passed: GetMedia on the created ACTIVE stream answered 200, ContentType video/webm, zero bytes.)

Model grounding: smithy_query list_operations on services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/smithy-model/com-amazonaws-kinesisvideo-kinesisvideo-20170930.json -> 32 operations, no PutMedia; the ingest front end is recorded behavior only.
Stream names, ports and FragmentNumbers in the replay output are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from the checkout and replay the kinesis-video behavioral suite.
2. kinesisvideo.create_stream(StreamName='kv-probe', DataRetentionInHours=1); poll describe_stream until StreamStatus == ACTIVE.
3. Read the empty stream first (this currently works): kinesis-video-archived-media.list_fragments(StreamName='kv-probe', ...) returns an empty Fragments list and kinesis-video-media.get_media(StreamName='kv-probe', StartSelector={'StartSelectorType':'EARLIEST'}) returns 200, ContentType video/webm, zero bytes.
4. ep = kinesisvideo.get_data_endpoint(StreamName='kv-probe', APIName='PUT_MEDIA')['DataEndpoint'].
5. SigV4-sign (payload = the literal string UNSIGNED-PAYLOAD, with x-amz-content-sha256 present and signed) a POST /putMedia to ep with headers x-amzn-stream-name: kv-probe, x-amzn-fragment-timecode-type: RELATIVE, x-amzn-producer-start-timestamp: <now-8s>, and a Matroska body containing three Clusters (timecodes 0/1000/2000, one SimpleBlock per frame on a declared V_MPEG4/ISO/AVC track).
6. Observed: HTTP 404 — either x-amzn-ErrorType: UnknownService with body {"__type":"UnknownService","message":"no emulated service is registered for 'kinesisvideo'"}, or the same 404 with an empty body. Expected: HTTP 200 whose body is a stream of JSON Acknowledgements with EventTypes BUFFERING/RECEIVED/PERSISTED, one PERSISTED per Cluster carrying FragmentNumber and FragmentTimecode.
7. Re-read: ListFragments still reports an empty archive and GetMedia still serves zero bytes, instead of the three fragments and the exact frame bytes that were written.
