// SPDX-License-Identifier: CC0-1.0

//! THE UPDATES MADE TO ONE DIRECTORY'S SETUP: what an update of a directory records, and
//! what `DescribeUpdateDirectory` reports about it.
//!
//! `DescribeUpdateDirectory` "describes the updates of a directory for a particular update
//! type", and its `UpdateActivities` member is "the list of update activities on a
//! directory for the requested update type" — a HISTORY of the updates that were asked
//! for, one entry per update per Region, not a property the directory carries. So an
//! activity is a record of its own, held on the directory it was made to
//! ([`super::Directory::update_activities`]) and taken with it when the directory is
//! deleted.
//!
//! Nothing here is invented at create time: a directory whose setup nobody has updated
//! holds no activity and is reported with an empty list, the same way a directory nobody
//! has configured LDAPS or client authentication for reports an empty settings list. The
//! operation that asks for such an update, `UpdateDirectorySetup`, is the one that appends
//! an entry here; it is still a scaffold stub, so no stored directory holds one yet.

use super::*;

/// One update of a directory's setup, as the modelled `UpdateInfoEntry` reports it.
///
/// The entry is the ACTIVITY and not the request: it carries the Region the update ran in,
/// the status it reached, who asked for it, and the value the update moved the setting from
/// and to — which is why the previous value is kept beside the new one rather than being
/// recomputed from the directory's current setup.
#[derive(Debug, Clone)]
pub struct UpdateActivityRecord {
    /// Which of the modelled `UpdateType` values this activity is an update of
    /// (`OS`, `NETWORK` or `SIZE`). `DescribeUpdateDirectory` reports the activities of one
    /// type, so this is what its request filters on; the member itself is not part of
    /// `UpdateInfoEntry`.
    pub update_type: String,
    /// The Region this update ran in, as the modelled `UpdateInfoEntry$Region` reports it.
    /// A directory replicated to several Regions is updated in each of them, so a request
    /// may narrow its reads to one.
    pub region: String,
    /// The status this update reached: one of the modelled `UpdateStatus` values
    /// ([`crate::ops::types::UPDATE_STATUS_VALUES`]).
    pub status: String,
    /// Why the update is in that status, when something other than the update itself
    /// decided it; `None` when nothing has to be explained.
    pub status_reason: Option<String>,
    /// Who asked for the update, as the modelled `UpdateInfoEntry$InitiatedBy` reports it.
    pub initiated_by: Option<String>,
    /// The setup value this update moves the directory TO.
    pub new_value: Option<UpdateValue>,
    /// The setup value the directory carried BEFORE this update.
    pub previous_value: Option<UpdateValue>,
    /// When the update was asked for.
    pub start_time: SystemTime,
    /// When its status last changed.
    pub last_updated: SystemTime,
}

impl UpdateActivityRecord {
    /// This activity as the modelled `UpdateInfoEntry`.
    pub fn to_update_info_entry(&self) -> UpdateInfoEntry {
        UpdateInfoEntry {
            region: Some(self.region.clone()),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            initiated_by: self.initiated_by.clone(),
            new_value: self.new_value.clone(),
            previous_value: self.previous_value.clone(),
            start_time: Some(as_timestamp(self.start_time)),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
        }
    }
}

impl super::Directory {
    /// This directory's update activities of `update_type`, oldest first, each with the
    /// RANK it holds among this directory's activities — the rank a page's `NextToken`
    /// resumes after, which is stable because an activity is appended and never removed.
    ///
    /// `region` narrows the reads to the activities of one Region, as
    /// `DescribeUpdateDirectory$RegionName` ("the name of the Region") asks for; a request
    /// that names none reads the activities of every Region the directory is updated in.
    pub fn update_activities_of(
        &self,
        update_type: &str,
        region: Option<&str>,
    ) -> Vec<(usize, &UpdateActivityRecord)> {
        activities_of(&self.update_activities, update_type, region)
    }
}

/// The entries of `activities` that an update of `update_type` recorded — narrowed to one
/// Region when `region` names one — each with the rank it holds in `activities`.
///
/// Held apart from [`super::Directory::update_activities_of`] so the selection can be
/// exercised on its own, over a history a directory does not have to be built to hold.
fn activities_of<'a>(
    activities: &'a [UpdateActivityRecord],
    update_type: &str,
    region: Option<&str>,
) -> Vec<(usize, &'a UpdateActivityRecord)> {
    activities
        .iter()
        .enumerate()
        .filter(|(_, activity)| activity.update_type == update_type)
        .filter(|(_, activity)| region.is_none_or(|wanted| activity.region == wanted))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One recorded activity, with only the two members the selection reads carrying
    /// anything distinguishing.
    fn activity(update_type: &str, region: &str) -> UpdateActivityRecord {
        UpdateActivityRecord {
            update_type: update_type.to_string(),
            region: region.to_string(),
            status: "Updated".to_string(),
            status_reason: None,
            initiated_by: None,
            new_value: None,
            previous_value: None,
            start_time: SystemTime::UNIX_EPOCH,
            last_updated: SystemTime::UNIX_EPOCH,
        }
    }

    /// RULE: a read of a directory's update activities reports the ones of the update type
    /// it asks about — narrowed to one Region when it names one — in the order they were
    /// recorded, each carrying the rank it holds in the whole history, so a page resumes
    /// where the last one ended whatever the filter was.
    #[test]
    fn the_activities_selected_are_the_ones_of_that_type_and_region() {
        let history = vec![
            activity("OS", "us-east-1"),
            activity("SIZE", "us-east-1"),
            activity("OS", "eu-west-1"),
            activity("NETWORK", "eu-west-1"),
            activity("OS", "us-east-1"),
        ];
        // (case, update type, region, the ranks the selection reports)
        let rows = [
            ("every Region", "OS", None, vec![0usize, 2, 4]),
            ("one Region", "OS", Some("us-east-1"), vec![0, 4]),
            ("the other Region", "OS", Some("eu-west-1"), vec![2]),
            ("another update type", "SIZE", None, vec![1]),
            ("a type recorded once", "NETWORK", Some("eu-west-1"), vec![3]),
            ("a type in no Region of its own", "NETWORK", Some("us-east-1"), vec![]),
            ("a type nothing recorded", "SIZE", Some("eu-west-1"), vec![]),
        ];
        for (case, update_type, region, expected) in rows {
            let selected: Vec<usize> = activities_of(&history, update_type, region)
                .into_iter()
                .map(|(rank, _)| rank)
                .collect();
            assert_eq!(
                selected, expected,
                "case {case:?}: expected the ranks {expected:?}, got {selected:?}"
            );
        }
        // A history holding nothing reports nothing, whatever is asked for.
        let selected = activities_of(&[], "OS", None);
        assert!(
            selected.is_empty(),
            "case \"an empty history\": expected no activity, got {} of them",
            selected.len()
        );
    }
}
