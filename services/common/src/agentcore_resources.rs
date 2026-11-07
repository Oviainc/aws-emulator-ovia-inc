// SPDX-License-Identifier: CC0-1.0

//! The AGENTCORE RESOURCE INVENTORY: which AgentCore Memory containers and which code
//! interpreters the account holds, so the AgentCore DATA plane can tell a resource this
//! account created from one that was never created at all.
//!
//! A Memory container is created by the AgentCore CONTROL plane (`CreateMemory`) and a code
//! interpreter by the same plane (`CreateCodeInterpreter`, `DeleteCodeInterpreter`); the
//! AgentCore DATA plane — which writes the events, the ingested content and the long-term
//! records INSIDE a memory container, and starts the sessions a code interpreter runs code
//! in — creates neither, and binds no call that does. So from its own records it cannot
//! know whether an identifier a request names belongs to anything: every write it serves
//! names its parent in the request path and nothing else.
//!
//! This is that record: ONE inventory in the shared engine, written by the control plane as
//! it creates and deletes those resources, read by the data plane before it writes anything
//! under one. Nothing here names a wire shape — the inventory holds identifiers — and
//! nothing here decides what a reader does with the answer: WHICH error a data-plane
//! request gets for an identifier the inventory does not hold is that service's own.
//!
//! An identifier the inventory does not hold is a FACT and not a guess: these resources
//! exist only because a create minted them, so a reader refuses an identifier with no
//! registration rather than treating it as undecided. The one exception is the code
//! interpreter the service itself provides to every account
//! ([`SYSTEM_CODE_INTERPRETER_ID`]), which no call creates and none deletes, so the
//! inventory answers for it without a registration.
//!
//! Both halves are BOUNDED — see [`MAX_RECORDED_MEMORIES`] and
//! [`MAX_RECORDED_CODE_INTERPRETERS`] — so nothing a caller loops on can grow the inventory
//! without end; the oldest registration is dropped first, which keeps the resource a live
//! workflow just created resolvable.
//!
//! LOCK ORDER: a service takes its OWN store first and this inventory second, and nothing
//! holds this inventory while taking another store — so two services cannot invert the
//! order between them. A reader that needs the inventory BEFORE it reaches its own store
//! takes the answer it needs and releases this one first.

use std::collections::{HashSet, VecDeque};

/// How many memory containers the inventory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this inventory lives in memory for the lifetime
/// of the process, so without a bound a caller creating containers in a loop would grow it
/// without end. It sits far above the control plane's own per-account memory ceiling, so
/// while that stands no container that plane still holds can be evicted from here; past it
/// the OLDEST registration is dropped — never a newer one — so a container a running
/// workflow just created is always still resolvable.
pub const MAX_RECORDED_MEMORIES: usize = 1_000;

/// How many code interpreters the inventory holds at once, an EMULATOR ceiling for the same
/// reason and read the same way as [`MAX_RECORDED_MEMORIES`].
pub const MAX_RECORDED_CODE_INTERPRETERS: usize = 1_000;

/// The `codeInterpreterId` of the code interpreter the service provides to EVERY account:
/// the first alternative of the control plane's `CodeInterpreterId` pattern
/// `^(aws\.codeinterpreter\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$` (source:
/// `smithy_query(query="operation", operation="GetCodeInterpreter")` on the AgentCore
/// control plane's model), which no create can mint: a minted id is `{name}-{suffix}` and
/// `SandboxName` admits no `.`.
///
/// Nothing creates this tool and nothing deletes it, so the inventory holds it without a
/// registration: it is in no account's store and in every account's reach.
pub const SYSTEM_CODE_INTERPRETER_ID: &str = "aws.codeinterpreter.v1";

/// The ARN infix a `memoryId` carries in its ARN spelling:
/// `arn:<partition>:bedrock-agentcore:<region>:<account>:memory/<memoryId>`, the optional
/// first half of `MemoryId`'s pattern
/// `^(arn:(aws|aws-cn|aws-us-gov):bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`
/// (source: `smithy_query(query="operation", operation="CreateEvent")`).
const MEMORY_ARN_INFIX: &str = ":memory/";

