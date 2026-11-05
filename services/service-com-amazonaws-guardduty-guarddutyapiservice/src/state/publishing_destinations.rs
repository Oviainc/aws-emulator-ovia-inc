// SPDX-License-Identifier: CC0-1.0

//! Publishing destinations: the export target a detector publishes its findings
//! to, and the verification status the clock owes it.

use super::*;

/// One publishing destination: the resource a detector exports its findings to,
/// plus the KMS key those findings are encrypted with.
///
/// The destination properties are stored exactly as the create accepted them —
/// nothing is published, so the ARNs are only ever reported back.
#[derive(Debug, Clone)]
pub struct PublishingDestinationRecord {
    pub id: String,
    /// `DestinationType`: a member of [`DESTINATION_TYPES`].
    pub destination_type: String,
    /// "The ARN of the resource to publish to"
    /// (`smithy_query(shape="DestinationProperties")`).
    pub destination_arn: String,
    /// "The ARN of the KMS key to use for encryption"
    /// (`smithy_query(shape="DestinationProperties")`).
    pub kms_key_arn: String,
    /// The expected owner of the destination bucket. "This field appears in the
    /// response only if it was provided when the publishing destination was
    /// created or updated"
    /// (`smithy_query(query="doc",shape="DestinationProperties",member="ExpectedBucketOwner")`),
    /// so it stays absent until a request supplies it.
    pub expected_bucket_owner: Option<String>,
    /// `PublishingStatus`: a member of [`PUBLISHING_STATUSES`].
    pub status: String,
    /// The instant the verification this destination is waiting on comes due, in
    /// epoch milliseconds; `None` once [`Self::settle`] has materialised it.
    pub verification_due_millis: Option<i64>,
    /// "The time, in epoch millisecond format, at which GuardDuty was first unable
    /// to publish findings to the destination"
    /// (`smithy_query(query="doc",shape="DescribePublishingDestinationResponse",member="PublishingFailureStartTimestamp")`).
    /// Zero while publishing to this destination has never failed, which is every
    /// destination this emulator holds: nothing is published, so nothing fails.
    pub publishing_failure_start_millis: i64,
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the create that minted this destination carried, if any —
    /// the `@idempotencyToken` index, kept on the record so it cannot outlive the
    /// destination it names. Read it through
    /// [`Detector::publishing_destination_for_token`].
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl PublishingDestinationRecord {
    /// Materialise the verification the clock already owes: a destination created
    /// at `t` reports [`PENDING_VERIFICATION`] until `t +
    /// VERIFICATION_DELAY_MILLIS` and [`PUBLISHING`] from then on.
    ///
    /// Only the clock moves the status here, and it moves it at an instant fixed
    /// by the create, so a read that settles it shows exactly what a read at the
    /// same instant would have shown had nobody called in between. A status a
    /// request moved away from `PENDING_VERIFICATION` is left alone.
    pub fn settle(&mut self, now: i64) {
        let Some(due) = self.verification_due_millis else {
            return;
        };
        if now < due {
            return;
        }
        self.verification_due_millis = None;
        if self.status == PENDING_VERIFICATION {
            self.status = PUBLISHING.to_string();
        }
    }

    /// Put the destination back in [`PENDING_VERIFICATION`] and owe the move to
    /// [`PUBLISHING`] again, as of `now`.
    ///
    /// The destination properties say WHERE GuardDuty publishes and with which key,
    /// so a destination whose properties changed is one GuardDuty has not verified
    /// it can write to — the same state a create leaves behind, reached again by the
    /// same clock-owed span. This is what makes the status a destination reports
    /// follow its current properties: a destination whose properties were fixed is
    /// verified again rather than keeping the verdict reached for the properties it
    /// no longer has.
    pub fn begin_verification(&mut self, now: i64) {
        self.status = PENDING_VERIFICATION.to_string();
        self.verification_due_millis = Some(now.saturating_add(VERIFICATION_DELAY_MILLIS));
    }
}
