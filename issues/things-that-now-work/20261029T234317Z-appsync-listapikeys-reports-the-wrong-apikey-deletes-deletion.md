Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-bloy5xafovz2vbi6jp3neogzuj"].deletes
Issue-Commit: a1f92506050befa9acc0ff29de706b69960d0a20

# AppSync ListApiKeys reports the wrong ApiKey deletes deletion-window value

> _Note: this issue was identified against the code base at commit `a1f92506050befa9acc0ff29de706b69960d0a20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
1 cause.

Cause 1 of 1: ListApiKeys projects each da2 API key's ApiKey$deletes with a shorter deletion grace period than AWS applies, so the listed deletion time lands earlier than the real one.

AWS: ApiKey$deletes is documented as "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour." (Smithy: ApiKey.deletes, target Long; ListApiKeys http GET /v1/apis/{apiId}/apikeys, output apiKeys: ApiKeys). AWS derives deletes from the key's expires value plus the service's fixed post-expiry deletion grace period, so for a given expires the deletes value is fully determined rather than arbitrary. The recorded response for a key created in the same fixture carries a deletes value a fixed interval (a 60-day grace window) past the key's expiry; the general requirement is that ListApiKeys must report the SAME deletes the service computed for that key, i.e. exactly the value CreateApiKey/UpdateApiKey returned for it.

Emulator: returns a deletes that is a fixed, shorter offset short of AWS's value for the same key — in the replayed step, expected 1803945600 and got 1798761600 for the listed key (a ~60-day shortfall). Status is 200 on both sides; only the member value diverges. Those two integers are this run's values filling the template, not the requirement: the requirement is the derivation (expires + AWS's grace period, floored to the hour).

Why: a client that enumerates keys with ListApiKeys and schedules rotation, re-issue or cleanup off deletes computes a deletion date that does not exist on AWS. It will believe keys have already been reclaimed while they are still live on AWS (or re-issue keys early), and an inventory reconciled against AWS never converges.

Fix: the API-key handler's deletes derivation as it is projected by the ListApiKeys handler/serializer — the same expiry-plus-grace-period arithmetic shared with CreateApiKey and UpdateApiKey must be used, so Create/Update/List all agree on one value per key.

Related open issue that does NOT own this replay key: "AppSync API-key deletion window and tagging responses diverge from AWS" (20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md). It describes the same deletes arithmetic and even mentions ListApiKeys, but its recorded ListApiKeys key (Issue-Key appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-dq5e45sgwr3mamypsk3zabjbfx"].deletes) is a different generated api-key id, so this run's ListApiKeys divergence is reported as owned by nothing and is filed here. A fixer landing the shared derivation fix should re-replay this key too.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, /__admin/health 200).

appsync-api-key-lifecycle#task#4 ListApiKeys, verdict mismatch, expected status 200 / actual status 200:
  $.apiKeys[id="da2-bloy5xafovz2vbi6jp3neogzuj"].deletes: value_mismatch (expected 1803945600, got 1798761600)

Divergence key: service=appsync|ListApiKeys|status=200->200|keys=$.apiKeys[id="da2-bloy5xafovz2vbi6jp3neogzuj"].deletes; affected_count 1; owned_by empty (no open issue carries this replay key); stub_path empty.

Expected vs actual, stated generally: expected ListApiKeys to echo the service-computed deletion time for the key (expires + AWS's fixed deletion grace period, floored to the hour — the same value Create/UpdateApiKey returned for that key); actual is a value short of it by a fixed interval. The api-key id and the two epoch seconds above are this run's values, not requirements.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4 (ListApiKeys).
3. By hand: CreateGraphqlApi, then CreateApiKey with an explicit expires value, note the returned apiKey.deletes; then call ListApiKeys for the same apiId and compare the listed key's deletes against the recorded AWS value (expires plus AWS's post-expiry deletion grace window, rounded down to the hour). The emulator's value is short of it by a fixed interval.