/// The bare `memoryId` inside either spelling `MemoryId` admits: the id itself, or the
/// `arn:…:memory/<memoryId>` form, both of which address ONE container.
///
/// The ONE reading of a `memoryId`, so the plane that registers a container and the plane
/// that resolves one cannot key it two different ways.
#[must_use]
pub fn memory_id(memory_id_or_arn: &str) -> &str {
    match memory_id_or_arn.split_once(MEMORY_ARN_INFIX) {
        Some((prefix, id)) if prefix.starts_with("arn:") => id,
        _ => memory_id_or_arn,
    }
}

/// What the AgentCore control plane created, as the data plane resolves it.
///
/// ONE inventory in the shared engine (the engine keys state by TYPE, so a copy inside
/// either service's own state struct would be invisible to the other).
#[derive(Debug, Default)]
pub struct AgentCoreResourceInventory {
    /// The memory containers the account holds, keyed by the bare `memoryId`
    /// ([`memory_id`]). Bounded by [`MAX_RECORDED_MEMORIES`].
    memories: HeldIds,
    /// The code interpreters the account CREATED, keyed by `codeInterpreterId`. The tool the
    /// service provides is not among them: nothing creates it, and
    /// [`Self::holds_code_interpreter`] answers for it on its own. Bounded by
    /// [`MAX_RECORDED_CODE_INTERPRETERS`].
    code_interpreters: HeldIds,
}

impl AgentCoreResourceInventory {
    /// Record that the account holds the memory container `memory_id_or_arn`, in either
    /// spelling `MemoryId` admits.
    ///
    /// Idempotent: a re-registration of a container already held changes nothing, so it
    /// cannot age another one out.
    pub fn register_memory(&mut self, memory_id_or_arn: &str) {
        self.memories
            .register(memory_id(memory_id_or_arn), MAX_RECORDED_MEMORIES);
    }

    /// Record that the account no longer holds that memory container.
    ///
    /// A no-op for a container the inventory holds no record of, so a repeated delete is
    /// harmless.
    pub fn forget_memory(&mut self, memory_id_or_arn: &str) {
        self.memories.forget(memory_id(memory_id_or_arn));
    }

    /// Whether the account holds the memory container `memory_id_or_arn`, whichever
    /// spelling the request carried.
    #[must_use]
    pub fn holds_memory(&self, memory_id_or_arn: &str) -> bool {
        self.memories.holds(memory_id(memory_id_or_arn))
    }

    /// How many memory containers the inventory holds.
    #[must_use]
    pub fn memories(&self) -> usize {
        self.memories.len()
    }

    /// Record that the account created the code interpreter `code_interpreter_id`.
    ///
    /// Idempotent in the same way [`Self::register_memory`] is.
    pub fn register_code_interpreter(&mut self, code_interpreter_id: &str) {
        self.code_interpreters
            .register(code_interpreter_id, MAX_RECORDED_CODE_INTERPRETERS);
    }

    /// Record that the code interpreter `code_interpreter_id` is gone: the account can no
    /// longer run code in it, so nothing may start a session on it again.
    ///
    /// A no-op for an id the inventory holds no record of, and for the tool the service
    /// provides, which no account's delete addresses.
    pub fn forget_code_interpreter(&mut self, code_interpreter_id: &str) {
        self.code_interpreters.forget(code_interpreter_id);
    }

    /// Whether `code_interpreter_id` names a code interpreter this account can reach: one it
    /// created and has not deleted, or the tool the service provides to every account
    /// ([`SYSTEM_CODE_INTERPRETER_ID`]).
    #[must_use]
    pub fn holds_code_interpreter(&self, code_interpreter_id: &str) -> bool {
        code_interpreter_id == SYSTEM_CODE_INTERPRETER_ID
            || self.code_interpreters.holds(code_interpreter_id)
    }

