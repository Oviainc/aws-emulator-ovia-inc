// SPDX-License-Identifier: CC0-1.0

//! `MergeShards`'s ADJACENCY rule, end to end over the wire: `CreateStream` ->
//! `MergeShards` -> `ListShards`.
//!
//! RULE: `MergeShards` is served for a pair of the stream's open shards exactly when the
//! union of their hash key ranges forms a contiguous set with no gaps — it then closes both
//! and lays down ONE open child covering their joined range, naming both as the shards it
//! was produced from — and every other pair of open shards is refused with
//! `InvalidArgumentException` (HTTP 400) whose message is the service's wording for an
//! ineligible pair: the two shard ids the request named, the stream it addressed and the
//! account that holds it, with the stored shard map left exactly as it was.
//!
//! The recorded flows this was derived from merge two adjacent shards of a four-shard
//! stream and then a pair two shards apart in the same map; each is one row of the table
//! below. The rows differ along what the rule generalises over: how many shards the stream
//! holds, which positions of the map the pair is taken from, how wide the gap between them
//! is (none, one shard, two, three), the order the request names the pair in, and a request
//! that names one shard twice — no range meets itself, so that pair is ineligible too.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.MergeShards` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::STREAM_MODE_PROVISIONED;

use crate::shard_scaling::{hash_key_range, is_open, parents, shard_id, shard_map};
use crate::stream_tags::{error_code, error_message, past_activation, Emulator};

/// The wording an ineligible pair of shards is refused with. The fixed text is the
/// service's own; its variable parts come from the request (the two shard ids and the
/// stream it addressed) and from the emulator's state (the account that holds the stream).
///
/// Written out here rather than taken from the implementation's own message helper, so a
/// case that asserts it would fail if the wording drifted.
pub(crate) fn not_an_adjacent_pair(
    shard_to_merge: &str,
    adjacent_shard_to_merge: &str,
    stream: &str,
) -> String {
    format!(
        "Shards {shard_to_merge} and {adjacent_shard_to_merge} in stream {stream} under \
         account {} are not an adjacent pair of shards eligible for merging",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One pair of open shards a row merges: the stream it is taken from, the two positions of
/// the map, and the order the request names them in.
#[derive(Debug, Clone, Copy)]
struct Pair {
    /// How many shards the stream is created with.
    shards: u32,
    /// The position in the open listing of the shard named as `ShardToMerge`.
    first: usize,
    /// The position in the open listing of the shard named as `AdjacentShardToMerge`.
    second: usize,
}

impl Pair {
    /// A freshly created provisioned stream tiles the whole hash key space onto contiguous
    /// slices in listing order, so two of its shards meet exactly when they sit side by
    /// side in that listing — and a shard never meets itself.
    fn adjacent(self) -> bool {
        self.first.abs_diff(self.second) == 1
    }

    /// The stream a row sets up for itself, named for the row so no two rows share state.
    fn stream(self, index: usize) -> String {
        format!("merge-adjacency-{index}-of-{}", self.shards)
    }
}

/// An eligible pair is merged and an ineligible one is refused with the service's wording,
/// over every gap a shard map can put between two shards.
#[tokio::test]
async fn merge_shards_serves_only_an_adjacent_pair() {
    let cases: Vec<Pair> = vec![
        // Side by side, so the union of their ranges has no gap: served.
        Pair { shards: 2, first: 0, second: 1 },
        Pair { shards: 3, first: 1, second: 2 },
        // Named in the other order: the pair is the same pair.
        Pair { shards: 4, first: 3, second: 2 },
        Pair { shards: 5, first: 1, second: 0 },
        // One shard between them, so their union has a gap: refused.
        Pair { shards: 3, first: 0, second: 2 },
        Pair { shards: 4, first: 1, second: 3 },
        // Two shards between them, and named in the other order.
        Pair { shards: 4, first: 0, second: 3 },
        Pair { shards: 5, first: 4, second: 1 },
        // Three shards between them: the widest gap a five-shard map has.
        Pair { shards: 5, first: 0, second: 4 },
        // One shard named twice: no range meets itself.
        Pair { shards: 2, first: 0, second: 0 },
        Pair { shards: 3, first: 2, second: 2 },
    ];

    let emulator = Emulator::new();
    // Every stream is created first and the create waited out ONCE: the rule is about the
    // merge call, not about how long a create takes.
    for (index, case) in cases.iter().enumerate() {
        let name = case.stream(index);
        emulator
            .ok(
                &name,
                "CreateStream",
                &json!({
                    "StreamName": name,
                    "ShardCount": case.shards,
                    "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
                }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for (index, case) in cases.iter().enumerate() {
        let name = case.stream(index);
        let before = shard_map(&emulator, &name, &name).await;
        let open: Vec<Value> = before
            .iter()
            .filter(|shard| is_open(&name, shard))
            .cloned()
            .collect();
        assert_eq!(
            open.len(),
            case.shards as usize,
            "case {case:?}: expected {} open shards to pick the pair from, got {}",
            case.shards,
            open.len()
        );

        let named = shard_id(&name, &open[case.first]);
        let adjacent = shard_id(&name, &open[case.second]);
        let (status, response) = emulator
            .call(
                "MergeShards",
                &json!({
                    "StreamName": name,
                    "ShardToMerge": named,
                    "AdjacentShardToMerge": adjacent,
                }),
            )
            .await;

        if !case.adjacent() {
            assert_eq!(
                status, 400,
                "case {case:?}: expected an ineligible pair to be refused, got {status} with \
                 body {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidArgumentException",
                "case {case:?}: expected InvalidArgumentException, got {response}"
            );
            let message = error_message(&response);
            assert_eq!(
                message,
                not_an_adjacent_pair(&named, &adjacent, &name),
                "case {case:?}: expected the service's wording for an ineligible pair, got \
                 {message:?}"
            );

            // Every refusal leaves the shard map exactly as it was.
            let after = shard_map(&emulator, &name, &name).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused merge must leave the shard map as it was, got \
                 {after:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected an adjacent pair to be merged, got {status} with body \
             {response}"
        );

        // Read the change back: both parents closed, and ONE open child covering their
        // joined range and naming both of them, lower parent first.
        let lower = case.first.min(case.second);
        let lower_id = shard_id(&name, &open[lower]);
        let upper_id = shard_id(&name, &open[case.first.max(case.second)]);
        let (starting, _) = hash_key_range(&name, &open[lower]);
        let (_, ending) = hash_key_range(&name, &open[case.first.max(case.second)]);

        let after = shard_map(&emulator, &name, &name).await;
        let closed: Vec<String> = after
            .iter()
            .filter(|shard| !is_open(&name, shard))
            .map(|shard| shard_id(&name, shard))
            .collect();
        assert_eq!(
            closed,
            vec![lower_id.clone(), upper_id.clone()],
            "case {case:?}: expected exactly the merged pair to be closed, got {closed:?}"
        );

        let children: Vec<&Value> = after
            .iter()
            .filter(|shard| {
                is_open(&name, shard) && parents(shard) == vec![lower_id.clone(), upper_id.clone()]
            })
            .collect();
        assert_eq!(
            children.len(),
            1,
            "case {case:?}: expected one child of {lower_id} and {upper_id}, got {children:?}"
        );
        let covered = hash_key_range(&name, children[0]);
        assert_eq!(
            covered,
            (starting, ending),
            "case {case:?}: expected the child to cover {starting}..{ending}, got {covered:?}"
        );
    }
}
