Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.signal-map-invalid-entry-point-rejected medialive.describe-channel-not-found
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.signal-map-invalid-entry-point-rejected|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException for a bogus entry point, got {code!r}"
Issue-Key: medialive.describe-channel-not-found|run#6|AssertionError|assert message == f"Channel {bogus_id} not found", message
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# MediaLive signal-map and channel-read defects: CreateSignalMap answers 501 NotImplemented instead of a modeled 404, and DescribeChannel's NotFoundException message is built in the wrong order

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSignalMap

## Description
Two net-new root causes in the MediaLive emulator, both reached without any cross-service setup.

Cause 1 of 2: CreateSignalMap is an unimplemented scaffold, so it cannot classify a bad discovery entry point.
AWS: Smithy (com.amazonaws.medialive#CreateSignalMap) binds POST /prod/signal-maps with success status 201 and models the errors BadRequestException (400), ConflictException (409), ForbiddenException (403), InternalServerErrorException (500), NotFoundException (404) and TooManyRequestsException (429). DiscoveryEntryPointArn is a required member (__stringMin1Max2048, 'A top-level supported AWS resource ARN to discovery a signal map from'). Recorded behavior: an ARN naming a MediaLive resource that does not exist is refused with HTTP 404 and error code NotFoundException (the recorded message embeds the request id of an internal downstream call, so it is an example only, not a requirement), and no signal map is created. 'NotImplemented' is not in this operation's modeled error set, nor in any AWS model.
Emulator: the handler returns the scaffold refusal — HTTP 501 with error code 'NotImplemented' and the message 'operation CreateSignalMap is scaffolded but not implemented in this emulator' — for every CreateSignalMap request, valid or not.
Why: a client cannot create a signal map at all, and a client that branches on NotFoundException for an unresolvable discovery entry point instead sees an unretryable server-side 501 with a code no SDK models; error handling, waiters on SignalMapStatus and any monitor-deployment flow built on signal maps are all broken.
Fix: the CreateSignalMap handler and its state (create the signal map, run discovery against the referenced resource, and return NotFoundException when the entry-point ARN names no existing resource). Note: this stub is NOT covered by the auto-filed stub issues in this run — the suite marked the CreateCloudWatchAlarmTemplateGroup, CreateEventBridgeRuleTemplateGroup, CreateInput, CreateInputSecurityGroup and CreateSdiSource stubs as ALREADY FILED, and CreateSignalMap is not among them because the check captured the 501 as a client error rather than crashing on it.
Traces: medialive.signal-map-invalid-entry-point-rejected; handler at services/service-com-amazonaws-medialive-medialive/src/ops/create_signal_map.rs:204 (and the SCAFFOLDED marker at line 4).

Cause 2 of 2: DescribeChannel's NotFoundException message is assembled with the channel id in the wrong position.
AWS: DescribeChannel (GET /prod/channels/{ChannelId}) models NotFoundException at HTTP 404. Recorded behavior: for a channel id that was never created the message is built from the request and reads 'Channel <ChannelId> not found' — the id follows the word 'Channel' and 'not found' trails the id. The rule producing the text is therefore known (it is composed from the requested ChannelId), which is why it is a requirement rather than an incidental string; the status (404) and code (NotFoundException) already match.
Emulator: the message is formatted as 'Channel not found: <ChannelId>' — same id, inverted template.
Why: clients and operator tooling that parse MediaLive's not-found text to extract the offending channel id (or match the documented phrasing) mis-parse it; the status and code are correct, so this is a representation divergence only.
Fix: the DescribeChannel handler's NotFoundException construction.
Traces: medialive.describe-channel-not-found; handler at services/service-com-amazonaws-medialive-medialive/src/ops/describe_channel.rs:189-193, where the message is formatted as format!("Channel not found: {}", input.channel_id).

## Evidence
Cause 1 — medialive.signal-map-invalid-entry-point-rejected [mismatch]:
File "check_medialive_signal_map_invalid_entry_point_rejected.py", line 28, in run
assert code == "NotFoundException", f"expected NotFoundException for a bogus entry point, got {code!r}"
AssertionError: expected NotFoundException for a bogus entry point, got 'NotImplemented'
Expected: HTTP 404, error code NotFoundException (modeled for CreateSignalMap), and no signal map left behind.
Actual: HTTP 501, error code NotImplemented, message 'operation CreateSignalMap is scaffolded but not implemented in this emulator'.
Smithy: CreateSignalMap errors = BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500), NotFoundException(404), TooManyRequestsException(429); http POST /prod/signal-maps, code 201.

Cause 2 — medialive.describe-channel-not-found [mismatch]:
File "check_medialive_describe_channel_not_found.py", line 25, in run
assert message == f"Channel {bogus_id} not found", message
AssertionError: Channel not found: 999999999
Expected: NotFoundException message 'Channel 999999999 not found' (id composed from the request, after 'Channel').
Actual: 'Channel not found: 999999999'. The preceding assertion on code == 'NotFoundException' passed, so only the message template diverges.

## Steps to Reproduce
Cause 1:
1. Boot the emulator.
2. Call medialive.CreateSignalMap(Name='sim2real-medialive-signalmap-<rand>', DiscoveryEntryPointArn='arn:aws:medialive:<region>:<account>:channel:9999999') with an entry-point ARN naming no existing resource.
3. Observe HTTP 501 with error code 'NotImplemented' instead of HTTP 404 NotFoundException.
4. Replay medialive.signal-map-invalid-entry-point-rejected.

Cause 2:
1. Boot the emulator.
2. Call medialive.DescribeChannel(ChannelId='999999999') for an id that was never created.
3. Observe NotFoundException with message 'Channel not found: 999999999' instead of 'Channel 999999999 not found'.
4. Replay medialive.describe-channel-not-found.
