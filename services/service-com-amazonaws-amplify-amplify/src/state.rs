// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplify-amplify service.
//!
//! The persisted record for a resource IS the modelled structure this service hands
//! back for it: [`App`] for an app, [`Branch`] for a branch, [`Webhook`] for a
//! webhook, [`Job`] for a job. Keeping one copy of each resource — rather than a
//! parallel struct with the same ~30 members plus a conversion — is what makes
//! `GetApp` after `CreateApp` return the created record field-for-field: a reader
//! clones what the writer stored, so the two cannot drift.
//!
//! Branches, webhooks and domain associations live INSIDE their owning app, so
//! `DeleteApp` removes them transitively and `ListBranches` / `ListWebhooks` /
//! `ListDomainAssociations` can never reach another app's records. Jobs live INSIDE their owning branch for the same reason: `DeleteBranch`
//! takes that branch's jobs with it, and `GetJob` can never reach a job of another
//! branch. A deployment reserved on a branch and not started yet lives there too, so
//! it shares its branch's lifetime and its `jobId` cannot collide with another
//! branch's.

use std::collections::BTreeMap;

use crate::ops::types::{App, Branch, DomainAssociation, Job, Step, Webhook};

/// One Amplify app as this service persists it: the modelled record it hands back,
/// plus the branches, domain associations and webhooks created on it.
///
/// The maps are `BTreeMap`s, not `HashMap`s, because the list operations are
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `nextToken` a page returns is the key of the first record it did NOT return, so
/// the next call resumes at that key.
#[derive(Debug, Clone)]
pub struct AmplifyApp {
    /// The app record, exactly as every reader returns it.
    pub app: App,
    /// The branches of this app, keyed by `branchName`.
    pub branches: BTreeMap<String, AmplifyBranch>,
    /// The domain associations of this app, keyed by `domainName`.
    ///
    /// A domain association is created under an app (`POST /apps/{appId}/domains`)
    /// and addressed afterwards by that app and its domain name
    /// (`GET /apps/{appId}/domains/{domainName}`), so it is held by the app rather
    /// than in a store-wide map: the same custom domain name associated with two
    /// apps is two records, and neither app's reader can reach the other's.
    pub domains: BTreeMap<String, AmplifyDomain>,
    /// The webhooks of this app, keyed by `webhookId`.
    ///
    /// A webhook is created under an app (`POST /apps/{appId}/webhooks`) and is
    /// addressed afterwards by its own id alone (`GET /webhooks/{webhookId}`), which
    /// [`ComAmazonawsAmplifyAmplifyState::webhook`] resolves across the apps.
    pub webhooks: BTreeMap<String, Webhook>,
}

impl AmplifyApp {
    /// A newly created app with no branches and no webhooks.
    pub fn new(app: App) -> Self {
        Self {
            app,
            branches: BTreeMap::new(),
            domains: BTreeMap::new(),
            webhooks: BTreeMap::new(),
        }
    }
}

/// One branch as this service persists it: the modelled record every reader returns,
/// plus the jobs started on it, keyed by the `jobId` this branch minted for each.
#[derive(Debug, Clone)]
pub struct AmplifyBranch {
    /// The branch record, exactly as every reader returns it.
    pub branch: Branch,
    /// The deployments `CreateDeployment` has reserved on this branch and nobody has
    /// started yet, keyed by the `jobId` reserved for each.
    ///
    /// A deployment is reserved by one call and started by another, so what the first
    /// call minted — the job id and the upload destinations — has to outlive it; this
    /// is where it lives until `StartDeployment` names that job id. The map is
    /// BOUNDED: a reservation the clock has let expire is dropped (see
    /// [`AmplifyBranch::expire_deployments`]) and a branch holds at most
    /// [`MAX_PENDING_DEPLOYMENTS_PER_BRANCH`] live reservations at once.
    pub deployments: BTreeMap<String, PendingDeployment>,
    /// The jobs started on this branch, keyed by `jobId`.
    ///
    /// BOUNDED, with an explicit retention policy: the map holds at most
    /// [`MAX_JOBS_PER_BRANCH`] jobs, and starting one more RETIRES the oldest job
    /// that has reached a terminal status (see
    /// [`AmplifyBranch::make_room_for_a_job`]). A branch's job history is therefore
    /// finite however many jobs are started on it, and a job that was retired is
    /// `NotFoundException` to every reader. `Branch$totalNumberOfJobs` counts the
    /// jobs the branch has STARTED ([`AmplifyBranch::jobs_started`]), not the
    /// retained ones, so retiring history does not rewrite that count.
    pub jobs: BTreeMap<String, BranchJob>,
    /// Monotonic counter feeding every `jobId` this branch hands out —
    /// [`AmplifyBranch::mint_job_id`] for a job it is starting and
    /// [`AmplifyBranch::reserve_job_id`] for a deployment it is reserving.
    ///
    /// ONE sequence for both, so a reserved id is never handed to a later job: the
    /// deployment that reserved it is started by its own call, under that id.
    pub next_job_seq: u64,
    /// How many jobs this branch has STARTED, which is what
    /// [`AmplifyBranch::jobs_started`] reports.
    ///
    /// Not the same as [`Self::next_job_seq`]: reserving a deployment takes an id out
    /// of that sequence without starting anything, and a job exists only once a
    /// start call creates it.
    pub started_jobs: u64,
}

