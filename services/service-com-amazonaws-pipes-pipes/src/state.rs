// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pipes-pipes service.
//!
//! An EventBridge Pipes pipe is a record here: the control plane (the record, its ARN,
//! its lifecycle states and its timestamps) is real and persisted in this module, and
//! what a RUNNING pipe DOES with no client action — poll the source, hand the batch to
//! the enrichment, deliver each event to the target — is [`data_plane`].

pub mod data_plane;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    FilterCriteria, NotFoundException, Pipe, PipeEnrichmentParameters, PipeLogConfiguration,
    PipeSourceParameters, PipeTargetParameters, ValidationException, ValidationExceptionField,
};

/// The `PipeState` members this slice reports. Source: the `PipeState` enum of
/// `smithy-model/com-amazonaws-pipes-pipes.json` (see `PIPE_STATE_VALUES`).
pub mod pipe_state {
    pub const CREATING: &str = "CREATING";
    pub const RUNNING: &str = "RUNNING";
    pub const STARTING: &str = "STARTING";
    pub const STOPPED: &str = "STOPPED";
    pub const STOPPING: &str = "STOPPING";
    pub const DELETING: &str = "DELETING";
    pub const UPDATING: &str = "UPDATING";
}

/// The `PipeState` values that mean "a move this pipe is already making has not landed
/// yet" — every state above that is not one a pipe rests in (`RUNNING`, `STOPPED`).
///
/// A pipe in one of these is mid-transition, so a call that would start ANOTHER
/// transition on it is refused rather than interleaved with the one in flight.
pub const TRANSIENT_PIPE_STATES: &[&str] = &[
    pipe_state::CREATING,
    pipe_state::STARTING,
    pipe_state::STOPPING,
    pipe_state::DELETING,
    pipe_state::UPDATING,
];

/// The states a caller may ask a pipe to be in (`RequestedPipeState`), plus the
/// `DELETED` that only a response reports (`RequestedPipeStateDescribeResponse`).
/// Source: those two enums in `smithy-model/com-amazonaws-pipes-pipes.json`.
pub mod requested_state {
    pub const RUNNING: &str = "RUNNING";
    pub const STOPPED: &str = "STOPPED";
    pub const DELETED: &str = "DELETED";
}

/// The `resourceType` this service names in a `ConflictException` / `NotFoundException`.
pub const PIPE_RESOURCE_TYPE: &str = "pipe";

/// The scheme of the one non-ARN `Source` this service accepts: a self-managed Apache
/// Kafka cluster. Source: the `^smk://…|arn:…$` alternation of `ArnOrUrl`'s pattern in
/// `smithy-model/com-amazonaws-pipes-pipes.json`.
pub const SELF_MANAGED_KAFKA_SCHEME: &str = "smk://";

/// The source type [`PipeRecord::source_type`] reports for a `Source` carrying
/// [`SELF_MANAGED_KAFKA_SCHEME`], which has no ARN service namespace to be named by.
pub const SELF_MANAGED_KAFKA: &str = "selfmanagedkafka";

/// The most pipe records this store holds, so a caller cannot grow the emulator's heap
/// without limit by creating distinct names.
///
/// This is the EMULATOR's bound, not an AWS number (AWS's own soft quota is larger): each
/// record is already size-bounded by the modelled `@length` constraints, so capping the
/// count bounds the store. `CreatePipe` refuses past it with the quota error its model
/// declares for exactly this case, `ServiceQuotaExceededException` (402).
pub const MAX_PIPES: usize = 100;

/// The most tags one pipe carries.
///
/// Source: the `TagResource` documentation in `smithy-model/com-amazonaws-pipes-pipes.json`
/// — "You can associate as many as 50 tags with a pipe." — the same bound as `TagMap`'s
/// `@length(max: 50)`. A single request past the bound is already refused by the boundary
/// validator; `TagResource` re-checks the bound against the tags ALREADY on the pipe, which
/// no per-request constraint can express.
pub const MAX_TAGS_PER_PIPE: usize = 50;

/// The length a tag key must be within: `@length(min: 1, max: 128)` on `TagKey`. Source:
/// the `TagMap` key member of `smithy-model/com-amazonaws-pipes-pipes.json`.
///
/// A constraint on a MAP KEY is not one the boundary validator checks (it walks the
/// collection's own `@length` and its values), so the writes that install tags enforce it
/// themselves — and refuse a key outside it with the `ValidationException` their models
/// declare, exactly as the boundary would have.
pub const TAG_KEY_LENGTH: std::ops::RangeInclusive<usize> = 1..=128;

