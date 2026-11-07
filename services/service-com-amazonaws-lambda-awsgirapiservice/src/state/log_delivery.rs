// SPDX-License-Identifier: CC0-1.0

// Superseded: an invocation's log delivery to CloudWatch Logs now lives in
// `execution/log_delivery.rs` (`execution::log_target`), which landed on mainline while
// this change was in review and delivers the log of EVERY run path rather than only the
// synchronous one. Nothing declares this module, so it is not compiled; it is left empty
// rather than writing every log line twice.