impl AmplifyBranch {
    /// A newly created branch, which has never been built and so holds no jobs.
    pub fn new(branch: Branch) -> Self {
        Self {
            branch,
            deployments: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_job_seq: 0,
            started_jobs: 0,
        }
    }

    /// Mint a `jobId` for a job this branch is STARTING, and count that job as started.
    ///
    /// `JobId` is modelled `length 0..255, pattern ^[0-9]+$`
    /// (`smithy_query(query="shape",shape="JobSummary")`), so a job id is decimal
    /// digits — not an opaque token — and a job is addressed within its branch
    /// (`/apps/{appId}/branches/{branchName}/jobs/{jobId}`). The ids are therefore a
    /// per-branch count: the first job a branch runs is `1`, the next `2`, which is
    /// also what `Branch$activeJobId` and `Branch$totalNumberOfJobs` report about.
    pub fn mint_job_id(&mut self) -> String {
        let job_id = self.reserve_job_id();
        self.started_jobs = self.started_jobs.wrapping_add(1);
        job_id
    }

    /// Take the next `jobId` of this branch's id sequence without starting a job on it.
    ///
    /// What `CreateDeployment` reserves: it hands the caller the id its later
    /// `StartDeployment` will run under, so the id is spent — no job this branch
    /// starts afterwards may be handed the same one — while the branch has still
    /// started no job, since nothing runs until that start call.
    pub fn reserve_job_id(&mut self) -> String {
        loop {
            self.next_job_seq = self.next_job_seq.wrapping_add(1);
            let candidate = self.next_job_seq.to_string();
            if !self.jobs.contains_key(&candidate) && !self.deployments.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// How many jobs this branch has STARTED, which is what
    /// `Branch$totalNumberOfJobs` reports.
    ///
    /// Not the number of jobs the branch still holds: the history is bounded (see
    /// [`AmplifyBranch::jobs`]), and a branch that has run a thousand jobs has run a
    /// thousand jobs whether or not the oldest are still readable. Not the number of
    /// ids it has handed out either: a reserved deployment nobody started is not a job.
    pub fn jobs_started(&self) -> u64 {
        self.started_jobs
    }

    /// Drop every reservation whose deployment window the clock has already closed.
    ///
    /// The window closes at [`PendingDeployment::expires_at`] whether or not anybody
    /// calls, so a call that reaches this branch materialises it rather than keeping a
    /// reservation that can no longer be started. Before that instant this is a
    /// no-op, so a caller that starts its deployment inside the window still finds it.
    pub fn expire_deployments(&mut self) {
        let now = now_millis();
        self.deployments
            .retain(|_, pending| now < pending.expires_at);
    }

    /// Retire old jobs until this branch can hold one more, and report whether it can.
    ///
    /// The retention policy of [`AmplifyBranch::jobs`], applied: the map never exceeds
    /// [`MAX_JOBS_PER_BRANCH`] entries, and the entry that makes room is the oldest
    /// (lowest-numbered, i.e. first-started) job that has reached a TERMINAL status — a
    /// job that is still running is never dropped out from under the caller watching
    /// it. The clock-owed settling is materialised first, because a job the clock has
    /// already finished is retirable even if nobody has read it yet.
    ///
    /// `false` means the branch is at its cap and every job it holds is still running,
    /// so there is nothing to retire: the caller refuses the start with the modelled
    /// `LimitExceededException` rather than growing the map.
    pub fn make_room_for_a_job(&mut self) -> bool {
        if self.jobs.len() < MAX_JOBS_PER_BRANCH {
            return true;
        }
        for stored in self.jobs.values_mut() {
            stored.settle();
        }
        while self.jobs.len() >= MAX_JOBS_PER_BRANCH {
            // A minted id is decimal, so numeric order IS the order the jobs started;
            // an id that does not parse sorts first and is retired first.
            let Some(oldest) = self
                .jobs
                .iter()
                .filter(|(_, stored)| stored.settles_at.is_none())
                .min_by_key(|(job_id, _)| job_id.parse::<u128>().unwrap_or(0))
                .map(|(job_id, _)| job_id.clone())
            else {
                return false;
            };
            self.jobs.remove(&oldest);
        }
        true
    }
}

/// How many jobs one branch's history holds at once.
///
/// CHOSEN, not documented: AWS publishes no per-branch job-history quota, so this is
/// the emulator's own retention bound — a branch's job map cannot grow without limit
/// however many jobs are started on it. It is the page maximum `ListJobs` models
/// (`maxResults` `range 0..50`, `smithy_query(query="operation",operation="ListJobs")`),
/// so a branch's whole retained history is reportable in one page. Reaching it retires
/// the oldest finished job rather than refusing the caller; see
/// [`AmplifyBranch::make_room_for_a_job`].
pub const MAX_JOBS_PER_BRANCH: usize = 50;

/// One deployment reserved on a branch and not started yet: the `jobId` it reserved,
/// where its content is uploaded, and the window it has to be started in.
///
/// A manual deployment is TWO calls — one reserves the job id and the upload
/// destinations, a later one starts the job that deploys what was uploaded — so
/// everything the first call minted is persisted here for the second to resolve from
/// the `jobId` the caller was handed.
#[derive(Debug, Clone)]
pub struct PendingDeployment {
    /// The `jobId` this reservation took out of its branch's id sequence, and so the
    /// id the deployment's job runs under once it is started.
    pub job_id: String,
    /// The files the reservation named, keyed by file name: empty when the caller
    /// reserved a single upload of the whole zipped deployment.
    pub files: BTreeMap<String, DeploymentFile>,
    /// Where the zipped deployment is uploaded, or the empty string when the
    /// reservation named its files individually and so has no single zip upload.
    pub zip_upload_url: String,
    /// Epoch milliseconds at which this reservation was created.
    pub created_at: i64,
    /// Epoch milliseconds past which this reservation can no longer be started:
    /// `created_at` plus [`DEPLOYMENT_WINDOW_MILLIS`].
    pub expires_at: i64,
}

/// One file of a [`PendingDeployment`]: the content hash its caller declared for it,
/// and the upload destination minted for that one file.
#[derive(Debug, Clone)]
pub struct DeploymentFile {
    /// The hash of the file's content, as the reservation's caller declared it
    /// (`FileMap` is modelled file name -> `MD5Hash`,
    /// `smithy_query(query="shape",shape="FileMap")`).
    pub content_md5: String,
    /// Where this one file's content is uploaded.
    pub upload_url: String,
}

/// How long a reserved deployment may wait to be started.
///
/// DOCUMENTED, not chosen: "The maximum duration between the CreateDeployment call
/// and the StartDeployment call cannot exceed 8 hours"
/// (`smithy_query(query="doc",shape="CreateDeployment")`), so the window a
/// reservation carries is exactly those 8 hours.
pub const DEPLOYMENT_WINDOW_MILLIS: i64 = 8 * 60 * 60 * 1_000;

/// How many deployments one branch may have reserved and unstarted at once, past
/// which reserving another answers the modelled `LimitExceededException` (HTTP 429,
/// "A resource could not be created because service quotas were exceeded").
///
/// CHOSEN, not a published quota: a reservation holds state for up to
/// [`DEPLOYMENT_WINDOW_MILLIS`] whether or not it is ever started, so without a bound
/// one branch's reservation map could grow without limit inside that window. It is
/// [`MAX_JOBS_PER_BRANCH`], because every reservation that IS started becomes one job
/// of that branch's equally bounded history.
pub const MAX_PENDING_DEPLOYMENTS_PER_BRANCH: usize = MAX_JOBS_PER_BRANCH;

/// One job as this service persists it: the modelled [`Job`] record, plus the instant
/// at which the clock owes that job its next status change.
///
/// Starting a job is ASYNCHRONOUS — `StartJob` answers with a non-terminal status and
/// the job settles later — so the record carries when that settling is due instead of
/// blocking the call that started it. [`BranchJob::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct BranchJob {
    /// The job record, exactly as every reader returns it.
    pub job: Job,
    /// Epoch milliseconds at which this job's non-terminal status is due to settle,
    /// or `None` once it has settled and the status is terminal.
    pub settles_at: Option<i64>,
}

impl BranchJob {
    /// A job that has just been started: [`JOB_STATUS_PENDING`] now, settled by the
    /// clock [`JOB_SETTLE_MILLIS`] from now.
    pub fn started(job: Job) -> Self {
        Self {
            job,
            settles_at: Some(now_millis().saturating_add(JOB_SETTLE_MILLIS)),
        }
    }

