// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplifybackend-amplifybackend service.
//!
//! One `BackendApp` per Amplify app id, each holding the backend environments the
//! app owns and every async job recorded against it. Every mutating
//! AmplifyBackend operation answers with a job record, and `GetBackendJob` reads
//! that same record back, so the jobs map is the single home for them.
//!
//! `BTreeMap` rather than `HashMap` everywhere a collection is reported on the
//! wire: `GetBackend`'s `BackendEnvironmentList` is derived from
//! [`BackendApp::environments`], and a hash map would order it differently on
//! every process.

use std::collections::btree_map::Entry as BTreeEntry;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};

use crate::ops::types::{
    BackendAPIResourceConfig, BadRequestException, CreateBackendAuthResourceConfig,
    CreateBackendStorageResourceConfig, GatewayTimeoutException, NotFoundException, ResourceConfig,
    TooManyRequestsException,
};

/// `NotFoundException::ResourceType` for an `AppId` that identifies no Amplify app.
pub const RESOURCE_TYPE_APP: &str = "app";
/// `NotFoundException::ResourceType` for an app no backend was ever created for.
pub const RESOURCE_TYPE_BACKEND: &str = "backend";
/// `NotFoundException::ResourceType` for a backend environment that does not exist.
pub const RESOURCE_TYPE_BACKEND_ENVIRONMENT: &str = "backend-environment";
/// `NotFoundException::ResourceType` for an auth resource that does not exist.
pub const RESOURCE_TYPE_BACKEND_AUTH: &str = "backend-auth";

/// The modelled `NotFoundException` payload for a resource that does not exist.
///
/// `resource_type` names the KIND of resource the lookup failed on (the
/// `RESOURCE_TYPE_*` constants above); `sentence` is the service's own sentence, WITHOUT
/// the prefix, which this function adds — so every message this service renders carries
/// [`ERROR_MESSAGE_PREFIX`] and no call site can forget it.
pub fn not_found(resource_type: &str, sentence: impl AsRef<str>) -> NotFoundException {
    NotFoundException {
        message: Some(prefixed(sentence)),
        resource_type: Some(resource_type.to_string()),
    }
}

/// `sentence` behind this service's fixed [`ERROR_MESSAGE_PREFIX`]: the ONE place the
/// prefix is written, so a message cannot reach the wire with it missing, doubled or
/// spelled a second way.
///
/// Every builder below renders through this, as does the pre-handler refusal in
/// `crate::service`, which words a `Refused` rather than a modelled payload and so cannot
/// use one of the builders.
pub fn prefixed(sentence: impl AsRef<str>) -> String {
    format!("{ERROR_MESSAGE_PREFIX}{}", sentence.as_ref())
}

/// The modelled `BadRequestException` payload for a request this service will not read as
/// properly formed — "An error returned if a request is not formed properly".
///
/// `sentence` is the service's own sentence, WITHOUT the prefix, which this function adds,
/// for the reason [`not_found`] does: a handler passes the SENTENCE and never the whole
/// message, so no client-input refusal of this service reaches the wire without
/// [`ERROR_MESSAGE_PREFIX`].
pub fn bad_request(sentence: impl AsRef<str>) -> BadRequestException {
    BadRequestException {
        message: Some(prefixed(sentence)),
    }
}

/// The modelled `GatewayTimeoutException` payload for a failure on this service's OWN side
/// — "An error returned if there's a temporary issue with the service".
///
/// `sentence` is the service's own sentence, WITHOUT the prefix, which this function adds:
/// the prefix is a property of this service's message channel and not of one status class,
/// so a server-side failure is worded like every other failure it renders.
pub fn gateway_timeout(sentence: impl AsRef<str>) -> GatewayTimeoutException {
    GatewayTimeoutException {
        message: Some(prefixed(sentence)),
    }
}

/// The fixed prefix AmplifyBackend puts in front of EVERY error message it renders.
///
/// Recorded behaviour, not modelled: each recorded AmplifyBackend failure reads
/// "An error occurred while processing your request: " followed by the service's own
/// sentence, whatever the error code — "Invalid appId passed.", "App <appId> not found.",
/// "BackendEnvironment <name> does not exist." — and where that sentence names a resource
/// it names it from the request. The prefix is written in ONE place, [`prefixed`], and
/// reached through the message builders ([`not_found`], [`resource_not_found`],
/// [`bad_request`], [`gateway_timeout`], [`limit_exceeded`], [`bound_identifier`], and the
/// fixed-message refusals below), so a handler passes the SENTENCE and never the whole
/// message.
///
/// It is a property of this service's error CHANNEL and not of one status class or one
/// error shape: a client-input refusal, a missing resource, a bound this emulator holds and
/// a failure on the service's own side are all worded this way, so every modelled payload
/// this crate constructs comes from one of those builders and no call site builds a message
/// of its own.
pub const ERROR_MESSAGE_PREFIX: &str = "An error occurred while processing your request: ";

/// The sentence a lookup of a backend environment that does not exist answers with:
/// recorded behaviour, naming the environment from the request.
///
/// One sentence for "never created" and "already removed" alike — the service does not
/// distinguish them, and neither does the caller's address.
pub fn environment_missing_sentence(environment_name: &str) -> String {
    format!("BackendEnvironment {environment_name} does not exist.")
}

/// The sentence an auth-resource read that does not resolve answers with: recorded
/// behaviour, naming nothing from the request for the same reason as
/// [`UNRESOLVABLE_JOB_SENTENCE`].
pub const AUTH_MISSING_SENTENCE: &str = "Amplify auth parameters file not found.";

/// The longest and shortest a `BackendEnvironmentName` this service will CREATE may be,
/// and the alphabet it is drawn from.
///
/// Recorded behaviour, not modelled: `CreateBackend$BackendEnvironmentName` targets a
/// bare `__string` with no `@length` and no `@pattern`, and the operation models no
/// client-input error for it, but the recorded service refuses a name outside 2-10
/// lowercase letters with HTTP 400 and the error code `ValidationException`. The bound is
/// on the name a create brings into being; a name a request merely ADDRESSES is not
/// checked against it (a read of an environment outside the form answers the same
/// not-found as any other environment that does not exist).
pub const ENVIRONMENT_NAME_MIN_CHARS: usize = 2;
/// See [`ENVIRONMENT_NAME_MIN_CHARS`].
pub const ENVIRONMENT_NAME_MAX_CHARS: usize = 10;

/// Whether `name` is a backend environment name this service may create: between
/// [`ENVIRONMENT_NAME_MIN_CHARS`] and [`ENVIRONMENT_NAME_MAX_CHARS`] characters, every one
/// of them a lowercase letter.
pub fn environment_name_is_valid(name: &str) -> bool {
    let characters = name.chars().count();
    (ENVIRONMENT_NAME_MIN_CHARS..=ENVIRONMENT_NAME_MAX_CHARS).contains(&characters)
        && name.chars().all(|character| character.is_ascii_lowercase())
}

/// The message a `BackendEnvironmentName` outside the form is refused with: the fixed
/// prefix plus the sentence that states the rule it broke.
///
/// The sentence states the RULE and quotes nothing back: it is the same text for every
/// name that breaks it, which is what the recording shows.
pub fn environment_name_message() -> String {
    prefixed(format!(
        "Environment name must be between {ENVIRONMENT_NAME_MIN_CHARS} and \
         {ENVIRONMENT_NAME_MAX_CHARS} characters, and lowercase only."
    ))
}

