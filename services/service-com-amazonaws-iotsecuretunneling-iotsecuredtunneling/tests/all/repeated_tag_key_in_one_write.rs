// SPDX-License-Identifier: CC0-1.0

//! What a tag write does when its own request names the SAME key more than once.
//!
//! RULE: a tag write stores, for every key its request names, the value of that
//! key's LAST occurrence in the request, and it is an update of the tunnel — the
//! record's `lastUpdatedAt`, "The last time the tunnel was updated" (the modelled
//! `Tunnel$lastUpdatedAt` documentation) — exactly when the set the resource
//! carries after the call differs from the set it carried BEFORE the call, never
//! from a value another element of the same call wrote. So a request whose net
//! effect on the stored set is the identity stamps nothing however many times it
//! names a key, and one whose last occurrence differs from the stored value
//! stamps even when an earlier occurrence matched it.
//!
//! Rows differ along what the rule generalises over: how many times one key is
//! repeated (twice, three times, nine times), how many keys are repeated, where
//! in the list the occurrence that wins sits, whether the net effect is a change
//! or the identity, whether the repeats come with a genuinely new key, which
//! operation writes (`TagResource`, `UntagResource`), whether the repeat is
//! already in the tunnel's opening `tags`, and the tunnel's status, since a
//! closed tunnel is still a tagged resource. `TagValue` is
//! `length(min: 0, max: 256)` — from
//! `smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#TagValue")`
//! — so an empty value is a value like any other and two rows write one.
//!
//! Whether the stamp must move is not a column: it is derived from whether the
//! set the tunnel reports changed, which is the rule itself.
//!
//! The harness and the assertions are the ones [`super::tag_write_stamps`] owns:
//! these rows are driven through its `check_case`, so they are held to the rule by
//! exactly the code that holds the single-occurrence rows to it.

use std::sync::Arc;

use super::tag_write_stamps::{check_case, mount, Case, Status, Write};

/// The rows: one per shape of repeated-key write.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one key twice, the last occurrence restoring the stored value",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "prod"), ("env", "test")]),
            expected: &[("env", "test")],
        },
        Case {
            label: "one key twice, the last occurrence differing from the stored value",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "test"), ("env", "prod")]),
            expected: &[("env", "prod")],
        },
        Case {
            label: "one key three times, the last occurrence restoring the stored value",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "a"), ("env", "b"), ("env", "test")]),
            expected: &[("env", "test")],
        },
        Case {
            label: "one key nine times, the last occurrence restoring the stored value",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[
                ("env", "v1"),
                ("env", "v2"),
                ("env", "v3"),
                ("env", "v4"),
                ("env", "v5"),
                ("env", "v6"),
                ("env", "v7"),
                ("env", "v8"),
                ("env", "test"),
            ]),
            expected: &[("env", "test")],
        },
        Case {
            label: "one key three times on a tunnel carrying none",
            initial: &[],
            status: Status::Open,
            write: Write::Tag(&[("env", "a"), ("env", "b"), ("env", "c")]),
            expected: &[("env", "c")],
        },
        Case {
            label: "two keys each named twice, both last occurrences restoring stored values",
            initial: &[("env", "test"), ("owner", "team")],
            status: Status::Open,
            write: Write::Tag(&[
                ("env", "x"),
                ("owner", "y"),
                ("owner", "team"),
                ("env", "test"),
            ]),
            expected: &[("env", "test"), ("owner", "team")],
        },
        Case {
            label: "two keys each named twice, one last occurrence changing its value",
            initial: &[("env", "test"), ("owner", "team")],
            status: Status::Open,
            write: Write::Tag(&[
                ("env", "x"),
                ("env", "test"),
                ("owner", "y"),
                ("owner", "other-team"),
            ]),
            expected: &[("env", "test"), ("owner", "other-team")],
        },
        Case {
            label: "a repeated key restoring its value beside a genuinely new key",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "prod"), ("env", "test"), ("stage", "beta")]),
            expected: &[("env", "test"), ("stage", "beta")],
        },
        Case {
            label: "a repeated key whose last occurrence is the empty value",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "test"), ("env", "")]),
            expected: &[("env", "")],
        },
        Case {
            label: "a repeated key whose last occurrence restores a stored empty value",
            initial: &[("env", "")],
            status: Status::Open,
            write: Write::Tag(&[("env", "x"), ("env", "")]),
            expected: &[("env", "")],
        },
        Case {
            label: "a key the opening request itself named twice, rewritten identically",
            initial: &[("env", "a"), ("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "test"), ("env", "test")]),
            expected: &[("env", "test")],
        },
        Case {
            label: "one key twice on a CLOSED tunnel, the last occurrence restoring the value",
            initial: &[("env", "test")],
            status: Status::Closed,
            write: Write::Tag(&[("env", "prod"), ("env", "test")]),
            expected: &[("env", "test")],
        },
        Case {
            label: "one key twice on a CLOSED tunnel, the last occurrence changing the value",
            initial: &[("env", "test")],
            status: Status::Closed,
            write: Write::Tag(&[("env", "x"), ("env", "prod")]),
            expected: &[("env", "prod")],
        },
        Case {
            label: "a stored key named twice in one removal",
            initial: &[("env", "test"), ("owner", "team")],
            status: Status::Open,
            write: Write::Untag(&["env", "env"]),
            expected: &[("owner", "team")],
        },
        Case {
            label: "a key the tunnel never carried named twice in one removal",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Untag(&["never-set", "never-set"]),
            expected: &[("env", "test")],
        },
    ]
}

#[tokio::test]
async fn a_write_naming_one_key_twice_stores_the_last_value_and_stamps_only_a_real_change() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for case in cases() {
        check_case(&router, &engine, &case).await;
    }
}
