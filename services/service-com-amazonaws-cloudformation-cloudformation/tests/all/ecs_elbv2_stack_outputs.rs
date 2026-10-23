// SPDX-License-Identifier: CC0-1.0

//! MOVED — this file is intentionally empty and is not declared in `tests/all.rs`, so
//! nothing here compiles or runs.
//!
//! Its rows now live in the sibling suite `all/nested_resource_outputs.rs`, under the name
//! that says what they pin: the stack outputs whose value is built from ANOTHER resource's
//! identity (an Amazon ECS service's ARN carries its cluster; an Elastic Load Balancing
//! listener's and rule's nest inside their parent's). The rows this file used to carry for
//! the types whose identifiers are functions of their own name alone — a cluster, a task
//! definition, a load balancer and a target group — are `all/provisioned_resources.rs`'s,
//! which already owns that harness; keeping a second copy would let the two drift.
//!
//! The content is gone rather than the file because this fixer's tool set can write a file
//! but not remove one; the next change to touch this directory should delete it.