/// Refuse the request unless `name` is a backend environment name this service may create
/// ([`environment_name_is_valid`]): HTTP 400 with the error code `ValidationException`,
/// which the model does not declare, so it is raised through the crate's own
/// [`crate::scaffolded::Scaffolded::validation`] channel — the same channel every other
/// recorded client-input refusal of this service uses.
///
/// Generic over the operation's modelled error enum, so one rule serves every operation
/// that creates an environment and cannot be worded two ways.
pub fn require_environment_name<E>(name: &str) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if environment_name_is_valid(name) {
        Ok(())
    } else {
        Err(crate::scaffolded::Scaffolded::validation(
            environment_name_message(),
        ))
    }
}

/// The message a request whose `AppId` identifies no registered Amplify app is refused
/// with: [`ERROR_MESSAGE_PREFIX`] plus the sentence that says the app id is the problem.
///
/// Built from the RULE, not from the request: the refusal is about the identity of the
/// app, which the service will not echo back, so the same sentence answers every
/// unregistered app id.
pub fn invalid_app_id_message() -> String {
    prefixed("Invalid appId passed.")
}

/// The message an operation AWS has withdrawn from SDK and AWS CLI callers IN FAVOUR OF
/// THE AMPLIFY CLI answers with: [`ERROR_MESSAGE_PREFIX`], the same "not supported
/// through SDK/AWS CLI" sentence as [`studio_only_message`], and then the Amplify CLI
/// command sequence that does the same work.
///
/// Recorded behaviour, not modelled: the refusal is input-independent down to its TEXT.
/// The recordings show the identical message for a `TargetEnvironmentName` that is free
/// and for one already in use, and the service's own command hint is fixed text — the
/// `<appId>` placeholder and the sample environment name are the words AWS puts in it, not
/// values read from the request, so nothing here is built from the request either. The
/// status (400) and the error code (`ValidationException`) come from the same recordings.
pub fn amplify_cli_only_message() -> String {
    prefixed(
        "Operation not supported through SDK/AWS CLI. \
         Please use \"amplify pull --appId <appId> --envName staging \
         && amplify add env && amplify push\" from Amplify CLI",
    )
}

/// Refuse an operation AWS serves only through the Amplify CLI, with
/// [`amplify_cli_only_message`]. As [`studio_only`], the refusal is the operation's WHOLE
/// behaviour: it is answered before the request is read or any state is touched, so the
/// call is deterministic in the operation alone and leaves nothing behind.
pub fn amplify_cli_only<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::validation(amplify_cli_only_message())
}

/// The message every AmplifyBackend operation AWS has WITHDRAWN from SDK and AWS CLI
/// callers answers with: [`ERROR_MESSAGE_PREFIX`] plus the sentence that names the
/// channel the operation is still reachable through.
///
/// Recorded behaviour, not modelled: AmplifyBackend's Studio-only mutations are refused
/// for EVERY SDK/CLI call, input-independent, with HTTP 400, the error code
/// `ValidationException` and this fixed text. The status and the code are the
/// requirement; the sentence is the one the recordings of this family carry.
pub fn studio_only_message() -> String {
    prefixed(
        "Operation not supported through SDK/AWS CLI. \
         Please use Amplify Studio",
    )
}

/// Refuse an operation AWS serves only through Amplify Studio.
///
/// The refusal is the operation's WHOLE behaviour: it is answered before the request is
/// read or any state is touched, so the call is deterministic in the operation alone and
/// leaves nothing behind. HTTP 400 with the error code `ValidationException`, which is
/// not a shape this service's model declares, so it is raised through the crate's own
/// [`crate::scaffolded::Scaffolded::validation`] channel — the same channel the
/// app-identity gate uses ([`require_registered_app`]), for the same reason: the code is
/// what a client matches on, and the recording decides it.
///
/// Generic over the operation's modelled error enum, so one refusal serves every
/// withdrawn operation and cannot be worded two ways.
pub fn studio_only<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::validation(studio_only_message())
}

/// The refusal a RESOURCE READ whose lookup does not resolve answers: HTTP 404 with the
/// error code `ResourceNotFoundException`, and a message that says what was asked for,
/// built from the request and carrying this service's fixed [`ERROR_MESSAGE_PREFIX`].
///
/// Recorded behaviour, not modelled: AmplifyBackend's resource reads answer the wire
/// error code `ResourceNotFoundException` for a resource that was never created, which no
/// shape in this service's model carries — the modelled `NotFoundException` would put a
/// different code on the wire than the service does, and the code is what a client
/// matches on. It is therefore raised through the crate's own
/// [`crate::scaffolded::Scaffolded::resource_not_found`] channel.
///
/// `sentence` is the service's own sentence, WITHOUT the prefix, which this function
/// adds: a caller passes "no storage resource … exists …", never the whole message.
pub fn resource_not_found<E>(sentence: impl AsRef<str>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::resource_not_found(prefixed(sentence))
}

/// The sentence a JOB READ whose address resolves to no job is refused with, WITHOUT the
/// prefix [`resource_not_found`] adds.
///
/// Recorded behaviour, not modelled: the refusal speaks about the PARAMETERS as a whole
/// and names none of them, so it has no variable part — one sentence answers every
/// unresolvable job address, and nothing of the request is echoed back.
pub const UNRESOLVABLE_JOB_SENTENCE: &str = "Unable to find matching job with provided parameters.";

/// The refusal a job read answers when the (app, environment, job id) it addresses
/// resolves to no job: HTTP 404 with the error code `ResourceNotFoundException` and
/// [`UNRESOLVABLE_JOB_SENTENCE`].
///
/// ONE refusal for the whole address BELOW the app-identity gate. A job read resolves
/// its `AppId` first ([`require_registered_app`]), so an app id no Amplify app exists for
/// never reaches here; what remains of the address is state, and AmplifyBackend does not
/// tell "this environment does not exist" apart from "this app minted no such job" — a
/// part that resolves to nothing makes the JOB absent, which is a 404 and not a
/// request-validation failure.
///
/// Generic over the operation's modelled error enum, like the other shared refusals, so a
/// job read cannot word it a second way.
pub fn job_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    resource_not_found(UNRESOLVABLE_JOB_SENTENCE)
}

/// Whether `app_id` identifies an Amplify app that EXISTS.
///
/// The registry is the Amplify service's own app store, read through the one shared
/// engine: an `AppId` is minted by Amplify's `CreateApp` and by nothing else, so that
/// store is the only place that knows whether an app exists. This crate keeps no second
/// copy of it — an app created or deleted over the Amplify API decides this answer with no
/// propagation step — and it is the same one-directional cross-service read
/// `ListS3Buckets` performs for S3's buckets.
///
/// It is deliberately an app-identity LOOKUP and not "does this resource exist": the two
/// answers differ, which is the whole point of the gate. An app id Amplify holds no app
/// for is refused as invalid INPUT (see [`require_registered_app`]) or as the missing app
/// it is (see [`require_existing_app`]), while a resource missing INSIDE an app that does
/// exist is answered as the missing resource it is — the modelled `NotFoundException` for
/// an environment, the recorded `ResourceNotFoundException` ([`resource_not_found`]) for
/// the reads that answer it, and `IsSuccess: true` on `DeleteToken`, which validates
/// nothing but the app. An app Amplify holds that has no backend here therefore reaches
/// its own per-resource answers rather than the identity refusal.
///
/// A JOB READ is gated like every other read that addresses an app: the identity refusal
/// first, and only under an app that exists is an address matching no job answered as the
/// absent state it is ([`job_not_found`]).
///
/// `false` when that store is unreachable as well (the engine is not the shared in-memory
/// one), which no caller can observe: every handler resolves its own [`service_state`] and
/// answers the modelled `GatewayTimeoutException` BEFORE running the gate.
///
/// The one question it does NOT answer is whether THIS service holds a backend record for
/// the app: that is [`backend_record_exists`], which only an operation addressing the
/// Amplify project rather than a backend resource has reason to ask (see
/// [`require_known_app`]).
///
/// LOCK ORDER: the Amplify store's lock is taken and released here, and this is called
/// with NO guard of this service's store held, so the cross-service read stays
/// one-directional and cannot be part of a lock cycle.
pub fn app_is_registered<Eng: Send + Sync + 'static>(engine: &Eng, app_id: &str) -> bool {
    ::service_com_amazonaws_amplify_amplify::state::service_state(engine)
        .is_some_and(|state| state.lock().apps.contains_key(app_id))
}

