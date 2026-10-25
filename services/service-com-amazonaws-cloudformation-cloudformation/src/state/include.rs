// SPDX-License-Identifier: CC0-1.0

//! The `AWS::Include` transform: splicing the Amazon S3 snippet an `Fn::Transform` names
//! into the template, before anything deploys it.
//!
//! ## The rule this module implements
//!
//! "The optional `Transform` section … specifies one or more macros that CloudFormation
//! uses to process your template", and `Fn::Transform` "specifies a macro to perform
//! custom processing on part of a stack template". `AWS::Include` is the macro that
//! "works with template snippets that are stored separately from the main CloudFormation
//! template": its one parameter, `Location`, "is an Amazon S3 URI, with a specific file
//! name in an S3 bucket", and processing it means READING that object and splicing what
//! it holds into the template where the intrinsic stood — before a single resource is
//! provisioned.
//!
//! Two things follow, and both are what this module is for:
//!
//! * a snippet that IS there contributes its members to the template, so the resource the
//!   including declaration describes is created with them — the intrinsic itself never
//!   reaches the service that owns the resource, because it is not a property of
//!   anything;
//! * a snippet that is NOT there fails the stack OPERATION, with a reason naming the
//!   transform and what could not be read ([`failure`]). The failure belongs to template
//!   processing, not to a resource: processing happens before provisioning, so nothing is
//!   asked of any owning service and the stack owns no resources.
//!
//! The snippet is read through the in-process callback into this same emulator, exactly as
//! [`super::provisioning`] creates a stack's resources: the object a stack includes is the
//! one a client's own `PutObject` stored.
//!
//! ## Where the processing happens
//!
//! A fetch is asynchronous, so the expansion cannot live in [`super::template::parse`]
//! (which every read of a stored body goes through, under the store lock). The operation
//! that SUBMITS a template processes it once — [`process`] — and the processed body is
//! what the stack deploys from ([`super::StoredStack::set_processed_template_body`]), so
//! every later read of that stack sees the spliced template without reading S3 again.
//!
//! ## Bounds
//!
//! The template body is caller-supplied and so is every snippet, so the work one request
//! can ask for is bounded three ways, each refusing rather than reading or growing
//! further: at most [`MAX_SNIPPETS_FETCHED`] distinct objects are read, each at most
//! [`MAX_SNIPPET_BYTES`] long (the ceiling on a template body itself), at most
//! [`MAX_SNIPPETS_SPLICED`] splices are performed in total, and a snippet that itself
//! includes another is followed at most [`MAX_INCLUDE_ROUNDS`] deep — which is also what
//! makes a snippet that includes ITSELF terminate. The processed template is held to the
//! same length ceiling as a submitted one.

use std::collections::BTreeMap;

use ::serde_json::Value;

/// The macro name an `Fn::Transform` names to include a snippet, as a template spells it.
pub const TRANSFORM: &str = "AWS::Include";

/// The intrinsic that applies a macro to one part of a template.
const INTRINSIC: &str = "Fn::Transform";

/// The members an `Fn::Transform` directive carries: the macro's name, and the parameters
/// passed to it.
const NAME: &str = "Name";
const PARAMETERS: &str = "Parameters";

/// The one parameter `AWS::Include` takes: "`Location` — the location is an Amazon S3 URI,
/// with a specific file name in an S3 bucket".
const LOCATION: &str = "Location";

/// How many DISTINCT Amazon S3 objects processing one template may read.
///
/// Chosen (CloudFormation publishes no ceiling on the snippets a template includes): a
/// template body is caller-supplied, and every include is a call into another service, so
/// the number of them one request can ask for is capped rather than unbounded.
pub const MAX_SNIPPETS_FETCHED: usize = 16;

/// How many splices processing one template may perform. The same body may include one
/// snippet at many places, so this bounds the SIZE the processed template can grow to,
/// which the fetch ceiling alone does not.
pub const MAX_SNIPPETS_SPLICED: usize = 32;

/// How deep a snippet that itself carries the intrinsic is followed. Chosen for the same
/// reason as the two above, and it is what makes a snippet including itself terminate
/// instead of growing the template on every pass.
pub const MAX_INCLUDE_ROUNDS: usize = 3;

/// The most bytes one snippet may carry: the ceiling on a template body itself, since a
/// snippet is a template fragment that becomes part of one.
pub const MAX_SNIPPET_BYTES: usize = super::TEMPLATE_BODY_MAX_BYTES;

/// The error code Amazon S3 answers for an object it does not hold ("`NoSuchKey` — the
/// specified key does not exist").
const NO_SUCH_KEY: &str = "NoSuchKey";

/// The error code it answers for a bucket it does not hold.
const NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// What processing a submitted template's `AWS::Include` transforms left.
#[derive(Debug, Clone)]
pub enum Processed {
    /// The body names no `AWS::Include`: nothing was read, and the submitted body is the
    /// one that deploys.
    Unchanged,
    /// The template as the transform spliced it — the body that deploys, spelled as JSON
    /// (one `serde_json::Value` represents either submitted spelling).
    Expanded(String),
    /// Processing could not be carried out, with the reason the stack operation records:
    /// the transform failed, so nothing is provisioned.
    Failed(String),
}

