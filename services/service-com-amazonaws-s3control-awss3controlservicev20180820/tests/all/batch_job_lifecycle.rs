// SPDX-License-Identifier: CC0-1.0

//! The S3 Batch Operations job lifecycle, driven end to end over HTTP.
//!
//! The rule under test: `CreateJob` records the submitted job under a job id
//! the SERVICE assigns, for the account the request names, and every later
//! `DescribeJob` of that id reports exactly what the create submitted — the
//! action, the manifest or manifest generator, the report configuration, the
//! priority, the description and the role — plus the status a created job has
//! (`Suspended` while it awaits confirmation, otherwise `New`, which the clock
//! moves on to `Ready` once the job's preparation falls due) and nothing about
//! tasks nobody ran. A repeated `ClientRequestToken` is the idempotency token
//! the model declares it to be: the same request under a token already used is
//! answered with the job that token created, and a different request reusing it
//! is refused. A request that names no one job — both or neither of `Manifest`
//! and `ManifestGenerator`, no single `Operation` action, or a report with
//! nowhere to be stored — is refused instead of recorded, and a job id no job
//! has is refused rather than answered.
//!
//! Every row below differs along something that rule generalises over: which of
//! the model's eleven actions the job runs, whether its object list comes from a
//! manifest or a generator and in which manifest format, whether a completion
//! report is asked for and over which scope, the priority (including each
//! published bound of `JobPriority` and one past each), whether a description
//! and a tag set are carried, whether confirmation is required, and the account
//! the job belongs to.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes
//! it per the Smithy model, runs the handler and serializes the restXml
//! response. The router, the request builder and the XML escaping are the
//! access point suite's, reused rather than copied.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_PREPARATION, JOB_STATUS_NEW, JOB_STATUS_READY, JOB_STATUS_SUSPENDED,
    SUSPENDED_AWAITING_CONFIRMATION,
};

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, xml_escape, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION,
    XMLNS,
};

/// The IAM role every job below runs its action with. Satisfies the modelled
/// `IAMRoleArn` pattern (`^arn:[^:]+:iam::\d{12}:role/`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3BatchOperations";

/// The `<Code>` every `CreateJob` refusal below carries: the S3-style envelope
/// the whole Batch Operations job surface answers a well-formed request it will
/// not carry out with, which is what this service was recorded answering rather
/// than one of the modelled exception shapes.
const CREATE_REFUSAL: &str = "InvalidRequest";

/// The fixed sentence that refusal carries for a create naming neither of the
/// two members a job's object list can come from.
const SOURCE_REQUIRED: &str = "Either a Manifest or a ManifestGenerator must be provided.";

/// The fixed sentence that refusal carries for a create naming BOTH of them:
/// the same words, closing with the clause that states which half of the one
/// rule the request broke.
const SOURCE_AMBIGUOUS: &str =
    "Either a Manifest or a ManifestGenerator must be provided, but not both";

/// The fixed sentence that refusal carries for a create reusing a
/// `ClientRequestToken` for different parameters: no variable part, so neither
/// the token nor the job it already created is restated to the caller.
const DUPLICATE_TOKEN: &str = "Duplicate ClientRequestToken";

/// The fixed sentence this surface answers every other request it will not
/// carry out with, whatever the member and whatever the value.
const REQUEST_INVALID: &str = "Request invalid";

// ---------------------------------------------------------------------------
// The `Operation` bodies: one action each, spelled with the member name
// `JobOperation` declares for it. Enum values are the ones the model's own
// enums carry (`S3GlacierJobTier`, `S3ObjectLockLegalHoldStatus`,
// `ComputeObjectChecksumAlgorithm`, `S3CannedAccessControlList`).
// ---------------------------------------------------------------------------

pub(crate) const ACTION_PUT_OBJECT_COPY: &str =
    "<S3PutObjectCopy><TargetResource>arn:aws:s3:::destination-bucket</TargetResource>\
     <StorageClass>STANDARD_IA</StorageClass></S3PutObjectCopy>";
pub(crate) const ACTION_PUT_OBJECT_TAGGING: &str =
    "<S3PutObjectTagging><TagSet><member><Key>stage</Key><Value>archived</Value></member>\
     </TagSet></S3PutObjectTagging>";
