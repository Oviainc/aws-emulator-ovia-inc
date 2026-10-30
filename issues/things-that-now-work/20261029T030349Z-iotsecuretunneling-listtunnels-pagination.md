Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# A ListTunnels nextToken carries only a store-wide creation sequence, so a token minted by another Region's listing silently restarts (or empties) the page instead of being refused

_Filed: 2026-10-29T03:03:49Z_
_Operation: ListTunnels | Class: pagination_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTunnels` mints its page token from `StoredTunnel::creation_seq` alone, and that sequence comes from ONE store-wide counter (`next_creation_seq` on the state, claimed by `OpenTunnel` whatever Region it serves). The token therefore carries no trace of the listing that minted it, while the listing itself is strictly Region-scoped: the handler walks `tunnels_in_region(&region)` and the crate's own rule is that a tunnel "is neither readable nor mutable through another's" endpoint.

The handler states its own rule for a token that cannot name a position in the listing it is replayed against: refuse it as invalid input "rather than silently restarting the listing". It applies that rule only to a token the service did not MINT (`decode_next_token` fails). A token minted by a DIFFERENT Region's listing decodes fine, so it is accepted, and the resulting page is wrong in exactly the two ways the comment forbids:

* a token whose sequence is higher than every tunnel of the called Region makes the filter `tunnel.creation_seq < seq` pass everything, so the listing SILENTLY RESTARTS from the newest tunnel of that Region instead of resuming anywhere;
* a token whose sequence is lower than every tunnel of the called Region makes the filter reject everything, so the call answers an empty `tunnelSummaries` with no `nextToken` — the client is told the Region holds no tunnels while the store holds several OPEN ones.

Every other cross-Region reference in this crate is refused outright (a `tunnelId` or a `resourceArn` of another Region answers the modelled 404); the page cursor is the one cross-Region identifier the handler accepts and acts on.

## Evidence

The rule the handler states for a token that cannot name a position in this listing — `src/ops/list_tunnels.rs:141-154`:

    // A page token this service did not mint cannot name a position in the
    // listing. The operation models no error for it, so refuse it as invalid
    // input rather than silently restarting the listing.
    let resume_after = match input.next_token.as_deref() {
        None => None,
        Some(token) => match crate::state::decode_next_token(token) {
            Some(seq) => Some(seq),
            None => {
                return Err(crate::scaffolded::Scaffolded::validation(
                    "nextToken is not a token this service issued",
                ))
            }
        },
    };

The listing the token is replayed against is Region-scoped — `src/ops/list_tunnels.rs:165-167` and `:179-191`:

    // The listing reports the tunnels of the account IN the Region being
    // called, which is the only Region whose tunnels this endpoint holds.
    let region = crate::state::request_region(ctx);
    ...
    let mut ordered: Vec<(u64, String)> = store
        .tunnels_in_region(&region)
    ...
        .filter(|tunnel| resume_after.is_none_or(|seq| tunnel.creation_seq < seq))

but the token carries only the sequence — `src/state.rs:796-807`:

    /// The `nextToken` that resumes a `ListTunnels` page after the tunnel whose
    /// creation sequence is `seq`.
    pub fn encode_next_token(seq: u64) -> String {
        format!("{NEXT_TOKEN_PREFIX}{seq:016x}")
    }

and the sequence space is ONE counter for the whole store, every Region included — `src/state.rs:363-364` and `:433-438`:

    /// The next creation sequence number handed to a tunnel.
    next_creation_seq: u64,
    ...
    pub fn next_creation_seq(&mut self) -> u64 {
        let seq = self.next_creation_seq;
        self.next_creation_seq = seq.saturating_add(1);
        seq
    }

claimed by `OpenTunnel` for whatever Region it is serving — `src/ops/open_tunnel.rs:287`:

    let creation_seq = store.next_creation_seq();

against the crate's own Region rule — `src/state.rs:11-15`:

    //! A tunnel is a REGIONAL resource, so the store is partitioned by the Region of
    //! the endpoint each record was created against ([`request_region`]) and every
    //! lookup names that Region

Test coverage: `tests/all/tunnel_lifecycle.rs:474-524` pages a single Region's listing at every page size, and `tests/all/tunnel_lifecycle.rs:527-544` (`list_tunnels_refuses_a_next_token_it_did_not_issue`) covers only tokens that fail `decode_next_token` ("not-a-token", "tnl-zzzz", "AAAA"). `tests/all/tunnel_region_isolation.rs:222-234` exercises `ListTunnels` across Regions but never with a `nextToken`, so no test pins what a token minted in another Region does.

## Steps to Reproduce

1. Mount the service on an `InMemoryEngine` through `MultiServiceRouter`, as `tests/all/tunnel_region_isolation.rs` does.
2. Signed for `us-east-1`, call `OpenTunnel` twice (`{"destinationConfig":{"thingName":"thing-1","services":["SSH"]}}`); these take creation sequences 0 and 1.
3. Signed for `us-east-1`, call `ListTunnels` with `{"maxResults":1}` and keep the returned `nextToken` (it encodes sequence 1).
4. Signed for `eu-west-1`, call `OpenTunnel` twice; these take creation sequences 2 and 3. Confirm `ListTunnels` signed for `eu-west-1` with `{}` reports both.
5. Signed for `eu-west-1`, call `ListTunnels` with `{"nextToken":"<the us-east-1 token>"}`: the answer is 200 with an empty `tunnelSummaries` and no `nextToken`, although that Region holds two OPEN tunnels — the token named a position in another Region's listing and was neither refused nor scoped.
6. For the mirror case, take a `nextToken` from the `eu-west-1` listing (it encodes sequence 3) and replay it signed for `us-east-1`: the whole `us-east-1` listing comes back from the newest tunnel, i.e. the listing silently restarts — the outcome the handler's own comment at `src/ops/list_tunnels.rs:141-143` says must be refused instead.