    /// Materialise the status change the clock has ALREADY made due, and nothing else.
    ///
    /// The change is owed at `settles_at` whether or not anybody calls, so a reporting
    /// operation applies it before answering rather than reporting a `PENDING` job that
    /// the clock settled seconds ago. Before that instant this is a no-op, so a caller
    /// that reads straight after `StartJob` still observes the non-terminal status.
    ///
    /// The status it settles to is [`JOB_STATUS_FAILED`]: a job builds a branch from
    /// its repository, this emulator has no build environment to run that in, and a
    /// build nothing ran cannot be reported as having succeeded. The reason is carried
    /// on the one modelled member that can hold it — `Step$statusReason` — because
    /// `JobSummary` models no reason of its own.
    pub fn settle(&mut self) {
        let Some(due) = self.settles_at else {
            return;
        };
        if now_millis() < due {
            return;
        }
        self.settles_at = None;
        let ended = ::aws_smithy_types::DateTime::from_millis(due);
        self.job.summary.status = JOB_STATUS_FAILED.to_string();
        self.job.summary.end_time = Some(ended);
        self.job.steps = vec![Step {
            step_name: BUILD_STEP_NAME.to_string(),
            start_time: self.job.summary.start_time,
            status: JOB_STATUS_FAILED.to_string(),
            end_time: ended,
            status_reason: Some(NO_BUILD_ENVIRONMENT_REASON.to_string()),
            // Every URL below addresses an artifact a build produced, and no build
            // ran; `screenshots` and `context` likewise only exist for a step that
            // executed.
            log_url: None,
            artifacts_url: None,
            test_artifacts_url: None,
            test_config_url: None,
            screenshots: None,
            context: None,
        }];
    }
}

/// How long after `StartJob` the clock settles a job out of its non-terminal status.
///
/// CHOSEN, not documented: a real Amplify build takes minutes, which no caller of an
/// emulator can wait for. The span is non-zero so a caller that reads the job straight
/// after starting it still sees the `PENDING` status a real client has to poll through.
pub const JOB_SETTLE_MILLIS: i64 = 500;

/// The status a job carries from `StartJob` until the clock settles it.
///
/// `JobStatus` is modelled `CREATED | PENDING | PROVISIONING | RUNNING | FAILED |
/// SUCCEED | CANCELLING | CANCELLED` (`smithy_query(query="shape",shape="JobStatus")`)
/// and `StartJob` "starts a new job": the job exists and has not run yet, which is
/// what `PENDING` says.
pub const JOB_STATUS_PENDING: &str = "PENDING";

/// The terminal status a job settles to; see [`BranchJob::settle`].
pub const JOB_STATUS_FAILED: &str = "FAILED";

/// The name of the execution step a settled job carries its failure reason on.
///
/// `Step$stepName` is modelled as free text (`smithy_query(query="shape",shape="Step")`)
/// and the phase a job fails in here is the build, which Amplify names `BUILD`.
pub const BUILD_STEP_NAME: &str = "BUILD";

/// Why a job this emulator started ends in [`JOB_STATUS_FAILED`].
pub const NO_BUILD_ENVIRONMENT_REASON: &str =
    "This emulator has no build environment, so the job's build was never executed.";

/// One domain association as this service persists it: the modelled
/// [`DomainAssociation`] record every reader returns, the DNS record that would
/// verify its certificate, and the instant at which the clock owes the association
/// its next status.
///
/// Associating a custom domain is ASYNCHRONOUS — the create call answers with a
/// non-terminal `domainStatus` and the association moves on afterwards — so the
/// record carries when that move is due instead of blocking the call that started
/// it. [`AmplifyDomain::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct AmplifyDomain {
    /// The domain-association record, exactly as every reader returns it.
    pub association: DomainAssociation,
    /// The DNS record a caller publishes to prove it controls the domain, minted
    /// with the association and reported from the status at which the certificate
    /// has been requested onwards.
    ///
    /// Held beside the record rather than inside it because a certificate that has
    /// not been requested yet has no verification record to report, while the value
    /// a later status reports has to be the same one on every read.
    pub certificate_verification_dns_record: String,
    /// Epoch milliseconds at which this association's status is due to move on, or
    /// `None` once it has reached the last status the clock owes it.
    pub settles_at: Option<i64>,
}

