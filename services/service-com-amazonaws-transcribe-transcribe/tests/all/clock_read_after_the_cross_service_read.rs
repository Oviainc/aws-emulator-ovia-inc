// SPDX-License-Identifier: CC0-1.0

//! WHICH INSTANT a start that has to await a cross-service read judges the clock by,
//! end to end over the wire.
//!
//! THE RULE UNDER TEST: an operation that stores a record reads its clock at the moment
//! it stores that record — after every cross-service read it awaits first — so
//! everything that instant decides is decided as of the store: the state the clock
//! already owes the resources the request names, how many jobs of the family the service
//! is still working on, and the instants the new record's own transitions fall due at. A
//! transition that falls due WHILE the request is awaiting that read has therefore
//! fallen due before the record is stored: the resource it belongs to is usable, the
//! slot it held is free, and the stored record's own transitions are all still ahead of
//! the caller that reads it straight back.
//!
//! This suite holds that read open on purpose. The emulator's own Amazon S3 is mounted
//! beside the service, the way the assembled emulator runs them, with the one read a
//! start awaits — the `HeadObject` a job's `Media` location is resolved with — answered
//! only after [`MEDIA_HOP`], which is longer than every transition span this crate
//! stamps. A start whose media is an `http(s)` location is resolved against no object
//! store at all, so the very same request through the very same emulator is the fast
//! counter-case, and the two differ in nothing but how long the service had to wait for
//! the answer.
//!
//! The rule is asserted over tables spanning what it generalises over: which start
//! operation awaits the read (all four of them), which decision the instant feeds (a
//! named resource's state, the family's concurrency count, the record's own transition
//! instants), which family of named resource carries that state (a custom vocabulary's
//! `VocabularyState`, a custom language model's `ModelStatus`) and which member path
//! names it (the job's own settings and the per-language `LanguageIdSettings` entry).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::state;
use smithy_server_runtime::{ErasedServiceDefinition, MultiServiceRouter, Service};

use crate::media_object_resolution::{
    call, given_object, BUCKET, MEDIA_BYTES, PAST_THE_TRANSITION, STAGED_KEYS,
};
use crate::transcription_jobs::Reply;

/// How long this suite's object store takes to answer the one read a job start awaits.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const MEDIA_HOP: Duration = Duration::from_millis(800);

// Longer than every transition span this crate stamps, so a transition that falls due
// while a start is awaiting the media read has fallen due before that start's record is
// stored — which is the whole point of holding the read open.
const _: () = assert!(MEDIA_HOP.as_millis() > state::JOB_TRANSITION_DELAY.as_millis());
const _: () = assert!(MEDIA_HOP.as_millis() > state::VOCABULARY_TRANSITION_DELAY.as_millis());
const _: () = assert!(MEDIA_HOP.as_millis() > state::MODEL_TRANSITION_DELAY.as_millis());

/// A media location no object store is ever asked about: an `http(s)` URI naming a host
/// rather than an object, which a start resolves against nothing and answers without
/// waiting. The fast counter-case to [`staged_media`].
const FAST_MEDIA: &str = "https://example.invalid/media/clip.mp4";

/// An IAM role ARN of the form `DataAccessRoleArn` is modelled to accept, for the
/// requests that model one `@required` and for a custom language model's input data.
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// The status a job of every family of this service is in while the service is still
/// working on it.
const IN_PROGRESS: &str = "IN_PROGRESS";

/// The status a job of every family of this service reaches once its processing span has
/// elapsed and nothing was wrong with its media.
const COMPLETED: &str = "COMPLETED";

/// This emulator's own Amazon S3 with the one read a job start awaits made SLOW: it
/// answers the `HeadObject` a start resolves its `Media` location with only after
/// [`MEDIA_HOP`], and every other request at full speed, so a suite can stage objects
/// and let builds read them cheaply while still holding one start's cross-service read
/// open across a transition span.
struct SlowObjectStore {
    inner: service_com_amazonaws_s3_amazons3::EmulatedService<engine::InMemoryEngine>,
}

impl Service for SlowObjectStore {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        self.inner.definition()
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        if !self.inner.can_handle(parts) {
            return false;
        }
        // The wait sits where the store CLAIMS the request — one point, reached once
        // per request and before its handler runs — and only for the method the media
        // read uses, so staging an object and the reads a build makes stay fast. A
        // blocking sleep is what the caller sees: the call is in-process, so the start
        // handler awaiting the answer is simply not answered until it is over.
        if parts.method == http::Method::HEAD {
            std::thread::sleep(MEDIA_HOP);
        }
        true
    }
}

