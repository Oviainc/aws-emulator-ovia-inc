Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-signaling-channel-ttl-update-version-guard
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-signaling-channel-ttl-update-version-guard kinesisvideo-signaling-channel-lifecycle kinesis-video-stream-lifecycle-create-describe-delete kinesisvideo-stream-listing-and-unconfigured-reads
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|DescribeSignalingChannel|2xx->2xx|$.ChannelInfo.Version
Issue-Key: kinesisvideo|ListStreams|2xx->2xx|$.StreamInfoList[*].Version
Issue-Key: kinesisvideo|ListSignalingChannels|2xx->2xx|$.ChannelInfoList[*].Version
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# kinesisvideo Version tokens are minted from a global counter and wall clock, so Describe/List projections diverge

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSignalingChannel

## Description
This issue bundles 4 net-new root causes, all of them the SAME underlying fault surfacing on four different read paths of the kinesisvideo resource index: the `Version` token the emulator mints for a stream or a signaling channel is not the token form AWS reports, so every response that projects `Version` diverges. A fixer must close all four (i.e. make the minted token's form right AND re-replay each of the four read paths), not just the representative.

Shared root cause. AWS: Smithy `StreamInfo$Version` and `ChannelInfo$Version` both target `com.amazonaws.kinesisvideo#Version` (string, @length min 1 max 64, @pattern ^[a-zA-Z0-9]+$), documented "The version of the stream" / "The current version of the signaling channel"; it is the value a caller feeds back as `CurrentVersion` to UpdateSignalingChannel / UpdateDataRetention / DeleteStream, and the service re-mints it on every mutation of the record. Beyond the pattern and length the model says nothing about its form, so for the form this is recorded behavior only: the corpus does not assert any single run's literal token (the expectation is the member's recorded SHAPE class, reported as '<shaped-not-asserted>'), it asserts that the member carries a token of the form the service emits - an opaque per-record identifier that encodes nothing a caller can read. Emulator: `ComAmazonawsKinesisvideoKinesisvideo20170930State::mint_version` builds the token as base36(version_seq) concatenated with base36(now_nanos()) - a lowercase-only base36 string whose leading digits are a store-GLOBAL mutation counter shared by streams and channels and whose remainder is the wall-clock nanosecond count at mint time. The replay rejects that token's form on every path that projects it. Hypothesis: the recorded form is a fixed-width mixed-case alphanumeric token, which a lowercase base36 counter+clock string cannot be in; the replay's verdict is the ground truth here, the composition of the recorded token is not something the model states.
Why: the token is not opaque. Its first characters are a monotonic global counter, so a client can read the account's mutation ordering out of it and can guess the next version a record will carry; its tail is the mint-time clock, so every record touched in the same run shares a long common prefix (both tokens in one ListStreams page differ only in the leading counter digit and a few clock digits). Anything that treats `Version` as an opaque concurrency token - caching it, comparing prefixes, storing it in a fixed-width column, or asserting it is unguessable - behaves differently against the emulator than against AWS, and a test that pins the member's form fails.
Fix: the state layer's version minter (`mint_version` in the service's state module, plus the per-record `version` field it fills) - mint an opaque per-record token in AWS's form instead of encoding the global sequence and the clock. The four Describe/List projections that carry the member need no change beyond re-replaying once the minter is right.

Cause 1 of 4: DescribeSignalingChannel projects the minted token as $.ChannelInfo.Version.
AWS: DescribeSignalingChannel (POST /describeSignalingChannel, 200) returns ChannelInfo, whose Version member carries the channel's current version token in the form described above.
Emulator: status 200 is correct and every other ChannelInfo member matches, but $.ChannelInfo.Version carries the minted counter+clock token (e.g. a 13-character lowercase base36 string) and the replay reports it as a mismatch against the recorded form.
Why: this is the read a caller makes specifically to obtain `CurrentVersion` before UpdateSignalingChannel, so it is the primary way the wrong token form reaches a client.
Fix: as the shared fix above.
Traces: kinesis-video-signaling-channel-ttl-update-version-guard#task#2 (also #task#4, kinesisvideo-signaling-channel-lifecycle#task#2, #task#4).

