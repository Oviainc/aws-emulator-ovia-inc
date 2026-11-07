Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-api-key-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-api-key-lifecycle wafv2-delete-apikey-nonexistent-rejected wafv2-api-key-token-domains-set-fidelity-and-sort-order wafv2-ipset-lifecycle-with-tagging wafv2-ipset-tag-value-empty-string-preserved wafv2-webacl-tag-unicode-value-round-trip
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|CreateAPIKey|2xx->2xx|$.APIKey
Issue-Key: wafv2|ListAPIKeys|2xx->2xx|$.APIKeySummaries[*]
Issue-Key: wafv2|ListAPIKeys|2xx->2xx|$.ApplicationIntegrationURL
Issue-Key: wafv2|GetDecryptedAPIKey|2xx->2xx|$.TokenDomains[*]
Issue-Key: wafv2|GetIPSet|2xx->2xx|$.IPSet.Addresses[*]
Issue-Key: wafv2|ListTagsForResource|2xx->2xx|$.NextMarker
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# wafv2 API-key responses and list ordering/marker fidelity diverge from WAF

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAPIKey

## Description
Six successful-status (200) responses carry bodies that do not match WAF's: the API-key surface omits/mis-sets members, and list members and pagination markers are not reproduced faithfully. This is ONE issue with SIX causes; every cause below must be closed before the fix is submitted.

Cause 1 of 6: CreateAPIKey returns an APIKey whose form is not WAF's encrypted key.
AWS: Smithy: CreateAPIKey output member APIKey targets `APIKey` with @length 1..2048 @pattern \S, documented on APIKeySummary as "The generated, encrypted API key. You can copy this for use in your JavaScript CAPTCHA integration." The value is an opaque encrypted token that encodes the key's token domains, and GetDecryptedAPIKey must be able to decrypt it back to those domains. Recorded behavior only for the exact encoding; the fixture asserts the key's shape, not a literal value.
Emulator: emits a 64-character lowercase hex digest, which fails the fixture's shape assertion for an APIKey.
Why: the API key is copied verbatim into a client's JavaScript CAPTCHA integration and into GetDecryptedAPIKey; a key of the wrong form passes any length check but is not something a real integration (or a client that validates the key's encoding) will accept, so the emulator cannot be used to exercise the CAPTCHA integration path.
Fix: the API-key generation in the CreateAPIKey handler/state — emit a key in WAF's encrypted-token form rather than a bare digest.
Traces: wafv2-api-key-lifecycle#task#1 (same cluster covers wafv2-delete-apikey-nonexistent-rejected#task#4).

Cause 2 of 6: ListAPIKeys omits ApplicationIntegrationURL and reports APIKeySummary.Version 0 for a freshly created key.
AWS: Smithy: ListAPIKeys output has ApplicationIntegrationURL (OutputUrl) alongside APIKeySummaries; APIKeySummary.Version targets APIKeyVersion (@default 0, @range min 0), documented as "Internal value used by WAF to manage the key." WAF always returns the account/scope's CAPTCHA integration URL (an https URL under the captcha-sdk.awswaf.com domain, derived from the account's integration id and the region) and reports Version 1 for keys created by the current key scheme — recorded behavior for the figure 1.
Emulator: ApplicationIntegrationURL is absent (null) and Version is 0.
Why: ApplicationIntegrationURL is the one place a client learns where to load the CAPTCHA/challenge script from, so an absent URL breaks the integration flow outright; a Version of 0 makes every key look like it predates the current scheme. (The timestamp difference in the same cluster is a per-run clock value and is not part of this requirement.)
Fix: the ListAPIKeys serializer and the API-key record in state (populate the integration URL for the scope and set the key version on create).
Traces: wafv2-api-key-lifecycle#task#2.

Cause 3 of 6: ApplicationIntegrationURL is absent on ListAPIKeys for an empty/other key set too.
AWS: as Cause 2 — ApplicationIntegrationURL is returned independently of how many summaries there are; the fixture asserts its shape, not a literal URL.
Emulator: null.
Why: the integration URL is unavailable even on the plain list path a client uses to discover it before creating keys.
Fix: same component as Cause 2 (ListAPIKeys serializer); listed separately because it has its own replay key and reproduces with no keys of the recorded shape present.
Traces: wafv2-delete-apikey-nonexistent-rejected#task#1 (cluster also covers #task#3).

Cause 4 of 6: GetDecryptedAPIKey returns TokenDomains in a different order than WAF.
AWS: Smithy: GetDecryptedAPIKey output TokenDomains targets the `TokenDomains` list (member TokenDomain, @length 1..253, @pattern ^[\w\.\-/]+$) — a list, not a set, so its element order is part of the response and WAF's order is stable for a given key. Recorded behavior: the domains come back in WAF's own order, which is not the order the emulator uses.
Emulator: two elements are transposed relative to the recording, so the same key decrypts to a differently ordered list.
Why: a client that compares the decrypted domains positionally — or hashes/diffs the list to detect drift — sees a spurious change on every read, even though the key is unchanged.
Fix: the token-domain ordering in the API-key state/GetDecryptedAPIKey serializer — preserve WAF's ordering for the key rather than the emulator's insertion or hash order.
Traces: wafv2-api-key-token-domains-set-fidelity-and-sort-order#task#2.

