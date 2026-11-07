Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4c560b92ea61305d4a9f12f94c4898dc6d4f1637

# Handler and state docs carry per-operation copies of the modelled error status (403/404) that scaffolded.rs says this service never emits

_Filed: 2026-11-05T11:05:27Z_
_Operation: CloseTunnel | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4c560b92ea61305d4a9f12f94c4898dc6d4f1637`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/scaffolded.rs` owns the one mapping from a modelled error's `@httpError` to the status this service actually answers, and it documents in so many words that the model's 404/403 are NOT statuses this service emits, and that a per-operation copy of that status is exactly what lets two operations disagree. Four handler modules and the state module nevertheless carry hand-written copies of that status — `CloseTunnel`'s summary claims a deleted tunnel makes `DescribeTunnel` answer 404, `RotateTunnelAccessToken`'s summary calls its modelled error "ResourceNotFoundException (404)" while naming the test that pins it at 400, and `OpenTunnel`/`TagResource`/`state.rs` call the tag and tunnel limit refusal "LimitExceededException (HTTP 403)". The copies have already drifted from the single place that computes the value: every one of these refusals leaves this service with HTTP 400, as `wire_status` computes and as two test suites pin.

This is the same rule spelled in two places — `scaffolded::wire_status` and the per-operation prose — and the prose copies now contradict it. The fix is to delete or correct the copies so the status is stated only where it is computed; no response and no existing test changes. (The generator-emitted `## Errors` bullets, e.g. `close_tunnel.rs:59`, echo the model's `@httpError` by construction and are re-rendered by the scaffold, so they are not the copies at issue — the hand-written sentences quoted below are.)

## Evidence

The one place the status is computed, and its own warning against a per-operation copy — `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/scaffolded.rs:240-257`:

    /// by the error shape's name in the body's `__type` (and `x-amzn-errortype`). The
    /// per-shape `@httpError` values the model records for the client errors — 404 on
    /// `ResourceNotFoundException`, 403 on `LimitExceededException` — are not statuses
    /// this service emits, and a client that classifies by status would otherwise read a
    /// plain client fault as a missing route or a denied call.
    ...
    /// The whole service's mapping lives here, deliberately in one place: a per-operation
    /// copy is what lets two operations disagree about one status.
    pub fn wire_status(modeled: u16) -> u16 {
        if (400..500).contains(&modeled) {
            CLIENT_ERROR_STATUS

The copies that disagree with it.

`src/ops/close_tunnel.rs:5-6` (hand-written `@op` summary):

    //! Moves the tunnel to `CLOSED` and disconnects both ends; with `delete` set it
    //! removes the tunnel's data outright, so a later `DescribeTunnel` answers 404.

`src/ops/rotate_tunnel_access_token.rs:22-28` — the copy and, in the next sentence, the test that refutes it:

    //! error enum. The operation's one MODELLED error, `ResourceNotFoundException`
    //! (404), answers the OTHER class and only it: a `tunnelId` this Region's
    ...
    //! refusal class to status, code and message is pinned by
    //! `tests/all/rotate_refusal_error_contract.rs`.

and that very test, `tests/all/rotate_refusal_error_contract.rs:108-111`:

    const MODELLED_NOT_FOUND: Refusal = Refusal {
        status: 400,
        code: "ResourceNotFoundException",
    };

`src/ops/open_tunnel.rs:19-21`, `:197-198` and `:236-237`:

    //! ([`crate::state::MAX_TAGS_PER_TUNNEL`]) — is refused with the modelled
    //! `LimitExceededException` (HTTP 403), equally before anything is minted.
    ...
            // `LimitExceededException` ("Thrown when a tunnel limit is exceeded",
            // HTTP 403) exists to report: a closed tunnel holds no slot, so closing
    ...
            // is reported with the same modelled `LimitExceededException` (HTTP 403)

`src/ops/tag_resource.rs:10-12` and `:188-189`:

    //! `ValidationException` and stores nothing; a request that would take the tunnel
    //! past [`crate::state::MAX_TAGS_PER_TUNNEL`] tags is refused with the modelled
    //! `LimitExceededException` (HTTP 403) and also stores nothing.
    ...
            // this service's `LimitExceededException` (HTTP 403) reports, so this

`src/state.rs:36`, `:86` and `:103`:

    /// The quota the modelled `LimitExceededException` (HTTP 403, "Thrown when a
    ...
    /// Reported with the same modelled `LimitExceededException` (HTTP 403) as
    ...
    /// refused — with the modelled `LimitExceededException` (HTTP 403), the one

What the code actually answers, pinned by two suites. `tests/all/modeled_error_wire_status.rs:39-41` and `:305-316`:

    /// The status the `awsJson1_1` client-error channel uses, which is the status
    /// under test.
    const CLIENT_ERROR_STATUS: u16 = 400;
    ...
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {case:?}: expected HTTP {CLIENT_ERROR_STATUS}, answered {status} {body:?}"
            );
            assert_ne!(
                status, row.expected.inert_http_error,
                "case {case:?}: the shape's @httpError is inert on this protocol, answered \
                 {status} {body:?}"
            );

(the row table at `:165-187` includes `DescribeTunnel` on a tunnel removed by `CloseTunnel(delete)`, i.e. exactly the call `close_tunnel.rs:6` says answers 404), and `tests/all/limit_refusals.rs:45-46`:

    const LIMIT_CODE: &str = "LimitExceededException";
    const LIMIT_STATUS: u16 = 400;

Test coverage for the behaviour that must not change: `tests/all/modeled_error_wire_status.rs` (all four tests, including `the_mapping_replaces_every_client_status_and_no_server_one` at `:402-420`), `tests/all/limit_refusals.rs`, and `tests/all/rotate_refusal_error_contract.rs`.

## Steps to Reproduce

1. Read `src/scaffolded.rs:240-257`: the module states that 404 on `ResourceNotFoundException` and 403 on `LimitExceededException` "are not statuses this service emits", and that "a per-operation copy is what lets two operations disagree about one status".
2. Read `src/ops/close_tunnel.rs:5-6`: the handler summary states that after `CloseTunnel` with `delete` set, "a later `DescribeTunnel` answers 404".
3. Run `tests/all/modeled_error_wire_status.rs::every_modelled_error_arrives_with_this_protocols_client_status`, whose table (`:165-187`, `:216-230`) issues exactly that `DescribeTunnel` call against the deleted tunnel and asserts HTTP 400 — and asserts `status != 404`.
4. Read `src/ops/rotate_tunnel_access_token.rs:22-28`, which calls the modelled error "`ResourceNotFoundException` (404)" and names `tests/all/rotate_refusal_error_contract.rs` as the pin; then read that file's `:108-111`, where `MODELLED_NOT_FOUND` is declared with `status: 400`.
5. Read `src/ops/open_tunnel.rs:21`, `src/ops/tag_resource.rs:12` and `src/state.rs:36,86,103` ("HTTP 403"), then `tests/all/limit_refusals.rs:46` (`const LIMIT_STATUS: u16 = 400;`) and its `assert_limit_refusal` at `:85-105`.
