Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.srt.endpoint
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.srt.passphrase
Issue-Commit: 4e4f234132863b23c523cf19a4d45a030ab8d36e

# CreateChannel srt block diverges from the recorded form: passphrase is a 48-char lowercase-hex digest

> _Note: this issue was identified against the code base at commit `4e4f234132863b23c523cf19a4d45a030ab8d36e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
The `srt` block CreateChannel returns does not have the form the recorded AWS response has: the auto-generated `passphrase` the emulator mints fails the fixture's shape assertion on $.channel.srt.passphrase (the recorded value is not asserted literally, only its shape, and the emulator's value does not satisfy that shape). ONE cause, one failing fixture step, which must be closed before submitting.

Cause 1 of 1: CreateChannel mints $.channel.srt.passphrase as a 48-character lowercase-hexadecimal digest. Fixture ivs-channel-lifecycle, step task#1 (status 200 -> 200; the step diverges only inside the srt block).

AWS: recorded behavior only. Smithy (com.amazonaws.ivs, restJson1): `Srt` has members `endpoint` (SrtEndpoint) and `passphrase` (SrtPassphrase, @sensitive), and NEITHER string shape carries a @length or @pattern constraint, so the model states no form; the doc for `passphrase` says only "Auto-generated passphrase to enable encryption. This field is applicable only if the end user has not enabled the insecureIngest option for the channel." What the recording constrains is therefore the VALUE CLASS the fixture captured as a shape assertion for this member: AWS returns an opaque auto-generated secret for a channel created with insecureIngest=false, and its character class/length is what the step asserts. Hypothesis: AWS's SRT passphrase is drawn from a mixed-case alphanumeric alphabet (as in the service's documented example responses) rather than lowercase hex, so a lowercase-hex-only string of a different length cannot satisfy the recorded class. The requirement is the class/derivation of the secret, never one run's literal passphrase.

Emulator: the replay reports `$.channel.srt.passphrase: value_mismatch (expected '<shaped-not-asserted>', got '505c1751faa9d7a69bc53da33d09fad9c9e1cea8bf4db78e')` — i.e. the key is present and the asserted shape is not met. The handler builds it with `store.mint_hex("srt-passphrase", 24)`, and mint_hex hex-encodes the first 24 bytes of SHA-256 over `"<label>|<mint counter>|<pid>|<nanos>"`, so every passphrase this emulator serves is exactly 48 characters drawn from [0-9a-f] and is derived from the store's monotonic mint counter plus the clock and process id. The same cluster also reports `$.channel.srt.endpoint` under the same shape assertion; the endpoint's host label is minted per environment, and the recorded responses elsewhere in this corpus show the endpoint has the form `<host-label>.global-contribute.live-video.net` and equals the channel's own `ingestEndpoint` — which the emulator already satisfies — so the host label itself is NOT part of this issue's requirement and must not be "fixed" to a recorded literal.

Why: a client or test that treats the SRT passphrase as an opaque credential and validates it against the shape AWS actually returns (alphabet and length) rejects or mis-handles the emulator's value, so SRT ingest setup code that passes against AWS fails here. The derivation is also the service's one ingest secret: deriving it from a per-store counter makes it reproducible across runs of the same process sequence, which a secret AWS generates is not.

Fix: the CreateChannel handler's srt minting (services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_channel.rs, where `srt_passphrase` is produced) together with the id/secret minting helper it calls in crate::state (`mint_hex`) — the passphrase needs its own generator producing the recorded opaque class and length rather than reusing the lowercase-hex id minter used for ARNs and host labels. No other handler projects a newly minted srt block.

Traces: ivs-channel-lifecycle#task#1 (CreateChannel, 200 -> 200, $.channel.srt.passphrase).

Related open issues, neither of which carries this key: 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md (the read-path channel projection; it explicitly excludes the srt/ingest HOST values as per-run and says nothing about the passphrase) and, in a different service, 20261030T181150Z-ivschat-createchattoken-mints-a-counter-derived-48-char.md (the same counter-derived-48-char-secret mechanism in ivschat). The replay reports NO owner for this cluster.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, 84 steps, 55 passed, 24 mismatched, score 65.5%, 17 divergence clusters.

Cluster service=ivs|CreateChannel|status=200->200|keys=$.channel.srt.endpoint,$.channel.srt.passphrase, fixture ivs-channel-lifecycle, step task#1, verdict mismatch, expected_status 200, actual_status 200:
- $.channel.srt.passphrase: value_mismatch (expected '<shaped-not-asserted>', got '505c1751faa9d7a69bc53da33d09fad9c9e1cea8bf4db78e') — 48 chars, all [0-9a-f]. The literal shown is one run's value; the requirement is the class.
- $.channel.srt.endpoint: value_mismatch (expected '<shaped-not-asserted>', got '79dd792d3c2f.global-contribute.live-video.net') — reported in the same cluster; the host label is generated per environment and is NOT claimed as a requirement here (the recorded BatchGetChannel responses in ivs-batch-get-channel-and-stream-key show ingestEndpoint == srt.endpoint with a `<12-hex>.global-contribute.live-video.net` host, a relation the emulator already keeps).

Code read: services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_channel.rs:231-254 (channel_id / ingest_id / srt_passphrase minted with mint_hex, srt.endpoint set to the ingest endpoint) and src/state.rs:92-107 (mint_hex = hex of SHA-256 over label|minted-counter|pid|nanos, documented as "a fresh lowercase-hex id of bytes bytes").

Smithy consulted: com.amazonaws.ivs#Srt, com.amazonaws.ivs#SrtPassphrase (string, @sensitive, no length/pattern), com.amazonaws.ivs#SrtEndpoint, com.amazonaws.ivs#Channel -> no modeled form, hence "recorded behavior only".

## Steps to Reproduce
Boot the emulator from this checkout and talk to the data listener in region us-west-2 (or just run replay_wire_corpus and look at the ivs-channel-lifecycle#task#1 cluster):
1. POST /CreateChannel {"name":"probe","insecureIngest":false} -> 200.
2. Inspect channel.srt in the response: passphrase is a 48-character string containing only [0-9a-f] (e.g. '505c1751faa9d7a69bc53da33d09fad9c9e1cea8bf4db78e'), which does not satisfy the shape the recorded AWS response's auto-generated passphrase has.
3. Repeat the call a few times in the same process: each passphrase is the hex SHA-256 of the store's mint counter, label, pid and clock, i.e. derived from the store's counter rather than an opaque generated secret.
4. Compare with the fixture's assertion on $.channel.srt.passphrase (shape-asserted, not literal) — the replay reports value_mismatch for that key while the step's status and every other member match.
