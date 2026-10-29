Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# RotateTunnelAccessToken samples the clock before taking the store lock, so a tunnel whose lifetime ran out in between is rotated instead of refused and is left stored as OPEN

_Filed: 2026-10-29T17:20:37Z_
_Operation: RotateTunnelAccessToken | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RotateTunnelAccessToken` reads the wall clock into `now` BEFORE it acquires the shared store lock, and then uses that stale instant as the one "now" of the whole critical section: it is the instant passed to `state::materialize_expired_tunnels`, which is the gate that settles the clock-owed close, and it is the instant passed to `state::stamp_tunnel_updated`.

Acquiring the lock is a blocking operation of unbounded duration (every one of this service's eight handlers takes the same single mutex, and `ListTunnels`/`DescribeTunnel` hold it while they project records). If a tunnel's `expires_at` falls inside the window between the clock sample and the lock acquisition, then at the moment the handler actually inspects the store:

* `materialize_expired_tunnels(now)` is handed an instant EARLIER than the expiry, so its gate (`Some(earliest) if now < earliest => return`) returns without closing anything;
* the resolved record therefore still carries `status == TunnelStatus::Open`, so the `tunnel.status != TunnelStatus::Open` refusal does not fire;
* the handler mints and stores two fresh client access tokens for a tunnel whose documented lifetime has already run out, answers 200, and writes `last_updated_at = now` — leaving an expired record stored as `OPEN` with no `closed_at`.

This turns the handler's own documented refusal into a success ("Credentials are only reissued for a tunnel that is still live"), and it leaves the store in the one shape the crate's own cross-cutting test says no entry point may leave behind. The same pre-lock sample is also what lets the stamp regress: a concurrent `TagResource`/`UntagResource` that sampled a LATER instant but won the lock first writes its stamp, and this call then overwrites `last_updated_at` with its own earlier `now`, so `DescribeTunnel` reports a `lastUpdatedAt` that moved backwards after a later successful update.

The fix is local to the handler: sample the instant inside the critical section, i.e. after `state.lock()` and before `materialize_expired_tunnels`, so the instant the call reasons with is the instant at which it actually holds the store. No constant, response field or error wording changes.

## Evidence

The clock is sampled before the lock, and the same value drives both the expiry sweep and the stamp — `src/ops/rotate_tunnel_access_token.rs:209-214`:

        let now = ::std::time::SystemTime::now();
        // Tokens are rotated through the endpoint of the tunnel's own Region and
        // no other: a tunnel of another Region is not a resource this one holds.
        let region = crate::state::request_region(ctx);
        let mut store = state.lock();
        store.materialize_expired_tunnels(now);

and `src/ops/rotate_tunnel_access_token.rs:260`:

        store.stamp_tunnel_updated(&region, &input.tunnel_id, now);

The refusal the handler claims to apply, `src/ops/rotate_tunnel_access_token.rs:225-239`:

        // Credentials are only reissued for a tunnel that is still live: a closed
        // tunnel can never be connected to, so AWS refuses to mint a client access
        // token for one. ... the check
        // runs before any token is minted or stored, so a refused call leaves the
        // record exactly as it was.
        if tunnel.status != crate::state::TunnelStatus::Open {
            ...
            return Err(crate::scaffolded::Scaffolded::validation(format!(
                "Tunnel {} is close.",

The gate that silently skips the sweep when it is handed a stale instant, `src/state.rs:620-626`:

    pub fn materialize_expired_tunnels(&mut self, now: SystemTime) {
        match self.earliest_open_expiry {
            // Nothing is open, or the soonest expiry is still in the future.
            None => return,
            Some(earliest) if now < earliest => return,
            Some(_) => {}
        }

and its documented contract, `src/state.rs:605-611`:

    /// Close every tunnel whose `maxLifetimeTimeoutMinutes` has run out.
    ///
    /// This is the one write a reporting operation may make: the transition was
    /// already owed at `now` by the clock — "The maximum amount of time (in
    /// minutes) a tunnel can remain open" — whether or not anybody called.

The crate's own cross-cutting rule the success violates, `tests/all/owed_close_settled_by_every_entry_point.rs:5-12`:

    //! RULE: a tunnel whose `maxLifetimeTimeoutMinutes` ("The maximum amount of time
    //! (in minutes) a tunnel can remain open") has run out is owed the close by the
    //! clock whether or not anybody called, so EVERY operation of this service —
    //! reporting or mutating, addressed by `tunnelId`, by `resourceArn` or
    //! addressing no existing tunnel at all — settles that owed transition before it
    //! answers, and none of them leaves an expired record behind still stored as
    //! `OPEN`.

and the stamp contract the regression breaks, `src/state.rs:380-387`:

    /// `Tunnel$lastUpdatedAt` is "The last time the tunnel was updated", so
    /// every call that changes something the tunnel reports goes through here
    /// with the instant of its change

Test coverage: `tests/all/owed_close_settled_by_every_entry_point.rs:302-309` drives exactly this operation against an expired tunnel and expects HTTP 400, but it rewinds the record and then issues ONE sequential call, so the sample/lock window is never exercised; `tests/all/rotate_refusal_error_contract.rs:288-315` (`HowItClosed::LifetimeElapsed`) is likewise sequential; `tests/all/member_update_stamp.rs:308-319` asserts `after.last_updated_at > before.last_updated_at` but only for one request at a time. No suite in `tests/all.rs` issues two requests concurrently, so neither the admitted rotate nor the stamp regression is caught.

## Steps to Reproduce

1. Mount the service on an `engine::InMemoryEngine` behind a `MultiServiceRouter` as `tests/all/rotate_refusal_error_contract.rs` does, on a multi-thread tokio runtime.
2. Call `OpenTunnel` with `{"destinationConfig":{"thingName":"thing-race","services":["SSH"]},"timeoutConfig":{"maxLifetimeTimeoutMinutes":1}}` and note `tunnelId`.
3. Rewind the stored record's `created_at`/`last_updated_at`/`expires_at` so that `expires_at` is a few milliseconds in the FUTURE, and call `store.note_open_expiry(expires_at)` (the same rewind helper the suites use).
4. Hold the shared store lock from another task for longer than that remaining margin (e.g. acquire `engine.state::<TunnelState>().lock()` and sleep), then issue `RotateTunnelAccessToken` with `{"tunnelId":"<id>","clientMode":"ALL"}` so the handler samples `now` at `src/ops/rotate_tunnel_access_token.rs:209` before the expiry and only reaches `state.lock()` at `:213` after it.
5. Observe HTTP 200 with two fresh access tokens, instead of the HTTP 400 `ValidationException` "Tunnel <id> is close." that `src/ops/rotate_tunnel_access_token.rs:232-238` promises for a tunnel that is no longer OPEN.
6. Read the record out of the store: `status` is still `OPEN` and `closed_at` is `None` although `expires_at` is in the past — the state `tests/all/owed_close_settled_by_every_entry_point.rs:5-12` says no entry point may leave behind.
7. For the stamp half: issue `TagResource` and `RotateTunnelAccessToken` concurrently against one tunnel (`futures::future::join_all`) and compare `DescribeTunnel`'s `tunnel.lastUpdatedAt` before and after — it can move backwards, because the rotate stamps the pre-lock `now` at `src/ops/rotate_tunnel_access_token.rs:260`.