/// The length a tag value must be within: `@length(min: 0, max: 256)` on `TagValue`.
/// Source: the `TagMap` value member of the same model. The empty string is therefore a
/// value a tag can hold, not a way of removing one.
pub const TAG_VALUE_LENGTH: std::ops::RangeInclusive<usize> = 0..=256;

/// The tag-key prefix AWS reserves for its own tags: "You cannot edit or delete tag keys
/// or values with this prefix. Tags with this prefix do not count against your tags per
/// resource limit." (AWS General Reference, "Tag naming limits and requirements"), so a
/// caller-supplied key carrying it is refused rather than stored. Matched
/// case-insensitively, the way the reservation is stated.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The special characters a tag may carry besides letters, numbers and spaces. Source:
/// the documented tag rule [`TAG_CHARACTER_SET_MESSAGE`] states verbatim.
const TAG_SPECIAL_CHARACTERS: &[char] = &['_', '.', ':', '/', '=', '+', '-', '@'];

/// The sentence a tag carrying a character outside the documented set is refused with.
/// It is a FIXED, documented rule rather than a message built from the request, so it
/// names neither the key nor the value that broke it.
pub const TAG_CHARACTER_SET_MESSAGE: &str =
    "Tags can only contain letters, numbers, spaces, and the following special \
     characters: _ . : / = + - @";

/// Whether `text` is within the documented tag character set: letters, numbers, spaces
/// and [`TAG_SPECIAL_CHARACTERS`]. "Letters" and "numbers" are NOT ASCII-only — a tag
/// written in another script is letters and round-trips — so both are judged by unicode
/// category rather than by byte range.
fn within_tag_character_set(text: &str) -> bool {
    text.chars().all(|character| {
        character.is_alphanumeric()
            || character == ' '
            || TAG_SPECIAL_CHARACTERS.contains(&character)
    })
}

/// The most characters of a tag KEY a refusal echoes back.
///
/// A key reaches a handler UNBOUNDED — the boundary validator walks a map's entry count
/// and its values but not its keys — so a message that interpolated one whole would let a
/// request be amplified into an arbitrarily long response. The same bound the framework's
/// own validation messages elide an echoed value at. A tag VALUE needs no such bound:
/// `TagValue`'s own `@length` is one the validator does walk.
const MAX_ECHOED_KEY: usize = 256;

/// `key` as a refusal echoes it: itself, or its first [`MAX_ECHOED_KEY`] characters with
/// an ellipsis. Cut on a CHARACTER boundary, so a multi-byte key stays valid text.
fn echoed_key(key: &str) -> String {
    if key.chars().count() <= MAX_ECHOED_KEY {
        return key.to_string();
    }
    let kept: String = key.chars().take(MAX_ECHOED_KEY).collect();
    format!("{kept}...")
}

/// Why a tag the request carries is not one this service accepts.
///
/// Every one of these is a rule of the resource-tagging layer that fronts the pipes API,
/// which refuses a tag breaking one with `BadRequestException` (400) rather than with the
/// modelled `ValidationException` the service states its own refusals as — see
/// [`tag_refusal`] for the sentence each is worded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagViolation {
    /// The key is outside `TagKey`'s `@length` bound ([`TAG_KEY_LENGTH`]).
    KeyLength,
    /// The value is outside `TagValue`'s `@length` bound ([`TAG_VALUE_LENGTH`]).
    ValueLength,
    /// The key uses the prefix AWS reserves for its own tags
    /// ([`RESERVED_TAG_KEY_PREFIX`]).
    ReservedKeyPrefix,
    /// The key or the value carries a character outside the documented tag set
    /// ([`TAG_CHARACTER_SET_MESSAGE`]).
    CharacterSet,
}

/// Which rule a tag breaks, or `None` when it breaks none.
///
/// `@length` counts CHARACTERS, not bytes (the framework's own constraint checker counts
/// `chars()`), so a unicode key or value is measured the same way AWS measures it.
pub fn tag_violation(key: &str, value: &str) -> Option<TagViolation> {
    if !TAG_KEY_LENGTH.contains(&key.chars().count()) {
        return Some(TagViolation::KeyLength);
    }
    if key
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
    {
        return Some(TagViolation::ReservedKeyPrefix);
    }
    if !TAG_VALUE_LENGTH.contains(&value.chars().count()) {
        return Some(TagViolation::ValueLength);
    }
    if !within_tag_character_set(key) || !within_tag_character_set(value) {
        return Some(TagViolation::CharacterSet);
    }
    None
}

