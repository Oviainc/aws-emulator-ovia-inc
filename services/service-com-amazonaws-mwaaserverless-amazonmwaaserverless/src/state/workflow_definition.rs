// SPDX-License-Identifier: CC0-1.0

//! The workflow definition a version SNAPSHOTS, and the schedule that definition
//! declares.
//!
//! `DefinitionS3Location` is documented as the place the service "takes a snapshot of
//! the definition file at the time of workflow creation", so the definition a workflow
//! reports is the CONTENT read once, when the version was created, and frozen in that
//! version's [`crate::state::WorkflowSnapshot`]. Every later read answers from the
//! frozen copy: overwriting the Amazon S3 object changes nothing a caller reads back,
//! and no reporting operation ever goes to Amazon S3.
//!
//! The object is read through the emulator's own Amazon S3 service over the in-process
//! callback, which is the framework's documented seam for calling another emulated
//! service: nothing here opens a socket or leaves the host.

use crate::ops::types::{DefinitionS3Location, ScheduleConfiguration};

/// The most definition content this emulator snapshots for ONE workflow version.
///
/// The content is caller-supplied data held per version, so it needs a ceiling for the
/// same reason the version list itself has one
/// ([`crate::state::MAX_VERSIONS_PER_WORKFLOW`]): with that ceiling and
/// [`crate::state::MAX_WORKFLOWS`] it is what keeps the store finite. The number is the
/// emulator's own, not a published service limit, and is set generously for a YAML DAG
/// declaration; an object larger than this is left unsnapshotted, so the version reports
/// no definition content rather than this process holding an object of any size.
///
/// It is deliberately well past the length trait `GenericString` carries (1..255), which
/// is the shape `WorkflowDefinition` targets: the member is documented as "The workflow
/// definition content", and a definition file cut to 255 characters would report
/// something other than the content the version snapshotted. The whole snapshotted
/// content is reported, and this ceiling is what bounds it.
pub const MAX_DEFINITION_BYTES: usize = 16 * 1024;

/// The most parser events the schedule walk reads out of one definition.
///
/// Every event but the stream and document framing consumes at least one byte of the
/// content, so a document within [`MAX_DEFINITION_BYTES`] cannot reach this budget: it
/// is the backstop that keeps the walk's running time bounded whatever the parser does
/// with input it cannot make sense of. A document that reaches it was not read to the
/// end, so the walk stops there.
const MAX_DEFINITION_EVENTS: usize = 4 * MAX_DEFINITION_BYTES;

/// How deep into a definition the schedule walk follows a document.
///
/// A DAG-level declaration sits at the document root or one mapping in, so nothing this
/// reader looks for is deeper than a handful of levels; the bound is set well clear of
/// that and is what keeps the walk's own frame stack small however deeply a
/// caller-supplied document nests. A document nesting past it is not followed further.
const MAX_DEFINITION_NESTING: usize = 64;

/// The keys an Apache Airflow DAG declares its schedule under, in the order they are
/// looked for: `schedule` is the DAG parameter's current name and `schedule_interval`
/// the name it carried before, which Airflow still accepts.
const SCHEDULE_KEYS: &[&str] = &["schedule", "schedule_interval"];

/// The key a definition may nest its DAG-level declarations under, so a document that
/// wraps them in one mapping declares its schedule at `dag.schedule` rather than at the
/// document root.
const DAG_KEY: &str = "dag";

/// Apache Airflow's cron PRESETS and the cron expression each one stands for, from the
/// preset table in Airflow's DAG scheduling documentation (`@hourly`, `@daily` /
/// `@midnight`, `@weekly`, `@monthly`, `@quarterly`, `@yearly` / `@annually`).
///
/// A preset is a shorthand for a cron expression, and `ScheduleConfiguration$CronExpression`
/// is documented as "a cron expression ... standard cron syntax", so a definition that
/// declares a preset reports the expression the preset stands for. The presets that stand
/// for NO cron schedule at all (`@once`, `@continuous`) are deliberately absent: a
/// workflow declaring one has no cron expression to report.
const CRON_PRESETS: &[(&str, &str)] = &[
    ("@hourly", "0 * * * *"),
    ("@daily", "0 0 * * *"),
    ("@midnight", "0 0 * * *"),
    ("@weekly", "0 0 * * 0"),
    ("@monthly", "0 0 1 * *"),
    ("@quarterly", "0 0 1 */3 *"),
    ("@yearly", "0 0 1 1 *"),
    ("@annually", "0 0 1 1 *"),
];