const ACTION_DELETE_OBJECT_TAGGING: &str = "<S3DeleteObjectTagging></S3DeleteObjectTagging>";
pub(crate) const ACTION_LAMBDA_INVOKE: &str =
    "<LambdaInvoke><FunctionArn>arn:aws:lambda:us-west-2:123456789012:function:transform\
     </FunctionArn><InvocationSchemaVersion>2.0</InvocationSchemaVersion></LambdaInvoke>";
pub(crate) const ACTION_INITIATE_RESTORE: &str =
    "<S3InitiateRestoreObject><ExpirationInDays>7</ExpirationInDays>\
     <GlacierJobTier>BULK</GlacierJobTier></S3InitiateRestoreObject>";
const ACTION_LEGAL_HOLD: &str =
    "<S3PutObjectLegalHold><LegalHold><Status>ON</Status></LegalHold></S3PutObjectLegalHold>";
pub(crate) const ACTION_COMPUTE_CHECKSUM: &str =
    "<S3ComputeObjectChecksum><ChecksumAlgorithm>SHA256</ChecksumAlgorithm>\
     </S3ComputeObjectChecksum>";
const ACTION_PUT_OBJECT_ACL: &str =
    "<S3PutObjectAcl><AccessControlPolicy><CannedAccessControlList>private\
     </CannedAccessControlList></AccessControlPolicy></S3PutObjectAcl>";

// ---------------------------------------------------------------------------
// Where the job's object list comes from: a manifest object, in either
// modelled `JobManifestFormat`, or a generator over a source bucket.
// ---------------------------------------------------------------------------

pub(crate) const MANIFEST_CSV: &str =
    "<Manifest><Spec><Format>S3BatchOperations_CSV_20180820</Format><Fields><member>Bucket\
     </member><member>Key</member></Fields></Spec><Location>\
     <ObjectArn>arn:aws:s3:::manifest-bucket/objects.csv</ObjectArn><ETag>etag-csv-0001</ETag>\
     </Location></Manifest>";
pub(crate) const MANIFEST_INVENTORY: &str =
    "<Manifest><Spec><Format>S3InventoryReport_CSV_20161130</Format></Spec><Location>\
     <ObjectArn>arn:aws:s3:::inventory-bucket/manifest.json</ObjectArn>\
     <ObjectVersionId>3HL4kqtJlcpXroDTDmjVBH40Nrjfkd</ObjectVersionId>\
     <ETag>etag-inventory-0002</ETag></Location></Manifest>";
pub(crate) const GENERATED_MANIFEST: &str =
    "<ManifestGenerator><S3JobManifestGenerator>\
     <SourceBucket>arn:aws:s3:::source-bucket</SourceBucket>\
     <EnableManifestOutput>false</EnableManifestOutput><Filter>\
     <ObjectSizeGreaterThanBytes>1024</ObjectSizeGreaterThanBytes><KeyNameConstraint>\
     <MatchAnyPrefix><member>logs/</member></MatchAnyPrefix></KeyNameConstraint></Filter>\
     </S3JobManifestGenerator></ManifestGenerator>";

// ---------------------------------------------------------------------------
// The `Report` bodies. `Format` is the single modelled `JobReportFormat`
// value; `ReportScope` is each of the two the model declares.
// ---------------------------------------------------------------------------

const REPORT_ALL_TASKS: &str = "<Bucket>arn:aws:s3:::job-report-bucket</Bucket>\
     <Format>Report_CSV_20180820</Format><Enabled>true</Enabled><Prefix>reports/batch</Prefix>\
     <ReportScope>AllTasks</ReportScope>";
const REPORT_FAILED_ONLY: &str = "<Bucket>arn:aws:s3:::failures-bucket</Bucket>\
     <Format>Report_CSV_20180820</Format><Enabled>true</Enabled>\
     <ReportScope>FailedTasksOnly</ReportScope>";
const REPORT_DISABLED: &str = "<Enabled>false</Enabled>";

/// One `CreateJob` request: the members the model lets a caller vary.
#[derive(Clone)]
pub(crate) struct JobCase {
    case: &'static str,
    /// The body of `<Operation>`: exactly one of the actions above.
    operation: &'static str,
    /// The whole `<Manifest>` or `<ManifestGenerator>` element.
    source: &'static str,
    /// The body of `<Report>`.
    report: &'static str,
    /// Rendered into the request verbatim, so a value outside the modelled
    /// `JobPriority` range can be a row of its own.
    priority: &'static str,
    description: Option<&'static str>,
    /// `None` leaves the member out of the request entirely, which is a
    /// different request from one carrying `false`.
    confirmation_required: Option<bool>,
    /// The job's tag set, which is a resource of its own: `CreateJob` records
    /// it and the job tagging operations report it — `JobDescriptor` declares
    /// no `Tags` member, so no `DescribeJob` answer carries it.
    tags: &'static [(&'static str, &'static str)],
}

