Issue-Source: trace-sweep-divergence
Issue-Trace: s3outposts-discover-outposts-with-s3
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3outposts-discover-outposts-with-s3 s3outposts-list-shared-endpoints-for-outpost
Issue-Shape-Id: com.amazonaws.s3outposts#S3Outposts
Issue-Key: s3-outposts|ListOutpostsWithS3|2xx->2xx|$.Outposts[*]
Issue-Key: s3-outposts|ListOutpostsWithS3|2xx->2xx|$.NextToken
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# S3 Outposts seeds phantom Outposts reported by ListOutpostsWithS3

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListOutpostsWithS3

## Description
The service's store pre-seeds a fixed set of Outpost records at init (services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs, `impl Default for ComAmazonawsS3outpostsS3outpostsState`), and ListOutpostsWithS3 reports them. The recorded corpus shows AWS reporting NO Outposts for an account with no S3 on Outposts capacity, so every ListOutpostsWithS3 answer the emulator gives is populated with resources that do not exist, and its pagination cursor is derived from them. Two causes: the first page, and the paged continuation.

Cause 1 of 2: unpaginated ListOutpostsWithS3 returns three fabricated Outposts.
AWS: Smithy: ListOutpostsWithS3 -> ListOutpostsWithS3Result{Outposts: Outposts (list), NextToken}, http GET /S3Outposts/ListOutpostsWithS3, success 200; Outposts is documented as "the list of Outposts that have S3 provisioned ... Active ... to which the the calling Amazon Web Services account has access". The inventory itself is recorded behavior only: the recording returns 200 with an EMPTY Outposts list (and no NextToken) — this account has no S3-on-Outposts capacity, which is consistent with the same corpus's unknown-outpost fixtures. Nothing in the model lets a service invent Outposts; there is no modeled operation that creates one, so an account that was never provisioned reports none.
Emulator: 200 with three Outpost entries, including one whose OwnerId is a different account from the caller's. Hypothesis: the seed exists so CreateEndpoint has a parent Outpost to attach to, per the state module's own "Why the Outposts are seeded" note; the recording shows that choice is not AWS's observable behavior.
Why: a client's discovery step is the gate for everything after it. Code that enumerates the account's S3-on-Outposts capacity sees three Outposts that do not exist, then builds ARNs, picks an OutpostId and attempts CreateEndpoint against hardware that was never provisioned; conformance and inventory tooling reports capacity for an empty account, and a cross-account OwnerId appears where the caller shares nothing. A test asserting "no Outposts provisioned" cannot pass against this emulator at all.
Fix: the service's state seeding and the ListOutpostsWithS3 handler — the store must start with an empty Outpost inventory (and whatever CreateEndpoint needs from a parent Outpost sourced some other way), so the handler computes an empty list from an empty store.
Traces: s3outposts-discover-outposts-with-s3#task#1 (same cluster also covers s3outposts-list-shared-endpoints-for-outpost#task#1).

Cause 2 of 2: the paged ListOutpostsWithS3 call also returns a NextToken for a listing AWS reports as complete and empty.
AWS: Smithy: @paginated(inputToken: NextToken, outputToken: NextToken, items: Outposts, pageSize: MaxResults); ListOutpostsWithS3Result$NextToken is "a token that you can use to call ListOutpostsWithS3 again and receive additional results, IF THERE ARE ANY". The recording returns 200 with an empty Outposts list and NO NextToken, i.e. the listing terminates on the first page.
Emulator: 200 with one Outpost entry and $.NextToken present (an Outpost id, e.g. 'op-1b2c3d4e5f6071829'), so the pagination advertises a further page.
Why: a paginating SDK client loops while NextToken is present, so it makes an extra round trip and accumulates phantom Outposts; a client that expects the listing to terminate immediately (empty account) instead walks a cursor over fabricated records. The token is only emitted because the seeded records fill the page.
Fix: the ListOutpostsWithS3 handler's paging (it sets next_token from the next key in the seeded store) together with the empty-store fix above — no NextToken may be returned once the page is the last one, and an empty inventory must yield no token.
Traces: s3outposts-discover-outposts-with-s3#task#2.

No existing open issue covers s3-outposts (list_existing_issues returned none for 'outpost'/'s3outposts').

## Evidence
Cause 1 — fixture s3outposts-discover-outposts-with-s3 step task#1, ListOutpostsWithS3: expected status 200, actual 200; $.Outposts[0], $.Outposts[1], $.Outposts[2] all 'extra (expected None)' — the recorded Outposts list is empty while the emulator returned three entries (OutpostIds op-0a1b2c3d4e5f60718 / op-1b2c3d4e5f6071829 owned by the caller account, and op-2c3d4e5f607182930 owned by a DIFFERENT account, with CapacityInBytes 52776558133248 / 105553116266496 / 26388279066624). The same divergence cluster also covers s3outposts-list-shared-endpoints-for-outpost#task#1 (affected_count 2).
Cause 2 — fixture s3outposts-discover-outposts-with-s3 step task#2, ListOutpostsWithS3: expected 200, actual 200; $.NextToken type_mismatch (expected None, got 'op-1b2c3d4e5f6071829') and $.Outposts[0] extra (expected None, got the op-0a1b2c3d4e5f60718 record). The exact ids/capacities/account numbers are examples of the seeded records, not requirements; the requirements are the empty Outposts list and the absent NextToken.
Source of the fabricated inventory: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs (Default impl seeds three OutpostRecords); the token is set in services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_outposts_with_s3.rs (next_token = Some(outpost_id.clone()) once the page fills).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture s3outposts-discover-outposts-with-s3 step task#1: GET /S3Outposts/ListOutpostsWithS3 on a freshly reset engine. The 200 body contains three Outposts; the recording's body has an empty Outposts list and no NextToken.
3. Replay the same fixture's step task#2 (the paged ListOutpostsWithS3 call with a page size). The 200 body contains one Outpost plus NextToken 'op-1b2c3d4e5f6071829'; the recording's body is an empty Outposts list with no NextToken.
4. Also replay s3outposts-list-shared-endpoints-for-outpost step task#1, which is the other member of the same cluster and shows the identical phantom inventory.
5. Expected after the fix: a freshly reset engine reports Outposts: [] with no NextToken for both the unpaginated and the paged call.