/// The member a refused tag is reported at by `TagResource`, whose request-validation
/// name for the tags it merges is not the wire member's. Source: the recorded refusal of
/// an over-long tag key, which names this member.
pub const ADDED_TAGS_MEMBER: &str = "addedTags";

/// The member a refused tag KEY is reported at by `UntagResource`, from the same source.
pub const REMOVED_TAG_KEYS_MEMBER: &str = "removedTagKeys";

/// The member a refused tag is reported at by `CreatePipe`. No recorded refusal names it,
/// so it is the model's own member in the casing a validation path uses.
pub const CREATE_TAGS_MEMBER: &str = "tags";

/// Both published bounds of a `@length`-constrained member, in the order and the wording
/// request validation states them in — the maximum first, each as one `Member must …`
/// clause. The bounds come from the model (the constants above), never from a request.
fn length_requirements(bound: &std::ops::RangeInclusive<usize>) -> String {
    format!(
        "Member must have length less than or equal to {}, \
         Member must have length greater than or equal to {}",
        bound.end(),
        bound.start(),
    )
}

/// One refused request as the validation envelope states it: how many clauses, then the
/// clause. The writes below refuse one tag at a time, so the envelope states one clause.
fn one_validation_error(clause: String) -> String {
    format!("1 validation error detected: {clause}")
}

/// The tags a request carried, as a validation message echoes the member's value: every
/// entry in key order, `{k1=v1, k2=v2}`. What such a message echoes is the value of the
/// MEMBER, not only the entry that broke the rule.
///
/// Each KEY is elided to what a refusal may echo (a key arrives unbounded); each value is
/// interpolated as given, being already within `TagValue`'s own bound — or, when it is
/// what broke that bound, already rendered by the validator that refused it.
pub fn echoed_tags<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let entries: Vec<String> = tags
        .into_iter()
        .map(|(key, value)| format!("{}={value}", echoed_key(key)))
        .collect();
    format!("{{{}}}", entries.join(", "))
}

/// The message a write that installs tags refuses `violation` with, for the `member` the
/// tags arrived on (the operations name it differently, so the caller passes its own).
/// `key` is the tag that broke the rule and `tags_shown` the member's value as
/// [`echoed_tags`] renders it.
///
/// A violated published BOUND is stated as request validation states one — the member,
/// its value, and the bounds that value had to satisfy, all from this request and the
/// model. The character-set and reserved-prefix rules are not modelled constraints, so
/// each is stated as the rule itself.
pub fn tag_refusal(
    member: &str,
    violation: TagViolation,
    key: &str,
    tags_shown: &str,
) -> String {
    match violation {
        TagViolation::KeyLength => one_validation_error(format!(
            "Value '{tags_shown}' at '{member}' failed to satisfy constraint: \
             Map keys must satisfy constraint: [{}]",
            length_requirements(&TAG_KEY_LENGTH),
        )),
        TagViolation::ValueLength => one_validation_error(format!(
            "Value '{tags_shown}' at '{member}' failed to satisfy constraint: \
             Map value must satisfy constraint: [{}]",
            length_requirements(&TAG_VALUE_LENGTH),
        )),
        TagViolation::ReservedKeyPrefix => format!(
            "Tag key '{}' uses the prefix '{RESERVED_TAG_KEY_PREFIX}', which is reserved \
             for Amazon Web Services use.",
            echoed_key(key),
        ),
        TagViolation::CharacterSet => TAG_CHARACTER_SET_MESSAGE.to_string(),
    }
}

/// The message the tagging layer refuses a tag KEY named for REMOVAL with when it is
/// outside `TagKey`'s bound: the same envelope as [`tag_refusal`], with the key list as
/// the value request validation echoes for a list member.
///
/// `key` is interpolated as given: a key named for removal travels as a list ELEMENT,
/// which the boundary validator does walk, so it reaches this wording already bounded by
/// that validator's own rendering.
pub fn tag_key_refusal(member: &str, key: &str) -> String {
    one_validation_error(format!(
        "Value '[{key}]' at '{member}' failed to satisfy constraint: \
         Member must satisfy constraint: [{}]",
        length_requirements(&TAG_KEY_LENGTH),
    ))
}

/// One entry of a [`validation_error`]'s list: the request field that was refused and why.
pub fn validation_field(
    name: impl Into<String>,
    message: impl Into<String>,
) -> ValidationExceptionField {
    ValidationExceptionField {
        name: name.into(),
        message: message.into(),
    }
}

