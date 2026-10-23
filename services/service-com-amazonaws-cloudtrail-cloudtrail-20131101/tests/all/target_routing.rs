// SPDX-License-Identifier: CC0-1.0

//! `X-Amz-Target` forms this service's own dispatch rule has to answer, for the
//! Lake query surface.
//!
//! `tests/all/target_prefix_dispatch.rs` owns the rule itself — the operation is
//! the segment after the LAST dot, over every prefix form, for the trail
//! lifecycle and for targets that name no operation of this service. These two
//! cases add only what it does not cover: the Lake query operation reached
//! through both published prefixes, and a target carrying NO prefix at all,
//! which names no service and so must not be resolved to an operation.

use serde_json::json;

use super::emulator::{Emulator, HOME_REGION, TARGET_PREFIX};

/// `DescribeQuery` answers its own modelled refusal — never the framework's
/// unmodelled `NotImplemented` — under the prefix an AWS SDK sends and under the
/// bare service shape name alike.
#[tokio::test]
async fn describe_query_dispatches_under_every_published_target_prefix() {
    let emulator = Emulator::new();

    for prefix in [TARGET_PREFIX, "CloudTrail_20131101."] {
        let target = format!("{prefix}DescribeQuery");
        let case = format!("target {target}");
        emulator
            .call_with_target(
                &target,
                HOME_REGION,
                &json!({ "QueryId": "0aaaaaaa-1111-2222-3333-444444444444" }),
            )
            .await
            .expect_error(&case, 400, "EventDataStoreNotFoundException");
    }
}

/// A target with no dot carries no service prefix, so it names no operation
/// either: the framework refuses it as a malformed header rather than this
/// service guessing an operation out of it.
#[tokio::test]
async fn a_target_with_no_prefix_names_no_operation() {
    let emulator = Emulator::new();
    let reply = emulator
        .call_with_target("DescribeQuery", HOME_REGION, &json!({}))
        .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "SerializationException"),
        "case target DescribeQuery: expected a malformed-target refusal, got {reply:?}"
    );
}
