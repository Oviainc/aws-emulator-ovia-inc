// SPDX-License-Identifier: CC0-1.0

use super::*;

/// One stored share of a workload: the principal the workload was shared WITH, the
/// permission that principal was granted, and where the share stands.
///
/// A share is its own resource, identified by its `ShareId`, and it belongs to the
/// workload it shares — `shared_by` is that workload's owning account, the principal that
/// did the sharing. It is recorded (rather than re-derived) because it is the modelled
/// `WorkloadShare$SharedBy` member, which the `WorkloadShare` shape carries and
/// `WorkloadShareSummary` does not: `ShareInvitationSummary$SharedBy` is what
/// `ListShareInvitations` reports it as, and [`WorkloadShareRecord::to_share`] — the
/// projection `UpdateWorkloadShare` answers with — reports it from this field, so the
/// principal that shared the workload survives every change to the share.
///
/// `share_invitation_id` is the id of the invitation the share's recipient has to answer.
/// The invitation is 1:1 with the share and exists from the moment the share is created
/// — "Shared access to a workload is not removed until the workload invitation is
/// deleted" (`CreateWorkloadShare` doc) — but it is a resource of its OWN, named by its
/// own `ShareInvitationId` rather than by the share's `ShareId`
/// (`smithy_query(query="shape", shape="ShareInvitationSummary")` carries both), so it
/// is minted and recorded beside it.
#[derive(Debug, Clone)]
pub struct WorkloadShareRecord {
    pub share_id: String,
    pub share_invitation_id: String,
    pub workload_id: String,
    pub shared_by: String,
    pub shared_with: String,
    pub permission_type: String,
    pub status: String,
    pub status_message: Option<String>,
}

