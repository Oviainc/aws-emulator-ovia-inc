Issue-Source: trace-sweep-divergence
Issue-Trace: braket-spending-limit-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-spending-limit-lifecycle braket-search-and-inspect-devices
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|SearchSpendingLimits|2xx->2xx|$.spendingLimits[*].createdAt
Issue-Key: braket|SearchSpendingLimits|2xx->2xx|$.spendingLimits[*].queuedSpend
Issue-Key: braket|SearchSpendingLimits|2xx->2xx|$.spendingLimits[*].spendingLimit
Issue-Key: braket|SearchSpendingLimits|2xx->2xx|$.spendingLimits[*].totalSpend
Issue-Key: braket|SearchSpendingLimits|2xx->2xx|$.spendingLimits[*].updatedAt
Issue-Key: braket|GetDevice|2xx->2xx|$.deviceCapabilities
Issue-Commit: 19755eee811c5d924ea257c82351bf2373edae9f

# Braket read paths serialize response bodies wrongly: spending-limit amounts/timestamps and the device capability document (2 causes)

> _Note: this issue was identified against the code base at commit `19755eee811c5d924ea257c82351bf2373edae9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchSpendingLimits

## Description
Two net-new braket divergences, both on 200-status read paths whose response BODY disagrees with the recorded AWS body. Neither is covered by an open issue: the existing braket issues cover CreateQuantumTask validation, GetQuantumTask/GetDevice/CreateSpendingLimit refusal WORDING and the ListTagsForResource / DeleteSpendingLimit stubs — none of them touches SearchSpendingLimits' body or GetDevice's capability document. A fixer must close BOTH causes before submitting.

Cause 1 of 2 — SearchSpendingLimits emits epoch-seconds numbers for date-time timestamps and pads USD amount strings to two decimals (fixture braket-spending-limit-lifecycle, step task#2; same fault again at task#4).
AWS: Smithy (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json) models SpendingLimitSummary$createdAt and $updatedAt as Timestamp with @timestampFormat("date-time"), so under restJson1 they are serialized as RFC3339 date-time STRINGS (e.g. "<yyyy-mm-dd>T<hh:mm:ss>.<millis>Z"), not as the protocol's default epoch-seconds number. SpendingLimitSummary$spendingLimit, $queuedSpend and $totalSpend are modeled as String (required), and the recorded response reports them as the plain USD amount with no forced cents padding — the limit amount as it was submitted, and an unspent device reported as "0". Recorded behavior only for the padding rule: the model states no pattern on these members, and the recorded SearchSpendingLimits response is the only statement of the form.
Emulator: createdAt/updatedAt come back as JSON NUMBERS of epoch seconds with fractional part (replay actual 1793681197.8448777 where the recording has an ISO-8601 date-time string), i.e. the member-level timestampFormat is dropped on the way out. The three amount members come back re-formatted to exactly two decimals — the recorded "500" is served as "500.00" and the recorded "0" (queuedSpend and totalSpend) as "0.00". The padding is state::usd (services/service-com-amazonaws-braket-braket/src/state.rs:620-624, format!("{amount:.2}")) for queuedSpend/totalSpend, and spendingLimit reaches the summary at services/service-com-amazonaws-braket-braket/src/ops/search_spending_limits.rs:306-310, which also hands createdAt/updatedAt to the serializer.
Why: a generated AWS SDK client deserializes createdAt/updatedAt per the model, as date-time; a bare number there is a deserialization failure or a silently wrong instant, so SearchSpendingLimits is unusable from a typed client even though it answers 200. A client that compares or displays the amount strings sees "500.00" where AWS says "500", so equality checks against a submitted limit, and any string-keyed reconciliation of spend against limit, break.
Fix: the SearchSpendingLimits output serializer / the generated SpendingLimitSummary output schema must honour the member's @timestampFormat("date-time") instead of falling back to restJson1 epoch-seconds; and the amount formatting in the operation's state projection (state::usd and the stored limit amount) must report the amount as AWS reports it rather than padding every value to two decimals.
Traces: braket-spending-limit-lifecycle#task#2 (representative), braket-spending-limit-lifecycle#task#4.

Cause 2 of 2 — GetDevice's deviceCapabilities document diverges from the published document (fixture braket-search-and-inspect-devices, step task#2).
AWS: Smithy models GetDevice$deviceCapabilities as JsonValue with @mediaType("application/json"), required — the device's PUBLISHED capability document, a stable catalog attribute of a device AWS publishes (devices are not created through the API). Recorded behavior only for its contents: the recording carries, for the gate-model simulator, a service section whose deviceDocumentation.imageUrl and externalDocumentationUrl are the device's real published documentation links (the guide's device-list page anchored on the device's documentation section, not on a slug of its name) and whose service.updatedAt is a fixed published instant that does not move between reads.
Emulator: the whole $.deviceCapabilities value diverges (value_mismatch on the one key, status 200 both sides). The document is synthesized per boot in state::capabilities_document (services/service-com-amazonaws-braket-braket/src/state.rs:1126-1171): deviceDocumentation.imageUrl is format!("{DEVICE_IMAGE_BASE}/{slug}.png") and externalDocumentationUrl is format!("{DOCUMENTATION_BASE}/braket-devices.html#{slug}") over url_slug(device name) (state.rs:860-862, 1387-1400), so both links are invented from the device NAME rather than being the device's published links; and service.updatedAt is as_iso8601(SystemTime::now()) captured at seeding (state.rs:1096, 1164), so the "published" document changes on every emulator restart instead of being a stable catalog value.
Why: the capability document is exactly what a Braket client parses before submitting work — it picks a device-schema model from the header and reads the service/action/paradigm properties to decide what a program may contain, and the Braket SDK surfaces deviceDocumentation to users. A document that does not match the published one sends clients to dead documentation/image URLs and makes the device's last-updated instant non-deterministic, so anything that caches or diffs the catalog sees a spurious change on every restart.
Fix: the seeded device inventory and its capability-document builder in the service state (state::capabilities_document / DEVICE_SEEDS) must publish the device's documented links and a fixed published updatedAt per seeded device instead of deriving them from the device name and the boot clock; GetDevice's handler itself just echoes the stored document.
Traces: braket-search-and-inspect-devices#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 31 steps, 18 passed, 13 mismatched, score 58.1%, 2 net-new divergence clusters (both service=braket, neither stub-blocked nor owned by an open issue).

Cause 1 — fixture braket-spending-limit-lifecycle, step task#2, operation SearchSpendingLimits, expected 200 / actual 200, verdict mismatch, cluster also covers task#4:
  $.spendingLimits[0].spendingLimit: expected '500', got '500.00'
  $.spendingLimits[0].queuedSpend:   expected '0',   got '0.00'
  $.spendingLimits[0].totalSpend:    expected '0',   got '0.00'
  $.spendingLimits[0].createdAt:     expected an ISO-8601 date-time string ('2026-09-06T03:10:22.332Z' in this run), got the number 1793681197.8448777
  $.spendingLimits[0].updatedAt:     same divergence as createdAt
(The literal instant and the literal epoch value are per-run; the requirement is the date-time STRING form the model's @timestampFormat("date-time") pins, and the unpadded amount form the recording shows.)

Cause 2 — fixture braket-search-and-inspect-devices, step task#2, operation GetDevice, expected 200 / actual 200, verdict mismatch:
  $.deviceCapabilities: value_mismatch over the whole JSON document. Recorded document carries service.deviceDocumentation.imageUrl / externalDocumentationUrl pointing at the device's published documentation assets and a fixed service.updatedAt ('2022-06-22T10:18:00' in the recording). Emulator serves imageUrl 'https://d1.awsstatic.com/braket/devices/<name-slug>.png', externalDocumentationUrl 'https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html#<name-slug>' and service.updatedAt = the emulator's seeding clock.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture braket-spending-limit-lifecycle, step task#2 (and task#4) — create a spending limit on a QPU device ARN, then POST /spending-limits (SearchSpendingLimits). Observe spendingLimits[0].createdAt/updatedAt serialized as epoch-seconds numbers instead of @timestampFormat("date-time") strings, and spendingLimit/queuedSpend/totalSpend padded to two decimals ('500.00', '0.00') instead of the unpadded amounts the recording carries ('500', '0').
4. Cause 2: fixture braket-search-and-inspect-devices, step task#2 — GET /device/{deviceArn} for the seeded gate-model simulator and compare the parsed deviceCapabilities document with the recorded one: deviceDocumentation.imageUrl / externalDocumentationUrl are synthesized from the device-name slug, and service.updatedAt is the emulator's boot instant (it changes if you restart the emulator and re-read the same device).