impl JobCase {
    pub(crate) const fn new(
        case: &'static str,
        operation: &'static str,
        source: &'static str,
    ) -> Self {
        Self {
            case,
            operation,
            source,
            report: REPORT_DISABLED,
            priority: "10",
            description: None,
            confirmation_required: None,
            tags: &[],
        }
    }

    /// The same create carrying `tags` as the job's tag set, for a suite that
    /// drives the job's tagging sub-resource over a job created already
    /// tagged.
    pub(crate) fn with_tags(mut self, tags: &'static [(&'static str, &'static str)]) -> Self {
        self.tags = tags;
        self
    }

    /// The same create carrying `description`, for a suite that asserts what a
    /// job listing reports for a job created with one.
    pub(crate) fn with_description(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }

    /// The same create requiring confirmation before the job runs, which is
    /// what leaves the created job `Suspended` rather than `New` — a suite that
    /// narrows a listing by status needs jobs in more than one status.
    pub(crate) fn awaiting_confirmation(mut self) -> Self {
        self.confirmation_required = Some(true);
        self
    }

    /// The same create asking for `priority`, rendered verbatim, for a suite
    /// that needs jobs carrying different priorities.
    pub(crate) fn with_priority(mut self, priority: &'static str) -> Self {
        self.priority = priority;
        self
    }

    fn request_body(&self, client_request_token: &str) -> String {
        let mut xml = format!("<CreateJobRequest xmlns=\"{XMLNS}\">");
        xml.push_str(&format!(
            "<ClientRequestToken>{}</ClientRequestToken>",
            xml_escape(client_request_token)
        ));
        xml.push_str(&format!("<Operation>{}</Operation>", self.operation));
        xml.push_str(&format!("<Report>{}</Report>", self.report));
        xml.push_str(self.source);
        if let Some(description) = self.description {
            xml.push_str(&format!(
                "<Description>{}</Description>",
                xml_escape(description)
            ));
        }
        xml.push_str(&format!("<Priority>{}</Priority>", self.priority));
        xml.push_str(&format!("<RoleArn>{ROLE_ARN}</RoleArn>"));
        if !self.tags.is_empty() {
            xml.push_str("<Tags>");
            for (key, value) in self.tags {
                xml.push_str(&format!(
                    "<member><Key>{}</Key><Value>{}</Value></member>",
                    xml_escape(key),
                    xml_escape(value)
                ));
            }
            xml.push_str("</Tags>");
        }
        if let Some(confirmation_required) = self.confirmation_required {
            xml.push_str(&format!(
                "<ConfirmationRequired>{confirmation_required}</ConfirmationRequired>"
            ));
        }
        xml.push_str("</CreateJobRequest>");
        xml
    }

    /// The status a job created by this request is in before the clock moves
    /// it: a job that requires confirmation awaits it in `Suspended`, any
    /// other created job starts in `New`.
    fn expected_initial_status(&self) -> &'static str {
        if self.confirmation_required == Some(true) {
            JOB_STATUS_SUSPENDED
        } else {
            JOB_STATUS_NEW
        }
    }

    /// Whether the job was asked for a completion report.
    fn report_enabled(&self) -> bool {
        self.report.contains("<Enabled>true</Enabled>")
    }
}

