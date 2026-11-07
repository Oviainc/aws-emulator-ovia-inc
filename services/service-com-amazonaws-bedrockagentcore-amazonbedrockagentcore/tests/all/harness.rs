// SPDX-License-Identifier: CC0-1.0

//! What every suite in this crate's test target needs, in ONE place.
//!
//! A suite reaches it as `use super::harness::label;` — `tests/all.rs` is the test
//! target's crate root, so each suite is a sibling module of this one.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use service_common::agentcore_resources::AgentCoreResourceInventory;

/// Everything outside RFC 3986's unreserved set (`ALPHA / DIGIT / "-" / "." / "_" /
/// "~"`), which is exactly what an AWS SDK escapes in a URI path segment or query value.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encode one `@httpLabel` / `@httpQuery` value the way an AWS SDK does, so an
/// `eventId`'s `#`, a memory ARN's `:` and `/`, and any other punctuation reach the
/// server as part of the value rather than as URI syntax.
pub(crate) fn label(value: &str) -> String {
    utf8_percent_encode(value, UNRESERVED).to_string()
}

/// Create an AgentCore Memory container the way the CONTROL plane's `CreateMemory` does:
/// by registering it in the ONE shared inventory in the engine, which is where this data
/// plane — whose own API creates no container — resolves the parent of a write against.
pub(crate) fn create_memory(engine: &engine::InMemoryEngine, memory_id: &str) {
    engine
        .state::<AgentCoreResourceInventory>()
        .lock()
        .register_memory(memory_id);
}

/// Create a code interpreter the way the CONTROL plane's `CreateCodeInterpreter` does,
/// in the same inventory and for the same reason. The interpreter the service provides to
/// every account needs no such call: the inventory holds it without a registration.
pub(crate) fn create_code_interpreter(engine: &engine::InMemoryEngine, code_interpreter_id: &str) {
    engine
        .state::<AgentCoreResourceInventory>()
        .lock()
        .register_code_interpreter(code_interpreter_id);
}

/// Delete a code interpreter the way the CONTROL plane's `DeleteCodeInterpreter` does: the
/// account can no longer run code in it, so nothing may start a session on it again.
pub(crate) fn delete_code_interpreter(engine: &engine::InMemoryEngine, code_interpreter_id: &str) {
    engine
        .state::<AgentCoreResourceInventory>()
        .lock()
        .forget_code_interpreter(code_interpreter_id);
}
