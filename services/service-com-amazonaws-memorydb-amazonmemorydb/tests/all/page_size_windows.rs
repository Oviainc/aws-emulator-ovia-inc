// SPDX-License-Identifier: CC0-1.0

//! RETIRED. Every page-size rule this service follows is asserted in the ONE page-size
//! suite beside this file, `tests/all/page_size_window.rs`: which page sizes the window
//! serves and which it refuses, that a served one is applied to the page it answers, and
//! that a `NextToken` resumes the listing at any other served size.
//!
//! The file is kept, empty, only because this checkout's build refuses a suite under
//! `tests/all/` that nothing declares, and the agent tooling that wrote it cannot remove
//! a file. It holds no fixture and no test, so there is no second page-size harness to
//! keep in step with the first.