/// The modelled `NotFoundException` payload for an `AppId` that identifies no Amplify app.
///
/// Recorded behaviour, not modelled: the create path answers HTTP 404 with the modelled
/// `NotFoundException` and the sentence that names the app id the request carried — the
/// request asked for work on an app that does not exist, so the app is the missing
/// resource.
pub fn app_not_found(app_id: &str) -> NotFoundException {
    not_found(RESOURCE_TYPE_APP, format!("App {app_id} not found."))
}

/// Refuse the request unless `app_id` identifies an app Amplify holds
/// ([`app_is_registered`]), with the modelled `NotFoundException` ([`app_not_found`]).
///
/// The app-identity gate for an operation that CREATES this service's state for an app:
/// app identity is settled before any environment work, so a mistyped or deleted app id
/// can never bring a backend of its own into being. The refusal `Err` carries is the
/// modelled payload, which the caller lifts into its own error channel.
pub fn require_existing_app<Eng: Send + Sync + 'static>(
    engine: &Eng,
    app_id: &str,
) -> Result<(), NotFoundException> {
    if app_is_registered(engine, app_id) {
        Ok(())
    } else {
        Err(app_not_found(app_id))
    }
}

/// Whether THIS service holds a backend record for `app_id` — the narrower question
/// [`app_is_registered`] does not answer.
///
/// `CreateBackend` is the one operation that brings such a record into being, and it only
/// does so for an app Amplify holds, so this is "the app has a backend here" and not "the
/// app exists": the two differ for an app whose Amplify record was deleted after its
/// backend was created, which is the case [`require_known_app`] keeps serving.
///
/// `false` when this service's own store is unreachable, which no caller can observe: every
/// handler resolves [`service_state`] and answers the modelled `GatewayTimeoutException`
/// first.
///
/// LOCK ORDER: this service's lock only, taken and released here, with no other guard
/// held.
pub fn backend_record_exists<Eng: Send + Sync + 'static>(engine: &Eng, app_id: &str) -> bool {
    service_state(engine).is_some_and(|state| state.lock().apps.contains_key(app_id))
}

/// Clean up the Amplify Console app `app_id` identifies, answering whether one was held.
///
/// `RemoveAllBackends`'s `CleanAmplifyApp` asks for the CONSOLE app to be cleaned up, and
/// the console app is the Amplify control plane's resource, not this service's: it is
/// removed from Amplify's OWN store — the same map and the same removal Amplify's
/// `DeleteApp` performs, so the app's branches, domains and webhooks go with it — rather
/// than from a second copy of it kept here. Nothing is created: an app Amplify does not
/// hold leaves this a no-op.
///
/// LOCK ORDER: as [`app_is_registered`] — Amplify's lock only, no guard of this
/// service held.
pub fn remove_amplify_console_app<Eng: Send + Sync + 'static>(engine: &Eng, app_id: &str) -> bool {
    let Some(state) = ::service_com_amazonaws_amplify_amplify::state::service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    store.apps.remove(app_id).is_some()
}

/// Refuse the request unless `app_id` identifies a registered Amplify app
/// ([`app_is_registered`]).
///
/// The app-identity gate every operation that addresses an existing app runs FIRST, so a
/// well-formed but unregistered app id is answered as the client-input error it is —
/// HTTP 400 with the error code `ValidationException` — rather than as a missing
/// resource. `ValidationException` is not a shape this service's model declares, so it is
/// raised through the crate's own [`crate::scaffolded::Scaffolded::validation`] channel,
/// which renders exactly that status and code; the modelled `NotFoundException` then
/// means what it says, "this app has no such resource", and is reachable only for an app
/// that does exist.
///
/// NOT the modelled `BadRequestException`, although this service declares it at the same
/// 400 and `crate::scaffolded`'s own note prefers a modelled client-input error in
/// general: the wire error code of that shape is `BadRequestException`, and the recorded
/// AmplifyBackend response for an unregistered app id carries the code
/// `ValidationException`. The code is what a client matches on, so the recording decides
/// it. Do not "correct" this to the modelled shape without a recording that says so.
///
/// Generic over the operation's modelled error enum: every operation's error channel is
/// `Scaffolded<{Op}ErrorModeled>`, so one gate serves them all and cannot word the same
/// refusal two ways.
///
/// NOT the gate a CREATE runs: the recorded refusal of an operation that creates state for
/// an app is the modelled 404 `NotFoundException` naming the app (see
/// [`require_existing_app`]), while the operations that address an app's EXISTING
/// resources answer this 400. Both read the same registry; only the answer differs.
pub fn require_registered_app<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    app_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if app_is_registered(engine, app_id) {
        Ok(())
    } else {
        Err(crate::scaffolded::Scaffolded::validation(
            invalid_app_id_message(),
        ))
    }
}

/// Refuse the request unless `app_id` identifies an app EITHER registry knows: the Amplify
/// control plane's app store, which is what mints an `AppId` ([`app_is_registered`]), or a
/// backend record this service still holds for it ([`backend_record_exists`]).
///
/// The gate for an operation that addresses the Amplify PROJECT rather than one of this
/// service's resources — `RemoveAllBackends`, which removes the project's backend
/// environments and, when asked, the Console app itself. The second registry is what keeps
/// such a call serving a project whose Amplify record is already gone while its backend
/// records are not, which [`require_registered_app`] refuses.
///
/// The REFUSAL is the same one: an app id neither registry holds is answered HTTP 400 with
/// the error code `ValidationException` and [`invalid_app_id_message`], which is what the
/// recordings show for an app id no app exists under, whichever operation was called. Only
/// the question differs, never the wording.
///
/// LOCK ORDER: both stores' locks are taken and released inside, one at a time, and this
/// is called with NO store guard held, so it cannot be part of a lock cycle.
pub fn require_known_app<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    app_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if app_is_registered(engine, app_id) || backend_record_exists(engine, app_id) {
        Ok(())
    } else {
        Err(crate::scaffolded::Scaffolded::validation(
            invalid_app_id_message(),
        ))
    }
}

/// The sentence a request that could not reach this service's store at all is answered
/// with, WITHOUT the prefix [`gateway_timeout`] adds.
pub const STORE_UNAVAILABLE_SENTENCE: &str = "The AmplifyBackend store is unavailable.";

/// The modelled server-side failure for a request that could not reach this service's
/// store at all. `GatewayTimeoutException` is the only server-side error every
/// AmplifyBackend operation declares, so it is what an unreachable store answers with.
pub fn store_unavailable() -> GatewayTimeoutException {
    gateway_timeout(STORE_UNAVAILABLE_SENTENCE)
}