impl WorkloadShareRecord {
    /// The `WorkloadShareSummary` projection `ListWorkloadShares` pages over.
    pub fn to_summary(&self) -> WorkloadShareSummary {
        WorkloadShareSummary {
            share_id: Some(self.share_id.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: Some(self.permission_type.clone()),
            status: Some(self.status.clone()),
            // "Optional message to compliment the Status field"
            // (`WorkloadShareSummary$StatusMessage` doc): a share that is simply waiting
            // on its invitation has nothing to add to its status.
            status_message: self.status_message.clone(),
        }
    }

    /// The `WorkloadShare` shape — "A workload share return object" — that the
    /// operations answering with the share itself return (`UpdateWorkloadShare`).
    ///
    /// It carries two members `WorkloadShareSummary` does not: `SharedBy`, the principal
    /// that did the sharing, read off the record, and `WorkloadName`, the name of the
    /// workload the share shares. The name is passed in rather than duplicated on the
    /// record because the workload owns it and `UpdateWorkload` can change it, so the
    /// share must never hold a stale copy.
    pub fn to_share(&self, workload_name: &str) -> WorkloadShare {
        WorkloadShare {
            share_id: Some(self.share_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: Some(self.permission_type.clone()),
            status: Some(self.status.clone()),
            workload_name: Some(workload_name.to_string()),
            workload_id: Some(self.workload_id.clone()),
        }
    }

    /// The `ShareInvitationSummary` projection `ListShareInvitations` pages over: this
    /// share seen as the invitation its recipient was offered.
    ///
    /// `workload_name` is the name of the workload the share shares — the summary's
    /// `WorkloadName` member, which the share record does not duplicate because the
    /// workload owns it and `UpdateWorkload` can change it. Only the members of the
    /// shared resource KIND are filled: a workload invitation names a workload, and the
    /// lens, profile and review-template members of the shape belong to invitations of
    /// those kinds.
    pub fn to_invitation_summary(&self, workload_name: &str) -> ShareInvitationSummary {
        ShareInvitationSummary {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: Some(self.permission_type.clone()),
            share_resource_type: Some(WORKLOAD_RESOURCE_TYPE.to_string()),
            workload_name: Some(workload_name.to_string()),
            workload_id: Some(self.workload_id.clone()),
            lens_name: None,
            lens_arn: None,
            profile_name: None,
            profile_arn: None,
            template_name: None,
            template_arn: None,
        }
    }

    /// The `ShareInvitation` projection `UpdateShareInvitation` answers with: the
    /// invitation this share raised, named by its own `ShareInvitationId` and by the
    /// resource it offers.
    ///
    /// Only the members of the shared resource KIND are filled — a workload invitation
    /// names a workload, and the `LensAlias`, `LensArn`, `ProfileArn` and `TemplateArn`
    /// members of the shape (`smithy_query(query="shape", shape="ShareInvitation")`)
    /// belong to invitations of those kinds.
    pub fn to_invitation(&self) -> ShareInvitation {
        ShareInvitation {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            share_resource_type: Some(WORKLOAD_RESOURCE_TYPE.to_string()),
            workload_id: Some(self.workload_id.clone()),
            lens_alias: None,
            lens_arn: None,
            profile_arn: None,
            template_arn: None,
        }
    }
}

/// What a `CreateWorkloadShare` idempotency token resolves to: the share the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateWorkloadShareToken {
    pub workload_id: String,
    pub share_id: String,
    pub fingerprint: String,
}

/// One stored share of a CUSTOM LENS: the principal the lens was shared WITH and where
/// the share stands.
///
/// The lens counterpart of [`WorkloadShareRecord`], and recorded for the same reasons: a
/// share is its own resource identified by its `ShareId`, it belongs to the lens it
/// shares, and `shared_by` is that lens's owning account — the principal that did the
/// sharing, which `ShareInvitationSummary$SharedBy` reports. A lens share carries NO
/// permission: `CreateLensShare` takes none (`smithy_query(query="operation",
/// operation="CreateLensShare")` publishes only `LensAlias`, `SharedWith` and
/// `ClientRequestToken`), so a lens invitation reports none either.
///
/// `share_invitation_id` is the id of the invitation the recipient has to answer. It is
/// 1:1 with the share and exists from the moment the share is created — "Shared access
/// to a lens is not removed until the lens invitation is deleted" (`CreateLensShare`
/// doc) — but it is a resource of its OWN, named by its own `ShareInvitationId`, so it
/// is minted and recorded beside the `ShareId`.
///
/// `lens_arn` is held by value because the invitation reports it
/// (`ShareInvitationSummary$LensArn`) and a lens's ARN never changes; the lens's NAME is
/// lens CONTENT, which a later `ImportLens` can change, so it is read from the lens
/// record at projection time instead.
#[derive(Debug, Clone)]
pub struct LensShareRecord {
    pub share_id: String,
    pub share_invitation_id: String,
    /// The lens id the share belongs to — the key of [`LensRecord`] in the store.
    pub lens_id: String,
    pub lens_arn: String,
    pub shared_by: String,
    pub shared_with: String,
    pub status: String,
    pub status_message: Option<String>,
}

impl LensShareRecord {
    /// The `LensShareSummary` projection `ListLensShares` pages over.
    ///
    /// `LensShareSummary` declares exactly `ShareId`, `SharedWith`, `Status` and
    /// `StatusMessage` (`smithy_query(query="shape", shape="LensShareSummary")`) — no
    /// permission, because a lens share grants none, and no lens member, because the
    /// summary describes the share within the lens the request already named.
    pub fn to_summary(&self) -> LensShareSummary {
        LensShareSummary {
            share_id: Some(self.share_id.clone()),
            shared_with: Some(self.shared_with.clone()),
            status: Some(self.status.clone()),
            // "Optional message to compliment the Status field"
            // (`LensShareSummary$StatusMessage` doc): a share that is simply waiting on its
            // invitation has nothing to add to its status.
            status_message: self.status_message.clone(),
        }
    }

