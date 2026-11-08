// SPDX-License-Identifier: CC0-1.0

//! What a READ of a metric alarm reports, in one place: the identity of the write that created
//! the alarm, and the metric identity it was configured with.
//!
//! Two rules live here, and every reader of a metric alarm goes through them so two readers of
//! the same alarm cannot answer differently:
//!
//! * an alarm carries the `CreationId` of the write that CREATED it
//!   ([`alarm_creation_id`]) — a uuid naming that write and the epoch milliseconds it happened
//!   at. A write that replaces the configuration of an alarm that already exists keeps the id
//!   the alarm was created with, exactly as it keeps the alarm's state ("When you update an
//!   existing alarm, its state is left unchanged, but the update completely overwrites the
//!   previous configuration of the alarm.", `PutMetricAlarm`), so an alarm that was deleted and
//!   written again is the one case that reports a new id — which is what lets a client that
//!   diffs alarm configuration tell a re-create from an update.
//! * the metric identity a read reports is the one the configuration named, in full: every
//!   `Dimensions` list on it is present, and names no dimension where the configuration named
//!   none ([`super::configured_list`], [`configured_queries`]) — the alarm's own list and the
//!   one on each metric of its `Metrics` array alike, so a caller iterating the dimensions of
//!   either never meets an absent list, and a `DescribeAlarms` result can be fed straight back
//!   into `PutMetricAlarm`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{configured_list, StoredAlarm};
use crate::ops::types::{Metric, MetricDataQuery, MetricStat, ReportedMetricAlarm};

impl StoredAlarm {
    /// This alarm as a read of it reports it: the modelled shape plus the `CreationId` of the
    /// write that created it.
    pub fn reported(&self) -> ReportedMetricAlarm {
        ReportedMetricAlarm {
            alarm: self.alarm.clone(),
            creation_id: Some(self.creation_id.clone()),
        }
    }
}

/// The `CreationId` a write that CREATES a metric alarm mints for it: `<uuid>/<epoch-millis>`
/// — a uuid identifying the write, and the instant it happened at in milliseconds.
///
/// The uuid is minted from this process's own opaque bytes ([`minted_bytes`]) and rendered by
/// the workspace's ONE rendering of 16 opaque bytes as a version-4 uuid, so neither the version
/// and variant bits nor the hyphenated layout is spelled out here. Nothing in it comes from the
/// request: two alarms created with identical configuration carry different creation
/// identities, and re-creating one alarm never reproduces the id it had.
pub fn alarm_creation_id(now_secs: f64) -> String {
    format!(
        "{}/{}",
        ::service_common::uuid_from_random_bytes(minted_bytes()),
        epoch_millis(now_secs)
    )
}

/// `secs` as whole epoch milliseconds, the unit a creation identity names its instant in.
///
/// Through [`super::to_timestamp`], the one conversion of an instant of this store to the
/// millisecond form, so the bound on how large an instant that form holds lives in one place —
/// and the milliseconds a creation identity names are exactly the ones the alarm's own time
/// stamps carry. An instant that form cannot hold — none this emulator's clock reaches — names
/// the epoch rather than wrapping to some other instant.
fn epoch_millis(secs: f64) -> i64 {
    super::to_timestamp(secs).to_millis().unwrap_or_default()
}

/// Unique, opaque bytes for one minted creation identity.
///
/// SHA-256 over three independent inputs: a process-wide monotonic counter (so two mints in the
/// same instant differ), the wall clock in nanoseconds (so two processes differ), and the
/// OS-seeded hash keys of a fresh [`std::collections::hash_map::RandomState`] (so the bytes are
/// not derivable from the clock). Nothing here comes from the request, so a creation identity
/// is never a function of client input.
///
/// This is adequate for an emulator's opaque values but it is not an OS CSPRNG: if
/// `getrandom`/`rand` is ever declared in the workspace manifest, mint from it and delete this
/// mixing.
fn minted_bytes() -> [u8; 16] {
    use ::sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    let mut keyed = std::collections::hash_map::RandomState::new().build_hasher();
    keyed.write_u64(seq);
    keyed.write_u128(nanos);

    let mut digest = Sha256::new();
    digest.update(seq.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(keyed.finish().to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    bytes
}

/// A `Metrics` query array as the stored alarm carries it, and so as every reader of that alarm
/// reports it: each query's metric identity complete, with the `Dimensions` list it names and
/// an EMPTY list where it names none.
///
/// The same rule [`super::configured_list`] states for a list member of a request, applied to
/// the metric nested inside each `MetricDataQuery`: a `Metric` reports the dimensions that
/// identify it, and "no dimension" is a list of none rather than no list — which is what lets a
/// caller feed a described alarm's `Metrics` array straight back into `PutMetricAlarm`. An
/// alarm configured with no array at all reports NO array: `Metrics` is the configuration's own
/// member, not a list every alarm has ("For each `PutMetricAlarm` operation, you must specify
/// either `MetricName`, a `Metrics` array, or an `EvaluationCriteria`",
/// `PutMetricAlarmInput$Metrics`).
pub fn configured_queries(queries: Option<&[MetricDataQuery]>) -> Option<Vec<MetricDataQuery>> {
    Some(
        queries?
            .iter()
            .map(|query| MetricDataQuery {
                metric_stat: query.metric_stat.as_ref().map(|stat| MetricStat {
                    metric: Metric {
                        dimensions: configured_list(stat.metric.dimensions.as_deref()),
                        ..stat.metric.clone()
                    },
                    ..stat.clone()
                }),
                ..query.clone()
            })
            .collect(),
    )
}
