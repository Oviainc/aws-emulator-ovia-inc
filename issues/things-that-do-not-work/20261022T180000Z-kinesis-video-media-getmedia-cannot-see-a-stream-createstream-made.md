Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-media.get-media-after-fragment-number-non-numeric-rejected kinesis-video-media.get-media-by-stream-arn-succeeds kinesis-video-media.get-media-concurrent-connection-limit-enforced kinesis-video-media.get-media-continuation-token-invalid-rejected kinesis-video-media.get-media-continuation-token-selector-missing-token-rejected kinesis-video-media.get-media-deleted-stream-rejected kinesis-video-media.get-media-earliest-selector-returns-stream kinesis-video-media.get-media-fragment-number-missing-after-rejected kinesis-video-media.get-media-nonexistent-stream-rejected kinesis-video-media.get-media-now-selector-succeeds kinesis-video-media.get-media-producer-timestamp-missing-start-rejected kinesis-video-media.get-media-server-timestamp-missing-start-rejected
Issue-Shape-Id: com.amazonaws.kinesisvideomedia#AWSAcuityInletService
Issue-Key: kinesis-video-media|GetMedia|ResourceNotFoundException
Issue-Commit: db7c2bf3

# GetMedia does not see a stream CreateStream made: the Kinesis Video family keeps two separate stream registries

> _Note: this issue was filed by the fixer of
> `20261022T145739Z-kinesis-video-media-router-rejects-every-kinesisvideo-prefix-request-with-ambiguousservice.md`,
> which made the whole `kinesisvideo` prefix reachable again. With routing fixed this is the
> next thing a GetMedia caller hits. The code base you are working in can have drifted, so
> trace the claim below back to it._

## Operation
GetMedia

## Description
One Kinesis video stream is ONE resource in AWS: `kinesisvideo:CreateStream` creates it,
`kinesisvideo:DescribeStream` reports it, `kinesisvideo:DeleteStream` removes it, and the
data planes reached through `GetDataEndpoint` — `kinesis-video-media:GetMedia`,
`kinesis-video-archived-media:ListFragments` and the rest — read THAT stream, identified
by the same `StreamName` or the same `StreamARN`. A stream a client just created is
therefore readable on the data plane (an empty one answers no media, not a 404), a stream
it deleted is `ResourceNotFoundException` there, and a `StreamARN` minted by `CreateStream`
resolves on both planes.

In the emulator each crate keeps its OWN stream registry:

* `services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/state.rs` holds
  `streams` / `stream_arn_index`, written by `CreateStream` and `DeleteStream`.
* `services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/src/state.rs`
  holds its own `streams` / `arn_index`, created only by `ensure_stream` / `put_fragment` —
  its producer entry point. Nothing on the control plane writes it.
* `services/service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader/src/state.rs`
  is a third such registry.

So `CreateStream` followed by `GetMedia` on the same name answers HTTP 404
`ResourceNotFoundException` (the media store has no such stream), the two planes mint two
DIFFERENT `StreamARN`s for one name because each builds the ARN from its own creation
instant, and `DeleteStream` cannot make the data plane forget a stream it never knew. The
AWS answers differ: a 200 with the stream's media (empty when nothing was produced), one
ARN, and a 404 only after the stream is really gone.

## Evidence
`GetMedia` resolves a request through `store.stream_by_name(..)` / `store.stream_by_arn(..)`
on `ComAmazonawsKinesisvideomediaAwsacuityinletserviceState`
(`services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/src/ops/get_media.rs`),
a map whose only writers are that same crate's `ensure_stream` and `put_fragment`. The
control plane's `CreateStream`
(`services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/create_stream.rs`)
writes `ComAmazonawsKinesisvideoKinesisvideo20170930State::streams`. Both crates build a
stream ARN of the modelled form `arn:aws:kinesisvideo:<region>:<account>:stream/<name>/<ms>`
from their own clock reading, so the two values disagree in their trailing segment.

`core/router/tests/all/kinesis_video_signing_name_routing.rs` shows the split from the
front door: with all five services mounted on one engine, `/createStream` answers 200 with
a `StreamARN` while `/getMedia` for a name nothing produced to answers 404
`ResourceNotFoundException`.

## Steps to Reproduce
1. Boot the emulator from the checkout.
2. `kinesisvideo.create_stream(StreamName='kv-probe-1', DataRetentionInHours=1)` -> 200 with a `StreamARN`.
3. `kinesisvideo.get_data_endpoint(APIName='GET_MEDIA', StreamName='kv-probe-1')` -> 200 with a `DataEndpoint`.
4. `kinesis-video-media.get_media(StreamName='kv-probe-1', StartSelector={'StartSelectorType': 'EARLIEST'})`
   against that endpoint -> HTTP 404 `ResourceNotFoundException` instead of a 200 whose
   `Payload` carries the stream's (currently empty) media.
5. Equivalently with `StreamARN` from step 2 instead of the name, and the same for
   `kinesis-video-archived-media.list_fragments`.

## Fix
Give the Kinesis Video family ONE stream registry, so a stream exists once for all five
crates: the control plane's record is the resource, and each data plane holds only the
per-stream media it owns (the fragment log, the sessions) keyed by that one stream
identity, rather than a second copy of the stream itself. Shared state belongs in a crate
under `services/` both sides depend on (the `services/common` pattern), and the stream ARN
must be minted in exactly one place so both planes report the same one. `GetMedia` then
answers a created-but-empty stream with an empty payload, and
`ResourceNotFoundException` only for a stream the registry does not hold.
