// SPDX-License-Identifier: CC0-1.0

//! The one assertion helper the suites of this test target share beyond the front door
//! itself.
//!
//! The wire plumbing — `service`, `call`, `ok`, `refused`, `text`, `digest_of` and the
//! registry constants — lives in `tests/all/repository_image_lifecycle.rs`, which every
//! suite here imports; this module adds only what a suite needs to read the MESSAGE of a
//! refusal, so no suite keeps a second copy of that reading.

use serde_json::Value;

/// The `message` an error body carries, whichever casing the envelope spells it with.
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