/// The create cases every assertion below is run over. No row is merely a
/// rename of another: each differs in something the behaviour generalises
/// over.
fn create_cases() -> Vec<JobCase> {
    vec![
        JobCase::new(
            "copy from a CSV manifest, no report",
            ACTION_PUT_OBJECT_COPY,
            MANIFEST_CSV,
        ),
        JobCase {
            report: REPORT_ALL_TASKS,
            description: Some("tag archived objects"),
            tags: &[("owner", "data-platform"), ("stage", "prod")],
            ..JobCase::new(
                "tagging from a CSV manifest, report over all tasks",
                ACTION_PUT_OBJECT_TAGGING,
                MANIFEST_CSV,
            )
        },
        JobCase {
            report: REPORT_FAILED_ONLY,
            // The minimum the modelled `JobPriority` range (0..2147483647)
            // admits.
            priority: "0",
            ..JobCase::new(
                "delete tagging from an inventory manifest, failed tasks only",
                ACTION_DELETE_OBJECT_TAGGING,
                MANIFEST_INVENTORY,
            )
        },
        JobCase {
            // The maximum the modelled `JobPriority` range admits.
            priority: "2147483647",
            description: Some("invoke the transform function"),
            ..JobCase::new(
                "lambda invoke from a generated manifest, maximum priority",
                ACTION_LAMBDA_INVOKE,
                GENERATED_MANIFEST,
            )
        },
        JobCase {
            confirmation_required: Some(true),
            report: REPORT_ALL_TASKS,
            ..JobCase::new(
                "restore from an inventory manifest, awaiting confirmation",
                ACTION_INITIATE_RESTORE,
                MANIFEST_INVENTORY,
            )
        },
        JobCase {
            confirmation_required: Some(false),
            tags: &[("only-tag", "x")],
            ..JobCase::new(
                "legal hold from a generated manifest, confirmation declined",
                ACTION_LEGAL_HOLD,
                GENERATED_MANIFEST,
            )
        },
        JobCase {
            priority: "2147483646",
            ..JobCase::new(
                "checksum from a CSV manifest",
                ACTION_COMPUTE_CHECKSUM,
                MANIFEST_CSV,
            )
        },
        JobCase {
            description: Some("re-apply the canned ACL"),
            ..JobCase::new(
                "object ACL from an inventory manifest",
                ACTION_PUT_OBJECT_ACL,
                MANIFEST_INVENTORY,
            )
        },
    ]
}

/// Create one job and answer the job id the service assigned.
pub(crate) async fn create_job(
    emulator: &Emulator,
    account_id: &str,
    case: &JobCase,
    token: &str,
) -> Reply {
    emulator
        .call(
            "POST",
            "/v20180820/jobs",
            account_id,
            &case.request_body(token),
        )
        .await
}

pub(crate) async fn describe_job(emulator: &Emulator, account_id: &str, job_id: &str) -> Reply {
    emulator
        .call("GET", &format!("/v20180820/jobs/{job_id}"), account_id, "")
        .await
}

/// The ARN a job of this id must report: the `job/<job-id>` resource in the
/// `s3` ARN namespace, in the signing region, under the emulator's canonical
/// account — the form `JobDescriptor$JobArn` states.
fn expected_arn(job_id: &str) -> String {
    format!(
        "arn:aws:s3:{REGION}:{}:job/{job_id}",
        ::emulator_server::emulator_account_id()
    )
}