    /// How many code interpreters the account CREATED, which does not count the tool the
    /// service provides.
    #[must_use]
    pub fn code_interpreters(&self) -> usize {
        self.code_interpreters.len()
    }
}

/// A set of identifiers in registration order, bounded by a ceiling its holder states: the
/// identifier registered EARLIEST gives way first, so the ones a live workflow just created
/// stay resolvable.
#[derive(Debug, Default)]
struct HeldIds {
    /// Every identifier held, for the one question a reader asks.
    held: HashSet<String>,
    /// The same identifiers in registration order, which is the order they are dropped in.
    order: VecDeque<String>,
}

impl HeldIds {
    /// Hold `id`, dropping the earliest registration once the set passes `limit`.
    fn register(&mut self, id: &str, limit: usize) {
        if !self.held.insert(id.to_owned()) {
            // Already held: a re-registration is not a new entry, so it moves nothing out.
            return;
        }
        self.order.push_back(id.to_owned());
        while self.held.len() > limit {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.held.remove(&oldest);
        }
    }

    /// Stop holding `id`, if it was held at all.
    fn forget(&mut self, id: &str) {
        if self.held.remove(id) {
            self.order.retain(|recorded| recorded != id);
        }
    }

    fn holds(&self, id: &str) -> bool {
        self.held.contains(id)
    }

