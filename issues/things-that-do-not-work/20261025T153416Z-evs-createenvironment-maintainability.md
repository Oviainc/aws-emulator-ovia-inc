Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# Hand-rolled Ipv4Cidr parse/overlap duplicates the CIDR rule this crate already evaluates through ec2's published helpers

_Filed: 2026-10-25T15:34:16Z_
_Operation: CreateEnvironment | Class: maintainability_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The "a VLAN subnet CIDR block must parse and must overlap no other block" rule is implemented twice in this crate's own code, through two different CIDR engines, so the two copies can drift:

1. `crate::state::Ipv4Cidr::parse` / `Ipv4Cidr::overlaps` — hand-written integer arithmetic in `state.rs`, used by `CreateEnvironment`'s `vlan_failures` to judge the ten blocks against each other.
2. `ec2::canonical_subnet_ipv4_cidr` / `ec2::ipv4_cidr_block_contains` / `ec2::ipv4_cidr_blocks_overlap` — EC2's published helpers, used by this crate's `state/ec2_resources.rs` to judge the SAME blocks against the VPC's address space and its existing subnets.

`ec2_resources.rs` itself states that the second is the same rule as the first, and `state.rs` itself states that the hand-rolled span is replaceable by `ipnet::Ipv4Net`, a crate the workspace already resolves. Because the two engines canonicalise and compare independently, a block one accepts and the other rejects (or two blocks one judges overlapping and the other does not) produces inconsistent refusals for one request. The fix is to put the parse and the overlap behind one implementation (the `ipnet` the comment names, or EC2's published helpers already in use), keeping `usable_addresses` — the one piece no general CIDR crate expresses — on top of it. Responses and existing tests stay unchanged.

## Evidence

The crate admits the hand-rolled span and names the replacement — `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:927-933`:

```
/// One IPv4 CIDR block, reduced to the inclusive address range it covers.
///
/// Hand-written rather than taken from `ipnet`, which the workspace already resolves:
/// the three rules this service needs are a mask, a range comparison and an offset into
/// the block, and the offset — the addresses AWS leaves usable in a subnet — is not
/// something a general CIDR crate expresses. Switching the parse and the overlap onto
/// `ipnet::Ipv4Net` is a behaviour-neutral follow-up.
```

Copy 1 — `state.rs:947-969`:

```rust
pub fn parse(cidr: &str) -> Option<Self> {
    let (address, prefix_len) = cidr.split_once('/')?;
    let address: ::std::net::Ipv4Addr = address.parse().ok()?;
    ...
}

/// Whether the two blocks share any address.
pub fn overlaps(&self, other: &Self) -> bool {
    self.first <= other.last && other.first <= self.last
}
```

used by `src/ops/create_environment.rs:1067` and `:1086`:

```rust
let Some(network) = Ipv4Cidr::parse(vlan.cidr) else { ... };
...
if let Some((other, _)) = networks.iter().find(|(_, other)| other.overlaps(&network)) {
```

Copy 2 — the same rule through EC2's helpers, in this same crate: `src/state/ec2_resources.rs:725` says

```
/// This is the same overlap rule `CreateEnvironment` applies among an environment's own
```

and `src/state/ec2_resources.rs:749,754,774`:

```rust
let Ok(canonical) = ec2::canonical_subnet_ipv4_cidr(vlan.cidr) else { ... };
    .any(|block| ec2::ipv4_cidr_block_contains(block, &canonical))
    .is_some_and(|held| ec2::ipv4_cidr_blocks_overlap(held, &canonical))
```

with the module header at `src/state/ec2_resources.rs:59-61` stating the intent of having ONE reading: "every judgement about a CIDR block a caller supplied is EC2's own published helper (`canonical_subnet_ipv4_cidr`, `ipv4_cidr_block_contains`, `ipv4_cidr_blocks_overlap`), so one reading of a block serves both services".

The named replacement is already resolved by the workspace — `Cargo.lock:3113`: `name = "ipnet"`.

Test coverage that must stay green and unchanged: `tests/all/create_environment_validation.rs` (header line 15: "subnet block is between /24 and /28 and overlaps no other"; the overlapping-HCX case at lines 168-170 and 223-224; the per-/24 layout note at line 246), `tests/all/create_environment_network_membership.rs`, `tests/all/vlan_subnet_propagation.rs`, and `tests/all/created_environment_hosts.rs` (host management addresses drawn from the `vmkManagement` block via `Ipv4Cidr::usable_addresses`, `state.rs:977-988`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:927-989 and read the `Ipv4Cidr` comment and the hand-written `parse`/`overlaps`.
2. Open src/ops/create_environment.rs:1047-1096 (`vlan_failures`) and see the overlap/prefix rule evaluated with that hand-rolled type.
3. Open src/state/ec2_resources.rs:59-61 and :721-781 and see the SAME overlap rule evaluated with `ec2::canonical_subnet_ipv4_cidr` / `ec2::ipv4_cidr_blocks_overlap`, with the comment at :725 stating it is the same rule.
4. grep Cargo.lock for `name = "ipnet"` (line 3113) to confirm the replacement the state.rs comment names is already a workspace dependency.
5. Run `cargo test -p service-com-amazonaws-evs-amazonelasticvmwareservice` to see the tests that pin the current clauses (create_environment_validation, create_environment_network_membership, vlan_subnet_propagation, created_environment_hosts) — they must all still pass unchanged after the two copies are collapsed into one.
