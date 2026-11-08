// SPDX-License-Identifier: CC0-1.0

//! The CONTRACT the account's EC2 network is modelled against, for every service that
//! places a resource in a subnet. Documentation only: it exports nothing, because the one
//! source it names is another emulated SERVICE rather than a function this crate could hold.
//!
//! Several AWS services take EC2 subnet identifiers on a create (`fsx`'s
//! `CreateFileSystem$SubnetIds`, `dax`'s `CreateSubnetGroup$SubnetIds`, and the other
//! VPC-placed resources) and each of them REFUSES a request naming a subnet the account
//! does not have. To refuse one, a service has to know which subnets the account HAS —
//! a fact of the account, not of any one service, so the services that resolve it must agree
//! about it exactly, or a client that places a DAX subnet group and an FSx file system in
//! "the same" subnet is talking to two different accounts.
//!
//! THE ONE ANSWER is Amazon EC2, the service that owns subnets:
//!
//! * A subnet of the account is one EC2 holds: either one a client created with
//!   `ec2:CreateSubnet` in a VPC it owns, or one of the default VPC's default subnets, which
//!   the EC2 crate materialises itself for every Region of the account ("when we create a
//!   default VPC, we create a default subnet in each Availability Zone in the Region") and
//!   reports with ids it minted. Nothing outside EC2 mints, derives or stores a subnet id.
//! * A service that places a resource in a subnet therefore resolves what a request names
//!   by ASKING EC2 — `ec2:DescribeSubnets`, filtered by `subnet-id`, in the Region the
//!   request was signed for, over the in-process callback
//!   (`smithy_server_runtime::OperationContext::callback`), so no service crate names
//!   another. `fsx`'s `src/state/ec2_subnets.rs` is the worked example.
//! * What a service reports about the placement — the VPC, the Availability Zone — is read
//!   off the record EC2 answered with, never built from the Region name or the request. An
//!   identifier EC2 reports no subnet for resolves to nothing, and that is the only case a
//!   request is refused for.
//! * A hop that could NOT be made (no EC2 in this process, an answer that could not be read)
//!   is evidence of nothing, and what a service does with it follows from what it needs the
//!   answer FOR:
//!   * A service that PLACES the resource in the subnet and reports that subnet's VPC and
//!     zone has no record to place it in or to report, so it resolves nothing and the create
//!     is refused — `fsx`'s reading. A deployment that mounts that service without EC2
//!     therefore serves no create at all, and says so at `warn` on every attempt, rather
//!     than storing a resource in a network nobody could confirm.
//!   * A service that only GATES on existence, keeping nothing of the subnet, refuses
//!     nothing on silence — `kafka`'s `src/state/network.rs`, which carries EC2's own
//!     `InvalidSubnetID.*` answer through and warns when it got no answer at all. A refusal
//!     there would be assumed rather than grounded, and would cost the create nothing to
//!     skip.
//!     Either way the refusal that IS served is the one EC2's answer grounds.
//!
//! `dax`'s `state.rs` still DERIVES a default network of its own (`region_subnets` /
//! `resolve_subnet` / `default_vpc_id`, from SHA-256 of the Region and zone names), which
//! EC2 holds no subnet for: it has its own open issue, and the fix for it is this contract —
//! resolve against EC2, in that service's own `resolve_subnet`. `elasticache` did the same
//! and now asks EC2 (`src/state/ec2_subnets.rs`).
//!
//! Nothing should invent a SECOND answer to "which subnets does the account have": ask the
//! service that owns them.
