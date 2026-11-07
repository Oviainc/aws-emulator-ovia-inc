Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage.origin-endpoint-origination-deny-blocks-playback mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage.origin-endpoint-origination-deny-blocks-playback|run#16|AssertionError|assert deny_status == 403, f"expected 403 while Origination=DENY, got {deny_status}"
Issue-Key: mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip|run#14|AssertionError|assert blocked_status == 403, f"expected 403 for a caller IP outside the allowlist, got {blocked_status}"
Issue-Commit: 980a4c09fd08f28bfbc7e3936204008444675b82

# MediaPackage origin-endpoint playback Url is unrouted (404), so neither Origination=DENY nor Whitelist is ever enforced (2 causes)

> _Note: this issue was identified against the code base at commit `980a4c09fd08f28bfbc7e3936204008444675b82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginEndpoint/UpdateOriginEndpoint (origin endpoint playback Url data plane)

## Description
Two net-new root causes on the mediapackage (v1) origin-endpoint playback/egress data plane, in crate services/service-com-amazonaws-mediapackage-mediapackage. The control plane is healthy in both: CreateOriginEndpoint/UpdateOriginEndpoint/DescribeOriginEndpoint all answer 200, Origination round-trips ALLOW -> DENY -> ALLOW, Whitelist round-trips and clears, and the harness's published_url_mismatch check on the published Url is clean. What diverges is the listener the published Url points at.

Shared observation behind both causes: a GET on the generated origin-endpoint playback Url answers HTTP 404 in EVERY configuration probed -- under the default Origination=ALLOW, under Origination=DENY, for an endpoint whose Whitelist excludes the caller, and for a sibling endpoint on the same channel with no Whitelist at all. The two "positive control" arms of these checks (`assert status != 403`) therefore pass only vacuously: 404 is not 403. The access-control legs then fail because the same 404 is returned where AWS returns 403. This is a sharper diagnosis than the open issue 20261022T213438Z-mediapackage-data-plane-hls-ingest-url-is-unrouted.md records for its causes 3 and 4 (which observed only a poll that never saw 403 and was killed at the harness timeout): the Url is not served at all, so there is no egress handler in which Origination or Whitelist could be consulted. That issue does not own these two checks' replay keys, which is why they are reported again here; its causes 1-2 (the HLS INGEST Url) are separate and remain its own.

Cause 1 of 2: the playback Url ignores Origination=DENY -- it answers 404 instead of 403.
AWS: Smithy -- CreateOriginEndpoint and UpdateOriginEndpoint both carry `Origination` (enum shape Origination, jsonName "origination") on input and output, and ForbiddenException/403 is a modeled error of the service. The playback wire protocol itself is not modeled, so for the data-plane status this is recorded behavior only: with Origination=DENY, a GET against the endpoint's live playback Url is refused with a bare HTTP 403, and toggling back to ALLOW removes that refusal on the IDENTICAL Url. The 403 is an authorization refusal of an endpoint that exists; the response body/headers carry no per-run values that are part of the requirement.
Emulator: DescribeOriginEndpoint reports Origination='DENY' correctly, but a GET on the published Url returns HTTP 404 -- the same 404 it returns before the toggle and after reverting to ALLOW. The check asserts `deny_status == 403` and gets 404.
Why: a client cannot distinguish "distribution is switched off for this endpoint" (403, an authorization decision, retry/alert on policy) from "this endpoint does not exist" (404, a routing/existence signal, reconfigure or recreate). Worse, because ALLOW also answers 404, an operator who flips to DENY to stop distribution gets no observable confirmation that the control took effect, and any test of the DENY path can never go green.
Fix: the origin-endpoint playback/egress handler -- the listener that must serve the Url the CreateOriginEndpoint handler publishes must exist, and must consult the endpoint's stored Origination and answer 403 while it is DENY (and serve normally under ALLOW).
Traces: mediapackage.origin-endpoint-origination-deny-blocks-playback (AssertionError: expected 403 while Origination=DENY, got 404).

Cause 2 of 2: the playback Url ignores Whitelist (the source-IP CIDR allow list) -- it answers 404 instead of 403 for a caller outside the list.
AWS: Smithy -- CreateOriginEndpoint and UpdateOriginEndpoint both carry `Whitelist` (target __listOf__string, jsonName "whitelist") on input and output, and ForbiddenException/403 is a modeled error of the service. Data-plane status is recorded behavior only: an endpoint whose Whitelist does not contain the caller's source address refuses playback GETs with HTTP 403; a sibling endpoint on the same channel with NO Whitelist is not refused; and clearing the Whitelist (UpdateOriginEndpoint with Whitelist=[]) restores playback on the identical Url. An empty list means "no source restriction", not "deny all".
Emulator: the Whitelist field round-trips through Create/Update/Describe (DescribeOriginEndpoint reports [] after the clear), but the playback Url answers HTTP 404 for the restricted endpoint, for the unrestricted sibling, and after the list is cleared. The check asserts `blocked_status == 403` for a caller outside the allow list and gets 404.
Why: the Whitelist is the endpoint's only source-address access control. An emulator that stores it but serves no egress listener reports a secured endpoint while nothing is ever evaluated, so an overly-permissive configuration mistake (an allow list that does not actually restrict anything) cannot be caught against the emulator; and the 404 again misreports an authorization outcome as a nonexistent resource.
Fix: the same origin-endpoint playback/egress handler must evaluate the caller's source address against the endpoint's stored Whitelist CIDRs and answer 403 on no match, serving normally when the list is empty.
Traces: mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip (AssertionError: expected 403 for a caller IP outside the allowlist, got 404).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), service mediapackage: 53/58 passed, 5 failed. Three of the five are owned by open issues (the two HLS-ingest checks by 20261022T213438Z-mediapackage-data-plane-hls-ingest-url-is-unrouted.md, native-ingest-endpoint-requires-basic-auth by 20261102T091035Z-mediapackage-hls-ingest-endpoint-unauthenticated-put-returns-no.md) and are skipped. The two below are the surviving failures.

Cause 1 -- [mismatch] mediapackage.origin-endpoint-origination-deny-blocks-playback:
  File check_mediapackage_origin_endpoint_origination_deny_blocks_playback.py, line 40, in run
  assert deny_status == 403, f"expected 403 while Origination=DENY, got {deny_status}"
  AssertionError: expected 403 while Origination=DENY, got 404
  Preceding steps all succeeded: create_channel 200; create_origin_endpoint(Id=<oe>, ChannelId=<ch>, HlsPackage={}) 200 with Origination=='ALLOW'; the ALLOW positive control `assert control_status != 403` passed vacuously because the Url already answers 404; update_origin_endpoint(..., Origination="DENY") 200 and describe_origin_endpoint reported 'DENY'.
  expected: HTTP 403 on GET <endpoint Url> while Origination=DENY, non-403 after reverting to ALLOW.
  actual:   HTTP 404 on GET <endpoint Url> in all three states (ALLOW, DENY, ALLOW again).

Cause 2 -- [mismatch] mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip:
  File check_mediapackage_origin_endpoint_whitelist_blocks_non_whitelisted_ip.py, line 43, in run
  assert blocked_status == 403, f"expected 403 for a caller IP outside the allowlist, got {blocked_status}"
  AssertionError: expected 403 for a caller IP outside the allowlist, got 404
  Setup: create_channel 200; create_origin_endpoint(Id=<oe-blocked>, HlsPackage={}, Whitelist=["203.0.113.0/32"]) 200 (RFC 5737 TEST-NET-3, never a real caller address); a sibling create_origin_endpoint(Id=<oe-open>, HlsPackage={}) with no Whitelist 200. The sibling control `assert open_status != 403` passed vacuously (its Url also answers 404). Polling the restricted Url for 403 on a 90s deadline never observed anything but 404.
  expected: HTTP 403 for a caller outside the allow list, non-403 on the identical Url once Whitelist is cleared.
  actual:   HTTP 404 throughout.

Model evidence: smithy_query(query="operation", operation="CreateOriginEndpoint,UpdateOriginEndpoint", model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json") shows Origination (enum Origination, jsonName "origination") and Whitelist (__listOf__string, jsonName "whitelist") as both input and output members of each operation, and lists ForbiddenException with httpError 403 among the service's modeled errors. The playback/egress wire protocol is not modeled, hence "recorded behavior only" for the 403 status itself.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point boto3's mediapackage client at it (region us-west-2).
2. Cause 1 (Origination): ch = mp.create_channel(Id="ch1"); oe = mp.create_origin_endpoint(Id="oe1", ChannelId="ch1", HlsPackage={}) -- note oe["Origination"] == "ALLOW". GET oe["Url"] -> 404 (AWS: a non-403 served/empty-manifest response). Then mp.update_origin_endpoint(Id="oe1", HlsPackage={}, Origination="DENY"); mp.describe_origin_endpoint(Id="oe1")["Origination"] == "DENY"; GET oe["Url"] repeatedly -> still 404, never 403. AWS answers 403 while DENY and stops answering 403 once reverted to ALLOW.
3. Cause 2 (Whitelist): blocked = mp.create_origin_endpoint(Id="oe-blocked", ChannelId="ch1", HlsPackage={}, Whitelist=["203.0.113.0/32"]); opened = mp.create_origin_endpoint(Id="oe-open", ChannelId="ch1", HlsPackage={}). GET blocked["Url"] -> 404, never 403 (AWS: 403, the caller's address is outside the list). GET opened["Url"] -> also 404 (AWS: non-403). Then mp.update_origin_endpoint(Id="oe-blocked", HlsPackage={}, Whitelist=[]) and GET blocked["Url"] again -> AWS serves it (non-403); the emulator is 404 in every one of these states.
4. Or replay directly: run_behavioral_suite and re-run the two check ids in trace_ids.