    /// The `ShareInvitationSummary` projection `ListShareInvitations` pages over: this
    /// share seen as the invitation its recipient was offered.
    ///
    /// `lens_name` is the name of the lens the share shares, which the lens's CONTENT
    /// owns, so the caller reads it off the lens record rather than off the share; it is
    /// `None` for a lens that carries no readable content. Only the members of the shared
    /// resource KIND are filled, and a lens share grants no permission, so
    /// `PermissionType` is left off too.
    pub fn to_invitation_summary(&self, lens_name: Option<&str>) -> ShareInvitationSummary {
        ShareInvitationSummary {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: None,
            share_resource_type: Some(LENS_RESOURCE_TYPE.to_string()),
            workload_name: None,
            workload_id: None,
            lens_name: lens_name.map(ToString::to_string),
            lens_arn: Some(self.lens_arn.clone()),
            profile_name: None,
            profile_arn: None,
            template_name: None,
            template_arn: None,
        }
    }

    /// The `ShareInvitation` projection `UpdateShareInvitation` answers with: the
    /// invitation this share raised, named by its own `ShareInvitationId` and by the lens
    /// it offers.
    ///
    /// The lens counterpart of [`WorkloadShareRecord::to_invitation`], filling only the
    /// members of the shared resource KIND. Both lens members the shape carries are the
    /// lens's ARN: `LensArn` is "The ARN for the lens", and "For custom lenses, this is
    /// the lens ARN" (`smithy_query(query="shape", shape="LensAlias")`), so the alias of a
    /// custom lens IS that ARN — a custom lens has no short alias of the kind an Amazon
    /// Web Services official lens answers to.
    pub fn to_invitation(&self) -> ShareInvitation {
        ShareInvitation {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            share_resource_type: Some(LENS_RESOURCE_TYPE.to_string()),
            workload_id: None,
            lens_alias: Some(self.lens_arn.clone()),
            lens_arn: Some(self.lens_arn.clone()),
            profile_arn: None,
            template_arn: None,
        }
    }
}

/// One stored share — of WHICHEVER resource kind — reached for writing through the
/// invitation that mediates it, which is what
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::share_by_invitation_mut`]
/// resolves.
///
/// Each resource kind holds its shares in its own map because the records differ (a
/// workload share carries a permission and a workload, a lens share carries the lens's
/// ARN, a profile share the profile's, a review-template share the template's), while an
/// invitation is named by its `ShareInvitationId` ALONE. This is what lets one resolution
/// span every map and one answer path do the three things an answer needs of a share
/// whatever resource it shares: read where it stands, move it, and project it as the
/// invitation it raised.
///
/// There is one variant per share map this store holds, so every invitation
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::pending_share_invitations_for`]
/// reports can be answered.
#[derive(Debug)]
pub enum ShareByInvitationMut<'a> {
    Workload(&'a mut WorkloadShareRecord),
    Lens(&'a mut LensShareRecord),
    Profile(&'a mut ProfileShareRecord),
    Template(&'a mut TemplateShareRecord),
}

impl ShareByInvitationMut<'_> {
    /// The `ShareStatus` member the share currently stands in.
    pub fn status(&self) -> &str {
        match self {
            Self::Workload(share) => &share.status,
            Self::Lens(share) => &share.status,
            Self::Profile(share) => &share.status,
            Self::Template(share) => &share.status,
        }
    }

    /// Record `status` as the `ShareStatus` the share now stands in.
    pub fn set_status(&mut self, status: &str) {
        match self {
            Self::Workload(share) => share.status = status.to_string(),
            Self::Lens(share) => share.status = status.to_string(),
            Self::Profile(share) => share.status = status.to_string(),
            Self::Template(share) => share.status = status.to_string(),
        }
    }

    /// The `ShareInvitation` this share raised, each kind through its own projection, so
    /// an answered invitation reports the members of the kind of resource it offers.
    pub fn to_invitation(&self) -> ShareInvitation {
        match self {
            Self::Workload(share) => share.to_invitation(),
            Self::Lens(share) => share.to_invitation(),
            Self::Profile(share) => share.to_invitation(),
            Self::Template(share) => share.to_invitation(),
        }
    }
}

/// What a `CreateLensShare` idempotency token resolves to: the share the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateLensShareToken {
    pub lens_id: String,
    pub share_id: String,
    pub fingerprint: String,
}

/// One stored share of a Well-Architected Tool PROFILE: the principal the profile was
/// shared WITH and where the share stands.
///
/// The profile counterpart of [`WorkloadShareRecord`], and recorded for the same reasons:
/// a share is its own resource identified by its `ShareId`, it belongs to the profile it
/// shares, and `shared_by` is that profile's owning account — the principal that did the
/// sharing, which `ShareInvitationSummary$SharedBy` reports. A profile share carries NO
/// permission: `CreateProfileShare` takes none (`smithy_query(query="operation",
/// operation="CreateProfileShare")` publishes only `ProfileArn`, `SharedWith` and
/// `ClientRequestToken`), so a profile invitation reports none either.
///
/// `share_invitation_id` is the id of the invitation the recipient has to answer. It is
/// 1:1 with the share and exists from the moment the share is created, but it is a
/// resource of its OWN, named by its own `ShareInvitationId`, so it is minted and
/// recorded beside the `ShareId`.
///
/// The profile's NAME is not held here: the profile owns it and `UpdateProfile` can
/// change it, so an invitation reads it off the profile record at projection time rather
/// than from a copy that could go stale.
#[derive(Debug, Clone)]
pub struct ProfileShareRecord {
    pub share_id: String,
    pub share_invitation_id: String,
    /// The `ProfileArn` the share belongs to — the key of [`ProfileRecord`] in the store.
    pub profile_arn: String,
    pub shared_by: String,
    pub shared_with: String,
    pub status: String,
    pub status_message: Option<String>,
}

impl ProfileShareRecord {
    /// The `ProfileShareSummary` projection `ListProfileShares` pages over.
    ///
    /// `ProfileShareSummary` declares exactly `ShareId`, `SharedWith`, `Status` and
    /// `StatusMessage` (`smithy_query(query="shape", shape="ProfileShareSummary")`) — no
    /// permission, because a profile share grants none.
    pub fn to_summary(&self) -> ProfileShareSummary {
        ProfileShareSummary {
            share_id: Some(self.share_id.clone()),
            shared_with: Some(self.shared_with.clone()),
            status: Some(self.status.clone()),
            // "Profile share invitation status message"
            // (`ProfileShareSummary$StatusMessage` doc): a share that is simply waiting on
            // its invitation has nothing to add to its status.
            status_message: self.status_message.clone(),
        }
    }

    /// The `ShareInvitationSummary` projection `ListShareInvitations` pages over: this
    /// share seen as the invitation its recipient was offered.
    ///
    /// `profile_name` is the name of the profile the share shares, read off the profile
    /// record by the caller rather than off the share; it is `None` for a profile this
    /// store no longer holds. Only the members of the shared resource KIND are filled, and
    /// a profile share grants no permission, so `PermissionType` is left off too.
    pub fn to_invitation_summary(&self, profile_name: Option<&str>) -> ShareInvitationSummary {
        ShareInvitationSummary {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: None,
            share_resource_type: Some(PROFILE_RESOURCE_TYPE.to_string()),
            workload_name: None,
            workload_id: None,
            lens_name: None,
            lens_arn: None,
            profile_name: profile_name.map(ToString::to_string),
            profile_arn: Some(self.profile_arn.clone()),
            template_name: None,
            template_arn: None,
        }
    }

    /// The `ShareInvitation` projection `UpdateShareInvitation` answers with: the
    /// invitation this share raised, named by its own `ShareInvitationId` and by the
    /// profile it offers.
    ///
    /// The profile counterpart of [`WorkloadShareRecord::to_invitation`] and
    /// [`LensShareRecord::to_invitation`], filling only the members of the shared resource
    /// KIND: `ShareInvitation` carries one member for a profile, `ProfileArn`, and the
    /// workload, lens and review-template members belong to invitations of those kinds
    /// (`smithy_query(query="shape", shape="ShareInvitation")`).
    pub fn to_invitation(&self) -> ShareInvitation {
        ShareInvitation {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            share_resource_type: Some(PROFILE_RESOURCE_TYPE.to_string()),
            workload_id: None,
            lens_alias: None,
            lens_arn: None,
            profile_arn: Some(self.profile_arn.clone()),
            template_arn: None,
        }
    }
}

/// What a `CreateProfileShare` idempotency token resolves to: the share the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateProfileShareToken {
    pub profile_arn: String,
    pub share_id: String,
    pub fingerprint: String,
}