// ---------------------------------------------------------------------------
// Job status vocabulary
//
// Recorded behaviour, not modelled: `GetBackendJob$Status` is a bare `__string`, and the
// recorded service reports a job that is accepted but not finished as PENDING and then
// RUNNING, and a finished one as COMPLETED, or FAILED when it carries an `Error`. The
// values are what a client's own state machine branches on — "keep polling while the
// status is PENDING or RUNNING" — so an in-flight value outside this vocabulary reads as
// terminal to the caller.
// ---------------------------------------------------------------------------

/// A job the service has accepted and not started yet.
pub const JOB_STATUS_PENDING: &str = "PENDING";

/// A job whose work has started and is not finished.
pub const JOB_STATUS_RUNNING: &str = "RUNNING";

/// A job whose work the clock has made due.
pub const JOB_STATUS_COMPLETED: &str = "COMPLETED";

/// A job that ended with an `Error`.
pub const JOB_STATUS_FAILED: &str = "FAILED";

/// How long a freshly recorded job stays [`JOB_STATUS_PENDING`] before it reports
/// [`JOB_STATUS_RUNNING`].
///
/// AWS documents no duration for either phase, so this span is CHOSEN: long enough that a
/// caller reading straight after the call still observes the job in the status the call
/// itself reported, short enough that the whole job settles inside
/// [`JOB_COMPLETION_DELAY`].
pub const JOB_START_DELAY: Duration = Duration::from_millis(250);

/// How long a freshly recorded job stays unfinished ([`JOB_STATUS_PENDING`], then
/// [`JOB_STATUS_RUNNING`]) before it reports [`JOB_STATUS_COMPLETED`].
///
/// AmplifyBackend mutations are asynchronous — that is why each one answers with a
/// `JobId` and why `GetBackendJob` exists — so a job is never already finished when
/// its own call returns. AWS documents no duration for the work, so this span is
/// CHOSEN: long enough that a caller polling straight after the call still observes
/// the transient statuses, short enough that a test need not wait long for completion.
pub const JOB_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// How many job records one app retains.
///
/// A job record outlives the environment it ran against (so a lookup after
/// `DeleteBackend` resolves) and nothing in this service deletes one, so the map needs a
/// bound: without it, a caller repeating any mutating call would grow this service's
/// state without limit. AWS publishes no retention count for AmplifyBackend jobs, so the
/// value is CHOSEN — large enough that an ordinary client never loses a job it is still
/// polling, small enough to bound the state one app can hold.
pub const MAX_RETAINED_JOBS_PER_APP: usize = 1000;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller, so each growth site carries a cap and
// nothing caller-sized reaches the store unchecked. AWS publishes no quota for these
// AmplifyBackend collections, so each constant says it is this emulator's resource
// guard and is set well above any realistic use. A request that would grow a store past
// its cap is REFUSED with the modelled `TooManyRequestsException` (429) before the
// insert, never served by growing.
//
// The product is therefore finite: at most MAX_APPS apps, each with at most
// MAX_ENVIRONMENTS_PER_APP environments, MAX_RETAINED_JOBS_PER_APP jobs and
// MAX_CHALLENGE_TOKENS_PER_APP live challenge tokens, each environment with at most
// MAX_AUTH_RESOURCES_PER_ENVIRONMENT auth resources,
// MAX_API_RESOURCES_PER_ENVIRONMENT API resources and
// MAX_STORAGE_RESOURCES_PER_ENVIRONMENT storage resources, and every identifier that
// keys one of them at most MAX_IDENTIFIER_CHARS characters.
// ---------------------------------------------------------------------------

/// Emulator resource guard, NOT an AWS quota: the most Amplify apps this emulator holds
/// a backend for. No operation in this slice reclaims an app record, so the map needs a
/// ceiling.
pub const MAX_APPS: usize = 200;

/// Emulator resource guard, NOT an AWS quota: the most backend environments one app may
/// hold at once. `DeleteBackend` reclaims one, so this is a ceiling, not a lifetime
/// total.
pub const MAX_ENVIRONMENTS_PER_APP: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the most auth resources one backend
/// environment may hold at once.
///
/// Not reached on this channel today: `CreateBackendAuth` is withdrawn from SDK and AWS
/// CLI callers (see `src/ops/create_backend_auth.rs`) and the importing and updating
/// siblings are still scaffolded, so nothing writes [`BackendEnvironment::auth`] and
/// `GetBackendAuth` answers the recorded not-found. The bound stays beside the map it
/// guards, for whichever operation writes it next.
pub const MAX_AUTH_RESOURCES_PER_ENVIRONMENT: usize = 25;

/// Emulator resource guard, NOT an AWS quota: the most API resources one backend
/// environment may hold at once. The `ResourceName` a caller supplies keys the map, so
/// the COUNT needs a ceiling as well as the key length.
pub const MAX_API_RESOURCES_PER_ENVIRONMENT: usize = 25;

/// Emulator resource guard, NOT an AWS quota: the most storage resources one backend
/// environment may hold at once. See [`MAX_API_RESOURCES_PER_ENVIRONMENT`].
pub const MAX_STORAGE_RESOURCES_PER_ENVIRONMENT: usize = 25;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied identifier
/// (`AppId`, `BackendEnvironmentName`, `ResourceName`) this emulator will key a record
/// by, so a bounded COUNT of records cannot hold an unbounded amount of text.
pub const MAX_IDENTIFIER_CHARS: usize = 256;

/// Emulator resource guard, NOT an AWS quota: the most LIVE challenge tokens one app may
/// hold at once. A token is reclaimed by `DeleteToken` or by its own expiry, so this is a
/// ceiling on what is live, not a lifetime total.
pub const MAX_CHALLENGE_TOKENS_PER_APP: usize = 100;

/// `TooManyRequestsException$LimitType` for the apps bound.
pub const LIMIT_TYPE_APPS: &str = "apps";
/// `TooManyRequestsException$LimitType` for the environments-per-app bound.
pub const LIMIT_TYPE_ENVIRONMENTS: &str = "backend-environments-per-app";
/// `TooManyRequestsException$LimitType` for the auth-resources-per-environment bound.
pub const LIMIT_TYPE_AUTH_RESOURCES: &str = "auth-resources-per-backend-environment";
/// `TooManyRequestsException$LimitType` for the api-resources-per-environment bound.
pub const LIMIT_TYPE_API_RESOURCES: &str = "api-resources-per-backend-environment";
/// `TooManyRequestsException$LimitType` for the storage-resources-per-environment bound.
pub const LIMIT_TYPE_STORAGE_RESOURCES: &str = "storage-resources-per-backend-environment";
/// `TooManyRequestsException$LimitType` for the live-challenge-tokens-per-app bound.
pub const LIMIT_TYPE_CHALLENGE_TOKENS: &str = "challenge-tokens-per-app";

/// The modelled `TooManyRequestsException` for a request that would grow a store past
/// its cap — "An error that is returned when a limit of a specific type has been
/// exceeded". `limit_type` names WHICH limit, as the modelled member asks; `sentence` is
/// this emulator's own sentence, built from the bound so the caller can see what it hit,
/// and this function puts the service's fixed [`ERROR_MESSAGE_PREFIX`] in front of it.
pub fn limit_exceeded(limit_type: &str, sentence: impl AsRef<str>) -> TooManyRequestsException {
    TooManyRequestsException {
        limit_type: Some(limit_type.to_string()),
        message: Some(prefixed(sentence)),
    }
}