/// The field counts a cron expression is written with: five is standard cron syntax
/// (minute, hour, day-of-month, month, day-of-week), and EventBridge Scheduler — which
/// this service's schedules run on, per `ScheduleConfiguration`'s own docs — adds a
/// sixth, year, field.
const CRON_FIELD_COUNTS: &[usize] = &[5, 6];

/// The characters that give a cron FIELD its structure, beside the values in it: the two
/// wildcards (`*`, `?`), the list, range and step separators (`,`, `-`, `/`) and the
/// nth-weekday qualifier (`#`) of standard cron and of EventBridge Scheduler's dialect.
const CRON_FIELD_PUNCTUATION: &[char] = &['*', '?', ',', '-', '/', '#'];

/// The names a cron month or day-of-week field may carry instead of a number: the twelve
/// three-letter month abbreviations and the seven day-of-week ones, as cron spells them.
const CRON_FIELD_NAMES: &[&str] = &[
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC", "SUN",
    "MON", "TUE", "WED", "THU", "FRI", "SAT",
];

/// The day-of-month qualifiers a cron field may carry on their own: the last day of the
/// month, and the weekday nearest a given day.
const CRON_FIELD_QUALIFIERS: &[&str] = &["L", "W", "LW"];

/// What one version froze out of the definition object: the content itself and the
/// schedule that content declares.
#[derive(Debug, Clone)]
pub struct DefinitionSnapshot {
    /// The definition content, exactly as the object held it when the version was
    /// created — what `WorkflowDefinition` reports.
    pub content: String,
    /// The schedule the content declares, or `None` when it declares none — what
    /// `ScheduleConfiguration` reports.
    pub schedule_configuration: Option<ScheduleConfiguration>,
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// `region` is the calling request's own signing region, so the definition object is
/// read where the caller's own Amazon S3 requests go. No network client is configured:
/// the callback routes in process, and this crate names no other service's crate.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Take the definition snapshot a new version freezes: read the object `location`
/// addresses ONCE, now, and keep its content and the schedule it declares.
///
/// `None` when there is nothing to snapshot — the location addresses no stored object,
/// the object holds no definition (empty, not text, or past
/// [`MAX_DEFINITION_BYTES`]) — in which case the version reports no definition content
/// and no schedule rather than content this service never read. Whether such a create
/// should be refused outright is the intake's business, not this reader's.
///
/// `VersionId` is passed through when the location names one, because it selects which
/// object version is snapshotted ("If not specified, the latest version is used").
pub async fn snapshot_definition(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &DefinitionS3Location,
) -> Option<DefinitionSnapshot> {
    if location.bucket.is_empty() || location.object_key.is_empty() {
        return None;
    }
    let object = s3_client(ctx, region)
        .get_object()
        .bucket(&location.bucket)
        .key(&location.object_key)
        .set_version_id(location.version_id.clone())
        .send()
        .await
        .ok()?;
    // The ceiling is applied to the size Amazon S3 REPORTS, before a byte is buffered,
    // so an oversized object costs no allocation; the check after the body is read is
    // the backstop for an object whose reported size was wrong.
    if object
        .content_length()
        .is_some_and(|length| length > MAX_DEFINITION_BYTES as i64)
    {
        return None;
    }
    let bytes = object.body.collect().await.ok()?.into_bytes();
    if bytes.len() > MAX_DEFINITION_BYTES {
        return None;
    }
    let content = String::from_utf8(bytes.to_vec()).ok()?;
    // `WorkflowDefinition` is modelled with a minimum length of 1, so an object holding
    // no bytes holds no definition content to report.
    if content.is_empty() {
        return None;
    }
    let schedule_configuration = declared_schedule(&content);
    Some(DefinitionSnapshot {
        content,
        schedule_configuration,
    })
}

/// The schedule a definition DECLARES: the cron expression its DAG-level schedule key
/// carries ([`SCHEDULE_KEYS`], at the document root or inside its [`DAG_KEY`] mapping),
/// resolved through Airflow's preset shorthands ([`CRON_PRESETS`]).
///
/// `None` when the definition declares no schedule, or declares one that is not a cron
/// expression at all: `ScheduleConfiguration$CronExpression` is a cron expression in
/// standard cron syntax, so a value that is not one is not reported as one.
pub fn declared_schedule(definition: &str) -> Option<ScheduleConfiguration> {
    let declared = declared_schedule_value(definition)?;
    Some(ScheduleConfiguration {
        cron_expression: Some(cron_expression(&declared)?),
    })
}

/// The cron expression a declared schedule value stands for: a preset's expression, or
/// the value itself when it is written as a cron expression — as many fields as cron has
/// ([`CRON_FIELD_COUNTS`]), each written the way a cron field is ([`is_cron_field`]).
/// Prose that happens to have as many words as cron has fields is therefore not reported
/// as a cron expression.
///
/// The value is reported as the definition writes it, only trimmed of the surrounding
/// whitespace YAML does not consider part of a scalar.
fn cron_expression(declared: &str) -> Option<String> {
    let declared = declared.trim();
    if let Some((_, expression)) = CRON_PRESETS.iter().find(|(preset, _)| *preset == declared) {
        return Some((*expression).to_string());
    }
    let fields: Vec<&str> = declared.split_whitespace().collect();
    (CRON_FIELD_COUNTS.contains(&fields.len()) && fields.iter().copied().all(is_cron_field))
        .then(|| declared.to_string())
}

/// Whether `field` is written as ONE cron field: taking its structure
/// ([`CRON_FIELD_PUNCTUATION`]) apart leaves only numbers, wildcards, the day-of-month
/// qualifiers ([`CRON_FIELD_QUALIFIERS`]) and the month and day-of-week names
/// ([`CRON_FIELD_NAMES`]) a cron field is allowed to carry.
fn is_cron_field(field: &str) -> bool {
    field
        .split(|character| CRON_FIELD_PUNCTUATION.contains(&character))
        .all(|token| {
            token.is_empty()
                || token.chars().all(|character| character.is_ascii_digit())
                || CRON_FIELD_QUALIFIERS
                    .iter()
                    .chain(CRON_FIELD_NAMES)
                    .any(|name| token.eq_ignore_ascii_case(name))
        })
}

/// The raw value the definition's DAG-level schedule key carries, read out of the FIRST
/// YAML document in `definition`.
///
/// Driven EVENT BY EVENT off the parser's own scanner, pulling one event at a time, and
/// never through the loader: the loader COMPOSES a document by descending once per
/// nesting level, so a document nesting as deep as the content allows would drive its
/// recursion as deep, and it resolves an ALIAS by copying the anchored node, which is how
/// a few hundred bytes of YAML become gigabytes. This walk does neither — it builds no
/// tree, recurses nowhere, and an alias costs it one event like any other node — so what
/// it needs is the three bounds it carries: [`MAX_DEFINITION_BYTES`] on the content,
/// [`MAX_DEFINITION_EVENTS`] on how many events are read, and
/// [`MAX_DEFINITION_NESTING`] on how deep the walk follows a document.
///
/// A document that runs past either bound, or that the parser cannot read to the end, is
/// not followed further: the walk stops there and answers with the DAG-level declaration
/// it had already read, because a declaration is decided by the first schedule key the
/// walk reaches and nothing past the stopping point can change it. YAML 1.2 is a
/// superset of JSON, so a definition written as JSON is read by the same walk.
fn declared_schedule_value(definition: &str) -> Option<String> {
    use ::yaml_rust2::parser::Event;

    let mut parser = ::yaml_rust2::parser::Parser::new_from_str(definition);
    let mut finder = ScheduleFinder::default();
    for _ in 0..MAX_DEFINITION_EVENTS {
        let Ok((event, _marker)) = parser.next_token() else {
            break;
        };
        if matches!(event, Event::StreamEnd) || !finder.accept(event) {
            break;
        }
    }
    finder.found
}

/// One open YAML collection, as the walk below tracks it.
#[derive(Debug)]
struct Frame {
    /// Whether this collection is a mapping, so its children alternate key and value.
    mapping: bool,
    /// Whether the next node in this mapping stands in the KEY position.
    expecting_key: bool,
    /// The key the node now being read sits under, for a mapping.
    key: Option<String>,
}

/// The walk that finds a definition's declared schedule: it tracks which key each scalar
/// sits under and keeps the first value at a DAG-level schedule path.
#[derive(Debug, Default)]
struct ScheduleFinder {
    /// The collections open at this point, outermost first.
    open: Vec<Frame>,
    /// How many documents have begun. Only the first document's declarations count: a
    /// definition file is one DAG declaration.
    documents: usize,
    /// The first declared schedule value seen, if any.
    found: Option<String>,
}

impl ScheduleFinder {
    /// Take ONE parser event, and report whether the walk may go on: `false` for a
    /// document that nests past [`MAX_DEFINITION_NESTING`], which is the point past
    /// which this reader stops following it.
    fn accept(&mut self, event: ::yaml_rust2::parser::Event) -> bool {
        use ::yaml_rust2::parser::Event;
        match event {
            Event::DocumentStart => {
                self.documents += 1;
                self.open.clear();
            }
            Event::MappingStart(..) | Event::SequenceStart(..) => {
                if self.open.len() >= MAX_DEFINITION_NESTING {
                    return false;
                }
                let mapping = matches!(event, Event::MappingStart(..));
                self.consume_node_slot();
                self.open.push(Frame {
                    mapping,
                    expecting_key: mapping,
                    key: None,
                });
            }
            Event::MappingEnd | Event::SequenceEnd => {
                self.open.pop();
            }
            Event::Scalar(value, ..) => {
                if let Some(frame) = self.open.last_mut() {
                    if frame.mapping && frame.expecting_key {
                        frame.key = Some(value);
                        frame.expecting_key = false;
                        return true;
                    }
                }
                let declares_schedule =
                    self.documents == 1 && self.found.is_none() && self.at_schedule_path();
                if let Some(frame) = self.open.last_mut() {
                    if frame.mapping {
                        frame.expecting_key = true;
                    }
                }
                if declares_schedule {
                    self.found = Some(value);
                }
            }
            // An alias refers to an anchored node. Nothing here expands one, so it only
            // occupies the position it stands in.
            Event::Alias(..) => self.consume_node_slot(),
            _ => {}
        }
        true
    }

