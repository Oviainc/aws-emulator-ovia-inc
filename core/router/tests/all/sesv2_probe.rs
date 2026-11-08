// SPDX-License-Identifier: CC0-1.0

//! EMPTY — this file carries no test. It is declared in `tests/all.rs` because the layout
//! check in `vendor/build-support` refuses an undeclared file under `tests/all/`, and this
//! fixer's tool set can write a file but not remove one. The next change to touch this
//! directory should delete it and its declaration.
//!
//! It held a scratch probe used while tracing the CloudFormation-deployed configuration-set
//! issue. What it established is asserted by two suites of this crate instead:
//! `all/cloudformation_deployed_configuration_set_reaches_the_email_api.rs` and
//! `all/ses_configuration_sets_shared_across_v1_and_v2.rs`.