/// Whether `job_id` is a value the modelled `JobId` admits: 5 to 36 characters
/// of `^[a-zA-Z0-9\-\_]+$`.
fn is_modelled_job_id(job_id: &str) -> bool {
    (5..=36).contains(&job_id.chars().count())
        && job_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

#[tokio::test]
async fn create_then_describe_reports_the_stored_job_for_every_shape_of_create() {
    let emulator = Emulator::start();
    let mut job_ids = BTreeSet::new();

    for (row, case) in create_cases().into_iter().enumerate() {
        let token = format!("token-{row}");
        let created = create_job(&emulator, ACCOUNT, &case, &token).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateJob to answer 200, got {} with body {}",
            case.case, created.status, created.body
        );

        let job_id = created.text("JobId").unwrap_or_default();
        assert!(
            is_modelled_job_id(&job_id),
            "case {:?}: expected a service-assigned JobId the model admits, got {job_id:?}",
            case.case
        );
        assert!(
            job_ids.insert(job_id.clone()),
            "case {:?}: expected a job id no other job has, got the already-assigned {job_id:?}",
            case.case
        );

        let described = describe_job(&emulator, ACCOUNT, &job_id).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected DescribeJob to answer 200, got {} with body {}",
            case.case, described.status, described.body
        );

        assert_eq!(
            described.text("JobId").as_deref(),
            Some(job_id.as_str()),
            "case {:?}: expected DescribeJob to report the created job id, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.text("JobArn").unwrap_or_default(),
            expected_arn(&job_id),
            "case {:?}: expected the computed job ARN, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.text("Status").as_deref(),
            Some(case.expected_initial_status()),
            "case {:?}: expected the status a created job starts in, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.text("Priority").as_deref(),
            Some(case.priority),
            "case {:?}: expected the submitted priority, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.text("RoleArn").as_deref(),
            Some(ROLE_ARN),
            "case {:?}: expected the submitted role, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.text("Description").as_deref(),
            case.description,
            "case {:?}: expected the submitted description, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.text("ConfirmationRequired").as_deref(),
            Some(if case.confirmation_required == Some(true) {
                "true"
            } else {
                "false"
            }),
            "case {:?}: expected the resolved ConfirmationRequired, got body {}",
            case.case,
            described.body
        );

        // The action: the one member of `Operation` the create named is the one
        // the descriptor carries, and no other action appears beside it.
        let action = case
            .operation
            .trim_start_matches('<')
            .split('>')
            .next()
            .unwrap_or_default();
        assert!(
            described.body.contains(&format!("<{action}>")),
            "case {:?}: expected the descriptor to carry the <{action}> action, got body {}",
            case.case,
            described.body
        );

        // Where the object list comes from. A manifest row reports the
        // manifest's own location and spec, down to the CSV field list; a
        // generated-manifest row reports the generator's source bucket and
        // filter, and neither row reports the other's container.
        if case.source.starts_with("<Manifest>") {
            let (object_arn, etag) = if case.source == MANIFEST_CSV {
                ("arn:aws:s3:::manifest-bucket/objects.csv", "etag-csv-0001")
            } else {
                (
                    "arn:aws:s3:::inventory-bucket/manifest.json",
                    "etag-inventory-0002",
                )
            };
            assert_eq!(
                described.text("ObjectArn").as_deref(),
                Some(object_arn),
                "case {:?}: expected the submitted manifest object ARN, got body {}",
                case.case,
                described.body
            );
            assert_eq!(
                described.text("ETag").as_deref(),
                Some(etag),
                "case {:?}: expected the submitted manifest ETag, got body {}",
                case.case,
                described.body
            );
            if case.source == MANIFEST_CSV {
                // The CSV manifest's column list, which is a LIST member: it
                // comes back in the order it was submitted, so the list
                // survived the round trip rather than being dropped.
                assert!(
                    described
                        .body
                        .contains("<Fields><member>Bucket</member><member>Key</member></Fields>"),
                    "case {:?}: expected the submitted manifest fields, got body {}",
                    case.case,
                    described.body
                );
            }
            assert!(
                !described.body.contains("<ManifestGenerator>"),
                "case {:?}: expected no ManifestGenerator on a job created from a manifest, \
                 got body {}",
                case.case,
                described.body
            );
        } else {
            assert_eq!(
                described.text("SourceBucket").as_deref(),
                Some("arn:aws:s3:::source-bucket"),
                "case {:?}: expected the generator's source bucket, got body {}",
                case.case,
                described.body
            );
            assert!(
                described
                    .body
                    .contains("<MatchAnyPrefix><member>logs/</member></MatchAnyPrefix>"),
                "case {:?}: expected the generator's key-name filter, got body {}",
                case.case,
                described.body
            );
            assert_eq!(
                described.text("ObjectSizeGreaterThanBytes").as_deref(),
                Some("1024"),
                "case {:?}: expected the generator's size filter, got body {}",
                case.case,
                described.body
            );
            assert!(
                !described.body.contains("<Manifest>"),
                "case {:?}: expected no Manifest on a job created from a generator, got body {}",
                case.case,
                described.body
            );
        }

        // The action's own members come back as submitted, including the tag
        // set of a tagging action (another LIST member).
        if case.operation == ACTION_PUT_OBJECT_TAGGING {
            assert!(
                described.body.contains(
                    "<TagSet><member><Key>stage</Key><Value>archived</Value></member></TagSet>"
                ),
                "case {:?}: expected the action's submitted tag set, got body {}",
                case.case,
                described.body
            );
        }

        // The completion report configuration, as submitted.
        assert_eq!(
            described.text("Enabled").as_deref(),
            Some(if case.report_enabled() { "true" } else { "false" }),
            "case {:?}: expected the submitted Report.Enabled, got body {}",
            case.case,
            described.body
        );
        if case.report_enabled() {
            assert!(
                described.text("Format").is_some() && described.text("ReportScope").is_some(),
                "case {:?}: expected the submitted report format and scope, got body {}",
                case.case,
                described.body
            );
        }

        assert!(
            described
                .text("CreationTime")
                .is_some_and(|time| !time.is_empty()),
            "case {:?}: expected a CreationTime on the created job, got body {}",
            case.case,
            described.body
        );

        // Nothing ran this job's tasks, so nothing is claimed about them: no
        // progress summary, no failure reasons, no termination date.
        for absent in [
            "<ProgressSummary>",
            "<TotalNumberOfTasks>",
            "<FailureReasons>",
            "<TerminationDate>",
        ] {
            assert!(
                !described.body.contains(absent),
                "case {:?}: expected no {absent} on a job whose tasks nothing ran, got body {}",
                case.case,
                described.body
            );
        }

        // A job's tag set is a separate resource: `JobDescriptor` declares no
        // `Tags` member, so even a job created with tags reports none here.
        assert!(
            !described.body.contains("<Tags>"),
            "case {:?}: expected no Tags on a JobDescriptor, got body {}",
            case.case,
            described.body
        );
    }
}