    fn len(&self) -> usize {
        self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Either spelling of a `memoryId` names the same container, and nothing else is
    /// treated as an ARN.
    #[test]
    fn a_memory_id_reads_the_same_in_both_spellings() {
        // (case, the spelling a request carries, the bare id inside it)
        let rows: [(&str, &str, &str); 7] = [
            ("the bare id", "demo-memory-0123456789", "demo-memory-0123456789"),
            (
                "the ARN spelling",
                "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/demo-memory-0123456789",
                "demo-memory-0123456789",
            ),
            (
                "the ARN spelling in another partition and region",
                "arn:aws-us-gov:bedrock-agentcore:us-gov-west-1:000000000000:memory/other-memory-abcdefghij",
                "other-memory-abcdefghij",
            ),
            (
                "an ARN of another resource type",
                "arn:aws:bedrock-agentcore:us-east-1:123456789012:code-interpreter/ci-1a2b3c4d5e",
                "arn:aws:bedrock-agentcore:us-east-1:123456789012:code-interpreter/ci-1a2b3c4d5e",
            ),
            (
                "a value that merely carries the infix",
                "not-an-arn:memory/demo-memory-0123456789",
                "not-an-arn:memory/demo-memory-0123456789",
            ),
            ("an empty value", "", ""),
            (
                "an id that merely starts like an ARN",
                "arnold-memory-0123456789",
                "arnold-memory-0123456789",
            ),
        ];
        for (case, spelling, expected) in rows {
            let actual = memory_id(spelling);
            assert_eq!(
                actual, expected,
                "case {case:?} ({spelling}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A registered container resolves in either spelling; one never registered, and one
    /// forgotten, resolve in neither.
    #[test]
    fn a_memory_resolves_only_while_it_is_held() {
        const ARN: &str =
            "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/demo-memory-0123456789";
        let mut inventory = AgentCoreResourceInventory::default();
        inventory.register_memory("demo-memory-0123456789");

        // (case, the spelling a request carries, whether it resolves)
        let rows: [(&str, &str, bool); 4] = [
            ("the bare id", "demo-memory-0123456789", true),
            ("the ARN spelling of the same container", ARN, true),
            ("a container never created", "ghost-memory-9876543210", false),
            (
                "the ARN spelling of one never created",
                "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/ghost-memory-9876543210",
                false,
            ),
        ];
        for (case, spelling, expected) in rows {
            let actual = inventory.holds_memory(spelling);
            assert_eq!(
                actual, expected,
                "case {case:?} ({spelling}): expected holds={expected}, got {actual}"
            );
        }

        // Registering the ARN spelling of a held container is the same container.
        inventory.register_memory(ARN);
        assert_eq!(
            inventory.memories(),
            1,
            "case the ARN spelling re-registered: expected one container, got {}",
            inventory.memories()
        );

        // Forgetting by the ARN spelling forgets the container the bare id registered,
        // and forgetting it twice is harmless.
        inventory.forget_memory(ARN);
        inventory.forget_memory("demo-memory-0123456789");
        assert!(
            !inventory.holds_memory("demo-memory-0123456789") && inventory.memories() == 0,
            "case a forgotten container: expected it gone, got {} container(s)",
            inventory.memories()
        );
    }

    /// A code interpreter resolves while it is held; the tool the service provides resolves
    /// without ever being registered, and cannot be forgotten.
    #[test]
    fn a_code_interpreter_resolves_while_held_and_the_service_tool_always_does() {
        let mut inventory = AgentCoreResourceInventory::default();
        inventory.register_code_interpreter("demo_interpreter-1a2b3c4d5e");

        // (case, the identifier a request carries, whether it resolves)
        let rows: [(&str, &str, bool); 4] = [
            ("a created interpreter", "demo_interpreter-1a2b3c4d5e", true),
            ("the tool the service provides", SYSTEM_CODE_INTERPRETER_ID, true),
            ("an interpreter never created", "other_interpreter-9z8y7x6w5v", false),
            ("a later version of the service tool", "aws.codeinterpreter.v2", false),
        ];
        for (case, identifier, expected) in rows {
            let actual = inventory.holds_code_interpreter(identifier);
            assert_eq!(
                actual, expected,
                "case {case:?} ({identifier}): expected holds={expected}, got {actual}"
            );
        }

        inventory.forget_code_interpreter("demo_interpreter-1a2b3c4d5e");
        inventory.forget_code_interpreter(SYSTEM_CODE_INTERPRETER_ID);
        assert!(
            !inventory.holds_code_interpreter("demo_interpreter-1a2b3c4d5e")
                && inventory.holds_code_interpreter(SYSTEM_CODE_INTERPRETER_ID),
            "case the deleted interpreter: expected only the created one to go"
        );
        assert_eq!(
            inventory.code_interpreters(),
            0,
            "case the deleted interpreter: expected no created interpreter left, got {}",
            inventory.code_interpreters()
        );
    }

    /// Each ceiling drops the EARLIEST registration and only it, and a re-registration at
    /// the ceiling moves nothing out.
    #[test]
    fn each_ceiling_drops_the_earliest_registration_first() {
        let mut inventory = AgentCoreResourceInventory::default();
        for index in 0..MAX_RECORDED_MEMORIES {
            inventory.register_memory(&format!("mem{index}-0123456789"));
        }
        inventory.register_memory("mem0-0123456789");
        assert!(
            inventory.holds_memory("mem0-0123456789")
                && inventory.memories() == MAX_RECORDED_MEMORIES,
            "case a re-registration at the ceiling: expected the record kept, got {} \
             container(s)",
            inventory.memories()
        );

        inventory.register_memory("one-more-0123456789");
        assert!(
            inventory.memories() == MAX_RECORDED_MEMORIES
                && !inventory.holds_memory("mem0-0123456789")
                && inventory.holds_memory("one-more-0123456789"),
            "case one past the ceiling: expected the earliest dropped and the newest held, \
             got {} container(s)",
            inventory.memories()
        );

        for index in 0..MAX_RECORDED_CODE_INTERPRETERS {
            inventory.register_code_interpreter(&format!("ci{index}-1a2b3c4d5e"));
        }
        inventory.register_code_interpreter("one_more-1a2b3c4d5e");
        assert!(
            inventory.code_interpreters() == MAX_RECORDED_CODE_INTERPRETERS
                && !inventory.holds_code_interpreter("ci0-1a2b3c4d5e")
                && inventory.holds_code_interpreter("one_more-1a2b3c4d5e"),
            "case one interpreter past the ceiling: expected the earliest dropped, got {}",
            inventory.code_interpreters()
        );
    }
}