/// One stored share of a REVIEW TEMPLATE: the principal the template was shared WITH and
/// where the share stands.
///
/// The review-template counterpart of [`WorkloadShareRecord`], and recorded for the same
/// reasons: a share is its own resource identified by its `ShareId`, it belongs to the
/// template it shares, and `shared_by` is that template's owning account
/// ([`ReviewTemplateRecord::owner`]) — the principal that did the sharing, which
/// `ShareInvitationSummary$SharedBy` reports. A template share carries NO permission:
/// `CreateTemplateShare` takes none (`smithy_query(query="operation",
/// operation="CreateTemplateShare")` publishes only `TemplateArn`, `SharedWith` and
/// `ClientRequestToken`), so a template invitation reports none either.
///
/// `share_invitation_id` is the id of the invitation the recipient has to answer. It is
/// 1:1 with the share and exists from the moment the share is created — "Shared access to
/// a review template is not removed until the review template share invitation is
/// deleted" (`CreateTemplateShare` doc) — but it is a resource of its OWN, named by its
/// own `ShareInvitationId`, so it is minted and recorded beside the `ShareId`.
///
/// The template's NAME is not held here: the template owns it and `UpdateReviewTemplate`
/// can change it, so an invitation reads it off the template record at projection time
/// rather than from a copy that could go stale.
#[derive(Debug, Clone)]
pub struct TemplateShareRecord {
    pub share_id: String,
    pub share_invitation_id: String,
    /// The `TemplateArn` the share belongs to — the key of [`ReviewTemplateRecord`] in
    /// the store.
    pub template_arn: String,
    pub shared_by: String,
    pub shared_with: String,
    pub status: String,
    pub status_message: Option<String>,
}