/// Refuse an identifier longer than [`MAX_IDENTIFIER_CHARS`], naming the member and the
/// bound it broke so the caller can fix the request from the message alone — behind the
/// service's fixed [`ERROR_MESSAGE_PREFIX`], like every other message this service
/// renders.
///
/// `Err` carries the modelled `BadRequestException` payload: an over-long identifier is a
/// request that is "not formed properly", which is what that error says.
pub fn bound_identifier(member: &str, value: &str) -> Result<(), BadRequestException> {
    let chars = value.chars().count();
    if chars > MAX_IDENTIFIER_CHARS {
        return Err(bad_request(format!(
            "{member} may be at most {MAX_IDENTIFIER_CHARS} characters, got {chars}."
        )));
    }
    Ok(())
}

/// The `Operation` a job records, per the operation that recorded it. The value is the
/// AmplifyBackend operation's own name, which is what `GetBackendJob`/`ListBackendJobs`
/// report.
pub const OPERATION_CREATE_BACKEND: &str = "CreateBackend";
/// See [`OPERATION_CREATE_BACKEND`].
pub const OPERATION_CREATE_BACKEND_AUTH: &str = "CreateBackendAuth";
/// See [`OPERATION_CREATE_BACKEND`].
pub const OPERATION_DELETE_BACKEND: &str = "DeleteBackend";
/// See [`OPERATION_CREATE_BACKEND`].
pub const OPERATION_REMOVE_ALL_BACKENDS: &str = "RemoveAllBackends";

/// The `BackendEnvironmentName` a job records when the call it answers for addresses the
/// WHOLE app rather than one of its environments (`RemoveAllBackends`): no name at all.
///
/// A job is addressed by (app, environment, job id) on every read path this service has —
/// `GetBackendJob` and `ListBackendJobs` both bind the environment name as a required
/// `@httpLabel`, and `RemoveAllBackends`'s own output carries no environment member — so
/// an app-wide job belongs to none of them and is recorded under a name no request can
/// address. Naming one of the environments it removed instead would claim the job was
/// that environment's, and would surface it in that environment's job list after the
/// environment itself is gone.
pub const JOB_ENVIRONMENT_APP_WIDE: &str = "";

/// Render an instant the way this service reports `CreateTime` / `UpdateTime`:
/// RFC-3339 in UTC, which is what `restJson1` carries for a timestamp rendered as a
/// string member.
pub fn format_time(at: SystemTime) -> String {
    // `fmt` fails only for an instant outside the representable year range, which no
    // value derived from `SystemTime::now()` is. A clock that far out is reported as
    // the epoch rather than panicking inside a handler.
    DateTime::from(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

// ---------------------------------------------------------------------------
// Admin UI challenge tokens
//
// `CreateToken` mints a one-time challenge code for signing in to an app's Amplify
// Admin UI, `GetToken` reports the minted token back (every member but the lifetime —
// see [`READ_PATH_TTL`]), and `DeleteToken` reclaims it.
// A token is NOT part of a backend environment — it addresses an app directly
// (`/backend/{AppId}/challenge`) and AmplifyBackend mints one whether or not a backend
// was ever created for that app — so the tokens live in their own map rather than on
// [`BackendApp`], which only exists for an app `CreateBackend` was called for.
// ---------------------------------------------------------------------------

/// How long a challenge code minted by `CreateToken` stays usable, and the value
/// `CreateToken` reports as `Ttl` (the READ path reports none — see [`READ_PATH_TTL`]).
///
/// CHOSEN, not documented: AWS publishes no lifetime for an Amplify Admin UI challenge
/// code. The span is short, the way a one-time sign-in code's is, and long enough that a
/// client that mints a code, opens the Admin UI and polls `GetToken` for it still finds
/// it.
pub const CHALLENGE_TOKEN_TTL: Duration = Duration::from_secs(300);

/// How many characters of store-minted secret material a challenge code carries.
///
/// CHOSEN: AWS publishes no length for the code. 20 characters of the credential store's
/// 64-symbol alphabet is ~120 bits, so a code cannot be guessed inside its lifetime.
pub const CHALLENGE_CODE_CHARS: usize = 20;

/// The `Ttl` a challenge-token READ reports: the empty string.
///
/// Recorded behaviour, not modelled: `GetToken` and `CreateToken` share an output shape
/// whose `Ttl` is a bare `__string` with no constraint and no documented read-path value,
/// and the recording shows the read NOT echoing the lifetime — `AppId`, `SessionId` and
/// `ChallengeCode` round-trip unchanged while `Ttl` comes back empty. Only the call that
/// GRANTS the lifetime reports it ([`ChallengeToken::ttl`], used by `CreateToken`), so a
/// client cannot read a fresh-looking lifetime off a read and mistake it for a liveness
/// guarantee.
///
/// PRESENT and empty, not absent: the recorded member is the empty string, so the read
/// path writes `Some(READ_PATH_TTL.to_string())` rather than `None`, which would omit the
/// member from the document altogether.
pub const READ_PATH_TTL: &str = "";

/// One live Admin UI challenge token: the code itself and when it stops being usable.
#[derive(Debug, Clone)]
pub struct ChallengeToken {
    /// The app the token authenticates into.
    pub app_id: String,
    /// The unique id `CreateToken` handed back, which addresses the token afterwards.
    pub session_id: String,
    /// The one-time challenge code. Minted by the emulator's credential store (see
    /// [`mint_challenge_token`]), never constructed here.
    pub challenge_code: String,
    /// When the token was minted.
    pub created_at: SystemTime,
    /// When the code stops being usable: [`CHALLENGE_TOKEN_TTL`] after
    /// [`Self::created_at`].
    pub expires_at: SystemTime,
}

impl ChallengeToken {
    /// A token minted at `now`, expiring [`CHALLENGE_TOKEN_TTL`] later.
    pub fn new(
        app_id: &str,
        session_id: String,
        challenge_code: String,
        now: SystemTime,
    ) -> Self {
        Self {
            app_id: app_id.to_string(),
            session_id,
            challenge_code,
            created_at: now,
            expires_at: now + CHALLENGE_TOKEN_TTL,
        }
    }

    /// Whether the code is still usable at `now`.
    pub fn is_live(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }

    /// The `Ttl` member AS THE MINT REPORTS IT: this code's LIFETIME — the whole seconds
    /// between the instant it was minted and the instant it expires — as a decimal
    /// string. `CreateToken` is the only caller: the READ path reports no lifetime at all
    /// ([`READ_PATH_TTL`]).
    ///
    /// A lifetime, not a countdown. The model does NOT fix this: it documents the member
    /// only as "The expiry time for the one-time generated token code" and gives it no
    /// format, no pattern and no units, so the member's NAME — `Ttl`, a time to live — is
    /// the ground for reporting a span in seconds, and this emulator's choice is recorded
    /// here rather than claimed from the model. The value is read from the token's own two
    /// instants, so every mint of one token carries the same value. What this service
    /// enforces is the EXPIRY INSTANT
    /// ([`Self::expires_at`], applied by
    /// [`ComAmazonawsAmplifybackendAmplifybackendState::expire_challenge_tokens`]) —
    /// `created_at + ttl`.
    pub fn ttl(&self) -> String {
        self.expires_at
            .duration_since(self.created_at)
            .unwrap_or(CHALLENGE_TOKEN_TTL)
            .as_secs()
            .to_string()
    }
}

/// A fresh challenge code, and the store-minted random text a `SessionId` is built from.
///
/// The challenge code is a secret handed to a caller — it signs the holder in to the
/// Amplify Admin UI — and nothing in a service may construct secret material itself, so
/// it is minted by the one component that generates secrets, the emulator's credential
/// store. It authenticates to the Admin UI and NOT to this emulator, so the credential is
/// removed from the store again straight away: the caller gets the code, the emulator
/// keeps no credential it would accept as SigV4, and nothing accumulates per call.
///
/// The second half is the credential's own ID — the store-generated, NON-secret part of a
/// minted credential. It is what
/// [`ComAmazonawsAmplifybackendAmplifybackendState::mint_session_id`] builds the session
/// id from, because a session id must not be guessable: `GetToken` resolves a token by
/// `(AppId, SessionId)`, so a predictable session id would be a predictable address for
/// somebody else's challenge code.
///
/// `None` when the store minted no session-token secret, which leaves `CreateToken` to
/// refuse rather than report a code it made up — with
/// [`CHALLENGE_CODE_UNAVAILABLE_SENTENCE`].
pub fn mint_challenge_token(
    store: &::emulator_server::CredentialStore,
) -> Option<(String, String)> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let challenge_code: Option<String> = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(|secret| secret.chars().take(CHALLENGE_CODE_CHARS).collect());
    // Reclaimed on EVERY path, before the `?` below: the code is not a credential of this
    // emulator (see above), so a mint that yielded no secret must not leave one behind
    // either.
    let _ = store.delete(&minted.id);
    Some((minted.id, challenge_code?))
}

/// The sentence a `CreateToken` the credential store minted no challenge code for is
/// answered with, WITHOUT the prefix [`gateway_timeout`] adds. A failure on this service's
/// own side, like [`STORE_UNAVAILABLE_SENTENCE`]: nothing in the request decides it.
pub const CHALLENGE_CODE_UNAVAILABLE_SENTENCE: &str =
    "No challenge code could be minted for this request.";

/// Per-service state for com-amazonaws-amplifybackend-amplifybackend. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifybackendAmplifybackendState {
    /// Every Amplify app a backend has been created for, keyed by `AppId`.
    pub apps: BTreeMap<String, BackendApp>,
    /// The LIVE Admin UI challenge tokens, keyed by `AppId` and then by `SessionId`.
    /// Separate from [`Self::apps`] because a challenge token addresses an app, not a
    /// backend environment, and is minted whether or not the app has a backend.
    pub challenge_tokens: BTreeMap<String, BTreeMap<String, ChallengeToken>>,
    /// The next sequence number [`Self::mint_session_id`] hands out, so two session ids
    /// are distinct even if the credential store repeats the random text they carry.
    pub next_challenge_session_seq: u64,
    /// The next job id [`Self::mint_app_wide_job_id`] hands out, for a call whose job this
    /// service retains no record of because the project it answered for holds no backend.
    pub next_app_wide_job_seq: u64,
}