Cause 5 of 6: GetIPSet returns IPSet.Addresses in a different order than they were supplied.
AWS: Smithy: IPSet.Addresses targets `IPAddresses`, a list of IPAddress (@length 1..50, @pattern \S). As a list its order is significant and WAF echoes the addresses as the set holds them; the recording shows a specific order after create/update.
Emulator: the first two addresses are swapped relative to the recording.
Why: clients diff GetIPSet output against their intended configuration (drift detection, idempotent apply); a reordered Addresses list reads as a change and triggers an unnecessary UpdateIPSet, and the required LockToken round-trip makes that an avoidable write.
Fix: the address ordering in the IP-set state / GetIPSet serializer.
Traces: wafv2-ipset-lifecycle-with-tagging#task#2.

Cause 6 of 6: ListTagsForResource returns a non-empty NextMarker when the whole tag list was returned.
AWS: Smithy: ListTagsForResourceResponse.NextMarker targets NextMarker (@length 1..256, @pattern \S), documented as returned only "if the number of objects that are still available for retrieval exceeds the limit" — when there is no further page WAF does not hand back a continuation value. The recording shows an empty marker on a complete single-page listing.
Emulator: NextMarker carries a value (a tag key from the page just returned), i.e. a marker is emitted even though the listing is complete.
Why: every paginating client loops: it sees a marker, re-requests with it, and either gets the same page forever or a duplicate page; SDK paginators on ListTagsForResource do not terminate.
Fix: the pagination logic in the ListTagsForResource handler — only set NextMarker when items remain beyond the returned page.
Traces: wafv2-ipset-lifecycle-with-tagging#task#5 (cluster also covers wafv2-ipset-tag-value-empty-string-preserved#task#3 and wafv2-webacl-tag-unicode-value-round-trip#task#3 and #task#5).

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). All six clusters are verdict=mismatch with expected_status 200 and actual_status 200.

1. CreateAPIKey wafv2-api-key-lifecycle#task#1 — "$.APIKey: value_mismatch (expected '<shaped-not-asserted>', got '0f25c0efea2a3d22b4fc91e61207b9d0c914f922f2254e51636e09ab304044e6')": the shape assertion for an APIKey rejects the 64-char hex digest. affected also wafv2-delete-apikey-nonexistent-rejected#task#4.
2. ListAPIKeys wafv2-api-key-lifecycle#task#2 — "$.APIKeySummaries[0]: missing (expected {... 'Version': 1}, got {... 'Version': 0})" and "$.ApplicationIntegrationURL: missing (expected 'https://<id>.us-west-2.captcha-sdk.awswaf.com/<id>/', got None)". The recorded URL's host id and the CreationTimestamp are per-run values; the requirement is that the member is present and carries the scope's integration URL, and that Version is 1.
3. ListAPIKeys wafv2-delete-apikey-nonexistent-rejected#task#1 — "$.ApplicationIntegrationURL: missing (expected '<shaped-not-asserted>', got None)"; affected also #task#3.
4. GetDecryptedAPIKey wafv2-api-key-token-domains-set-fidelity-and-sort-order#task#2 — "$.TokenDomains[2]: value_mismatch (expected 'myapp-wire.example-corp.com', got 'test-domain-wire.net'); $.TokenDomains[4]: value_mismatch (expected 'test-domain-wire.net', got 'myapp-wire.example-corp.com')" — same multiset, two positions transposed.
5. GetIPSet wafv2-ipset-lifecycle-with-tagging#task#2 — "$.IPSet.Addresses[0]: value_mismatch (expected '10.0.1.0/24', got '10.0.0.0/24'); $.IPSet.Addresses[1]: value_mismatch (expected '10.0.0.0/24', got '10.0.1.0/24')".
6. ListTagsForResource wafv2-ipset-lifecycle-with-tagging#task#5 — "$.NextMarker: value_mismatch (expected '', got 'team')", where 'team' is a tag key from the page just returned; affected_count 4 (wafv2-ipset-tag-value-empty-string-preserved#task#3, wafv2-webacl-tag-unicode-value-round-trip#task#3, #task#5).

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: CreateAPIKey with Scope=REGIONAL and one token domain -> inspect APIKey; emulator returns a 64-char hex digest instead of WAF's encrypted-token form.
3. Causes 2-3: ListAPIKeys for that scope -> ApplicationIntegrationURL is null (expected the scope's https://<id>.<region>.captcha-sdk.awswaf.com/<id>/ URL) and APIKeySummaries[].Version is 0 (expected 1). Reproduces both with keys present and on the plain list path.
4. Cause 4: CreateAPIKey with five token domains, then GetDecryptedAPIKey with that key -> TokenDomains comes back in a different order than WAF's.
5. Cause 5: CreateIPSet/UpdateIPSet with several CIDRs, then GetIPSet -> IPSet.Addresses order does not match the recorded order.
6. Cause 6: TagResource an IP set or web ACL with a few tags, then ListTagsForResource with no Limit -> NextMarker comes back non-empty on a complete page, so an SDK paginator never terminates.
