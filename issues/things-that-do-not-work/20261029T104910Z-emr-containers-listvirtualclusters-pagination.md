Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6270af2faafa317884be9e4627608e6b01112bc2

# A nextToken minted at one Region's endpoint is honoured at another's, so the page is positioned in a different Region's order and records are silently skipped

_Filed: 2026-10-29T10:49:10Z_
_Operation: ListVirtualClusters | Class: pagination_

> _Note: this issue was identified against the code base at commit `6270af2faafa317884be9e4627608e6b01112bc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The paginated listings whose cursor scope is a region-independent CONSTANT — `ListVirtualClusters` (`LIST_VIRTUAL_CLUSTERS_SCOPE`), `ListJobTemplates` (`LIST_JOB_TEMPLATES_SCOPE`) and `ListSecurityConfigurations` (`LIST_SECURITY_CONFIGURATIONS_SCOPE`) — encode the cursor as `{scope}:{createdAtMillis}:{sequence}` with nothing in it that names the Region that issued it, while the record set the cursor is applied to IS Region-filtered (`region_virtual_clusters(&region)`).

So a `nextToken` handed back by one Region's endpoint decodes successfully at another Region's endpoint, and `page_of` positions into THAT Region's own newest-first order: it reports everything ordered at or after a position computed over records the requesting endpoint does not hold. A client whose paginator carries a token across endpoints gets a silently truncated (often empty) listing and a 200, instead of the refusal this crate gives for every other cursor its own scope did not issue.

This is the one half of the cursor-scope rule the crate does not apply. The same store keys every other cross-request token by Region and says why; the listing half of the cursor rule is even pinned by a test, while the Region half is not applied at all. The two listings whose scope is a cluster id (`ListJobRuns`, `ListManagedEndpoints`) are immune only incidentally, because a cluster id resolves in exactly one Region.

(Distinct from the open issue `20261029T090200Z-emr-containers-listjobruns-pagination`, which is about decode accepting `+`-signed / leading-zero spellings of a cursor's numeric fields.)

## Evidence

The cursor carries the scope and nothing else — state.rs:402-414:

    /// The opaque `nextToken` a `@paginated` list in this service hands back: the place
    /// `(createdAt, creation sequence)` in a newest-first order the next page starts at,
    /// prefixed with the SCOPE that issued it, so a cursor one listing minted is never read as
    /// a position in another's order.
    ...
    pub fn encode_list_cursor(scope: &str, created_at_millis: i64, sequence: u64) -> String {
        format!("{scope}:{created_at_millis}:{sequence}")
    }

and the scope of three listings is a Region-independent constant — state.rs:430-437:

    /// The scope [`encode_list_cursor`] tags a `ListJobTemplates` cursor with.
    pub const LIST_JOB_TEMPLATES_SCOPE: &str = "jobtemplates";
    /// The scope [`encode_list_cursor`] tags a `ListSecurityConfigurations` cursor with.
    pub const LIST_SECURITY_CONFIGURATIONS_SCOPE: &str = "securityconfigurations";
    /// The scope [`encode_list_cursor`] tags a `ListVirtualClusters` cursor with.
    pub const LIST_VIRTUAL_CLUSTERS_SCOPE: &str = "virtualclusters";

while the records the cursor is applied to are Region-filtered — ops/list_virtual_clusters.rs:228-235:

        let page = crate::state::page_of(
            crate::state::LIST_VIRTUAL_CLUSTERS_SCOPE,
            state
                .region_virtual_clusters(&region)
                .filter(|record| matches_filters(record, &input)),
            input.max_results,
            input.next_token.as_deref(),
        )

and `decode_list_cursor` checks only the scope prefix — state.rs:420-427:

    pub fn decode_list_cursor(scope: &str, token: &str) -> Option<(i64, u64)> {
        let rest = token.strip_prefix(scope)?.strip_prefix(':')?;

The same crate states the Region rule it breaks here — state.rs:28-31:

    //! Every record carries the region it was created in, and every lookup here takes that
    //! region: EMR on EKS is a regional service, so a resource exists only at the endpoint
    //! that minted it, and an id offered to another region's endpoint names nothing.

and carries the Region in its other cross-request token key, with the reason — state.rs:1780-1784:

    /// `(region, idempotency scope, clientToken) -> minted resource id` ...
    /// The region is part of the key because the resource a token names
    /// lives in one region: the same token replayed against another region's endpoint is
    /// a different request there, and mints that region's own resource.

Test coverage: `tests/all/paginated_page_bounds.rs` pins the LISTING half of the rule only — line 35 ("a scope that belongs to ANOTHER of this service's"), line 327 `let foreign_cursor = state::encode_list_cursor(&listing.foreign_scope, 1, 2);` and line 368 ("a cursor another listing issued") are refused; `tests/all/region_isolation.rs` exercises every listing per Region (lines 349-383) but never carries a token between the two endpoints, so nothing pins the behaviour reported here.

## Steps to Reproduce

1. Create two virtual clusters through an endpoint signed for us-east-1 (`POST /virtualclusters`), and one virtual cluster through an endpoint signed for us-west-2.
2. Call `GET /virtualclusters?maxResults=1` signed for us-east-1 and keep the `nextToken` it returns (it is `virtualclusters:<millis>:<sequence>` of the us-east-1 record that did not fit on the page).
3. Replay that token against the other Region: `GET /virtualclusters?nextToken=<token>` signed for us-west-2.
4. Observe a 200 whose `virtualClusters` is positioned by a cursor us-west-2 never issued — the us-west-2 cluster is omitted when it sorts after the foreign cursor — instead of the `ValidationException` ("nextToken is not a valid pagination token") this service answers for every other cursor its own scope did not mint.
5. Repeat with `GET /jobtemplates?maxResults=1` and `GET /securityconfigurations?maxResults=1`, whose cursors carry the same Region-independent constant scopes.