impl ComAmazonawsAmplifybackendAmplifybackendState {
    /// The app `app_id` names, or `None` when no backend was ever created for it.
    pub fn app(&self, app_id: &str) -> Option<&BackendApp> {
        self.apps.get(app_id)
    }

    /// As [`Self::app`], for a caller that mutates the app.
    pub fn app_mut(&mut self, app_id: &str) -> Option<&mut BackendApp> {
        self.apps.get_mut(app_id)
    }

    /// Whether a NEW app record may be created for `app_id`: an app that already exists
    /// always may (nothing grows), a new one only while the store is under [`MAX_APPS`].
    /// Call this BEFORE [`Self::app_or_create`].
    pub fn has_room_for_new_app(&self, app_id: &str) -> bool {
        self.apps.contains_key(app_id) || self.apps.len() < MAX_APPS
    }

    /// The app `app_id` names, created with `app_name` when absent. `CreateBackend` is
    /// the only operation that may bring an app record into being — every other one
    /// answers for an app that has no backend record as the missing resource it is.
    ///
    /// The record is this service's BACKEND record for the app, not the app itself: the
    /// app's existence is settled against Amplify's registry before this is reached
    /// ([`require_existing_app`]), so a record here is only ever created for an app that
    /// exists.
    pub fn app_or_create(&mut self, app_id: &str, app_name: &str) -> &mut BackendApp {
        let app = self
            .apps
            .entry(app_id.to_string())
            .or_insert_with(|| BackendApp::new(app_id, app_name));
        // `CreateBackend` carries the app's name as a required member, so a later call
        // for the same app restates it.
        app.app_name = app_name.to_string();
        app
    }

    /// Forget the app `app_id` names — its environments, its job records and the live
    /// challenge tokens minted for it — answering whether an app record was held.
    ///
    /// Everything this service keys by an app id goes in one call, so no map keeps a
    /// record of an app the store no longer holds: [`Self::apps`] owns the environments
    /// and the jobs, while [`Self::challenge_tokens`] is a second map keyed by the same id
    /// and would otherwise retain a token addressing an app that is gone.
    pub fn remove_app(&mut self, app_id: &str) -> bool {
        self.challenge_tokens.remove(app_id);
        self.apps.remove(app_id).is_some()
    }

    /// Mint a job id for a call that had nothing to remove: an app-wide job of an Amplify
    /// project this service holds NO backend record for.
    ///
    /// Such a job has nowhere to live — a job record is retained on the app's backend
    /// record ([`BackendApp::jobs`]), which is where `GetBackendJob` and `ListBackendJobs`
    /// read it from, and the project has none — so the id is minted from the store's own
    /// sequence and no record is kept. Two calls therefore never report the same id, and
    /// no app record is brought into being for a project that has no backend: a later read
    /// answers about the project exactly as it did before the call.
    ///
    /// A caller that has not looked the app up yet may take an id and not use it: this
    /// hands out a sequence number, so an unused one costs nothing but the number.
    pub fn mint_app_wide_job_id(&mut self, now: SystemTime) -> String {
        mint_job_id(now, &mut self.next_app_wide_job_seq)
    }

    /// Mint a `SessionId` for a challenge token: the store-minted random text from
    /// [`mint_challenge_token`], lower-cased, plus this store's own sequence number.
    ///
    /// Both halves are needed. The random half is what makes a session id UNGUESSABLE,
    /// which matters because `GetToken` resolves a challenge code by `(AppId, SessionId)`.
    /// The sequence half makes uniqueness EXACT rather than probabilistic: the credential
    /// store only avoids ids it currently holds, and `mint_challenge_token` releases the
    /// credential as soon as it has the code, so the random text alone could repeat.
    pub fn mint_session_id(&mut self, minted_credential_id: &str) -> String {
        let sequence = self.next_challenge_session_seq;
        self.next_challenge_session_seq = sequence.saturating_add(1);
        format!(
            "{}-{sequence:016x}",
            minted_credential_id.to_ascii_lowercase()
        )
    }

    /// Materialise the expiry the clock has ALREADY made due: forget every challenge
    /// token whose [`CHALLENGE_TOKEN_TTL`] has run out, and the app entries that leaves
    /// empty.
    ///
    /// Owed at each token's `expires_at` whether or not anybody called, so every handler
    /// of a token operation runs this first and then reads only live tokens. Idempotent.
    pub fn expire_challenge_tokens(&mut self, now: SystemTime) {
        for sessions in self.challenge_tokens.values_mut() {
            sessions.retain(|_, token| token.is_live(now));
        }
        self.challenge_tokens.retain(|_, sessions| !sessions.is_empty());
    }

    /// The live challenge token `(app_id, session_id)` addresses, or `None` when this
    /// service holds none. Call [`Self::expire_challenge_tokens`] first so an expired
    /// token is not reported.
    pub fn challenge_token(&self, app_id: &str, session_id: &str) -> Option<&ChallengeToken> {
        self.challenge_tokens
            .get(app_id)
            .and_then(|sessions| sessions.get(session_id))
    }

