Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-d7mhzpv4vvmggnryoxrkv2xr5u"].deletes
Issue-Commit: cbb2413755bc084fc374b465f31a36da1f726078

# AppSync ListApiKeys reports the wrong ApiKey deletes value (deletion grace period offset)

> _Note: this issue was identified against the code base at commit `cbb2413755bc084fc374b465f31a36da1f726078`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
ListApiKeys projects each da2 API key's `deletes` field from the same deletion-grace-period arithmetic the create/update paths use, and that arithmetic is short by a fixed interval, so the listing reports a deletion time that does not exist on AWS. 1 net-new cause.

Cause 1 of 1 - ListApiKeys returns an ApiKey$deletes value computed with the wrong deletion grace period.
AWS: Smithy: ApiKey.deletes, target Long, documented as "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour"; the ApiKey doc further states that for da2 keys "ListApiKeys returns the expiration time and deletion time in seconds". AWS derives deletes deterministically from the key's `expires` value plus the service's fixed deletion grace period, so for a given expires the deletes value is determined, not arbitrary. In the recording AWS's deletes sits exactly 5,184,000 seconds (60 days) beyond the value the emulator computes for the same key - i.e. the emulator applies a shorter grace period than AWS does. The listed value must equal whatever CreateApiKey/UpdateApiKey returned for that same key.
Emulator: ListApiKeys 200 returns the stored/derived deletes with the shorter offset, so the listed deletion time is earlier than AWS's by that fixed interval (recorded: expected 1803945600, got 1798761600 for the listing's single da2 key - those two numbers are one run's values filling the template, the requirement is the derivation, not the literals).
Why: a client that enumerates keys and schedules rotation, alarms or cleanup off the listed deletes acts on a date AWS never produces - it believes keys are reclaimed while they are still live (or expires them early), and a client that cross-checks CreateApiKey's deletes against the listing sees a consistent but globally wrong window.
Fix: the API-key handler's expiry/deletion arithmetic - the shared `deletes` derivation that CreateApiKey, UpdateApiKey and ListApiKeys all project; ListApiKeys' projection must reuse it rather than diverge from it.
Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys, 200).

Open issue that does NOT cover this step: "AppSync API-key deletion window and tagging responses diverge from AWS (10 causes)" (20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md) describes the same underlying grace-period arithmetic and even lists a ListApiKeys replay key, but that key is pinned to a different, per-run generated api-key id (`$.apiKeys[id="da2-dq5e45sgwr3mamypsk3zabjbfx"].deletes`), so this replay's ListApiKeys cluster (`$.apiKeys[id="da2-d7mhzpv4vvmggnryoxrkv2xr5u"].deletes`) is not matched or owned by it and is reported unowned. Fixing the shared derivation should close both; this issue carries the replay key that actually gates this step.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false).

appsync-api-key-lifecycle#task#4 ListApiKeys, status 200 expected and 200 received, body mismatch:
  $.apiKeys[id="da2-d7mhzpv4vvmggnryoxrkv2xr5u"].deletes: value_mismatch (expected 1803945600, got 1798761600)

Delta = 5,184,000 s = 60 days. Status, error code and headers agree; only the derived deletion timestamp diverges. The api-key id and the two epoch seconds above are per-run values from this recording - the invariant is deletes = expires + AWS's fixed deletion grace period, rounded down to the hour, and identical to the value Create/UpdateApiKey returned for the same key.

## Steps to Reproduce
1. git clone the repo, boot the emulator built from that checkout, replay the simparity-wire-aws-v1 wire corpus.
2. Fixture appsync-api-key-lifecycle mismatches at step task#4 (ListApiKeys) on $.apiKeys[...].deletes.
3. By hand: CreateGraphqlApi with API_KEY auth, CreateApiKey with an explicit `expires` value, then ListApiKeys on that api. Compare the listed `deletes` with expires plus AWS's documented deletion grace period (and with the value CreateApiKey returned); the emulator's value is short by a fixed 60-day interval.