#[tokio::test]
async fn a_created_job_leaves_new_for_ready_when_its_preparation_falls_due() {
    let emulator = Emulator::start();

    // One job of each kind: one the clock owes a transition, and one that waits
    // for a confirmation instead.
    let prepares = JobCase::new("prepares", ACTION_PUT_OBJECT_COPY, MANIFEST_CSV);
    let awaits_confirmation = JobCase {
        confirmation_required: Some(true),
        ..JobCase::new("awaits confirmation", ACTION_PUT_OBJECT_COPY, MANIFEST_CSV)
    };

    let preparing_id = create_job(&emulator, ACCOUNT, &prepares, "prepares")
        .await
        .text("JobId")
        .expect("CreateJob answers a JobId");
    let suspended_id = create_job(&emulator, ACCOUNT, &awaits_confirmation, "awaits")
        .await
        .text("JobId")
        .expect("CreateJob answers a JobId");

    // The span is never zero: a caller reading straight after the create still
    // sees the status the create left.
    let described = describe_job(&emulator, ACCOUNT, &preparing_id).await;
    assert_eq!(
        described.text("Status").as_deref(),
        Some(JOB_STATUS_NEW),
        "case {:?}: a job read straight after its create is still New, got body {}",
        prepares.case,
        described.body
    );
    let described = describe_job(&emulator, ACCOUNT, &suspended_id).await;
    assert_eq!(
        described.text("Status").as_deref(),
        Some(JOB_STATUS_SUSPENDED),
        "case {:?}: expected Suspended, got body {}",
        awaits_confirmation.case,
        described.body
    );
    assert_eq!(
        described.text("SuspendedCause").as_deref(),
        Some(SUSPENDED_AWAITING_CONFIRMATION),
        "case {:?}: expected the awaiting-confirmation cause, got body {}",
        awaits_confirmation.case,
        described.body
    );
    assert!(
        described
            .text("SuspendedDate")
            .is_some_and(|date| !date.is_empty()),
        "case {:?}: expected a SuspendedDate on a suspended job, got body {}",
        awaits_confirmation.case,
        described.body
    );

    // Once the preparation the clock owed falls due, the job is Ready — and
    // reading it again does not move it on, because nothing ran its tasks.
    tokio::time::sleep(JOB_PREPARATION + Duration::from_millis(250)).await;
    for read in 1..=2 {
        let described = describe_job(&emulator, ACCOUNT, &preparing_id).await;
        assert_eq!(
            described.text("Status").as_deref(),
            Some(JOB_STATUS_READY),
            "case {:?}, read {read}: expected Ready once the preparation fell due, got body {}",
            prepares.case,
            described.body
        );
    }

    // A suspended job is owed nothing by the clock: only a confirmation moves
    // it.
    let described = describe_job(&emulator, ACCOUNT, &suspended_id).await;
    assert_eq!(
        described.text("Status").as_deref(),
        Some(JOB_STATUS_SUSPENDED),
        "case {:?}: expected a suspended job to stay suspended, got body {}",
        awaits_confirmation.case,
        described.body
    );
}