impl AmplifyDomain {
    /// A newly created association: the first status of [`DOMAIN_STATUS_SEQUENCE`],
    /// with the next one due [`DOMAIN_STATUS_STEP_MILLIS`] from now.
    pub fn created(association: DomainAssociation, verification_dns_record: String) -> Self {
        Self {
            association,
            certificate_verification_dns_record: verification_dns_record,
            settles_at: Some(now_millis().saturating_add(DOMAIN_STATUS_STEP_MILLIS)),
        }
    }

    /// Materialise every status change the clock has ALREADY made due, and nothing
    /// else.
    ///
    /// Each change is owed at `settles_at` whether or not anybody calls, so a
    /// reporting operation applies it before answering rather than reporting a
    /// status the clock left behind seconds ago. The loop walks as many steps as the
    /// elapsed time owes, so a reader that arrives late sees the status the
    /// association would have reached while nobody was watching. Before the next due
    /// instant this is a no-op, so a caller that reads straight after the create
    /// still observes the first, non-terminal status.
    pub fn settle(&mut self) {
        while let Some(due) = self.settles_at {
            if now_millis() < due {
                return;
            }
            let Some(next) = next_domain_status(&self.association.domain_status) else {
                self.settles_at = None;
                return;
            };
            self.association.domain_status = next.to_string();
            // The certificate is requested on the way into this status, so this is
            // the first status that has a verification record to report; it stays
            // the same value for every later status and every later read.
            if next == DOMAIN_STATUS_REQUESTING_CERTIFICATE {
                self.association.certificate_verification_dns_record =
                    Some(self.certificate_verification_dns_record.clone());
                if let Some(certificate) = self.association.certificate.as_mut() {
                    certificate.certificate_verification_dns_record =
                        Some(self.certificate_verification_dns_record.clone());
                }
            }
            if next == DOMAIN_STATUS_PENDING_VERIFICATION {
                self.association.status_reason = DNS_UNVERIFIED_REASON.to_string();
            }
            self.settles_at = next_domain_status(next)
                .map(|_| due.saturating_add(DOMAIN_STATUS_STEP_MILLIS));
        }
    }
}