/// One emulator: transcribe and the slow object store above on a single shared engine
/// behind the production router.
fn emulator() -> Arc<MultiServiceRouter> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_transcribe_transcribe::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(SlowObjectStore {
        inner: service_com_amazonaws_s3_amazons3::EmulatedService::new(engine),
    });
    Arc::new(router)
}

/// The same emulator with the media every row below names already stored, which is the
/// upload a caller performs before a start names the location.
async fn emulator_with_staged_media() -> Arc<MultiServiceRouter> {
    let emulator = emulator();
    for key in STAGED_KEYS {
        given_object(&emulator, BUCKET, key, MEDIA_BYTES).await;
    }
    emulator
}

/// A media location the object store above HOLDS, so a start naming it waits out
/// [`MEDIA_HOP`] for the answer. One of the staged keys per row, so no row is pinned to
/// one location.
fn staged_media(row: usize) -> String {
    format!("s3://{BUCKET}/{}", STAGED_KEYS[row % STAGED_KEYS.len()])
}

/// A family of resource a job's settings can NAME whose records carry a processing
/// member the clock moves: a start may apply such a resource only once that processing
/// has finished successfully, and a create leaves it in its non-terminal value until the
/// transition its family's own span owes it falls due.
#[derive(Debug, Clone, Copy)]
enum Reference {
    /// A custom vocabulary built from the terms the create LISTS: `PENDING` until its
    /// build falls due one `state::VOCABULARY_TRANSITION_DELAY` later, then `READY`.
    Vocabulary,
    /// A custom language model: `IN_PROGRESS` until its training falls due one
    /// `state::MODEL_TRANSITION_DELAY` later, then `COMPLETED`.
    LanguageModel,
}

impl Reference {
    /// Create the resource of this family named `name`, and answer the state its own
    /// family's reader reports for it right afterwards.
    async fn created(&self, emulator: &MultiServiceRouter, name: &str, case: &str) -> String {
        match self {
            Self::Vocabulary => {
                call(
                    emulator,
                    "CreateVocabulary",
                    json!({
                        "VocabularyName": name,
                        "LanguageCode": "en-US",
                        "Phrases": ["Aardvark", "Transcribe"]
                    }),
                )
                .await
                .ok(case);
                let read = call(emulator, "GetVocabulary", json!({"VocabularyName": name})).await;
                read.member(case, "VocabularyState")
            }
            Self::LanguageModel => {
                // "Here's an example URI path: s3://DOC-EXAMPLE-BUCKET/\
                // my-model-training-data/" (the model's doc for
                // `InputDataConfig$S3Uri`): the training data is a PREFIX holding
                // files, and what the training run establishes is that the prefix holds
                // any — a listing, which this store answers at full speed.
                let prefix = format!("training/{name}/");
                given_object(
                    emulator,
                    BUCKET,
                    &format!("{prefix}corpus.txt"),
                    MEDIA_BYTES,
                )
                .await;
                call(
                    emulator,
                    "CreateLanguageModel",
                    json!({
                        "ModelName": name,
                        "LanguageCode": "en-US",
                        "BaseModelName": "WideBand",
                        "InputDataConfig": {
                            "S3Uri": format!("s3://{BUCKET}/{prefix}"),
                            "DataAccessRoleArn": ROLE_ARN
                        }
                    }),
                )
                .await
                .ok(case);
                let read = call(emulator, "DescribeLanguageModel", json!({"ModelName": name})).await;
                read.ok(case)["LanguageModel"]["ModelStatus"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a ModelStatus, got {:?}", read.body)
                    })
                    .to_string()
            }
        }
    }

    /// The value of that processing member the resource carries while the clock still
    /// owes it its transition — the one value a start may NOT apply it in.
    fn unfinished(&self) -> &'static str {
        match self {
            Self::Vocabulary => "PENDING",
            Self::LanguageModel => IN_PROGRESS,
        }
    }
}

/// One start operation, one member path of it that names a resource whose state the
/// clock moves, and the family that resource belongs to.
struct NamedReference {
    case: &'static str,
    reference: Reference,
    start: &'static str,
    /// The start request for the job named first, applying the resource named second,
    /// with the media named third.
    request: fn(&str, &str, &str) -> Value,
    /// The operation that reads a job of this family back, and the member it names the
    /// job by.
    read: &'static str,
    name_member: &'static str,
}

