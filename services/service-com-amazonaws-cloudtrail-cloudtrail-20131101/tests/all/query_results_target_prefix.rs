// SPDX-License-Identifier: CC0-1.0

//! `GetQueryResults` as a client reaches it: under the `X-Amz-Target` prefix forms this
//! service answers, and for every `QueryId` that addresses no query.
//!
//! The prefix rule itself — the operation is the segment after the target's LAST dot —
//! is asserted over the trail lifecycle in `target_prefix_dispatch.rs`; what this suite
//! pins is that THIS Lake read is reached under either spelling and answers the same
//! modelled refusal, HTTP 400 `QueryIdNotFoundException`, rather than the framework's
//! unmodelled 501.

use serde_json::json;

use super::emulator::{Emulator, HOME_REGION};

/// The `X-Amz-Target` prefix forms this service answers, each `(case, prefix)`: the
/// fully qualified prefix an AWS SDK sends, and the bare service shape name from the
/// model's service shape id (`com.amazonaws.cloudtrail#CloudTrail_20131101`).
const TARGET_PREFIXES: [(&str, &str); 2] = [
    (
        "the fully qualified prefix an AWS SDK sends",
        "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.",
    ),
    ("the bare service shape name", "CloudTrail_20131101."),
];

/// Query ids of the modelled `UUID` form (36 characters of `[a-f0-9-]`) that address no
/// stored query, so every one of them is refused the same way.
const UNADDRESSED_QUERY_IDS: [&str; 3] = [
    "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
];

/// Under either target-prefix form, a `QueryId` that addresses no query is refused with
/// the modelled `QueryIdNotFoundException` and the HTTP 400 CloudTrail answers it with
/// — never the unmodelled 501 of a request that named no operation.
#[tokio::test]
async fn an_unaddressed_query_id_is_refused_the_same_way_under_every_target_prefix() {
    let emulator = Emulator::new();

    for (form, prefix) in TARGET_PREFIXES {
        for query_id in UNADDRESSED_QUERY_IDS {
            let case = format!("{form}, QueryId {query_id}");
            let reply = emulator
                .call_with_target(
                    &format!("{prefix}GetQueryResults"),
                    HOME_REGION,
                    &json!({"QueryId": query_id}),
                )
                .await;
            reply.expect_error(&case, 400, "QueryIdNotFoundException");
            assert_eq!(
                reply.body["Message"],
                json!(format!(
                    "The QueryId value {query_id} does not exist or is not in the same region."
                )),
                "case {case}: the refusal must name the id the request used, got {reply:?}"
            );
        }
    }
}
