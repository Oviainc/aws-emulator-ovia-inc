// SPDX-License-Identifier: CC0-1.0

//! The schedule a STORED rotation describes, as the shared shift computation reads it.
//!
//! [`super::rotation_shifts`] computes shifts from a `ShiftSchedule` and the overrides whoever
//! asks hands it: `ListPreviewRotationShifts` builds both from its request, and a listing of a
//! stored rotation's shifts builds them from the [`RotationRecord`] and the
//! [`RotationOverrideRecord`]s stored against it. THAT reading is this module — one place where
//! a stored rotation becomes the computation's input, so no handler reads a rotation's own
//! fields its own way.
//!
//! What it carries is OWNED, unlike the borrowed `ShiftSchedule` the computation takes, because
//! a handler reads the store under the state lock and computes after releasing it: the snapshot
//! has to outlive the guard.

use ::chrono_tz::Tz;

use crate::ops::types::RecurrenceSettings;

use super::rotation_shifts::{ShiftOverride, ShiftSchedule};
use super::{resolve_time_zone_id, RotationOverrideRecord, RotationRecord};

/// What a stored rotation's shifts are computed from: its team in shift order, its recurrence,
/// the resolved zone its local hand-off times are read in, and the instant it goes into effect.
#[derive(Debug, Clone)]
pub struct StoredSchedule {
    members: Vec<String>,
    recurrence: RecurrenceSettings,
    time_zone: Tz,
    rotation_start: i64,
}

impl StoredSchedule {
    /// The schedule `record` describes, or the message saying why it describes none — which is
    /// only ever a `TimeZoneId` no zone database resolves, refused by every rotation write
    /// ([`super::check_time_zone_id`]), so a reader that gets `Err` is looking at a record this
    /// service could not have stored.
    pub fn of(record: &RotationRecord) -> Result<Self, String> {
        Ok(Self {
            members: record.contact_ids.clone(),
            recurrence: record.recurrence.clone(),
            time_zone: resolve_time_zone_id(&record.time_zone_id)?,
            // The effective start `CreateRotation` assigned and persisted: "the first shift is
            // calculated from this date and time".
            rotation_start: record.start_time.secs(),
        })
    }

    /// This snapshot as the computation's own input.
    #[must_use]
    pub fn schedule(&self) -> ShiftSchedule<'_> {
        ShiftSchedule {
            members: &self.members,
            recurrence: &self.recurrence,
            time_zone: self.time_zone,
            rotation_start: self.rotation_start,
        }
    }

    /// The instant the rotation goes into effect, which is where a range that names no lower
    /// bound begins: no shift precedes it.
    #[must_use]
    pub fn rotation_start(&self) -> i64 {
        self.rotation_start
    }
}

/// The overrides stored against a rotation, as the computation reads them: a window with both
/// ends named, since `CreateRotationOverride` persists both and refuses a window whose end does
/// not follow its start.
#[must_use]
pub fn stored_overrides(records: &[&RotationOverrideRecord]) -> Vec<ShiftOverride> {
    records
        .iter()
        .map(|record| ShiftOverride {
            new_members: record.new_contact_ids.clone(),
            start: Some(record.start_time.secs()),
            end: Some(record.end_time.secs()),
        })
        .collect()
}