/// Every start operation that both awaits the media read and can name a resource whose
/// state the clock moves, through each member path that names one: the job's own
/// settings and the per-language `LanguageIdSettings` entry
/// (`smithy_query(query="shape", shape="Settings,ModelSettings,\
/// CallAnalyticsJobSettings,LanguageIdSettings,MedicalScribeSettings")`).
///
/// `StartMedicalScribeJob` is the counter-row: that sibling start already read its clock
/// after the media read, so the rule is asserted where it was broken and where it was
/// not, through one table.
fn named_references() -> Vec<NamedReference> {
    vec![
        NamedReference {
            case: "StartTranscriptionJob Settings.VocabularyName",
            reference: Reference::Vocabulary,
            start: "StartTranscriptionJob",
            request: |job, reference, media| {
                json!({
                    "TranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": media},
                    "Settings": {"VocabularyName": reference}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
        },
        NamedReference {
            case: "StartTranscriptionJob LanguageIdSettings[en-US].VocabularyName",
            reference: Reference::Vocabulary,
            start: "StartTranscriptionJob",
            request: |job, reference, media| {
                json!({
                    "TranscriptionJobName": job,
                    "IdentifyLanguage": true,
                    "Media": {"MediaFileUri": media},
                    "LanguageIdSettings": {"en-US": {"VocabularyName": reference}}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
        },
        NamedReference {
            case: "StartTranscriptionJob ModelSettings.LanguageModelName",
            reference: Reference::LanguageModel,
            start: "StartTranscriptionJob",
            request: |job, reference, media| {
                json!({
                    "TranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": media},
                    "ModelSettings": {"LanguageModelName": reference}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
        },
        NamedReference {
            case: "StartCallAnalyticsJob Settings.VocabularyName",
            reference: Reference::Vocabulary,
            start: "StartCallAnalyticsJob",
            request: |job, reference, media| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": media},
                    "Settings": {"VocabularyName": reference}
                })
            },
            read: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
        },
        NamedReference {
            case: "StartCallAnalyticsJob Settings.LanguageIdSettings[en-GB].LanguageModelName",
            reference: Reference::LanguageModel,
            start: "StartCallAnalyticsJob",
            request: |job, reference, media| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": media},
                    "Settings": {
                        "LanguageOptions": ["en-GB", "en-AU"],
                        "LanguageIdSettings": {"en-GB": {"LanguageModelName": reference}}
                    }
                })
            },
            read: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
        },
        NamedReference {
            case: "StartMedicalScribeJob Settings.VocabularyName",
            reference: Reference::Vocabulary,
            start: "StartMedicalScribeJob",
            request: |job, reference, media| {
                json!({
                    "MedicalScribeJobName": job,
                    "Media": {"MediaFileUri": media},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {
                        "ShowSpeakerLabels": true,
                        "MaxSpeakerLabels": 2,
                        "VocabularyName": reference
                    }
                })
            },
            read: "GetMedicalScribeJob",
            name_member: "MedicalScribeJobName",
        },
    ]
}

/// One job family: how a job of it is started and read back, and where its name and its
/// status live on the wire.
struct Family {
    case: &'static str,
    start: &'static str,
    get: &'static str,
    /// The member both the start request and the read request carry the job name in.
    name_member: &'static str,
    /// The member of the read response the job's projection sits under.
    job_member: &'static str,
    /// The member of that projection carrying the job's `*JobStatus`.
    status_member: &'static str,
    /// How many jobs of this family the service processes at a time in one Region.
    ceiling: usize,
    /// A start request for the job named first, with the media named second. It opts
    /// into no queue, so at the ceiling it is refused rather than parked.
    request: fn(&str, &str) -> Value,
}

/// Every job family of this service, each started through the operation that awaits the
/// media read.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "batch transcription jobs",
            start: "StartTranscriptionJob",
            get: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
            job_member: "TranscriptionJob",
            status_member: "TranscriptionJobStatus",
            ceiling: state::MAX_CONCURRENT_TRANSCRIPTION_JOBS,
            request: |job, media| {
                json!({
                    "TranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": media}
                })
            },
        },
        Family {
            case: "medical transcription jobs",
            start: "StartMedicalTranscriptionJob",
            get: "GetMedicalTranscriptionJob",
            name_member: "MedicalTranscriptionJobName",
            job_member: "MedicalTranscriptionJob",
            status_member: "TranscriptionJobStatus",
            ceiling: state::MAX_CONCURRENT_MEDICAL_TRANSCRIPTION_JOBS,
            request: |job, media| {
                json!({
                    "MedicalTranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": media},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION"
                })
            },
        },
        Family {
            case: "Call Analytics jobs",
            start: "StartCallAnalyticsJob",
            get: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
            job_member: "CallAnalyticsJob",
            status_member: "CallAnalyticsJobStatus",
            ceiling: state::MAX_CONCURRENT_CALL_ANALYTICS_JOBS,
            request: |job, media| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": media}
                })
            },
        },
        Family {
            case: "Medical Scribe jobs",
            start: "StartMedicalScribeJob",
            get: "GetMedicalScribeJob",
            name_member: "MedicalScribeJobName",
            job_member: "MedicalScribeJob",
            status_member: "MedicalScribeJobStatus",
            ceiling: state::MAX_CONCURRENT_MEDICAL_SCRIBE_JOBS,
            request: |job, media| {
                json!({
                    "MedicalScribeJobName": job,
                    "Media": {"MediaFileUri": media},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 30}
                })
            },
        },
    ]
}