impl TemplateShareRecord {
    /// The `TemplateShareSummary` projection `ListTemplateShares` pages over.
    ///
    /// `TemplateShareSummary` declares exactly `ShareId`, `SharedWith`, `Status` and
    /// `StatusMessage` (`smithy_query(query="shape", shape="TemplateShareSummary")`) — no
    /// permission, because a review-template share grants none.
    pub fn to_summary(&self) -> TemplateShareSummary {
        TemplateShareSummary {
            share_id: Some(self.share_id.clone()),
            shared_with: Some(self.shared_with.clone()),
            status: Some(self.status.clone()),
            // "Review template share invitation status message"
            // (`TemplateShareSummary$StatusMessage` doc): a share that is simply waiting
            // on its invitation has nothing to add to its status.
            status_message: self.status_message.clone(),
        }
    }

    /// The `ShareInvitationSummary` projection `ListShareInvitations` pages over: this
    /// share seen as the invitation its recipient was offered.
    ///
    /// `template_name` is the name of the review template the share shares, read off the
    /// template record by the caller rather than off the share; it is `None` for a
    /// template this store no longer holds. Only the members of the shared resource KIND
    /// are filled, and a template share grants no permission, so `PermissionType` is left
    /// off too.
    pub fn to_invitation_summary(&self, template_name: Option<&str>) -> ShareInvitationSummary {
        ShareInvitationSummary {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: None,
            share_resource_type: Some(TEMPLATE_SHARE_RESOURCE_TYPE.to_string()),
            workload_name: None,
            workload_id: None,
            lens_name: None,
            lens_arn: None,
            profile_name: None,
            profile_arn: None,
            template_name: template_name.map(ToString::to_string),
            template_arn: Some(self.template_arn.clone()),
        }
    }

