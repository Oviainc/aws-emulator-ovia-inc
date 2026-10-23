// SPDX-License-Identifier: CC0-1.0

//! No type definitions: `ListAvailableSolutionStacks` declares its input, its
//! output and `SolutionStackDescription` in its own handler file
//! (`src/ops/list_available_solution_stacks.rs`), because it is the only operation
//! in this service that reaches those shapes. This module is kept as the place
//! that says so — declaring the shapes a second time here would declare one
//! modelled shape twice, which `types/mod.rs` exists to prevent.