#[tokio::test]
async fn one_client_request_token_creates_one_job_and_a_different_request_is_refused() {
    let emulator = Emulator::start();

    for (row, case) in create_cases().into_iter().enumerate() {
        let token = format!("idempotent-{row}");

        let first = create_job(&emulator, ACCOUNT, &case, &token).await;
        let job_id = first.text("JobId").unwrap_or_default();
        assert_eq!(
            first.status, 200,
            "case {:?}: expected the first create to answer 200, got {} with body {}",
            case.case, first.status, first.body
        );

        // The same request under the same token names the job it already
        // created, rather than a second job.
        let repeated = create_job(&emulator, ACCOUNT, &case, &token).await;
        assert_eq!(
            repeated.status, 200,
            "case {:?}: expected the repeated create to answer 200, got {} with body {}",
            case.case, repeated.status, repeated.body
        );
        assert_eq!(
            repeated.text("JobId").as_deref(),
            Some(job_id.as_str()),
            "case {:?}: expected the repeat of one token to answer the same job id, got body {}",
            case.case,
            repeated.body
        );

        // A DIFFERENT request under that token is the modelled
        // `IdempotencyException`. Each variation differs in one member only —
        // a description, a tag set, the manifest the object list comes from —
        // so every one of them has to be part of what the token is compared
        // on.
        let variations = [
            (
                "a different description",
                JobCase {
                    description: Some("a different job under the same token"),
                    ..case.clone()
                },
            ),
            (
                "a different tag set",
                JobCase {
                    tags: &[("owner", "someone-else")],
                    ..case.clone()
                },
            ),
            (
                "a different object list",
                JobCase {
                    source: if case.source == GENERATED_MANIFEST {
                        MANIFEST_CSV
                    } else {
                        GENERATED_MANIFEST
                    },
                    ..case.clone()
                },
            ),
        ];
        for (variation, different) in variations {
            let refused = create_job(&emulator, ACCOUNT, &different, &token).await;
            assert_eq!(
                refused.status, 400,
                "case {:?} with {variation:?}: expected a reused token with different \
                 parameters to be refused 400, got {} with body {}",
                case.case, refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                CREATE_REFUSAL,
                "case {:?} with {variation:?}: expected {CREATE_REFUSAL}, got body {}",
                case.case,
                refused.body
            );
            let message = refused.text("Message").unwrap_or_default();
            assert_eq!(
                message, DUPLICATE_TOKEN,
                "case {:?} with {variation:?}: expected the fixed message \
                 {DUPLICATE_TOKEN:?}, got body {}",
                case.case, refused.body
            );
            assert!(
                !message.contains(&token) && !message.contains(&job_id),
                "case {:?} with {variation:?}: expected the refusal to restate neither the \
                 token nor the job it already created, got body {}",
                case.case,
                refused.body
            );
        }

        // The same request under a DIFFERENT token is a different job: the
        // token is what makes a create idempotent, not the parameters.
        let under_new_token = create_job(&emulator, ACCOUNT, &case, &format!("{token}-again")).await;
        assert_ne!(
            under_new_token.text("JobId").as_deref(),
            Some(job_id.as_str()),
            "case {:?}: expected a different token to create a different job, got body {}",
            case.case,
            under_new_token.body
        );

        // A create addressed in another account is refused before the token is
        // looked at, so no job is created for it and the token keeps naming the
        // caller's own job.
        assert_foreign_account_refused(
            case.case,
            OTHER_ACCOUNT,
            &create_job(&emulator, OTHER_ACCOUNT, &case, &token).await,
        );
        let still_mine = describe_job(&emulator, ACCOUNT, &job_id).await;
        assert_eq!(
            still_mine.status, 200,
            "case {:?}: expected the caller's own job to be unchanged by the refused create, \
             got {} with body {}",
            case.case, still_mine.status, still_mine.body
        );
    }
}

