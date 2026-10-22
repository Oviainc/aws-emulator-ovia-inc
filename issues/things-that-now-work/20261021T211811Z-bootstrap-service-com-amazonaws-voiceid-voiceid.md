Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Amazon Voice ID emulator

_Filed: 2026-10-21T21:18:11Z_
_Operation: service-com-amazonaws-voiceid-voiceid | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 29 operations in `services/service-com-amazonaws-voiceid-voiceid/src/ops/` are unmodified scaffolds: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. The service holds no state, so no client workflow works.

Implement a FIRST stateful slice: the domain + watchlist lifecycle, which is the entry point for every other Voice ID call (every other operation takes a `DomainId`). Wire protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: VoiceID.<OperationName>` (see README/OPERATIONS.md).

Happy path, in order:

1. **CreateDomain** — input `Name` (required, 1..256, `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`), `Description`, `ServerSideEncryptionConfiguration` (required, with required `KmsKeyId`), `ClientToken` (@idempotencyToken), `Tags`. Must mint a fresh 22-char `[a-zA-Z0-9]{22}` DomainId, build `Arn` matching `^arn:aws(-[^:]+)?:voiceid.+:[0-9]{12}:domain/[a-zA-Z0-9]{22}$`, set `DomainStatus` = `ACTIVE`, set `CreatedAt`/`UpdatedAt`, create the domain's DEFAULT watchlist and report its id in `Domain.WatchlistDetails.DefaultWatchlistId`, persist all of it, and return the stored `Domain`.
2. **DescribeDomain** — input `DomainId`; returns the `Domain` **read back from the store** (same DomainId, Arn, Name, Description, DomainStatus, ServerSideEncryptionConfiguration, CreatedAt/UpdatedAt, WatchlistDetails as CreateDomain returned). Unknown DomainId -> `ResourceNotFoundException` (HTTP 404); DomainId not matching the 22-char pattern -> `ValidationException` (HTTP 400).
3. **ListDomains** — `MaxResults` (1..10) and `NextToken`; returns `DomainSummaries` built from the stored domains (one entry per created, not-yet-deleted domain) plus a `NextToken` only when more remain. Must honour MaxResults and resume correctly from the token.
4. **CreateWatchlist** — input `DomainId` (required), `Name` (required), `Description`, `ClientToken`; mints a 22-char WatchlistId, persists the watchlist under that domain with `DefaultWatchlist = false`, `CreatedAt`/`UpdatedAt`, and returns the stored `Watchlist`. Unknown `DomainId` -> `ResourceNotFoundException`.
5. **ListWatchlists** — input `DomainId`, `MaxResults` (1..100), `NextToken`; returns `WatchlistSummaries` for that domain, which must include BOTH the domain's default watchlist (`DefaultWatchlist = true`) and every watchlist created in step 4. (`DescribeWatchlist` by id should return the same stored record.)
6. **DeleteDomain** — input `DomainId`; removes the domain and its watchlists from the store, returns an empty body. A subsequent `DescribeDomain`/`ListWatchlists` for that id must fail with `ResourceNotFoundException` (404), and the domain must be gone from `ListDomains`.

Every one of these must compute its response from persisted state — no canned structs, no echoing the input back without storing it, no random id without a stored record behind it. `ClientToken` on CreateDomain/CreateWatchlist (both `@idempotent`) should return the SAME resource when replayed with the same token rather than minting a second one.

## State Model

In `src/state.rs` (`ComAmazonawsVoiceidVoiceidState`, reached via `crate::state::service_state(&*self.engine)`):

- `domains: HashMap<String /* DomainId (22 chars) */, Domain>` where `Domain { domain_id, arn, name, description: Option<String>, domain_status: DomainStatus /* ACTIVE */, sse_config: ServerSideEncryptionConfiguration { kms_key_id }, created_at: SystemTime, updated_at: SystemTime, default_watchlist_id: String, tags: Vec<Tag> }`
- `watchlists: HashMap<String /* DomainId */, HashMap<String /* WatchlistId */, Watchlist>>` where `Watchlist { domain_id, watchlist_id, name, description: Option<String>, default_watchlist: bool, created_at, updated_at, fraudster_ids: HashSet<String> }` (the `fraudster_ids` set is unused by this slice but is where AssociateFraudster will later write)
- `idempotency: HashMap<String /* ClientToken */, String /* created resource id */>` so replayed CreateDomain/CreateWatchlist return the original resource
- insertion-ordered iteration (e.g. keep a `Vec<String>` of domain ids, or use an ordered map) so ListDomains/ListWatchlists pagination tokens are stable

Out of scope for this first slice (stays 501, filed separately later): speakers (DescribeSpeaker/ListSpeakers/OptOutSpeaker/DeleteSpeaker), fraudsters (AssociateFraudster/DisassociateFraudster/DescribeFraudster/ListFraudsters/DeleteFraudster), the batch jobs (Start*/Describe*/List* registration + enrollment jobs), EvaluateSession, UpdateDomain/UpdateWatchlist/DeleteWatchlist, and the tagging trio.

## Evidence

`grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-voiceid-voiceid/src/ops/` matches all 29 operation files (create_domain.rs, describe_domain.rs, list_domains.rs, create_watchlist.rs, list_watchlists.rs, delete_domain.rs, ...), i.e. every operation answers HTTP 501 and persists nothing. Contract grounded in `services/service-com-amazonaws-voiceid-voiceid/OPERATIONS.md`: CreateDomain (required `Name`, `ServerSideEncryptionConfiguration`; output `Domain`), DescribeDomain (@readonly, input `DomainId` 22-char, output `Domain`, errors incl. ResourceNotFoundException 404 / ValidationException 400), ListDomains (@paginated inputToken=NextToken/outputToken=NextToken/items=DomainSummaries, MaxResults 1..10), CreateWatchlist (@idempotent, required `DomainId`+`Name`, output `Watchlist`), ListWatchlists (@paginated items=WatchlistSummaries, MaxResults 1..100), DeleteDomain (@idempotent, no output members). Field lists for `Domain`, `DomainSummary`, `Watchlist`, `WatchlistSummary`, `ServerSideEncryptionConfiguration` taken from the Smithy model via smithy_query (Domain carries DomainId/Arn/Name/Description/DomainStatus/ServerSideEncryptionConfiguration/CreatedAt/UpdatedAt/WatchlistDetails; Watchlist carries DomainId/WatchlistId/Name/Description/DefaultWatchlist(default false)/CreatedAt/UpdatedAt). Well-established Voice ID behaviour: a domain is created ACTIVE and owns a default watchlist whose id is reported in WatchlistDetails.DefaultWatchlistId. No tests exist for these operations.

## Steps to Reproduce

1. POST / with X-Amz-Target: VoiceID.CreateDomain, body {"Name":"bootstrap-domain","Description":"demo","ServerSideEncryptionConfiguration":{"KmsKeyId":"alias/aws/voiceid"}} — expect 200 with Domain.DomainId (22 chars), Domain.Arn, DomainStatus=ACTIVE, WatchlistDetails.DefaultWatchlistId; today returns 501 NotImplemented.
2. POST VoiceID.DescribeDomain {"DomainId":"<id from step 1>"} — expect the identical Domain record read back from state.
3. POST VoiceID.ListDomains {"MaxResults":10} — expect DomainSummaries containing exactly the created domain.
4. POST VoiceID.CreateWatchlist {"DomainId":"<id>","Name":"vip-list"} — expect 200 with Watchlist.WatchlistId (22 chars), DefaultWatchlist=false.
5. POST VoiceID.ListWatchlists {"DomainId":"<id>"} — expect two summaries: the default watchlist (DefaultWatchlist=true) and vip-list.
6. POST VoiceID.DeleteDomain {"DomainId":"<id>"} — expect 200 empty body.
7. POST VoiceID.DescribeDomain {"DomainId":"<id>"} again — expect ResourceNotFoundException (HTTP 404); and ListDomains no longer lists it.
8. POST VoiceID.DescribeDomain {"DomainId":"too-short"} — expect ValidationException (HTTP 400).
