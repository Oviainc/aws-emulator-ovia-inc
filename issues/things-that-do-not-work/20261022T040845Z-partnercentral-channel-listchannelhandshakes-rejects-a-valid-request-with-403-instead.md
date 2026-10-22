Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit
Issue-Shape-Id: com.amazonaws.partnercentralchannel#PartnerCentralChannel
Issue-Key: partnercentral-channel|ListChannelHandshakes|2xx->4xx|$.items
Issue-Key: partnercentral-channel|ListChannelHandshakes|2xx->4xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# ListChannelHandshakes rejects a valid request with 403 instead of returning an empty page

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannelHandshakes

## Description
Cause 1 of 1: ListChannelHandshakes denies a well-formed, authorized list request instead of serving it.

AWS: ListChannelHandshakes is a read-only, paginated list operation (Smithy: ListChannelHandshakes, http POST /ListChannelHandshakes, code 200, @readonly, @paginated{inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: items}). Its input requires handshakeType, catalog (@length 1..64, @pattern ^[a-zA-Z]*$) and participantType, with optional maxResults (@range 1..100, default 20), statuses, associatedResourceIdentifiers, handshakeTypeFilters, handshakeTypeSort and nextToken. The output shape ListChannelHandshakesResponse carries items (ChannelHandshakeSummaries) and an optional nextToken. 403 is modeled only for AccessDeniedException; a request that satisfies the input constraints and is authorized is answered 200 with an items list, which is empty — not absent — when no handshake matches the filters. The recorded response for this step is HTTP 200 with $.items = [] (recorded behavior for the empty-result shape; the model does not mark items @required, but the recording shows it present and empty).

Emulator: the same request is answered HTTP 403 and the body has no items member at all ($.items: expected [], got None). Replay reports status: expected 200, got 403 for both steps of the fixture, so the operation is unusable rather than merely returning a different page.

Why: a client listing channel handshakes cannot distinguish "you may not do this" from "there is nothing to list". An AWS SDK maps the 403 to AccessDeniedException, which is not retryable and not a pagination terminator, so callers abort their enumeration (or escalate a permissions failure) where AWS would hand them an empty page and no nextToken. Code that iterates the paginator never completes a single page, and any flow that depends on an empty-list answer meaning "no handshakes exist" breaks outright.

Fix: the ListChannelHandshakes handler and its authorization/precondition gate in the partnercentral-channel service — the gate must not reject a request that meets the modeled input constraints, and the success path must serialize ListChannelHandshakesResponse with items always present (an empty list when nothing matches) and nextToken omitted on the last page.

Traces: fixture partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit, steps task#1 and task#2 (both diverged identically: expected 200, got 403).

## Evidence
Divergence key service=partnercentral-channel|ListChannelHandshakes|status=200->403|keys=$.items,status, verdict=mismatch.
Expected (recorded AWS response): HTTP 200, body with $.items = [].
Actual (emulator): HTTP 403, $.items missing (got None).
Affected steps (2): partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit#task#1, partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit#task#2.
Model support: Smithy ListChannelHandshakes -> output ListChannelHandshakesResponse{items: ChannelHandshakeSummaries, nextToken}, http code 200, @readonly, @paginated(items=items); 403 is modeled only for AccessDeniedException.
No open issue covers this operation (open partnercentral issues are a bootstrap issue for partnercentral-revenue-measurement and three partnercentral-benefits stub issues).

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
3. Observe fixture partnercentral-channel-list-channel-handshakes-succeeds-without-partner-benefit, step task#1 (and task#2): a POST /ListChannelHandshakes with the recorded required input (handshakeType, catalog, participantType) is answered HTTP 403 with no items member, where the recording expects HTTP 200 and $.items = [].