#[tokio::test]
async fn a_create_that_names_no_single_job_is_refused_and_records_nothing() {
    let emulator = Emulator::start();

    let manifest_and_generator = format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\"><ClientRequestToken>both</ClientRequestToken>\
         <Operation>{ACTION_PUT_OBJECT_COPY}</Operation><Report>{REPORT_DISABLED}</Report>\
         {MANIFEST_CSV}{GENERATED_MANIFEST}<Priority>5</Priority>\
         <RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>"
    );
    let neither = format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\"><ClientRequestToken>neither</ClientRequestToken>\
         <Operation>{ACTION_PUT_OBJECT_COPY}</Operation><Report>{REPORT_DISABLED}</Report>\
         <Priority>5</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>"
    );
    let no_action = format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\"><ClientRequestToken>no-action</ClientRequestToken>\
         <Operation></Operation><Report>{REPORT_DISABLED}</Report>{MANIFEST_CSV}\
         <Priority>5</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>"
    );
    let two_actions = format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\"><ClientRequestToken>two-actions</ClientRequestToken>\
         <Operation>{ACTION_PUT_OBJECT_COPY}{ACTION_DELETE_OBJECT_TAGGING}</Operation>\
         <Report>{REPORT_DISABLED}</Report>{MANIFEST_CSV}<Priority>5</Priority>\
         <RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>"
    );
    let report_without_bucket = format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\"><ClientRequestToken>no-bucket</ClientRequestToken>\
         <Operation>{ACTION_PUT_OBJECT_COPY}</Operation>\
         <Report><Enabled>true</Enabled><Format>Report_CSV_20180820</Format></Report>\
         {MANIFEST_CSV}<Priority>5</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>"
    );

    // Each of these is well-formed against the model and still names no one
    // job, so the service refuses it in the fixed words it answers that class
    // with: the sentence naming the two members a job's object list can come
    // from for the create that names NEITHER of them, and otherwise the one
    // sentence this surface answers a request it will not carry out with.
    let modelled_refusals = [
        (
            "both a manifest and a generator",
            manifest_and_generator,
            SOURCE_AMBIGUOUS,
        ),
        (
            "neither a manifest nor a generator",
            neither,
            SOURCE_REQUIRED,
        ),
        ("an Operation naming no action", no_action, REQUEST_INVALID),
        (
            "an Operation naming two actions",
            two_actions,
            REQUEST_INVALID,
        ),
        (
            "an enabled report with no bucket",
            report_without_bucket,
            REQUEST_INVALID,
        ),
    ];
    for (case, body, message) in modelled_refusals {
        let refused = emulator
            .call("POST", "/v20180820/jobs", ACCOUNT, &body)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected CreateJob to answer 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            CREATE_REFUSAL,
            "case {case:?}: expected {CREATE_REFUSAL}, got body {}",
            refused.body
        );
        assert_eq!(
            refused.text("Message").unwrap_or_default(),
            message,
            "case {case:?}: expected the fixed message {message:?}, got body {}",
            refused.body
        );
        assert!(
            refused.text("JobId").is_none(),
            "case {case:?}: expected a refused create to record no job, got body {}",
            refused.body
        );
    }

    // One past each published bound of `JobPriority` (range 0..2147483647) is
    // refused too — by the boundary validator, before this handler runs, so
    // only the status is this suite's to pin.
    for (case, priority) in [
        ("one below the minimum priority", "-1"),
        ("one above the maximum priority", "2147483648"),
    ] {
        let out_of_range = JobCase {
            priority,
            ..JobCase::new(case, ACTION_PUT_OBJECT_COPY, MANIFEST_CSV)
        };
        let refused = create_job(&emulator, ACCOUNT, &out_of_range, case).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a priority outside the modelled range to be refused 400, \
             got {} with body {}",
            refused.status, refused.body
        );
        assert!(
            refused.text("JobId").is_none(),
            "case {case:?}: expected a refused create to record no job, got body {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn describe_refuses_a_job_id_the_account_owns_no_job_for() {
    let emulator = Emulator::start();

    // A job that does exist, so the refusals below are about the id and not
    // about an empty store.
    let case = JobCase::new("existing", ACTION_PUT_OBJECT_COPY, MANIFEST_CSV);
    let existing = create_job(&emulator, ACCOUNT, &case, "existing")
        .await
        .text("JobId")
        .expect("CreateJob answers a JobId");

    let unknown_ids = [
        ("a well-formed id no job has", "00000000-0000-0000-0000-000000".to_string()),
        ("the shortest id the model admits", "aaaaa".to_string()),
        ("an id with the underscores the model admits", "job_id_000".to_string()),
        ("the existing id with one character changed", {
            let mut characters: Vec<char> = existing.chars().collect();
            let last = characters.len() - 1;
            characters[last] = if characters[last] == 'z' { 'y' } else { 'z' };
            characters.into_iter().collect()
        }),
    ];
    for (case, job_id) in unknown_ids {
        let refused = describe_job(&emulator, ACCOUNT, &job_id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected DescribeJob to answer 400 for {job_id:?}, got {} with \
             body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "InvalidRequest",
            "case {case:?}: expected InvalidRequest for {job_id:?}, got body {}",
            refused.body
        );
    }

    // And the job that does exist is still readable: a refusal recorded
    // nothing.
    let described = describe_job(&emulator, ACCOUNT, &existing).await;
    assert_eq!(
        described.status, 200,
        "expected the existing job to stay readable, got {} with body {}",
        described.status, described.body
    );
}