/// The statuses the clock walks a new domain association through, in order.
///
/// `DomainStatus` is modelled `PENDING_VERIFICATION | IN_PROGRESS | AVAILABLE |
/// IMPORTING_CUSTOM_CERTIFICATE | PENDING_DEPLOYMENT | AWAITING_APP_CNAME | FAILED |
/// CREATING | REQUESTING_CERTIFICATE | UPDATING`
/// (`smithy_query(query="shape",shape="DomainStatus")`), and associating a custom
/// domain is created, then has its certificate requested, and then waits for the
/// domain's DNS records to prove the caller controls the domain — which is where it
/// stops here: nothing in this emulator resolves DNS, so no record of the caller's
/// is ever observed and no later status is owed. See [`DNS_UNVERIFIED_REASON`].
pub const DOMAIN_STATUS_SEQUENCE: &[&str] = &[
    DOMAIN_STATUS_CREATING,
    DOMAIN_STATUS_REQUESTING_CERTIFICATE,
    DOMAIN_STATUS_PENDING_VERIFICATION,
];

/// The status a domain association carries from its create call until the clock moves
/// it on: the association exists and nothing has been provisioned for it yet.
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";

/// The status at which the association's certificate has been requested, and so the
/// first one that reports a `certificateVerificationDNSRecord`.
pub const DOMAIN_STATUS_REQUESTING_CERTIFICATE: &str = "REQUESTING_CERTIFICATE";

/// The status at which the association waits for the domain's DNS records, which is
/// the last status the clock owes a new association here.
pub const DOMAIN_STATUS_PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The status after `current` in [`DOMAIN_STATUS_SEQUENCE`], or `None` when `current`
/// is the last one (or a status the sequence does not name, which the clock owes
/// nothing).
pub fn next_domain_status(current: &str) -> Option<&'static str> {
    let at = DOMAIN_STATUS_SEQUENCE
        .iter()
        .position(|status| *status == current)?;
    DOMAIN_STATUS_SEQUENCE.get(at + 1).copied()
}

/// How long after a status the clock owes the next one.
///
/// CHOSEN, not documented: AWS publishes no duration for a domain association's
/// provisioning, and a real one takes minutes, which no caller of an emulator can
/// wait for. The span is non-zero so a caller that reads the association straight
/// after creating it still sees the first status a real client has to poll through.
pub const DOMAIN_STATUS_STEP_MILLIS: i64 = 500;

/// Why a domain association of this emulator stops at
/// [`DOMAIN_STATUS_PENDING_VERIFICATION`], carried on `statusReason`.
pub const DNS_UNVERIFIED_REASON: &str =
    "Waiting for the domain's DNS records. This emulator resolves no DNS, so the \
     certificate verification record and the subdomain records are never observed.";

/// The certificate type an association created without `certificateSettings`
/// carries.
///
/// `CertificateType` is modelled `AMPLIFY_MANAGED | CUSTOM`
/// (`smithy_query(query="shape",shape="CertificateType")`) and the model states that
/// "If a certificate type isn't specified, Amplify uses the default AMPLIFY_MANAGED
/// certificate" (`smithy_query(query="shape",shape="CertificateSettings")`).
pub const CERTIFICATE_TYPE_AMPLIFY_MANAGED: &str = "AMPLIFY_MANAGED";

/// The certificate type that names a certificate the CALLER already holds, and so
/// the one whose ARN the request has to carry.
pub const CERTIFICATE_TYPE_CUSTOM: &str = "CUSTOM";

/// How many domain associations one app may hold at once, past which creating
/// another answers the modelled `LimitExceededException` (HTTP 429, "A resource
/// could not be created because service quotas were exceeded").
///
/// CHOSEN, not a published quota: without a bound one app's domain map could grow
/// without limit. It is the page maximum `ListDomainAssociations` models
/// (`maxResults` `range 0..50`,
/// `smithy_query(query="operation",operation="ListDomainAssociations")`), so an app's
/// whole set of associations is reportable in one page.
pub const MAX_DOMAIN_ASSOCIATIONS_PER_APP: usize = 50;

/// `DomainName`'s modelled `@pattern`, verbatim from the model
/// (`smithy_query(query="shape",shape="DomainAssociation")`).
pub const DOMAIN_NAME_PATTERN: &str =
    r"^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])(\.)?$";