Cause 2 of 4: ListStreams projects the minted token as $.StreamInfoList[*].Version.
AWS: ListStreams (POST /listStreams, 200) returns StreamInfoList, one StreamInfo per stream, each carrying the same Version token DescribeStream reports for that stream.
Emulator: status 200 is correct, but the Version of each listed stream is the minted counter+clock token and the replay reports $.StreamInfoList[0].Version (and, where the page holds two streams, $.StreamInfoList[1].Version) as a value mismatch against the recorded form. In the kinesis-video-stream-lifecycle-create-describe-delete#task#3 cluster the two sides print the same characters, so the divergence there is in the member's recorded form rather than in its characters - the emulator is handing back its own run-local minted token on the list path as well as the describe path.
Why: a client enumerating streams to collect their versions for a later conditional update gets tokens in a form AWS never emits, and the whole page shares a prefix.
Fix: as the shared fix above; nothing in the ListStreams projection itself.
Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#3 (also kinesisvideo-stream-listing-and-unconfigured-reads#task#4, #task#5) and kinesisvideo-stream-listing-and-unconfigured-reads#task#3 (two streams in one page: $.StreamInfoList[0].Version and $.StreamInfoList[1].Version).

Cause 3 of 4: ListSignalingChannels projects the minted token as $.ChannelInfoList[*].Version.
AWS: ListSignalingChannels (POST /listSignalingChannels, 200) returns ChannelInfoList, one ChannelInfo per channel, each carrying that channel's current version token.
Emulator: status 200 is correct, but $.ChannelInfoList[0].Version carries the minted counter+clock token and the replay reports it as a value mismatch against the recorded form (here too both sides print the same characters, i.e. the divergence is the token's form, not a per-run value).
Why: same as cause 2, on the signaling-channel surface - the channel list is where a caller collects versions before a bulk update.
Fix: as the shared fix above.
Traces: kinesisvideo-signaling-channel-lifecycle#task#9.

Cause 4 of 4: the minted token's form is identical for streams and channels because the counter is store-global.
AWS: recorded behavior only - the recordings show stream versions and channel versions as independent per-record tokens; nothing in a stream's version reveals how many channels exist, and two resources created in the same second do not share most of their token.
Emulator: one `version_seq` counter is shared by the stream index and the signaling-channel index and is simply incremented per mutation, so the leading base36 digits of a stream's Version and of a channel's Version come out of the same sequence and the trailing digits are both the mint-time clock; a multi-stream page therefore reports Versions that differ only in their first digit and their last few clock digits.
Why: Version stops being opaque at the account level - a caller can infer the total number of mutations the account has performed, and can distinguish "re-minted" from "unchanged" by arithmetic instead of by comparison, so code that would break against AWS's opaque tokens passes against the emulator.
Fix: as the shared fix above - give each record its own opaque token rather than deriving it from a shared counter and the clock.
Traces: kinesisvideo-stream-listing-and-unconfigured-reads#task#3 (the two listed streams' tokens differ only in the leading counter digit), kinesisvideo-signaling-channel-lifecycle#task#9.

Related open issues that do NOT cover this: 20261029T194209Z-kinesisvideo-stream-state-transitions-are-not-modeled-deletestream.md owns the DescribeStream clusters (stream status/retention), and 20261029T194209Z-kinesisvideo-refusal-sentences-and-signaling-channel-endpoint-projections.md owns the signaling-channel endpoint ordering and the refusal sentences; neither one owns the Version member's form on the four read paths above.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator up; 147 steps, 85 passed, 62 mismatched, score 57.8%). All four clusters below are reported with verdict=mismatch, service=kinesisvideo, empty stub_path and empty owned_by (i.e. no open issue carries their replay key).

Cause 1 - kinesis-video-signaling-channel-ttl-update-version-guard#task#2, DescribeSignalingChannel: status expected 200, got 200 (match); $.ChannelInfo.Version expected '<shaped-not-asserted>' (the recorded token form), got a 13-character lowercase base36 token (the run that produced this report emitted '1dmhp9mnf717t'; the literal is a per-run value, the FORM is the divergence). Same cluster: kinesis-video-signaling-channel-ttl-update-version-guard#task#4, kinesisvideo-signaling-channel-lifecycle#task#2, kinesisvideo-signaling-channel-lifecycle#task#4 (affected_count 4).

Cause 2 - kinesis-video-stream-lifecycle-create-describe-delete#task#3, ListStreams: status 200 matches; $.StreamInfoList[0].Version reported value_mismatch, with both sides rendering the same characters ('1dmhp9q40gpdh' in this run) - the member's recorded form is not reproduced. Same cluster: kinesisvideo-stream-listing-and-unconfigured-reads#task#4, #task#5 (affected_count 3). Second ListStreams cluster - kinesisvideo-stream-listing-and-unconfigured-reads#task#3: $.StreamInfoList[0].Version expected '<shaped-not-asserted>', got '1dmhp9uval5hq'; $.StreamInfoList[1].Version expected '<shaped-not-asserted>', got '2dmhp9uvdc67o' (affected_count 1).

Cause 3 - kinesisvideo-signaling-channel-lifecycle#task#9, ListSignalingChannels: status 200 matches; $.ChannelInfoList[0].Version reported value_mismatch, both sides rendering '2dmhp9skpo136' in this run (affected_count 1).

Cause 4 - the shape of the minted tokens across those clusters: '1dmhp9uval5hq' and '2dmhp9uvdc67o' for two streams listed in one page, '2dmhp9skpo136' for a channel, '1dmhp9mnf717t' for another channel - leading digit = the store-global mutation counter in base36, remainder = base36 of the mint-time nanosecond clock, which is why every token minted in one run shares the 'dmhp9' run-local clock prefix. Source of that composition: the service state module's `mint_version`, which returns format!("{}{}", base36(version_seq), base36(now_nanos())) after incrementing a single store-wide `version_seq`.

Not filed from this sweep, and why: the $.DataEndpoint cluster on GetDataEndpoint (kinesis-video-stream-get-data-endpoint#task#2, affected_count 8) reports only the local listener's host:port - the handler derives the endpoint from the request's own Host header, so the value is environment-dependent and not a requirement.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: CreateSignalingChannel, then POST /describeSignalingChannel for it (kinesis-video-signaling-channel-ttl-update-version-guard#task#2). Read $.ChannelInfo.Version: it is a lowercase base36 string of a global counter followed by the mint-time nanosecond clock, not a token in the form the recording carries.
3. Cause 2: CreateStream twice, then POST /listStreams (kinesisvideo-stream-listing-and-unconfigured-reads#task#3). Read $.StreamInfoList[*].Version: the two tokens differ only in their leading counter digit and a few trailing clock digits. Repeat on kinesis-video-stream-lifecycle-create-describe-delete#task#3, where the same member is reported as a value mismatch on the list path.
4. Cause 3: CreateSignalingChannel, then POST /listSignalingChannels (kinesisvideo-signaling-channel-lifecycle#task#9) and read $.ChannelInfoList[0].Version - same minted form.
5. Cause 4: in one emulator lifetime create a stream and a channel and mutate each once, then describe both: the two Version tokens come out of the same incrementing sequence (first digit 1, then 2, ...) and share the run's clock prefix, so the token is readable rather than opaque.
6. Re-replay all four of the steps named in 2-5 after changing the minter; a run that only re-checks the DescribeSignalingChannel step leaves the three list/multi-record paths unexercised.