    /// The `ShareInvitation` projection `UpdateShareInvitation` answers with: the
    /// invitation this share raised, named by its own `ShareInvitationId` and by the
    /// review template it offers.
    ///
    /// The review-template counterpart of [`WorkloadShareRecord::to_invitation`],
    /// [`LensShareRecord::to_invitation`] and [`ProfileShareRecord::to_invitation`],
    /// filling only the members of the shared resource KIND: `ShareInvitation` carries one
    /// member for a review template, `TemplateArn` — "The review template ARN" — and the
    /// workload, lens and profile members belong to invitations of those kinds
    /// (`smithy_query(query="shape", shape="ShareInvitation")`). The template's NAME is
    /// not among them, which is why this projection needs nothing read off the template.
    pub fn to_invitation(&self) -> ShareInvitation {
        ShareInvitation {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            share_resource_type: Some(TEMPLATE_SHARE_RESOURCE_TYPE.to_string()),
            workload_id: None,
            lens_alias: None,
            lens_arn: None,
            profile_arn: None,
            template_arn: Some(self.template_arn.clone()),
        }
    }
}

/// What a `CreateTemplateShare` idempotency token resolves to: the share the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateTemplateShareToken {
    pub template_arn: String,
    pub share_id: String,
    pub fingerprint: String,
}

/// What a `CreateReviewTemplate` idempotency token resolves to: the review template the
/// original request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateReviewTemplateToken {
    pub template_arn: String,
    pub fingerprint: String,
}

/// The `ProfileVersion` a profile carries the moment it is created.
///
/// A profile is VERSIONED — `GetProfile` reads one version of it
/// (`GetProfileInput$ProfileVersion`), `UpgradeProfileVersion` moves a workload from the
/// version it holds to a later one, and `ProfileNotificationSummary` compares a
/// workload's `CurrentProfileVersion` with the profile's `LatestProfileVersion`
/// (`smithy_query(query="shape", shape="ProfileNotificationSummary")`) — so the version
/// is the SERVICE's own counter over a profile's revisions, not a value a request
/// carries. A profile nothing has revised yet is at its first revision, and `1`
/// satisfies the modelled `ProfileVersion` (`@length(1, 32)`, `^[A-Za-z0-9-]+$`).
/// Minting the next one belongs to `UpdateProfile` ([`next_profile_version`]).
pub const INITIAL_PROFILE_VERSION: &str = "1";

/// The version a revision of a profile moves it to: the next value of the service's own
/// counter over that profile's revisions.
///
/// A profile's version is that counter — [`INITIAL_PROFILE_VERSION`] for a profile nothing
/// has revised, and one more for each revision after it — so the next version is the
/// current count plus one, which is what a workload reviewed against the earlier version
/// is then behind by (`ProfileNotificationSummary$CurrentProfileVersion` against
/// `$LatestProfileVersion`). Only this service writes the member, so a stored version that
/// is not a count has none to advance and the counter starts over at the first revision.
pub fn next_profile_version(current: &str) -> String {
    current.parse::<u64>().map_or_else(
        |_| INITIAL_PROFILE_VERSION.to_string(),
        |count| count.saturating_add(1).to_string(),
    )
}

/// The most Well-Architected Tool profiles this emulator holds for its one account and
/// Region.
///
/// `CreateProfile` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") for an account that has reached its profile quota; the
/// real quota is adjustable through Service Quotas, so the number here is this emulator's
/// own ceiling rather than one read off a doc page. It is what bounds
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::profiles`] (and,
/// with it, the create ledger that is 1:1 with a live profile), and it sits far above any
/// realistic client workflow, so a caller never meets it by accident.
pub const MAX_PROFILES: usize = 1_000;

/// The modelled input member a profile-question finding names
/// (`CreateProfileInput$ProfileQuestions` / `UpdateProfileInput$ProfileQuestions`).
pub const PROFILE_QUESTIONS_MEMBER: &str = "ProfileQuestions";