    /// Whether a token may be minted for an app the token map does not hold yet: one it
    /// already holds always may (nothing grows), a new one only while the map is under
    /// [`MAX_APPS`]. Call this BEFORE [`Self::store_challenge_token`].
    pub fn has_room_for_new_challenge_token_app(&self, app_id: &str) -> bool {
        self.challenge_tokens.contains_key(app_id) || self.challenge_tokens.len() < MAX_APPS
    }

    /// Whether one more LIVE token may be minted for `app_id`, i.e. whether the app is
    /// under [`MAX_CHALLENGE_TOKENS_PER_APP`]. Call this BEFORE
    /// [`Self::store_challenge_token`], on a store whose expired tokens are already
    /// forgotten, so a caller is never refused over tokens the clock has retired.
    pub fn has_room_for_new_challenge_token(&self, app_id: &str) -> bool {
        self.challenge_tokens
            .get(app_id)
            .is_none_or(|sessions| sessions.len() < MAX_CHALLENGE_TOKENS_PER_APP)
    }

    /// Store `token` and hand the stored record back, so the response reports exactly
    /// what a later `GetToken` will.
    pub fn store_challenge_token(&mut self, token: ChallengeToken) -> &ChallengeToken {
        let sessions = self.challenge_tokens.entry(token.app_id.clone()).or_default();
        match sessions.entry(token.session_id.clone()) {
            BTreeEntry::Vacant(slot) => slot.insert(token),
            // A session id is a fresh UUID per mint, so this arm is not reached by
            // `CreateToken`; it keeps the insert total rather than leaving a panic path.
            BTreeEntry::Occupied(mut slot) => {
                slot.insert(token);
                slot.into_mut()
            }
        }
    }

    /// Forget the token `(app_id, session_id)` addresses, answering whether one was
    /// held. `DeleteToken` is what reclaims a token before its expiry.
    pub fn remove_challenge_token(&mut self, app_id: &str, session_id: &str) -> bool {
        let Some(sessions) = self.challenge_tokens.get_mut(app_id) else {
            return false;
        };
        let removed = sessions.remove(session_id).is_some();
        if sessions.is_empty() {
            self.challenge_tokens.remove(app_id);
        }
        removed
    }
}

/// Mint a job id from `next_seq`: a decimal epoch-millisecond value, bumped past the last
/// one handed out so ids are unique and increasing even when two jobs are minted inside
/// one millisecond.
///
/// One implementation for every job id this service hands out, so the id a caller gets is
/// the same shape whichever sequence it came from: an app's own
/// ([`BackendApp::next_job_seq`]) or the store's
/// ([`ComAmazonawsAmplifybackendAmplifybackendState::next_app_wide_job_seq`]).
fn mint_job_id(now: SystemTime, next_seq: &mut u64) -> String {
    let millis = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let candidate = u64::try_from(millis).unwrap_or(u64::MAX);
    let id = candidate.max(*next_seq);
    *next_seq = id.saturating_add(1);
    id.to_string()
}

/// One Amplify app's backend: its name, the environments created for it, and the jobs
/// recorded against it.
#[derive(Debug, Default)]
pub struct BackendApp {
    /// The app id this record is keyed by.
    pub app_id: String,
    /// The app name, as `CreateBackend` stated it.
    pub app_name: String,
    /// The backend environments that currently exist, keyed by `BackendEnvironmentName`.
    pub environments: BTreeMap<String, BackendEnvironment>,
    /// Every job recorded for this app, keyed by `JobId`. Jobs OUTLIVE the environment
    /// they ran against, so a job lookup after `DeleteBackend` still resolves.
    pub jobs: BTreeMap<String, BackendJob>,
    /// The next job id to hand out, as a decimal epoch-millisecond value. Kept so two
    /// jobs minted in the same millisecond still get distinct, increasing ids.
    pub next_job_seq: u64,
}

impl BackendApp {
    /// A new app record with no environments and no jobs.
    pub fn new(app_id: &str, app_name: &str) -> Self {
        Self {
            app_id: app_id.to_string(),
            app_name: app_name.to_string(),
            environments: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_job_seq: 0,
        }
    }

    /// The environment `name` names, or `None` when it does not exist (never created,
    /// or removed by `DeleteBackend`).
    pub fn environment(&self, name: &str) -> Option<&BackendEnvironment> {
        self.environments.get(name)
    }

    /// As [`Self::environment`], for a caller that mutates the environment.
    pub fn environment_mut(&mut self, name: &str) -> Option<&mut BackendEnvironment> {
        self.environments.get_mut(name)
    }

    /// Whether a NEW environment may be created under `name`: one that already exists
    /// always may (nothing grows), a new one only while the app is under
    /// [`MAX_ENVIRONMENTS_PER_APP`]. Call this BEFORE inserting.
    pub fn has_room_for_new_environment(&self, name: &str) -> bool {
        self.environments.contains_key(name) || self.environments.len() < MAX_ENVIRONMENTS_PER_APP
    }

    /// The environment names this app currently owns, in the order `GetBackend`
    /// reports them.
    pub fn environment_names(&self) -> Vec<String> {
        self.environments.keys().cloned().collect()
    }

    /// Mint a job id: a decimal epoch-millisecond value, bumped past the last one
    /// handed out so ids are unique and increasing even within one millisecond.
    fn mint_job_id(&mut self, now: SystemTime) -> String {
        mint_job_id(now, &mut self.next_job_seq)
    }

    /// Apply the job map's bound: reclaim this app's OLDEST job records until there is
    /// room for one more, so the map never holds more than
    /// [`MAX_RETAINED_JOBS_PER_APP`].
    ///
    /// Public and called from each mutating handler's pre-insert guard block, beside the
    /// app and environment count bounds, so every store a request can grow is bounded in
    /// one visible place. [`Self::record_job`] applies it too, so the bound holds however
    /// a job is recorded; a second call is a no-op.
    ///
    /// Reclaiming rather than refusing is deliberate: a job record is a receipt for work
    /// already accepted, so an app that has run many jobs must stay usable. Refusing the
    /// 1001st mutation would wedge the app forever.
    pub fn reclaim_job_capacity(&mut self) {
        self.evict_oldest_jobs(MAX_RETAINED_JOBS_PER_APP.saturating_sub(1));
    }

    /// Record a job for `operation` against `environment_name`, minting and STORING its
    /// id, and hand back the stored record. The job starts [`JOB_STATUS_PENDING`], reports
    /// [`JOB_STATUS_RUNNING`] once [`JOB_START_DELAY`] has passed and
    /// [`JOB_STATUS_COMPLETED`] once [`JOB_COMPLETION_DELAY`] has (see
    /// [`BackendJob::settle`]).
    ///
    /// The job map is BOUNDED, not growing: recording a job first reclaims the app's
    /// oldest ones through [`Self::reclaim_job_capacity`], so the map holds at most
    /// [`MAX_RETAINED_JOBS_PER_APP`] records however many times this is called.
    pub fn record_job(
        &mut self,
        environment_name: &str,
        operation: &str,
        now: SystemTime,
    ) -> &BackendJob {
        let job_id = self.mint_job_id(now);
        let job = BackendJob {
            job_id: job_id.clone(),
            app_id: self.app_id.clone(),
            backend_environment_name: environment_name.to_string(),
            operation: operation.to_string(),
            status: JOB_STATUS_PENDING.to_string(),
            error: None,
            create_time: now,
            update_time: now,
            starts_at: now + JOB_START_DELAY,
            completes_at: now + JOB_COMPLETION_DELAY,
        };
        // Reclaim BEFORE inserting, leaving room for this one record, so the job being
        // recorded is never itself a candidate for eviction: a wall clock that stepped
        // backwards would otherwise make the newest record the oldest. The insert is the
        // last write, so the record handed back is always present.
        self.reclaim_job_capacity();
        self.jobs.insert(job_id.clone(), job);
        self.jobs
            .get(&job_id)
            .expect("the job inserted on the line above is in the map")
    }

