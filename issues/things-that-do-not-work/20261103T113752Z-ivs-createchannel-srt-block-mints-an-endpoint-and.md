Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle ivs-viewer-session-revocation
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.srt.endpoint
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.srt.passphrase
Issue-Commit: a103a48da053e546868794c5f131713ed52edfb1

# IVS CreateChannel srt block mints an endpoint and passphrase outside AWS's recorded shape

> _Note: this issue was identified against the code base at commit `a103a48da053e546868794c5f131713ed52edfb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Replaying the wire corpus against the IVS emulator (service token `ivs`, crate services/service-com-amazonaws-ivs-amazoninteractivevideoservice) shows that the `Srt` block CreateChannel returns carries two generated values whose SHAPE does not match the shape the recording holds for them. The corpus does not assert these two values literally (they are per-run secrets/hosts, reported as `<shaped-not-asserted>`), it asserts their shape — and both fail that shape check. One cause.

Cause 1 of 1: CreateChannel's response member `$.channel.srt.endpoint` and `$.channel.srt.passphrase`. Fixture ivs-channel-lifecycle, step task#1; the same cluster also covers ivs-viewer-session-revocation#setup#1, so both are closed by one fix.

AWS: recorded behavior only. Smithy (com.amazonaws.ivs, restJson1): `Srt.endpoint` targets `SrtEndpoint` (plain string, no @length and no @pattern) and `Srt.passphrase` targets `SrtPassphrase` (plain string, @sensitive, no @length/@pattern), documented as the "Auto-generated passphrase to enable encryption" that applies only when `insecureIngest` is not enabled — so the model states no width, alphabet or host form and the RECORDING is the only authority for their shape. What the recording establishes about the SRT endpoint elsewhere in the same corpus (fixture ivs-batch-get-channel-and-stream-key, step task#3) is that AWS's `srt.endpoint` is a host of the form `<lowercase-hex-id>.global-contribute.live-video.net` that is the SAME host as the channel's `ingestEndpoint`; the hex id itself is per-run and not a requirement. For the passphrase the recording carries an auto-generated mixed-case alphanumeric secret whose width is the part the shape assertion pins, and the emulator's value falls outside it. The requirement is the shape class (host form for the endpoint, alphabet and width for the passphrase), never one run's literal value.

Emulator: the replay reports both keys as value mismatches against the recorded shape — `$.channel.srt.endpoint` got `<12-lowercase-hex>.global-contribute.live-video.net` and `$.channel.srt.passphrase` got a 32-character mixed-case alphanumeric secret, where the recording's shape for these two members is a different one. The code is explicit about where each comes from: the handler formats ONE `ingest_endpoint` from a 6-byte (12 hex char) minted id and re-uses that same string for `Srt.endpoint`, and the passphrase comes from `crate::state::mint_srt_passphrase`, which cuts `SRT_PASSPHRASE_LEN = 32` characters over `char::is_ascii_alphanumeric` — a width the comment itself describes as chosen because "the model states no width for SrtPassphrase", i.e. chosen rather than taken from AWS.

Why: a client that reads the `Srt` block to configure an SRT encoder, or that validates/parses the passphrase and endpoint it is handed (length-checking the passphrase, splitting the host, storing it in a typed field), sees values of a different shape than AWS issues, so code that works against AWS can reject or mis-handle what this emulator serves. It is also the only place in the channel response where a generated secret's shape is observable, so a test that pins the shape AWS issues fails here while the status and the rest of the channel projection are correct.

Fix: the CreateChannel handler's srt minting and the state helper that generates the secret — services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_channel.rs (the `ingest_endpoint`/`Srt { endpoint, passphrase }` construction) and crate::state's `mint_srt_passphrase` / `SRT_PASSPHRASE_ALPHABET` / `SRT_PASSPHRASE_LEN`; nothing in the serializer or the store is involved, and no other operation mints an `Srt` block.

Related open issues, none of which covers this step: 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md (the same channel projection, but the missing `failoverSlateConfigurationArn` member and the literal ingest-host assertions on BatchGetChannel/ListChannels) and 20261030T181150Z-ivschat-createchattoken-mints-a-counter-derived-48-char.md (the same CLASS of defect — a generated secret whose shape is not AWS's — in a different service). The replay reports no owner for this cluster.

## Evidence
Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, 84 steps, 55 passed, 24 mismatched, 5 blocked, score 65.5%, 16 divergence clusters.

Cluster service=ivs|CreateChannel|status=200->200|keys=$.channel.srt.endpoint,$.channel.srt.passphrase, verdict mismatch, owned_by [] (net-new), stub_path empty. Status is correct (expected 200, got 200); the two diverging keys are:
- $.channel.srt.endpoint: value_mismatch (expected '<shaped-not-asserted>', got 'e495b09fd13c.global-contribute.live-video.net')
- $.channel.srt.passphrase: value_mismatch (expected '<shaped-not-asserted>', got 'VRUJQGDY3bwnPuQw7vqp9aI0ngXuzWNm')
Representative step: ivs-channel-lifecycle#task#1; affected_count 2, the other member being ivs-viewer-session-revocation#setup#1.

'e495b09fd13c' and 'VRUJQGDY3bwnPuQw7vqp9aI0ngXuzWNm' are ONE run's generated values and are not themselves requirements — the corpus marks both members shaped-not-asserted; what diverges is the shape class each value belongs to (host form; passphrase alphabet and width).

Corroborating recorded shape for the endpoint, from the same corpus: ivs-batch-get-channel-and-stream-key#task#3 records $.channels[0].ingestEndpoint and $.channels[0].srt.endpoint as the SAME host '37cd928af5e8.global-contribute.live-video.net'.

Emulator source backing the actual values: src/ops/create_channel.rs lines 231-261 (one 6-byte minted `ingest_id`, `format!("{ingest_id}.global-contribute.live-video.net")`, re-used verbatim as `Srt.endpoint`) and src/state.rs lines 462-484 (`SRT_PASSPHRASE_ALPHABET = char::is_ascii_alphanumeric`, `SRT_PASSPHRASE_LEN = 32`, `mint_srt_passphrase`). Smithy: Srt.endpoint -> SrtEndpoint (string, no constraints), Srt.passphrase -> SrtPassphrase (string, @sensitive, no constraints).

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or by hand against the data listener in region us-west-2:
1. POST /CreateChannel with a body naming only `name` (fixture ivs-channel-lifecycle#task#1; ivs-viewer-session-revocation#setup#1 does the same in its setup phase).
2. Read `$.channel.srt.endpoint` and `$.channel.srt.passphrase` from the 200 response.
3. Compare both against the corpus's recorded shape for those two members: the endpoint's host form and the passphrase's alphabet and width. The emulator answers a 12-lowercase-hex host under .global-contribute.live-video.net (the same string it put in `ingestEndpoint`) and a 32-character mixed-case alphanumeric passphrase; the replay reports both as shape mismatches.
4. Note that the per-run hex id and the per-run secret text are NOT the divergence — re-running changes both and the mismatch persists.
