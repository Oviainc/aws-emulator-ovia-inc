Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# LimitExceededException counts CLOSED tunnels, so closing a tunnel never frees tunnel capacity

_Filed: 2026-10-24T00:49:34Z_
_Operation: OpenTunnel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tunnel quota AWS enforces on `OpenTunnel` (reported by the modeled `LimitExceededException`, "Thrown when a tunnel limit is exceeded", HTTP 403) is a quota on tunnels that are concurrently OPEN. Expected: after `CloseTunnel` on a tunnel (with `delete` absent/false, i.e. the record stays describable), the account is below the concurrency quota again and the next `OpenTunnel` succeeds; the record that remains readable does not consume capacity. The quota AWS publishes for concurrent tunnels is also far smaller than this emulator's bound, so an account that holds many open tunnels is refused by AWS while the emulator accepts.

Actual: the emulator's bound counts every STORED record regardless of status — `is_at_tunnel_limit()` is `self.tunnels.len() >= MAX_TUNNELS` with `MAX_TUNNELS = 500`, and a record leaves the store only through `CloseTunnel` with `delete: true`. So a client that closes tunnels normally (the documented, data-preserving close) permanently loses capacity, and the refusal message even instructs the caller to "close a tunnel with delete set before opening another" — behavior and wording AWS never produces. Conversely, for the first 500 opens the emulator never refuses, including when far more tunnels are concurrently open than AWS's concurrent-tunnel quota permits.

## Evidence

Spec: `com.amazonaws.iotsecuretunneling#LimitExceededException` is modeled on `OpenTunnel` with `httpError 403` and documents itself as thrown when a tunnel limit is exceeded; `CloseTunnel$delete` is documented as "When set to true, IoT Secure Tunneling deletes the tunnel data immediately", i.e. a plain close leaves a CLOSED record behind rather than keeping the tunnel active. Well-established service behavior: the secure-tunneling tunnel quota is on concurrently open tunnels, so closing one frees a slot.

Handler: `src/state.rs` lines 28-39 (`MAX_TUNNELS = 500`) and `is_at_tunnel_limit` (status-blind count); `src/ops/open_tunnel.rs` lines 171-187 (refusal plus the "close a tunnel with delete set" message); `src/ops/close_tunnel.rs` lines 150-165 (only `delete: true` removes the record).

Tests: `tests/all/tunnel_lifecycle.rs::open_tunnel_refuses_once_the_store_holds_its_limit_of_tunnels` asserts the current status-blind behavior (it fills the store and then frees a slot via a delete-close); no test asserts that a plain close frees capacity.

## Steps to Reproduce

1. Open tunnels until the emulator's bound is reached (500 with the default `MAX_TUNNELS`).
2. Call `CloseTunnel` on one of them WITHOUT `delete` (or with `delete: false`); `DescribeTunnel` confirms the record is still readable with `status: CLOSED`.
3. Call `OpenTunnel` again: the emulator still answers HTTP 403 `LimitExceededException` because the closed record keeps occupying capacity; AWS, whose quota is on concurrently open tunnels, accepts the open.
4. Conversely, open more tunnels than the service's published concurrent-tunnel quota (all left OPEN) and observe the emulator keeps answering 200 where AWS answers 403 `LimitExceededException`.
