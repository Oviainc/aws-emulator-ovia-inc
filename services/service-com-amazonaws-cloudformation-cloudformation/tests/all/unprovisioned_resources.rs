// SPDX-License-Identifier: CC0-1.0

//! What a stack reports for the resources its template declares: the rule lives
//! in `stack_outputs.rs`, whose table this file's rows were folded into.
//!
//! The EventBridge Scheduler fix added a row there for the resource types its
//! issue's templates declare — `AWS::Scheduler::ScheduleGroup` and
//! `AWS::Scheduler::Schedule`, neither of which the return-value catalogue
//! carries — rather than keeping a second copy of that suite's harness here.
//! This module is deliberately empty: one table for the rule beats two.