impl Family {
    /// Start one job of this family, named `name`, with the media at `media`.
    async fn start_job(&self, emulator: &MultiServiceRouter, name: &str, media: &str) -> Reply {
        call(emulator, self.start, (self.request)(name, media)).await
    }

    /// The status this family's own reader reports for the job named `name`.
    async fn status_of(&self, emulator: &MultiServiceRouter, name: &str) -> String {
        let case = format!("{}: reading {name} back", self.case);
        let reply = call(emulator, self.get, json!({self.name_member: name})).await;
        let body = reply.ok(&case);
        body.get(self.job_member)
            .and_then(|job| job.get(self.status_member))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {} in the answer, got {body:?}",
                    self.status_member
                )
            })
            .to_string()
    }

    /// Whether the job named `name` reports a completion instant yet.
    async fn has_completion_time(&self, emulator: &MultiServiceRouter, name: &str) -> bool {
        let case = format!("{}: reading {name} back", self.case);
        let reply = call(emulator, self.get, json!({self.name_member: name})).await;
        !reply.ok(&case)[self.job_member]["CompletionTime"].is_null()
    }
}

/// A resource whose clock-owed transition fell due WHILE the start was awaiting the
/// media read is one the job may apply: the state the start is judged against is the one
/// the clock owes the resource at the moment the record is stored, not at the moment the
/// request arrived.
///
/// The fast counter-case through the same member path is the control: the same start,
/// made at as good as the same moment but resolving a media location no object store is
/// asked about, is judged while the resource is still being built and is refused.
#[tokio::test]
async fn a_reference_whose_transition_fell_due_during_the_media_read_is_applied() {
    for (row, named) in named_references().into_iter().enumerate() {
        // A fresh emulator per row, so a resource one row built cannot be what makes
        // the next row's start succeed.
        let emulator = emulator_with_staged_media().await;
        let case = named.case;
        let reference = format!("reference-{row}");

        // The row starts from a resource the clock still owes its transition, read back
        // through that family's own reader rather than assumed.
        let unfinished = named.reference.created(&emulator, &reference, case).await;
        assert_eq!(
            unfinished,
            named.reference.unfinished(),
            "case {case:?}: a just-created {:?} must still be being processed, got \
             {unfinished:?}",
            named.reference
        );

        // The control: nothing is awaited for an `http(s)` media location, so this
        // start is judged while that processing is still running and is refused.
        let refused_job = format!("fast-{row}");
        let refused = call(
            &emulator,
            named.start,
            (named.request)(&refused_job, &reference, FAST_MEDIA),
        )
        .await;
        refused.refused(case, 400, "BadRequestException");
        call(
            &emulator,
            named.read,
            json!({named.name_member: &refused_job}),
        )
        .await
        .refused(case, 400, "BadRequestException");

        // The same start, differing only in that its media is an object this suite's
        // store answers for slowly: the resource's transition falls due during that
        // read, so by the time the record is stored the resource is usable and the
        // start is served.
        let job = format!("slow-{row}");
        let started_at = Instant::now();
        let served = call(
            &emulator,
            named.start,
            (named.request)(&job, &reference, &staged_media(row)),
        )
        .await;
        let waited = started_at.elapsed();
        assert!(
            waited >= MEDIA_HOP,
            "case {case:?}: the start must have awaited the slow media read ({MEDIA_HOP:?}), \
             got {waited:?}"
        );
        served.ok(case);
        call(&emulator, named.read, json!({named.name_member: &job}))
            .await
            .ok(case);
    }
}