/// Does one DNS label satisfy the label half of [`DOMAIN_NAME_PATTERN`]:
/// `(?!-)[A-Za-z0-9-]{min_len-1,62}[A-Za-z0-9]`?
///
/// Letters, digits and hyphens only, never opening with a hyphen, always closing on
/// a letter or a digit, and at most 63 characters. `min_len` is 1 for a label the
/// pattern writes `{0,62}` and 2 for the final label it writes `{1,62}`.
fn is_domain_label(label: &str, min_len: usize) -> bool {
    let bytes = label.as_bytes();
    bytes.len() >= min_len
        && bytes.len() <= 63
        && bytes.first() != Some(&b'-')
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// Does `name` satisfy [`DOMAIN_NAME_PATTERN`]: two or more DNS labels, optionally
/// closed by the root dot?
///
/// Checked here rather than left to the boundary validator because that pattern uses
/// negative look-ahead, which the framework's regex engine cannot compile, and a
/// `@pattern` it cannot compile is treated as matching every value. The
/// `@length(0, 64)` half of the constraint IS enforced upstream, so it is not
/// repeated here.
pub fn is_domain_name(name: &str) -> bool {
    // `(\.)?$` — one optional trailing dot, which is the DNS root and not a label.
    let name = name.strip_suffix('.').unwrap_or(name);
    let mut labels: Vec<&str> = name.split('.').collect();
    // `((…)\.)+(…)`: the final label, preceded by at least one dotted label.
    let Some(last) = labels.pop() else {
        return false;
    };
    !labels.is_empty()
        && labels.iter().all(|label| is_domain_label(label, 1))
        && is_domain_label(last, 2)
}

/// The refusal a `domainName` outside its modelled `@pattern` takes, or `None` when
/// the name satisfies it.
///
/// A value outside a MODELLED constraint is refused at request validation, before
/// the operation runs — the same class of refusal as an `appId` outside its pattern
/// or a `maxResults` outside its range, which the boundary validator answers with
/// HTTP 400 `ValidationException` naming the failing member path and the constraint
/// it violated. This pattern reaches a handler only because the framework's regex
/// engine cannot compile a negative look-ahead, so the refusal states the same two
/// facts in the clause shape that renderer uses
/// (`N validation error(s) detected: Value '…' at '<path>' failed to satisfy
/// constraint: <requirement>` — [`::aws_smithy_schema_server::Violation::message`]),
/// with the member path and the pattern the model declares.
///
/// `member` is the Smithy member name the value arrived on, lowerCamel-ed by the
/// framework's own path builder, so every operation that takes a `domainName`
/// refuses a malformed one in one wording.
pub fn domain_name_violation(member: &str, name: &str) -> Option<String> {
    if is_domain_name(name) {
        return None;
    }
    let path = ::aws_smithy_schema_server::constraints::path::member(member);
    Some(format!(
        "1 validation error detected: Value '{name}' at '{path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {DOMAIN_NAME_PATTERN}"
    ))
}

/// Per-service state for com-amazonaws-amplify-amplify. A handler reaches it through [`service_state`].
///
/// The apps map is NOT partitioned by region: an app's ARN records the region of the
/// request that created it, but any later call reaches every app in the store
/// whatever region it is signed for. Partitioning the control plane by region is a
/// separate change; nothing here claims it.
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifyAmplifyState {
    /// Every app that exists, keyed by its minted `appId`.
    pub apps: BTreeMap<String, AmplifyApp>,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_app_id`].
    pub next_app_id_seq: u64,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_webhook_id`].
    pub next_webhook_id_seq: u64,
    /// Monotonic counter feeding [`ComAmazonawsAmplifyAmplifyState::mint_upload_token`].
    pub next_upload_token_seq: u64,
    /// Monotonic counter feeding
    /// [`ComAmazonawsAmplifyAmplifyState::mint_certificate_verification_record`].
    pub next_certificate_token_seq: u64,
}