    /// Whether the node now being read is the value of a DAG-level schedule key: the
    /// open collections are all mappings, and their keys spell either `<schedule key>`
    /// at the document root or `dag.<schedule key>` one level in.
    fn at_schedule_path(&self) -> bool {
        let mut keys = Vec::with_capacity(self.open.len());
        for frame in &self.open {
            if !frame.mapping {
                return false;
            }
            match frame.key.as_deref() {
                Some(key) => keys.push(key),
                None => return false,
            }
        }
        match keys.as_slice() {
            [key] => SCHEDULE_KEYS.contains(key),
            [DAG_KEY, key] => SCHEDULE_KEYS.contains(key),
            _ => false,
        }
    }

    /// Account for a node that occupies a position in the enclosing mapping without being
    /// a scalar — a nested collection, or an alias standing in for one — so its key and
    /// value positions stay in step.
    ///
    /// Either position can hold one: YAML allows a collection or an alias as a KEY, and
    /// such a key is one this reader cannot name, so the mapping's key is cleared and the
    /// node that follows is read as its value.
    fn consume_node_slot(&mut self) {
        if let Some(frame) = self.open.last_mut() {
            if !frame.mapping {
                return;
            }
            if frame.expecting_key {
                frame.key = None;
                frame.expecting_key = false;
            } else {
                frame.expecting_key = true;
            }
        }
    }
}