/// The `ValidationException` this service refuses a request with: the fields that failed
/// validation reported BOTH as the modelled `fieldList` and, in the exception's `message`,
/// as the JSON document this service words a refusal as — a `ValidationErrorFields` array
/// whose entries carry each field's `Name` and `Message`. Both are built from the one list,
/// so a client that parses the document and one that reads `fieldList` are told the same
/// thing.
pub fn validation_error(fields: Vec<ValidationExceptionField>) -> ValidationException {
    // Written out rather than serialized from a map so the two keys keep the order this
    // service reports them in; the values go through `serde_json` so any character they
    // carry is escaped as JSON requires.
    let entries = fields
        .iter()
        .map(|field| {
            format!(
                r#"{{"Name":{},"Message":{}}}"#,
                ::serde_json::Value::String(field.name.clone()),
                ::serde_json::Value::String(field.message.clone()),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    ValidationException {
        message: Some(format!(r#"{{"ValidationErrorFields":[{entries}]}}"#)),
        field_list: Some(fields),
    }
}

/// The `NotFoundException` the NAME-addressed operations of this service answer with for
/// a pipe this account does not hold.
///
/// The message is built from the request — the pipe the caller named interpolated into ONE
/// fixed sentence — so an absent pipe reads the same whichever of those operations was
/// asked for it. The ARN-addressed tagging operations answer [`resource_not_found`]
/// instead.
pub fn pipe_not_found(pipe: &str) -> NotFoundException {
    NotFoundException {
        message: Some(format!("Pipe {pipe} does not exist.")),
    }
}

/// The sentence the ARN-addressed tagging operations report an unresolvable
/// `resourceArn` with.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The `NotFoundException` the ARN-addressed tagging operations
/// (`TagResource`/`UntagResource`/`ListTagsForResource`) answer with for a `resourceArn`
/// no stored pipe carries.
///
/// A CONSTANT, resource-agnostic sentence: those operations are served by the generic
/// resource-tagging layer in front of this service, which resolved no pipe and so has no
/// name to interpolate — and echoes no part of the ARN it was given back to the caller.
pub fn resource_not_found() -> NotFoundException {
    NotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The `Description` to STORE for the one a request carried.
///
/// `PipeDescription` is `@length(min: 0)`, so the empty string is a value a request may
/// carry — but a pipe holds a description or holds none, and the empty string is how a
/// caller says "none": recorded behaviour is that `Description=""` is stored exactly as an
/// omitted `Description` is, so a later read reports the member for neither. The same
/// normalization the other "empty string clears it" members of this service get
/// (`Enrichment`, `KmsKeyIdentifier`), which is why it is stated once here rather than in
/// each write.
pub fn stored_description(requested: Option<String>) -> Option<String> {
    requested.filter(|text| !text.is_empty())
}

/// This service's own ARN namespace: `arn:<partition>:pipes:…`.
pub const PIPES_SERVICE: &str = "pipes";

/// The sentence the tagging layer refuses a `resourceArn` belonging to ANOTHER service
/// with, or `None` when the ARN names this service's own vendor.
///
/// The generic resource-tagging layer resolves the ARN's vendor before it looks for a
/// resource: an ARN of another service — and text that is not a resource ARN at all — can
/// never name a pipe, however this account's pipes change, so it is a BAD REQUEST rather
/// than a missing resource. An ARN of THIS vendor that resolves to no pipe (a resource that
/// is not a pipe, a pipe this account does not hold) is the 404 those operations model.
///
/// Which ARNs are of this vendor is `service_common::arn`'s reading, the one parse of an
/// ARN's fields in this tree, and the sentence is that layer's own
/// (`service_common::validation_message::expected_vendor`), which names the vendor it
/// expected and echoes no part of the ARN it was handed — as [`RESOURCE_NOT_FOUND_MESSAGE`]
/// echoes none either.
pub fn foreign_resource_arn(arn: &str) -> Option<String> {
    (!::service_common::arn::is_resource_arn_of(arn, PIPES_SERVICE))
        .then(|| ::service_common::validation_message::expected_vendor(PIPES_SERVICE))
}

/// The largest batch a pipe may read for a target, by the target's ARN service namespace.
///
/// A pipe's batch size is bounded by what the TARGET takes in ONE delivery, not by the
/// source: each entry is the maximum number of entries that target service's own batch API
/// accepts per call — Amazon SQS `SendMessageBatch` and EventBridge `PutEvents` 10 entries,
/// Kinesis Data Streams `PutRecords` 500 records, CloudWatch Logs `PutLogEvents` 10,000
/// events.
///
/// A target whose delivery is ONE call per batch (Lambda, Step Functions, ECS, Batch,
/// SageMaker, an API destination, Timestream, Redshift Data) is deliberately absent: it has
/// no per-call entry limit to cap the batch at, so only the source's own modelled `@range`
/// bounds it.
const TARGET_BATCH_SIZE_CAPS: &[(&str, i32)] = &[
    ("events", 10),
    ("kinesis", 500),
    ("logs", 10_000),
    (PIPES_SERVICE_SQS, 10),
];

/// The ARN service namespace of Amazon SQS, named once: it is both a batch cap above and
/// the service whose FIFO queues need a message group id.
const PIPES_SERVICE_SQS: &str = "sqs";

/// The suffix the name of an Amazon SQS FIFO queue carries — a queue whose name does not
/// end in it is a standard queue. Source: Amazon SQS requires the name of a FIFO queue to
/// end with `.fifo`.
const FIFO_QUEUE_SUFFIX: &str = ".fifo";

/// The vendor the resource `target` names and the largest batch it accepts per delivery, or
/// `None` when that target has no per-call entry limit ([`TARGET_BATCH_SIZE_CAPS`]) and when
/// `target` is not a resource ARN at all.
///
/// The vendor travels WITH the cap because the refusal states both, and the one that proved
/// the cap is the one the sentence has to name.
pub fn target_batch_size_cap(target: &str) -> Option<(&str, i32)> {
    let vendor = ::service_common::arn::vendor(target)?;
    TARGET_BATCH_SIZE_CAPS
        .iter()
        .find(|(namespace, _)| *namespace == vendor)
        .map(|(_, cap)| (vendor, *cap))
}

/// The batch size a pipe's source parameters configure, whichever source type they are
/// for: every `PipeSource*Parameters` shape that can batch carries the same `BatchSize`
/// member, and a pipe reads from exactly one source.
pub fn configured_batch_size(parameters: &PipeSourceParameters) -> Option<i32> {
    let from_each = [
        parameters.sqs_queue_parameters.as_ref().and_then(|p| p.batch_size),
        parameters.kinesis_stream_parameters.as_ref().and_then(|p| p.batch_size),
        parameters.dynamo_db_stream_parameters.as_ref().and_then(|p| p.batch_size),
        parameters.active_mq_broker_parameters.as_ref().and_then(|p| p.batch_size),
        parameters.rabbit_mq_broker_parameters.as_ref().and_then(|p| p.batch_size),
        parameters
            .managed_streaming_kafka_parameters
            .as_ref()
            .and_then(|p| p.batch_size),
        parameters
            .self_managed_kafka_parameters
            .as_ref()
            .and_then(|p| p.batch_size),
    ];
    from_each.into_iter().flatten().next()
}

/// The field a `FilterCriteria` this service cannot accept is refused as, or `None` when
/// every pattern it carries is one.
///
/// A pipe's filter patterns are EventBridge EVENT PATTERNS: each is a JSON object matched
/// against the event, so text that is not a JSON object is not a pattern at all. The model
/// types `Filter.Pattern` as a plain string, which no boundary constraint can express, so
/// the write that stores one parses it. An absent or empty pattern carries no event pattern
/// and is left alone — `FilterCriteria` with an empty `Filters` array is how a caller
/// REMOVES filtering.
fn filter_criteria_field(criteria: &FilterCriteria) -> Option<ValidationExceptionField> {
    let patterns = criteria.filters.iter().flat_map(|filters| filters.iter());
    for filter in patterns {
        let pattern = match filter.pattern.as_deref() {
            None | Some("") => continue,
            Some(pattern) => pattern,
        };
        if let Some(reason) = event_pattern_reason(pattern) {
            return Some(validation_field(
                PATTERN_FIELD,
                format!("Invalid Event Pattern. Reason: {reason}."),
            ));
        }
    }
    None
}

/// Why `pattern` is not an event pattern, as the parser that read it reports it — or
/// `None` when it is one.
///
/// The reason is the PARSER's own diagnostic, read through
/// [`::service_common::json_parse_fault`] — the one place this tree states where a
/// submitted JSON document stopped parsing and what was expected there — so two services
/// handed the same text cannot disagree about it. Two readings are this service's own:
///
/// * a text that PARSES but is not an object is not a filter, which is the one fault with
///   no parse failure to report ([`NOT_AN_OBJECT_REASON`]);
/// * the parser this service hands a pattern to reads the filter's MEMBERS, so its first
///   expectation is a member name. Where the text does not open the filter object, the
///   parse never begins and stops on its very first character — so that parse is the one
///   RUN (the text read as the object's body, the opening brace the pattern never had
///   prepended), rather than a guess about which fault the text carries: the character and
///   the expectation still come from the parse itself.
fn event_pattern_reason(pattern: &str) -> Option<String> {
    let error = match ::serde_json::from_str::<::serde_json::Value>(pattern) {
        Ok(::serde_json::Value::Object(_)) => return None,
        Ok(_) => return Some(NOT_AN_OBJECT_REASON.to_string()),
        Err(error) => error,
    };
    if !pattern.trim_start().starts_with(FILTER_OBJECT_OPENING) {
        let body = format!("{FILTER_OBJECT_OPENING}{pattern}");
        if let Err(body_error) = ::serde_json::from_str::<::serde_json::Value>(&body) {
            return Some(::service_common::json_parse_fault::unexpected_input(
                &body,
                &body_error,
            ));
        }
    }
    Some(::service_common::json_parse_fault::unexpected_input(
        pattern, &error,
    ))
}

/// The character a filter object opens with, which an event pattern has to.
const FILTER_OBJECT_OPENING: char = '{';

/// The member a refused event pattern is reported at: the one that carried it.
const PATTERN_FIELD: &str = "Pattern";

/// What this service reports for a pattern that parses but is not an object — the one
/// fault with no parse failure behind it, so no parser states it.
const NOT_AN_OBJECT_REASON: &str = "Filter is not an object";

/// The field a batch size the target cannot take is reported at, and the one a FIFO target
/// with no message group id is: the member that would have had to carry something else.
const BATCH_SIZE_FIELD: &str = "BatchSize";
const MESSAGE_GROUP_ID_FIELD: &str = "MessageGroupId";

/// The `ValidationException` a pipe configuration is refused with when it breaks a rule
/// about the RESOURCES the pipe names rather than about the shape of the request, or `None`
/// when it breaks none.
///
/// These are the rules no modelled constraint can express, because each spans two members:
/// the batch size the source reads is bounded by what the TARGET accepts per delivery
/// ([`target_batch_size_cap`]), a FIFO queue target needs the message group id its sends
/// are ordered by, and a filter pattern has to be an event pattern
/// ([`filter_criteria_field`]). Checked by both writes that can configure a pipe, against
/// the configuration the pipe would END UP with, so neither a create nor an update can
/// leave a pipe AWS would never have accepted.
pub fn cross_resource_violation(
    source_parameters: Option<&PipeSourceParameters>,
    target: &str,
    target_parameters: Option<&PipeTargetParameters>,
) -> Option<ValidationException> {
    let field = cross_resource_field(source_parameters, target, target_parameters)?;
    Some(validation_error(vec![field]))
}

/// The one field [`cross_resource_violation`] reports, in the order the rules are stated.
fn cross_resource_field(
    source_parameters: Option<&PipeSourceParameters>,
    target: &str,
    target_parameters: Option<&PipeTargetParameters>,
) -> Option<ValidationExceptionField> {
    if let Some(criteria) = source_parameters.and_then(|p| p.filter_criteria.as_ref()) {
        if let Some(field) = filter_criteria_field(criteria) {
            return Some(field);
        }
    }

    if let (Some(batch_size), Some((target_type, cap))) = (
        source_parameters.and_then(configured_batch_size),
        target_batch_size_cap(target),
    ) {
        if batch_size > cap {
            return Some(validation_field(
                BATCH_SIZE_FIELD,
                format!(
                    "The configured batch size {batch_size} is greater than the max \
                     supported batch size {cap} for target {target_type}."
                ),
            ));
        }
    }

    // A FIFO queue orders its messages by message group, and `SendMessage` to one requires
    // the group to send in — which for a pipe is the target parameter, there being no event
    // member to read it from. A FIFO queue is one whose NAME carries the suffix, which is
    // the resource field of its ARN (`service_common::arn` reads both fields).
    let fifo_target = ::service_common::arn::parse_of(target, PIPES_SERVICE_SQS)
        .is_some_and(|queue| queue.resource.ends_with(FIFO_QUEUE_SUFFIX));
    let message_group_id = target_parameters
        .and_then(|p| p.sqs_queue_parameters.as_ref())
        .and_then(|p| p.message_group_id.as_deref())
        .filter(|id| !id.is_empty());
    if fifo_target && message_group_id.is_none() {
        return Some(validation_field(
            MESSAGE_GROUP_ID_FIELD,
            "MessageGroupId is required for a FIFO queue target.",
        ));
    }

    None
}

/// The region an ARN carries when the request names none.
const DEFAULT_REGION: &str = "us-east-1";

/// How long after a lifecycle call a pipe settles out of its transient state
/// (`CREATING`, `STARTING`, `STOPPING`, `UPDATING`) into the state the caller asked for.
///
/// AWS does not publish this duration, so it is CHOSEN, not cited: long enough that a
/// caller acting straight after the call still observes the transient state, short
/// enough to be waited out. It is never zero and nothing ever sleeps for it — the due
/// instant is stored and the move is materialised by whichever later call reads the
/// pipe (see [`ComAmazonawsPipesPipesState::settle_due_transitions`]).
pub const STATE_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// A state move the clock owes a pipe: at `due_at` its `CurrentState` becomes `to`.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The monotonic instant the move became (or becomes) due.
    pub due_at: Instant,
    /// The `PipeState` the pipe settles into.
    pub to: String,
}

/// The move owed to the clock when a lifecycle call starts a transition into `to`.
pub fn pending_transition(to: &str) -> PendingTransition {
    PendingTransition {
        due_at: Instant::now() + STATE_SETTLE_DELAY,
        to: to.to_string(),
    }
}

/// One persisted pipe. Every member `CreatePipe` accepts is stored here and read back
/// by `DescribePipe`, so the record — not the request — is what every response is
/// computed from.
#[derive(Debug, Clone)]
pub struct PipeRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub desired_state: String,
    pub current_state: String,
    pub state_reason: Option<String>,
    pub source: String,
    pub source_parameters: Option<PipeSourceParameters>,
    pub enrichment: Option<String>,
    pub enrichment_parameters: Option<PipeEnrichmentParameters>,
    pub target: String,
    pub target_parameters: Option<PipeTargetParameters>,
    pub role_arn: String,
    /// Tags of this pipe, keyed by tag key. They live on the record (reachable from the
    /// pipe ARN) so the later `TagResource`/`UntagResource`/`ListTagsForResource` slice
    /// reads the same copy.
    pub tags: BTreeMap<String, String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub log_configuration: Option<PipeLogConfiguration>,
    pub kms_key_identifier: Option<String>,
    /// The move the clock owes this pipe, if any. See [`STATE_SETTLE_DELAY`].
    pub pending: Option<PendingTransition>,
}

impl PipeRecord {
    /// Put the pipe into `transient` now and owe it the move to `settled`.
    pub fn begin_transition(&mut self, transient: &str, settled: &str) {
        self.current_state = transient.to_string();
        self.pending = Some(pending_transition(settled));
    }

    /// Materialise the move the clock has ALREADY made due, if it has. A no-op while the
    /// move is still in the future, so a caller acting immediately still sees the
    /// transient state.
    pub fn settle(&mut self, now: Instant) {
        let settled = match &self.pending {
            Some(pending) if now >= pending.due_at => pending.to.clone(),
            _ => return,
        };
        self.current_state = settled;
        self.pending = None;
    }

    /// The transient state this pipe is in, or `None` when it is resting in a settled
    /// one. Call [`ComAmazonawsPipesPipesState::settle_due_transitions`] first, so a move
    /// the clock has already made due is not mistaken for one still in flight.
    pub fn transient_state(&self) -> Option<&str> {
        TRANSIENT_PIPE_STATES
            .iter()
            .find(|state| **state == self.current_state)
            .copied()
    }

    /// The source type this pipe reads from, as the token this service names a source by.
    ///
    /// It is resolved from the pipe's `Source`, which is fixed when the pipe is created
    /// (no update shape carries a `Source`): the vendor of its ARN as
    /// `service_common::arn` reads one — the one parse of an ARN's fields in this tree —
    /// or [`SELF_MANAGED_KAFKA`] for the one non-ARN form `ArnOrUrl` admits.
    pub fn source_type(&self) -> String {
        if self
            .source
            .get(..SELF_MANAGED_KAFKA_SCHEME.len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SELF_MANAGED_KAFKA_SCHEME))
        {
            return SELF_MANAGED_KAFKA.to_string();
        }
        ::service_common::arn::vendor(&self.source)
            .unwrap_or_default()
            .to_string()
    }

    /// This record as the `Pipe` summary `ListPipes` returns.
    pub fn summary(&self) -> Pipe {
        Pipe {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            desired_state: Some(self.desired_state.clone()),
            current_state: Some(self.current_state.clone()),
            state_reason: self.state_reason.clone(),
            creation_time: Some(timestamp(self.creation_time)),
            last_modified_time: Some(timestamp(self.last_modified_time)),
            source: Some(self.source.clone()),
            target: Some(self.target.clone()),
            enrichment: self.enrichment.clone(),
        }
    }

    /// The stored tags as the modelled `TagMap`, or `None` when the pipe carries none:
    /// `TagMap` is `@length(min: 1)`, so an empty map is not a legal value for one SENT
    /// to this service, and `DescribePipe` reports the member only when the pipe has tags.
    pub fn tag_map(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(self.tags_reported())
    }

    /// The stored tags as the map the tagging layer ALWAYS reports, empty when the pipe
    /// carries no tag.
    ///
    /// Recorded behaviour: `ListTagsForResource` answers with its `tags` member present
    /// for a pipe with no tags, so a caller can read it without first asking whether it
    /// is there. `TagMap`'s `@length(min: 1)` bounds what a request may SEND, which is
    /// what the boundary validator enforces it on.
    pub fn tags_reported(&self) -> std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// Per-service state for com-amazonaws-pipes-pipes. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPipesPipesState {
    /// Every pipe this account holds, keyed by name. A `BTreeMap` so `ListPipes` and its
    /// `NextToken` paging enumerate in a single deterministic order.
    pub pipes: BTreeMap<String, PipeRecord>,
    /// The pipes a data-plane poller is currently running for, by name. One poller per
    /// pipe: a pipe is polled once however many lifecycle calls asked it to run (see
    /// [`data_plane`]).
    pollers: BTreeSet<String>,
}

impl ComAmazonawsPipesPipesState {
    /// The stored pipe whose ARN is `arn`, if this account holds one.
    ///
    /// The tagging operations address a pipe by its `PipeArn` rather than by name, so the
    /// lookup is over the ARN each record minted at create time. A pipe that was never
    /// created — or that `DeletePipe` has removed — is simply absent here, which is the
    /// 404 those operations model.
    pub fn pipe_by_arn(&self, arn: &str) -> Option<&PipeRecord> {
        self.pipes.values().find(|pipe| pipe.arn == arn)
    }

    /// [`Self::pipe_by_arn`] for the operations that write the record's tags.
    pub fn pipe_by_arn_mut(&mut self, arn: &str) -> Option<&mut PipeRecord> {
        self.pipes.values_mut().find(|pipe| pipe.arn == arn)
    }

    /// Claim the one data-plane poller of the pipe named `pipe`, answering whether this
    /// caller is the one that got it. `false` means a poller is already running for that
    /// pipe, so the caller starts none.
    pub fn claim_poller(&mut self, pipe: &str) -> bool {
        self.pollers.insert(pipe.to_string())
    }

    /// Release the claim of [`Self::claim_poller`], so a later lifecycle call can start a
    /// poller for this pipe again. Called by the poller itself as it ends.
    pub fn release_poller(&mut self, pipe: &str) {
        self.pollers.remove(pipe);
    }

    /// Apply every state move the clock has already made due. Call this after locking,
    /// before reading any pipe's `CurrentState`: it writes only what was owed at this
    /// instant even if nobody had called, so a reporting operation may do it too.
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        for pipe in self.pipes.values_mut() {
            pipe.settle(now);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPipesPipesState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPipesPipesState>)
}

/// The region this request is signed for, for the ARNs a handler mints.
///
/// Read from the SigV4 credential scope of the `Authorization` header (the same scope
/// the router already routes on); an unsigned request falls back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the pipe named `name`: `arn:aws:pipes:<region>:<account>:pipe/<name>`.
pub fn pipe_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("pipes", region, &format!("pipe/{name}"))
}

/// A stored instant as the modelled `Timestamp`.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Encode the exclusive start key of the next `ListPipes` page as an opaque `NextToken`.
///
/// Hex rather than the bare name: `NextToken` is an opaque continuation token to its
/// caller, who must pass it back unchanged rather than build one.
pub fn encode_next_token(last_returned_name: &str) -> String {
    ::hex::encode(last_returned_name.as_bytes())
}

/// The exclusive start key a `NextToken` carries, or `None` when the token was not
/// produced by [`encode_next_token`].
pub fn decode_next_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}