impl ComAmazonawsAmplifyAmplifyState {
    /// Mint an `appId` no app in state is using.
    ///
    /// `AppId` is modelled `length 1..20, pattern ^d[a-z0-9]+$`
    /// (`smithy_query(query="shape",shape="App")`), so the id is a literal `d`
    /// followed by 13 base-36 digits — 14 characters, inside the bound, and inside
    /// the pattern's alphabet.
    ///
    /// The 13 digits come from a monotonic counter mixed with the wall clock through
    /// the SplitMix64 finaliser, so ids do not expose a guessable sequence, and the
    /// loop re-mints on the (astronomically unlikely) collision, so two `CreateApp`
    /// calls can never be handed the same id.
    pub fn mint_app_id(&mut self) -> String {
        loop {
            self.next_app_id_seq = self.next_app_id_seq.wrapping_add(1);
            let candidate = format!("d{}", base36_digits(mixed(self.next_app_id_seq)));
            if !self.apps.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `webhookId` no webhook in state is using, and the opaque token that
    /// authorises the webhook's own URL.
    ///
    /// `WebhookId` is modelled `length 0..255, pattern ^(?s)`
    /// (`smithy_query(query="shape",shape="Webhook")`), so the format is the
    /// service's rather than the model's: Amplify hands back a UUID, which is what
    /// [`uuid_v4`] renders — 36 characters, inside the modelled bound.
    ///
    /// The token is minted in the same call because it is part of the same secret:
    /// knowing a webhook's id is not enough to trigger it, so the URL carries a
    /// second unguessable value. Both come from the same mixer that mints an app id,
    /// so neither exposes the counter behind it, and the loop re-mints on a
    /// collision, so two creates can never be handed the same id.
    pub fn mint_webhook_id(&mut self) -> (String, String) {
        loop {
            self.next_webhook_id_seq = self.next_webhook_id_seq.wrapping_add(1);
            let (high, low) = mixed_pair(self.next_webhook_id_seq);
            let candidate = uuid_v4(high, low);
            if self.webhook(&candidate).is_none() {
                let (token_high, token_low) = mixed_pair(low ^ high);
                return (candidate, format!("{token_high:016x}{token_low:016x}"));
            }
        }
    }

    /// Mint the opaque token that authorises ONE upload of a reserved deployment.
    ///
    /// A deployment's upload destination is handed to the caller and to nobody else,
    /// so what authorises writing to it cannot be the app id, the branch name or the
    /// job id the caller already knows: it is 128 bits from the same mixer that mints
    /// an app id, rendered as hexadecimal, minted once per upload so no two uploads of
    /// a deployment — and no two deployments — share a destination.
    pub fn mint_upload_token(&mut self) -> String {
        self.next_upload_token_seq = self.next_upload_token_seq.wrapping_add(1);
        let (high, low) = mixed_pair(self.next_upload_token_seq);
        format!("{high:016x}{low:016x}")
    }

    /// Mint the DNS record that verifies ONE domain association's certificate.
    ///
    /// A verification record proves the caller controls the domain, so neither half
    /// of it can be something the caller already knows: both the record's own label
    /// and the name it points at are 128 bits from the same mixer that mints an app
    /// id, rendered as hexadecimal, minted once per association so no two
    /// associations share a record. The domain name the record sits under is the one
    /// the request named, because that is the zone the caller publishes it in.
    pub fn mint_certificate_verification_record(&mut self, domain_name: &str) -> String {
        self.next_certificate_token_seq = self.next_certificate_token_seq.wrapping_add(1);
        let (label_high, label_low) = mixed_pair(self.next_certificate_token_seq);
        let (value_high, value_low) = mixed_pair(label_low ^ label_high);
        format!(
            "_{label_high:016x}{label_low:016x}.{domain_name}. CNAME \
             _{value_high:016x}{value_low:016x}.acm-validations.aws."
        )
    }

    /// The webhook `webhook_id` names, whichever app holds it.
    ///
    /// `GetWebhook`, `UpdateWebhook` and `DeleteWebhook` address a webhook by its id
    /// alone, and an id is unique across the store (see [`Self::mint_webhook_id`]), so
    /// the lookup is a scan of the apps for the one holding that id.
    pub fn webhook(&self, webhook_id: &str) -> Option<&Webhook> {
        self.apps
            .values()
            .find_map(|app| app.webhooks.get(webhook_id))
    }
}

/// A counter value mixed with the wall clock through the SplitMix64 finaliser: the
/// entropy every minted identifier in this service is built from, so an id does not
/// expose a guessable sequence.
fn mixed(seq: u64) -> u64 {
    let clock = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    splitmix64(seq ^ clock.rotate_left(17))
}

/// 128 bits of [`mixed`] entropy, for an identifier wider than a `u64`.
fn mixed_pair(seq: u64) -> (u64, u64) {
    let high = mixed(seq);
    let low = splitmix64(high.rotate_left(32) ^ seq);
    (high, low)
}

/// `high`/`low` rendered as a version-4 UUID: 32 hexadecimal digits in the 8-4-4-4-12
/// grouping, with RFC 4122's version nibble (`4`) and variant bits (`10`) set.
fn uuid_v4(high: u64, low: u64) -> String {
    let high = (high & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let low = (low & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xFFFF,
        high & 0xFFFF,
        low >> 48,
        low & 0xFFFF_FFFF_FFFF,
    )
}

/// The SplitMix64 finaliser: an invertible bit mixer, so distinct inputs give
/// distinct outputs and a counter does not show through the minted id.
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly 13 base-36 digits over `[0-9a-z]`, zero-padded on the left.
///
/// 36^13 > `u64::MAX`, so 13 digits represent every `u64` without truncation.
fn base36_digits(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = [b'0'; 13];
    for slot in out.iter_mut().rev() {
        let digit = usize::try_from(value % 36).unwrap_or(0);
        *slot = DIGITS[digit];
        value /= 36;
    }
    out.iter().map(|byte| char::from(*byte)).collect()
}

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own shared
/// AWS config names, so an unsigned call and a default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the one
/// in its SigV4 credential scope, so that is what an ARN minted while serving it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Now, as epoch milliseconds.
///
/// The one clock this service reads, so a stored timestamp and a stored due instant
/// (see [`BranchJob::settles_at`]) are measured against the same thing.
pub fn now_millis() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Now, truncated to milliseconds.
///
/// restJson1 writes a timestamp as epoch seconds with a fractional part, i.e. as a
/// JSON number: an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond, so a nanosecond-precision instant would not survive the round trip
/// and `createTime` would differ between the create response and a later read.
/// Milliseconds round-trip exactly, which keeps a stored timestamp stable across reads.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(now_millis())
}

/// The page size to serve for a `@paginated` operation whose `maxResults` member is
/// `max_results` and whose modelled `range` maximum is `modelled_max`.
///
/// `maxResults` is modelled with `default: 0`, so an absent member arrives as `0`:
/// that is "the caller set no bound", which the operation serves with its modelled
/// maximum rather than with an empty page. A caller-supplied bound is already inside
/// the modelled range by the time a handler runs (the boundary validator refuses
/// anything else); clamping again costs nothing and keeps this total.
pub fn page_size(max_results: i32, modelled_max: usize) -> usize {
    match usize::try_from(max_results) {
        Ok(0) | Err(_) => modelled_max,
        Ok(bound) => bound.min(modelled_max),
    }
}

/// One page of an ordered `(key, record)` sequence, plus the `nextToken` to continue
/// from — `None` when the page reached the end.
///
/// The token is the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts
/// at the first key at or after the token. Resuming is therefore well defined even
/// when the record the token names was deleted between the two calls: the scan simply
/// continues at the next key.
pub fn page<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if next_token.is_some_and(|token| key.as_str() < token) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(key));
        }
        records.push(record);
    }
    (records, None)
}