/// Process every `AWS::Include` the body names, reading each snippet from Amazon S3.
///
/// `region` is the Region the stack belongs to, which is the scope the in-process S3 call
/// is signed for — the same one [`super::provisioning`] creates a stack's resources in.
pub async fn process(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    body: &str,
) -> Processed {
    let Some(mut document) = super::template::document(body) else {
        // A body this service cannot read declares no transform either: the operations
        // that submit one answer for an unreadable body on their own terms.
        return Processed::Unchanged;
    };
    // Each location read once, however many places include it.
    let mut snippets: BTreeMap<String, Value> = BTreeMap::new();
    let mut spliced = 0usize;
    let mut rounds = 0usize;
    loop {
        let wanted = match locations(&document) {
            Ok(wanted) => wanted,
            Err(detail) => return Processed::Failed(failure(&detail)),
        };
        if wanted.is_empty() {
            break;
        }
        if rounds == MAX_INCLUDE_ROUNDS {
            return Processed::Failed(failure(&format!(
                "an included snippet may not include another more than \
                 {MAX_INCLUDE_ROUNDS} deep."
            )));
        }
        rounds += 1;
        for location in wanted {
            if snippets.contains_key(&location) {
                continue;
            }
            if snippets.len() >= MAX_SNIPPETS_FETCHED {
                return Processed::Failed(failure(&format!(
                    "a template may include at most {MAX_SNIPPETS_FETCHED} snippets."
                )));
            }
            let Some(object) = s3_location(&location) else {
                return Processed::Failed(failure(&format!(
                    "[{location}] is not an Amazon S3 URI of the form {}bucket/key.",
                    super::template::S3_URI_SCHEME
                )));
            };
            match snippet(ctx, region, &object).await {
                Ok(read) => {
                    snippets.insert(location, read);
                }
                Err(detail) => return Processed::Failed(failure(&detail)),
            }
        }
        if let Err(detail) = splice(&mut document, &snippets, &mut spliced) {
            return Processed::Failed(failure(&detail));
        }
    }
    if spliced == 0 {
        return Processed::Unchanged;
    }
    let processed = document.to_string();
    if processed.len() > super::TEMPLATE_BODY_MAX_BYTES {
        return Processed::Failed(failure(&format!(
            "the template the snippets are included in is larger than the {} bytes a \
             template may carry.",
            super::TEMPLATE_BODY_MAX_BYTES
        )));
    }
    Processed::Expanded(processed)
}

/// The stack-level reason a failed transform records.
///
/// The failure is the TRANSFORM's: it happens while the template is being processed, which
/// is before any resource is provisioned, so the reason names the macro that failed and
/// what about it failed rather than a resource nothing was asked to create.
fn failure(detail: &str) -> String {
    format!("Transform {TRANSFORM} failed with: {detail}")
}

/// Every `AWS::Include` location the document names, each once.
///
/// Iterative, with an explicit stack: the document is caller-supplied, so a recursive walk
/// would spend the process stack on nesting the caller chose. `Err` is a directive this
/// transform cannot act on at all, which fails the operation rather than being passed
/// through as if it were data.
fn locations(document: &Value) -> Result<Vec<String>, String> {
    let mut found: Vec<String> = Vec::new();
    let mut pending: Vec<&Value> = vec![document];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items.iter()),
            Value::Object(entries) => {
                if let Some(directive) = entries.get(INTRINSIC) {
                    if let Some(location) = include_location(directive)? {
                        if !found.contains(&location) {
                            found.push(location);
                        }
                    }
                }
                pending.extend(entries.values());
            }
            _ => {}
        }
    }
    Ok(found)
}

/// The Amazon S3 URI an `Fn::Transform` directive includes.
///
/// `None` where the directive names another macro — a private one, which this emulator
/// runs nothing for and therefore leaves exactly as the template declares it.
fn include_location(directive: &Value) -> Result<Option<String>, String> {
    if directive.get(NAME).and_then(Value::as_str) != Some(TRANSFORM) {
        return Ok(None);
    }
    match directive
        .get(PARAMETERS)
        .and_then(|parameters| parameters.get(LOCATION))
    {
        Some(Value::String(location)) => Ok(Some(location.clone())),
        _ => Err(format!(
            "{PARAMETERS}.{LOCATION} must be the Amazon S3 URI of the snippet to include."
        )),
    }
}

/// The Amazon S3 object an `AWS::Include` `Location` names.
struct Object {
    bucket: String,
    key: String,
    /// The object VERSION the URI addresses, where it addresses one.
    version: Option<String>,
}

