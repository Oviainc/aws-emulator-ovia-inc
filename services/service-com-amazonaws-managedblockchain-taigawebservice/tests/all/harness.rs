// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness this target's suites share.
//!
//! The definitions live in [`crate::network_and_node_lifecycle`], the suite that grew
//! them first; this module re-exports them (and adds the engine-keeping router every
//! suite that reads the store needs), so a suite can reach the harness without importing
//! from another suite's module.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

pub(crate) use crate::network_and_node_lifecycle::{
    available_network, call, call_in_region, create_network, create_network_body, is_date_time,
    is_resource_id, member_configuration, new_router, router_over, Answer, ACCOUNT,
    PAST_TRANSITION, REGION,
};

/// A router and the store its handlers read and write — the SAME engine, so a suite can
/// reach the records the handlers answer from.
pub(crate) fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}