/// Replace a stored @required member when the request supplied one.
///
/// A partial update is "every member the request carries, and nothing else": an
/// absent member is not a value, so it leaves the stored one as it was. Shared by
/// `UpdateApp` and `UpdateBranch` so the two cannot drift on that rule.
pub fn apply<T>(stored: &mut T, supplied: Option<T>) {
    if let Some(value) = supplied {
        *stored = value;
    }
}

/// Replace a stored OPTIONAL member when the request supplied one.
///
/// The same rule as [`apply`], for a member that may legitimately be unset: an
/// omitted member leaves the stored value alone rather than clearing it. Clearing a
/// member is therefore NOT expressible by omission — a caller that wants a member
/// emptied sends its empty value, which [`apply`] and this function store as given.
pub fn apply_optional<T>(stored: &mut Option<T>, supplied: Option<T>) {
    if supplied.is_some() {
        *stored = supplied;
    }
}

/// The `message` an Amplify `NotFoundException` carries for a resource the request
/// addressed and this store does not hold.
///
/// The text is built FROM THE REQUEST and is a complete sentence: the kind of entity,
/// the identifier the request used for it, and `not found` closed by the
/// sentence-terminating period. Every 404 in this crate is formatted through this one
/// function, so `GetApp`, `DeleteApp`, `GetBranch`, `DeleteBranch`, `CreateBranch`,
/// `ListBranches`, `UpdateApp`, `UpdateBranch`, the webhook operations, `StartJob` and
/// `GetJob` cannot drift from one another — and
/// an identifier is NEVER inspected here, so the message is the same construction
/// whatever the request named.
pub fn not_found_message(entity: &str, identifier: &str) -> String {
    format!("{entity} {identifier} not found.")
}

/// The `message` an Amplify `BadRequestException` carries when a create is refused
/// because the owning app ALREADY HOLDS a resource of that kind under the name the
/// request asked for.
///
/// Like [`not_found_message`], the text is built FROM THE REQUEST and from nothing
/// else: Amplify names the kind of resource it failed to create, then the name the
/// request used for it, then the app it already exists under —
/// `Failed to create <entity>. The <entity> <name> already exists for the app <appId>`,
/// with the entity in lower case, as recorded behaviour shows (the model does not
/// spell the message out). Every duplicate-name refusal in this crate is formatted
/// through this one function, so the creates cannot drift from one another, and an
/// identifier is NEVER inspected here: the message is the same construction whatever
/// the request named.
pub fn already_exists_message(entity: &str, identifier: &str, app_id: &str) -> String {
    let entity = entity.to_lowercase();
    format!(
        "Failed to create {entity}. The {entity} {identifier} already exists for the app {app_id}"
    )
}

/// The entity name [`not_found_message`] uses for an app addressed by `appId`.
pub const APP_ENTITY: &str = "App";

/// The entity name [`not_found_message`] and [`already_exists_message`] use for a
/// branch addressed by `branchName`.
pub const BRANCH_ENTITY: &str = "Branch";

/// The entity name [`not_found_message`] uses for a job addressed by `jobId`.
pub const JOB_ENTITY: &str = "Job";

/// The entity name [`not_found_message`] uses for a webhook addressed by `webhookId`.
pub const WEBHOOK_ENTITY: &str = "Webhook";

/// The entity name [`not_found_message`] and [`already_exists_message`] use for a
/// domain association addressed by `domainName`.
pub const DOMAIN_ENTITY: &str = "Domain";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifyAmplifyState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifyAmplifyState>)
}
