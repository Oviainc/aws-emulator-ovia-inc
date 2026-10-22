// SPDX-License-Identifier: CC0-1.0

//! The CONTRACT the account's EC2 network is modelled against, for every service that
//! places a resource in a subnet. Documentation only: it exports nothing, because the
//! derivation itself still lives in the two service crates that model it (see below).
//!
//! Several AWS services take EC2 subnet identifiers on a create (`fsx`'s
//! `CreateFileSystem$SubnetIds`, `dax`'s `CreateSubnetGroup$SubnetIds`, and the other
//! VPC-placed resources) and each of them REFUSES a request naming a subnet the account
//! does not have. To refuse one, a service has to know which subnets the account HAS —
//! a fact of the account, not of any one service, so the services that model it must agree
//! about it exactly, or a client that places a DAX subnet group and an FSx file system in
//! "the same" subnet is talking to two different accounts.
//!
//! The agreement, which `fsx`'s and `dax`'s `state.rs` both implement today
//! (`region_subnets` / `resolve_subnet` / `default_vpc_id` in each, constant for
//! constant), and which is where the derivation itself belongs once one change may carry
//! both service crates' manifests:
//!
//! * AWS gives an account a default VPC in every Region, "with a size /16 IPv4 CIDR block
//!   and a default subnet in each Availability Zone" (EC2's `CreateDefaultVpc` /
//!   `CreateDefaultSubnet`). Those are facts of the account's Region rather than resources
//!   a request creates, so they are DERIVED from the Region name — stable, so every read
//!   reports the same network, and holding no state, so nothing has to be seeded or
//!   reset — exactly as the EC2 service's own state module derives a Region's
//!   Availability Zones instead of storing them.
//! * A Region's default subnets sit one per Availability Zone, in the zones named
//!   `<region>a`, `<region>b`, `<region>c`.
//! * Each identifier is derived from the name of what it names (the Region for the VPC,
//!   the zone for a subnet) through SHA-256, rendered as hex and cut to 8 characters for a
//!   VPC and 17 for a subnet, so it carries EC2's own id form and satisfies the
//!   `^(vpc-[0-9a-f]{8,})$` / `^(subnet-[0-9a-f]{8,})$` patterns (and the 12..21 / 15..24
//!   lengths) the service models constrain these members with.
//! * A subnet is the identifier, the VPC it belongs to and the Availability Zone it sits
//!   in; an identifier that is not one of the Region's names nothing at all — which is
//!   what lets a request naming it be refused instead of stored.
//! * EC2's own subnet operations still answer 501. When they are implemented, each
//!   service's `resolve_subnet` is the one place that has to start consulting the stored
//!   subnets beside the derived ones.
//!
//! Nothing new should invent a SECOND answer to "which subnets does the account have":
//! derive it exactly as this contract states.