    /// Drop the oldest job records until this app holds at most `keep` of them.
    ///
    /// A job is retained past the environment it ran against so a later lookup still
    /// resolves, but it is retained BOUNDEDLY: no operation in this service reclaims a
    /// job (`DeleteBackend` adds one), so without a cap a caller repeating a mutating
    /// call would grow this map without limit. The oldest record goes first, which keeps
    /// the jobs a caller is still polling.
    fn evict_oldest_jobs(&mut self, keep: usize) {
        while self.jobs.len() > keep {
            // Ordered by creation instant, then by id so two jobs minted in the same
            // millisecond still evict in a defined order (the id increases with age;
            // compared by length first, since a longer decimal id is the larger value).
            // `min_by` over BORROWED keys: only the victim's id is cloned, so an
            // eviction does not allocate once per retained job.
            let oldest = self
                .jobs
                .iter()
                .min_by(|(left_id, left), (right_id, right)| {
                    left.create_time
                        .cmp(&right.create_time)
                        .then_with(|| left_id.len().cmp(&right_id.len()))
                        .then_with(|| left_id.cmp(right_id))
                })
                .map(|(job_id, _)| job_id.clone());
            match oldest {
                Some(job_id) => {
                    self.jobs.remove(&job_id);
                }
                None => break,
            }
        }
    }

    /// The job `job_id` names, when it belongs to `environment_name`. A job is
    /// addressed by (app, environment, job id) on the wire, so a job id that exists
    /// under a DIFFERENT environment is not found.
    pub fn job_mut(&mut self, environment_name: &str, job_id: &str) -> Option<&mut BackendJob> {
        self.jobs
            .get_mut(job_id)
            .filter(|job| job.backend_environment_name == environment_name)
    }
}

/// One backend environment of an app, and the resources created inside it.
#[derive(Debug)]
pub struct BackendEnvironment {
    /// The environment name this record is keyed by.
    pub name: String,
    /// The `ResourceConfig` `CreateBackend` carried, when it carried one.
    pub resource_config: Option<ResourceConfig>,
    /// The `ResourceName` `CreateBackend` carried, when it carried one.
    pub resource_name: Option<String>,
    /// Auth resources, keyed by `ResourceName`; stored verbatim so `GetBackendAuth`
    /// reports exactly what `CreateBackendAuth` was given.
    pub auth: BTreeMap<String, CreateBackendAuthResourceConfig>,
    /// API resources, keyed by `ResourceName`. Bounded by
    /// [`MAX_API_RESOURCES_PER_ENVIRONMENT`]: an inserting handler asks
    /// [`Self::has_room_for_new_api_resource`] first.
    pub api: BTreeMap<String, BackendAPIResourceConfig>,
    /// Storage resources, keyed by `ResourceName`. Bounded by
    /// [`MAX_STORAGE_RESOURCES_PER_ENVIRONMENT`]: an inserting handler asks
    /// [`Self::has_room_for_new_storage_resource`] first.
    pub storage: BTreeMap<String, CreateBackendStorageResourceConfig>,
    /// When the environment was created.
    pub created_at: SystemTime,
}

impl BackendEnvironment {
    /// Whether a NEW auth resource may be stored under `resource_name`: replacing one
    /// that already exists always may (nothing grows), a new one only while the
    /// environment is under [`MAX_AUTH_RESOURCES_PER_ENVIRONMENT`]. Call this BEFORE
    /// inserting.
    pub fn has_room_for_new_auth_resource(&self, resource_name: &str) -> bool {
        self.auth.contains_key(resource_name)
            || self.auth.len() < MAX_AUTH_RESOURCES_PER_ENVIRONMENT
    }

    /// Whether a NEW API resource may be stored under `resource_name`; as
    /// [`Self::has_room_for_new_auth_resource`], for [`Self::api`] and
    /// [`MAX_API_RESOURCES_PER_ENVIRONMENT`]. Call this BEFORE inserting, and refuse an
    /// over-cap insert with the modelled `TooManyRequestsException`
    /// ([`LIMIT_TYPE_API_RESOURCES`]).
    pub fn has_room_for_new_api_resource(&self, resource_name: &str) -> bool {
        self.api.contains_key(resource_name) || self.api.len() < MAX_API_RESOURCES_PER_ENVIRONMENT
    }

    /// Whether a NEW storage resource may be stored under `resource_name`; as
    /// [`Self::has_room_for_new_auth_resource`], for [`Self::storage`] and
    /// [`MAX_STORAGE_RESOURCES_PER_ENVIRONMENT`]. Call this BEFORE inserting, and refuse
    /// an over-cap insert with the modelled `TooManyRequestsException`
    /// ([`LIMIT_TYPE_STORAGE_RESOURCES`]).
    pub fn has_room_for_new_storage_resource(&self, resource_name: &str) -> bool {
        self.storage.contains_key(resource_name)
            || self.storage.len() < MAX_STORAGE_RESOURCES_PER_ENVIRONMENT
    }

    /// A new, empty environment created at `created_at`.
    pub fn new(name: &str, created_at: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            resource_config: None,
            resource_name: None,
            auth: BTreeMap::new(),
            api: BTreeMap::new(),
            storage: BTreeMap::new(),
            created_at,
        }
    }
}

/// One async job: what it was, which environment it ran against, and when.
#[derive(Debug)]
pub struct BackendJob {
    /// The id the job was minted with and is addressed by.
    pub job_id: String,
    /// The app the job belongs to.
    pub app_id: String,
    /// The environment the job ran against. Kept on the job because the environment
    /// record itself is removed by `DeleteBackend`.
    pub backend_environment_name: String,
    /// The AmplifyBackend operation that recorded the job.
    pub operation: String,
    /// The job's status: see [`JOB_STATUS_PENDING`], [`JOB_STATUS_RUNNING`],
    /// [`JOB_STATUS_COMPLETED`] and [`JOB_STATUS_FAILED`].
    pub status: String,
    /// Why the job failed, absent while it has not.
    pub error: Option<String>,
    /// When the job was recorded.
    pub create_time: SystemTime,
    /// When the job last changed. Never earlier than [`Self::create_time`].
    pub update_time: SystemTime,
    /// The instant the job's work starts; until then it reports [`JOB_STATUS_PENDING`].
    pub starts_at: SystemTime,
    /// The instant the job's work becomes due; between [`Self::starts_at`] and this one it
    /// reports [`JOB_STATUS_RUNNING`].
    pub completes_at: SystemTime,
}

impl BackendJob {
    /// Materialise the progress the clock has ALREADY made due: the start of the work at
    /// [`Self::starts_at`] and its completion at [`Self::completes_at`].
    ///
    /// This is the one write a reporting operation performs: each transition was owed at
    /// its own instant whether or not anybody read the job, so a reader only records
    /// what the clock decided. Idempotent, and it never moves a job that has reached a
    /// terminal status. `update_time` is set to the instant the transition became due —
    /// not to "now" — so two reads of a job in the same phase report the same value.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != JOB_STATUS_PENDING && self.status != JOB_STATUS_RUNNING {
            return;
        }
        if now >= self.completes_at {
            self.status = JOB_STATUS_COMPLETED.to_string();
            self.update_time = self.completes_at;
        } else if now >= self.starts_at {
            self.status = JOB_STATUS_RUNNING.to_string();
            self.update_time = self.starts_at;
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifybackendAmplifybackendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifybackendAmplifybackendState>)
}