/// A job stored after a slow media read still OWES its transitions: the instants they
/// fall due at are stamped from the moment the record is stored, so the caller that
/// reads it straight back still observes the non-terminal status — which is what the
/// span the service stamps exists to make observable — and the terminal one only once
/// that span has elapsed.
#[tokio::test]
async fn a_job_stored_after_a_slow_media_read_still_owes_its_transitions() {
    for (row, family) in families().into_iter().enumerate() {
        let emulator = emulator_with_staged_media().await;
        let case = family.case;
        let job = format!("slow-media-{row}");

        let started_at = Instant::now();
        let started = family.start_job(&emulator, &job, &staged_media(row)).await;
        let waited = started_at.elapsed();
        assert!(
            waited >= MEDIA_HOP,
            "case {case:?}: the start must have awaited the slow media read ({MEDIA_HOP:?}), \
             got {waited:?}"
        );
        let answered = started.ok(case)[family.job_member][family.status_member].clone();
        assert_eq!(
            answered,
            json!(IN_PROGRESS),
            "case {case:?}: the start must answer the status the job is in, got {answered:?}"
        );

        // Read straight back: the completion is still ahead of this caller, because it
        // was stamped from the instant the record was stored and not from the older one
        // the request arrived at.
        let status = family.status_of(&emulator, &job).await;
        assert_eq!(
            status, IN_PROGRESS,
            "case {case:?}: a job read back straight after the start must still be \
             processing, got {status:?}"
        );
        assert!(
            !family.has_completion_time(&emulator, &job).await,
            "case {case:?}: a job that has not completed reports no CompletionTime"
        );

        // Nothing sleeps inside the service: the transition is due at an instant stored
        // on the record, so the test simply waits past it.
        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = family.status_of(&emulator, &job).await;
        assert_eq!(
            settled, COMPLETED,
            "case {case:?}: the job must read back {COMPLETED} once its span elapsed, got \
             {settled:?}"
        );
        assert!(
            family.has_completion_time(&emulator, &job).await,
            "case {case:?}: a completed job reports when it completed"
        );
    }
}

/// A concurrency slot the clock released WHILE the start was awaiting the media read is
/// a free slot: the jobs counted against the family's ceiling are the ones it is still
/// working on at the moment the record is stored, so a start whose media read outlasted
/// the jobs holding every slot is served rather than refused the quota error.
///
/// The control is the same start through a media location no object store is asked
/// about: judged while those jobs are all still in flight, it IS refused — asserted only
/// when the fill and that start together ran inside one processing span, since past it
/// the slots have been released and there is nothing left to refuse.
#[tokio::test]
async fn a_slot_released_during_the_media_read_is_free() {
    for (row, family) in families().into_iter().enumerate() {
        let emulator = emulator_with_staged_media().await;
        let case = family.case;

        // Every slot of this family taken by a job the service is working on. The fill
        // names an `http(s)` media location, so no fill start waits for the store and
        // the whole ceiling is taken inside one processing span.
        let filling_began = Instant::now();
        for index in 0..family.ceiling {
            family
                .start_job(&emulator, &format!("fill-{index}"), FAST_MEDIA)
                .await
                .ok_status(&format!("{case}: job {index} of the ceiling"), 200);
        }

        // The control: this start is judged while the fill is still in flight.
        let refused = family
            .start_job(&emulator, "ceiling-probe-fast", FAST_MEDIA)
            .await;
        if filling_began.elapsed() < state::JOB_TRANSITION_DELAY {
            refused.refused(case, 400, "LimitExceededException");
        }

        // The same start whose media the store answers for slowly: every slot the fill
        // took falls free during that read, so the job is admitted and processing.
        let job = "ceiling-probe-slow";
        let started_at = Instant::now();
        let served = family.start_job(&emulator, job, &staged_media(row)).await;
        let waited = started_at.elapsed();
        assert!(
            waited >= MEDIA_HOP,
            "case {case:?}: the start must have awaited the slow media read ({MEDIA_HOP:?}), \
             got {waited:?}"
        );
        served.ok(case);
        let status = family.status_of(&emulator, job).await;
        assert_eq!(
            status, IN_PROGRESS,
            "case {case:?}: the admitted job must be processing, not waiting, got {status:?}"
        );
    }
}