/// The object an Amazon S3 URI names, or `None` for text that is not one — read through the
/// crate's one reader of that spelling ([`super::template::s3_uri_parts`]).
fn s3_location(location: &str) -> Option<Object> {
    let (bucket, key, version) = super::template::s3_uri_parts(location)?;
    Some(Object {
        bucket: bucket.to_string(),
        key: key.to_string(),
        version: version.map(str::to_owned),
    })
}

/// Splice every include this walk holds a snippet for, in place.
///
/// Iterative for the same reason as [`locations`]. A site whose snippet is not read YET is
/// left alone: that is a snippet which itself includes another, and the next round of
/// [`process`] reads it.
fn splice(
    document: &mut Value,
    snippets: &BTreeMap<String, Value>,
    spliced: &mut usize,
) -> Result<(), String> {
    let mut pending: Vec<&mut Value> = vec![document];
    while let Some(value) = pending.pop() {
        if value.is_object() {
            splice_here(value, snippets, spliced)?;
        }
        match value {
            Value::Array(items) => pending.extend(items.iter_mut()),
            Value::Object(entries) => pending.extend(entries.values_mut()),
            _ => {}
        }
    }
    Ok(())
}

/// Splice the one include `value` itself carries, if it carries one.
///
/// The intrinsic is REMOVED either way: it is a processing instruction, never a member of
/// the thing it was written in, so nothing downstream — least of all the service that owns
/// a resource whose properties carried it — may see it.
///
/// A snippet that is a set of members contributes them to the object the intrinsic stood
/// in; one that is any other value can only stand WHERE the intrinsic stood, so it is
/// spliced only where the intrinsic was the whole of that value. A member the including
/// template declares itself is kept: the snippet contributes what is missing rather than
/// overwriting what is written beside it.
fn splice_here(
    value: &mut Value,
    snippets: &BTreeMap<String, Value>,
    spliced: &mut usize,
) -> Result<(), String> {
    let Some(entries) = value.as_object() else {
        return Ok(());
    };
    let Some(directive) = entries.get(INTRINSIC) else {
        return Ok(());
    };
    let Some(location) = include_location(directive)? else {
        return Ok(());
    };
    let Some(included) = snippets.get(&location) else {
        return Ok(());
    };
    *spliced += 1;
    if *spliced > MAX_SNIPPETS_SPLICED {
        return Err(format!(
            "a template may include a snippet at no more than {MAX_SNIPPETS_SPLICED} places."
        ));
    }
    let Some(entries) = value.as_object_mut() else {
        return Ok(());
    };
    entries.remove(INTRINSIC);
    match included {
        Value::Object(members) => {
            for (name, member) in members {
                entries
                    .entry(name.clone())
                    .or_insert_with(|| member.clone());
            }
            Ok(())
        }
        other if entries.is_empty() => {
            *value = other.clone();
            Ok(())
        }
        _ => Err(format!(
            "the snippet at [{location}] is not a set of template members, so it cannot be \
             included beside the members declared with it."
        )),
    }
}

/// The snippet one Amazon S3 object holds, read through the in-process callback into this
/// same emulator.
///
/// `Err` is what the transform failed with, worded from the request: the KEY the template
/// named, since that is what the client has to go on.
async fn snippet(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: &Object,
) -> Result<Value, String> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let Object {
        bucket,
        key,
        version,
    } = object;
    let read = super::provisioner::s3_client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version.clone())
        .send()
        .await
        .map_err(|error| match error.code() {
            Some(NO_SUCH_KEY) => format!("S3 key [{key}] does not exist."),
            Some(NO_SUCH_BUCKET) => format!("S3 bucket [{bucket}] does not exist."),
            // Any other refusal: the object was not read, reported under the code Amazon
            // S3 named rather than the message it worded.
            code => format!(
                "S3 key [{key}] could not be read: {}.",
                code.unwrap_or("Amazon S3 did not answer the read")
            ),
        })?;
    // The length the object reports, before a single byte of it is held.
    if read
        .content_length()
        .is_some_and(|length| length > MAX_SNIPPET_BYTES as i64)
    {
        return Err(too_large(key));
    }
    let body = read
        .body
        .collect()
        .await
        .map_err(|_| format!("S3 key [{key}] could not be read."))?
        .into_bytes();
    if body.len() > MAX_SNIPPET_BYTES {
        return Err(too_large(key));
    }
    let text = String::from_utf8(body.to_vec()).map_err(|_| not_a_snippet(key))?;
    super::template::document(&text).ok_or_else(|| not_a_snippet(key))
}

/// The detail for an object past the snippet ceiling.
fn too_large(key: &str) -> String {
    format!("S3 key [{key}] is larger than the {MAX_SNIPPET_BYTES} bytes a template snippet may carry.")
}

/// The detail for an object that is not a template fragment at all: a snippet is "a
/// template snippet … stored separately from the main CloudFormation template", so it is
/// read the way a template body is.
fn not_a_snippet(key: &str) -> String {
    format!("S3 key [{key}] does not hold a JSON or YAML template snippet.")
}
