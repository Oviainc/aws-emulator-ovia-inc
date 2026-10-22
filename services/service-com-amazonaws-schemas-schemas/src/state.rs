// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-schemas-schemas service.
//!
//! This is the service's single store: the registry → schema → schema-version tree
//! every handler reads and writes. It also holds the few helpers more than one
//! handler needs (ARN synthesis, the request's region, tag conversion, listing
//! pagination and the error payloads), because `src/lib.rs` and `src/ops/mod.rs` are
//! generated and cannot declare another module.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BadRequestException, ConflictException, InternalServerErrorException, NotFoundException,
};

/// Per-service state for com-amazonaws-schemas-schemas. A handler reaches it through [`service_state`].
///
/// `BTreeMap` rather than `HashMap`: every listing operation this service models is
/// `@paginated` with an opaque `NextToken`, and a token can only resume a page
/// deterministically if the underlying order is stable. `BTreeMap`'s key order is.
#[derive(Debug, Default)]
pub struct ComAmazonawsSchemasSchemasState {
    /// Registries by `RegistryName`.
    pub registries: BTreeMap<String, Registry>,
}

/// A schema registry, as `CreateRegistry` persisted it.
#[derive(Debug, Clone)]
pub struct Registry {
    pub name: String,
    /// `arn:aws:schemas:<region>:<account>:registry/<name>`, minted once at create time.
    pub arn: String,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// Schemas by `SchemaName`.
    pub schemas: BTreeMap<String, Schema>,
}

/// A schema and all of its versions.
#[derive(Debug, Clone)]
pub struct Schema {
    pub name: String,
    /// `arn:aws:schemas:<region>:<account>:schema/<registry>/<name>`.
    pub arn: String,
    pub description: Option<String>,
    /// One of [`crate::ops::types::TYPE_VALUES`].
    pub r#type: String,
    pub tags: BTreeMap<String, String>,
    /// The highest version number stored; rendered as the decimal `SchemaVersion`.
    pub latest_version: u64,
    /// When the schema itself was last written (created, or a new version added).
    pub last_modified: SystemTime,
    /// Versions by number. Numeric keys, so the map's order is the version order —
    /// a string key would sort `"10"` before `"2"`.
    pub versions: BTreeMap<u64, SchemaVersion>,
    /// The code bindings `PutCodeBinding` has started for this schema, keyed by the
    /// schema version they were generated from and the language they were generated
    /// for: a binding is identified by that pair, so one schema version can carry a
    /// binding per language and one language a binding per version.
    pub code_bindings: BTreeMap<(u64, String), CodeBinding>,
}

/// One immutable version of a schema: the content a caller sent for it.
#[derive(Debug, Clone)]
pub struct SchemaVersion {
    pub version: u64,
    /// Returned verbatim by `DescribeSchema`.
    pub content: String,
    pub r#type: String,
    pub description: Option<String>,
    pub created_date: SystemTime,
}

/// One code binding: the code generation `PutCodeBinding` started for one schema
/// version in one language.
///
/// It holds no generated source — this slice models the binding's LIFECYCLE only, as
/// `PutCodeBinding` and `DescribeCodeBinding` report it. `GetCodeBindingSource`, which
/// is what serves the generated source itself, is still a scaffold stub and answers
/// 501; it owns the decision of what that source is.
#[derive(Debug, Clone)]
pub struct CodeBinding {
    /// The language generation was asked for: one of [`CODE_BINDING_LANGUAGES`],
    /// which is what [`code_binding_language`] admits.
    ///
    /// The model types `Language` as an unconstrained `__string`, but the service
    /// does constrain it — it refuses anything outside the pattern quoted on
    /// [`CODE_BINDING_LANGUAGES`] — so only a language this service generates for
    /// ever reaches the store.
    pub language: String,
    /// The schema version the binding was generated from, resolved at put time: an
    /// absent request `SchemaVersion` means the then-latest version, and the binding
    /// stays attached to that number afterwards.
    pub schema_version: u64,
    /// When `PutCodeBinding` started this generation.
    pub created_date: SystemTime,
    /// The instant generation is due to finish; [`CodeBinding::status_at`] reports
    /// `CREATE_IN_PROGRESS` before it and `CREATE_COMPLETE` from it on.
    ///
    /// Stored as an instant rather than waited for: the transition is the clock's, so
    /// every later read derives it and no request sleeps.
    pub generation_due: SystemTime,
}

/// The `CodeGenerationStatus` of a generation that has not finished yet. One of
/// [`crate::ops::types::CODE_GENERATION_STATUS_VALUES`].
pub const CODE_GENERATION_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The `CodeGenerationStatus` of a generation that has finished. One of
/// [`crate::ops::types::CODE_GENERATION_STATUS_VALUES`].
pub const CODE_GENERATION_COMPLETE: &str = "CREATE_COMPLETE";

/// How long after `PutCodeBinding` a code binding's generation is due to complete.
///
/// AWS publishes no duration for code generation, so this span is the emulator's own
/// choice. It is deliberately NOT zero: a caller that describes the binding straight
/// after starting it must still observe the transient `CREATE_IN_PROGRESS` status that
/// the real service reports, and the 202 `PutCodeBinding` answers with says the work
/// is accepted, not done. It is short enough that a caller polling the binding reaches
/// `CREATE_COMPLETE` in well under a second.
pub const CODE_GENERATION_DURATION: Duration = Duration::from_millis(400);

/// What both `PutCodeBinding` and `DescribeCodeBinding` report about one binding.
///
/// The two operations answer the same four members from the same stored binding, so
/// they render it through one function ([`CodeBinding::report_at`]) and cannot drift.
#[derive(Debug, Clone)]
pub struct CodeBindingReport {
    pub creation_date: SystemTime,
    pub last_modified: SystemTime,
    /// The version number in decimal, as `SchemaVersion` is rendered everywhere else.
    pub schema_version: String,
    pub status: &'static str,
}

impl CodeBinding {
    /// The status of this generation as of `now`: in progress until the instant it is
    /// due, complete from then on. Derived, so a read never has to write.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.generation_due {
            CODE_GENERATION_IN_PROGRESS
        } else {
            CODE_GENERATION_COMPLETE
        }
    }

    /// When this binding last changed, as of `now`: it was created, and the only later
    /// change is the completion the clock makes due.
    pub fn last_modified_at(&self, now: SystemTime) -> SystemTime {
        if now < self.generation_due {
            self.created_date
        } else {
            self.generation_due
        }
    }

    /// This binding as the two code-binding operations report it.
    pub fn report_at(&self, now: SystemTime) -> CodeBindingReport {
        CodeBindingReport {
            creation_date: self.created_date,
            last_modified: self.last_modified_at(now),
            schema_version: self.schema_version.to_string(),
            status: self.status_at(now),
        }
    }
}

impl Schema {
    /// The version a request asked for: `SchemaVersion` when given, else the latest.
    ///
    /// `None` means the request named a version this schema does not hold — including
    /// a value that is not a decimal version number at all, which no stored version
    /// can match. The raw label is parsed as given: a version number is the whole
    /// value, so surrounding space is not a version number.
    pub fn version_for(&self, requested: Option<&str>) -> Option<&SchemaVersion> {
        match requested {
            None => self.versions.get(&self.latest_version),
            Some(raw) => self.versions.get(&raw.parse::<u64>().ok()?),
        }
    }
}

/// The languages this service generates code bindings for, in the identifiers the
/// API takes in the `Language` URI label.
///
/// Source: the `Language` member is modelled as a plain `__string` with no enum and
/// no pattern (`smithy_query(query="operation",operation="PutCodeBinding")`), so the
/// accepted set is the SERVICE's, not the model's. The service states it in the
/// refusal it answers a bad `Language` with — "Parameter CodeBindingLanguage doesn't
/// match pattern `^(Java8|TypeScript3|Python36|Go1)$`" — i.e. the code-binding
/// languages EventBridge Schemas generates for: Java 8+, TypeScript 3+, Python 3.6+
/// and Go 1.x. Anything else is a language this service cannot generate, and is
/// refused rather than recorded.
///
/// The order is the pattern's: the alternation in the refusal below is built from
/// this list, so the two cannot drift apart.
pub const CODE_BINDING_LANGUAGES: &[&str] = &["Java8", "TypeScript3", "Python36", "Go1"];

/// The name the service's own validation gives the `Language` URI label it checks
/// against [`code_binding_language_pattern`].
pub const CODE_BINDING_LANGUAGE_PARAMETER: &str = "CodeBindingLanguage";

/// The pattern an acceptable `Language` matches: the alternation of
/// [`CODE_BINDING_LANGUAGES`], anchored at both ends, which is why a language family
/// without its version suffix and a differently-cased spelling are both refused.
pub fn code_binding_language_pattern() -> String {
    format!("^({})$", CODE_BINDING_LANGUAGES.join("|"))
}

/// The canonical language identifier a request named, or `None` when it named a
/// language this service does not generate bindings for.
///
/// The comparison is exact: these identifiers are wire values, and a service that
/// quietly accepted another spelling would persist a binding under a language a
/// later read cannot ask for.
pub fn code_binding_language(requested: &str) -> Option<&'static str> {
    CODE_BINDING_LANGUAGES
        .iter()
        .copied()
        .find(|language| *language == requested)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSchemasSchemasState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSchemasSchemasState>)
}

/// The region a request's ARNs name when the request carries no SigV4 scope to read
/// it from. It is the region the emulator writes into the shared AWS config it hands
/// clients (`vendor/server/src/lib.rs`, `[default] region = us-east-1`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for.
///
/// An ARN is regional, and the region a client's ARNs must carry is the one it
/// signed with, so it is read from the SigV4 credential scope rather than fixed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:schemas:<region>:<account>:registry/<registry>`.
pub fn registry_arn(region: &str, registry: &str) -> String {
    ::engine::emulator_arn("schemas", region, &format!("registry/{registry}"))
}

/// `arn:aws:schemas:<region>:<account>:schema/<registry>/<schema>`.
pub fn schema_arn(region: &str, registry: &str, schema: &str) -> String {
    ::engine::emulator_arn("schemas", region, &format!("schema/{registry}/{schema}"))
}

/// Store the `Tags` member of a request. An absent map and an empty one are the same
/// thing: a resource with no tags.
pub fn tags_from_input(tags: Option<&HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>()
    })
    .unwrap_or_default()
}

/// Render a stored tag set for a response. Always present — an untagged resource
/// reports an empty map, not an absent member.
pub fn tags_to_output(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

/// One page of a key-ordered listing.
#[derive(Debug)]
pub struct Page<T> {
    /// The items this page returns, in key order.
    pub items: Vec<T>,
    /// The token that resumes after the last item, or `None` on the last page.
    pub next_token: Option<String>,
}

/// Mint a `NextToken` that resumes strictly after `last_key`, for a listing selected
/// by `selection`.
///
/// Both halves are hex, so the token is a single opaque word: it is safe in a query
/// string whatever the key or the selection contains, a client cannot mistake it for
/// a value it may construct, and the two halves cannot run into each other.
///
/// The selection is carried IN the token because a token names a position within one
/// listing: resuming a differently-filtered listing from it would skip or repeat
/// items with no error. A token presented with another selection is therefore not a
/// token for that listing, and is refused.
fn encode_next_token(selection: &str, last_key: &str) -> String {
    format!("{}.{}", ::hex::encode(selection), ::hex::encode(last_key))
}

/// Read back the key a `NextToken` [`encode_next_token`] minted for `selection`
/// resumes after. `None` for anything else, including a token minted for a different
/// selection.
fn decode_next_token(selection: &str, token: &str) -> Option<String> {
    let (encoded_selection, encoded_key) = token.split_once('.')?;
    if ::hex::decode(encoded_selection).ok()? != selection.as_bytes() {
        return None;
    }
    String::from_utf8(::hex::decode(encoded_key).ok()?).ok()
}

/// Take one page out of a key-ordered sequence.
///
/// `entries` must be ordered by key (a `BTreeMap`'s iteration order is). The page
/// starts strictly after the key `token` encodes and holds at most `page_size` items;
/// `next_token` is minted only when a further entry exists, so the last page of a
/// listing carries none.
///
/// `page_size` is a count, never "unbounded": a listing that materialised its whole
/// selection into one response would be a response-amplification path out of a store
/// the caller fills. Callers resolve it through [`requested_page_size`].
///
/// `selection` identifies the listing being walked — every request member that
/// decides WHICH items it contains (the parent resource, a name prefix, a scope).
/// It is bound into the token and checked when one comes back, so a token cannot
/// resume a listing it was not issued for.
///
/// Returns `Err` for a token this service did not mint for this selection — the
/// caller answers that with its own modelled client error.
pub fn page_by_key<K: AsRef<str>, V>(
    entries: impl Iterator<Item = (K, V)>,
    selection: &str,
    token: Option<&str>,
    page_size: usize,
) -> Result<Page<V>, InvalidToken> {
    let after = match token {
        None => None,
        Some(token) => Some(decode_next_token(selection, token).ok_or(InvalidToken)?),
    };
    let mut remaining =
        entries.filter(|(key, _)| after.as_deref().is_none_or(|after| key.as_ref() > after));

    let mut items = Vec::new();
    let mut last_key = String::new();
    while items.len() < page_size {
        match remaining.next() {
            Some((key, value)) => {
                last_key = key.as_ref().to_string();
                items.push(value);
            }
            // The sequence ran out before the page filled: this is the last page.
            None => {
                return Ok(Page {
                    items,
                    next_token: None,
                })
            }
        }
    }

    // The page filled. Only a sequence that still has an entry gets a token, so a
    // listing whose total is an exact multiple of `limit` does not hand back a token
    // that would resolve to an empty page.
    let next_token = remaining
        .next()
        .map(|_| encode_next_token(selection, &last_key));
    Ok(Page { items, next_token })
}

/// A `NextToken` this service did not mint for the listing it was presented to.
#[derive(Debug)]
pub struct InvalidToken;

/// The ceiling on how many registries this store holds, on how many schemas one
/// registry holds, and on the total schema content it retains: the caps that bound
/// caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// a create is the only thing that adds one, so the store needs explicit maxima —
/// neither a TTL nor an eviction policy is available to it, because `DescribeSchema`
/// must still answer what `CreateSchema` created. Without them, one `CreateRegistry`
/// per caller-chosen `RegistryName` and one `CreateSchema` per caller-chosen
/// `SchemaName` grow the store for as long as a client keeps calling.
///
/// The entry counts bound the PRODUCT at
/// `MAX_REGISTRIES * MAX_SCHEMAS_PER_REGISTRY` schemas, and
/// [`MAX_STORED_CONTENT_BYTES`] bounds what those entries can retain: the model caps
/// one `Content` at 100 000 characters, so the entry counts alone would still admit
/// that many hundred-kilobyte payloads. Both creates refuse past their cap with the
/// service's own `BadRequestException` (HTTP 400) — this model declares no quota
/// error, and a client error is the honest class: the store cannot accept the
/// request, and no retry of it will succeed.
///
/// The numbers are the emulator's own bound, not a published AWS default: AWS
/// publishes these quotas as adjustable, so no single value is the contract, and they
/// are set high enough that no ordinary workflow meets one.
pub const MAX_REGISTRIES: usize = 500;

/// See [`MAX_REGISTRIES`].
pub const MAX_SCHEMAS_PER_REGISTRY: usize = 500;

/// See [`MAX_REGISTRIES`]. Counted over every version of every schema in every
/// registry, which is everything a caller's `Content` can make this service retain.
///
/// This slice stores exactly one version per schema (only `CreateSchema` writes a
/// version), so the version map is bounded at one entry per schema today; an
/// `UpdateSchema` that mints further versions needs its own per-schema version cap
/// beside this budget.
pub const MAX_STORED_CONTENT_BYTES: usize = 64 * 1024 * 1024;

/// See [`MAX_REGISTRIES`]. The ceiling on how many code bindings one schema holds.
///
/// A binding is keyed by schema version and language. The language is one of the
/// finitely many [`CODE_BINDING_LANGUAGES`], so what a caller can grow is the number
/// of VERSIONS it starts bindings for; this cap is the explicit ceiling on the
/// product, kept so the binding map is bounded however many versions a schema comes
/// to hold. A put past the cap is refused with the service's own
/// `BadRequestException`, as the other caps are.
pub const MAX_CODE_BINDINGS_PER_SCHEMA: usize = 500;

/// The most items one page of a listing may hold, whatever `Limit` asks for.
///
/// Without it, an absent or very large `Limit` would materialise the whole selected
/// store into a single response — the amplifying half of an unbounded store. A
/// listing that has more to give says so with a `NextToken`, which is how a caller
/// reads past one page, so clamping loses nothing.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most keywords one `SearchSchemas` request may name.
///
/// A keyword search costs the PRODUCT of two factors: how many keywords the request
/// names, and how much stored text each one is matched against. The stored side is
/// already bounded ([`MAX_STORED_CONTENT_BYTES`]), but the request side is not —
/// `Keywords` is one query-string value that yields one more keyword for every
/// separator a caller puts in it, so without this cap a single small GET could ask
/// for an arbitrary multiple of a scan of the whole store. With it, one request costs
/// at most `MAX_KEYWORDS_PER_SEARCH` passes over the text of the registry it names.
///
/// Refused past the cap with the service's own `BadRequestException` (HTTP 400): this
/// model declares no quota error, and the store cannot serve the request as asked, so
/// no retry of it will succeed. The number is the emulator's own bound, not a
/// published AWS limit, and is set high enough that no ordinary search meets it.
pub const MAX_KEYWORDS_PER_SEARCH: usize = 10;

/// The page size to serve a request whose `Limit` member is `limit`.
///
/// `Err` when the request asked for a page of fewer than one item: the model gives
/// `Limit` no range, so that bound is the handler's, and a page of zero items is not
/// a page. Anything larger than [`MAX_PAGE_SIZE`] is served as `MAX_PAGE_SIZE`, and
/// an absent `Limit` means "as much as one page may hold".
pub fn requested_page_size(limit: Option<i32>) -> Result<usize, BadRequestException> {
    match limit {
        None => Ok(MAX_PAGE_SIZE),
        Some(limit) if limit >= 1 => Ok(usize::try_from(limit)
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE)),
        Some(limit) => Err(limit_out_of_range(limit)),
    }
}

/// How many bytes of schema `Content` this store currently retains, across every
/// version of every schema in every registry — what [`MAX_STORED_CONTENT_BYTES`]
/// bounds.
///
/// Walked on demand rather than tracked in a counter: a counter has to be decremented
/// on every delete path too, and a counter that drifts reports a budget the store is
/// not actually using.
pub fn stored_content_bytes(state: &ComAmazonawsSchemasSchemasState) -> usize {
    state
        .registries
        .values()
        .flat_map(|registry| registry.schemas.values())
        .flat_map(|schema| schema.versions.values())
        .map(|version| version.content.len())
        .sum()
}

/// The `Code` member of this service's error shapes is documented as "the error
/// code", and the code a restJson1 client matches on is the error's shape name, so
/// every error this emulator raises carries its own shape name there.
pub fn not_found(message: impl Into<String>) -> NotFoundException {
    NotFoundException {
        code: "NotFoundException".to_string(),
        message: message.into(),
    }
}

/// See [`not_found`] for the `Code` member.
pub fn conflict(message: impl Into<String>) -> ConflictException {
    ConflictException {
        code: "ConflictException".to_string(),
        message: message.into(),
    }
}

/// See [`not_found`] for the `Code` member.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: "BadRequestException".to_string(),
        message: message.into(),
    }
}

/// See [`not_found`] for the `Code` member.
///
/// Raised only where a handler cannot reach its own store, which is a server fault.
pub fn internal_error() -> InternalServerErrorException {
    InternalServerErrorException {
        code: "InternalServerErrorException".to_string(),
        message: "The schemas emulator could not reach its own state store.".to_string(),
    }
}

/// Message for a registry no caller has created.
pub fn registry_not_found(registry: &str) -> NotFoundException {
    not_found(format!("Registry {registry} does not exist."))
}

/// Message for a schema the named registry does not hold.
pub fn schema_not_found(registry: &str, schema: &str) -> NotFoundException {
    not_found(format!(
        "Schema {schema} does not exist in registry {registry}."
    ))
}

/// Message for a version the named schema does not hold — including a `SchemaVersion`
/// that is not a version number at all.
pub fn schema_version_not_found(registry: &str, schema: &str, requested: Option<&str>) -> NotFoundException {
    match requested {
        Some(version) => not_found(format!(
            "Schema {schema} in registry {registry} has no version {version}."
        )),
        None => not_found(format!(
            "Schema {schema} in registry {registry} has no versions."
        )),
    }
}

/// Message for a code binding nobody has started for this schema version and
/// language.
pub fn code_binding_not_found(
    registry: &str,
    schema: &str,
    version: u64,
    language: &str,
) -> NotFoundException {
    not_found(format!(
        "No code binding exists for language {language} of version {version} of schema \
         {schema} in registry {registry}."
    ))
}

/// Refuse a `Language` this service generates no code bindings for.
///
/// The model gives `Language` no enum, so this is the service's own constraint, and
/// the refusal states it the way the service does: it names the parameter it checked
/// and the pattern that parameter must match ([`code_binding_language_pattern`]), so
/// a caller can fix the request from the message alone. The rejected value is not
/// echoed back — the pattern is what the caller needs.
pub fn unsupported_code_binding_language() -> BadRequestException {
    bad_request(format!(
        "Parameter {CODE_BINDING_LANGUAGE_PARAMETER} doesn't match pattern {}",
        code_binding_language_pattern()
    ))
}

/// Refuse a `Limit` the model cannot express a bound for: a page of fewer than one
/// item is not a page. Names the member and the bound, as a validation error does.
pub fn limit_out_of_range(limit: i32) -> BadRequestException {
    bad_request(format!("Limit must be at least 1, but was {limit}."))
}

/// Refuse a create that would take the store past a cap it keeps so caller-driven
/// growth stays bounded. Names the cap, so the caller knows a retry cannot help.
pub fn store_cap_reached(what: &str, cap: usize) -> BadRequestException {
    bad_request(format!(
        "This emulator holds at most {cap} {what}; delete one before creating another."
    ))
}

/// Refuse a search that names more keywords than one request may carry. Names the
/// member, the bound and what the request asked for, as a validation error does.
pub fn too_many_keywords(named: usize) -> BadRequestException {
    bad_request(format!(
        "Keywords may name at most {MAX_KEYWORDS_PER_SEARCH} keywords, but this request named {named}."
    ))
}

/// Refuse a `NextToken` this service did not mint for this listing, without echoing
/// it back.
pub fn invalid_next_token() -> BadRequestException {
    bad_request(
        "The NextToken provided is not a token this service issued for this listing. \
         A NextToken resumes the listing it was returned by, so every request that \
         follows it must repeat that listing's selection unchanged.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selection the helper's own tests page under; its shape does not matter
    /// here, only that it is the same one on both sides of a token.
    const SELECTION: &str = "prefix=;scope=Local";

    /// A listing shorter than the page size is one page and carries no token.
    #[test]
    fn a_single_short_page_has_no_token() {
        let entries = vec![("a", 1), ("b", 2)];
        let page = page_by_key(entries.into_iter(), SELECTION, None, 10).expect("fresh token");
        assert_eq!(page.items, vec![1, 2], "every entry is on the page");
        assert!(
            page.next_token.is_none(),
            "a short page must not carry a token, got {:?}",
            page.next_token
        );
    }

    /// A page filled exactly by the last entries carries no token either, so a
    /// client never follows a token to an empty page.
    #[test]
    fn an_exactly_filled_final_page_has_no_token() {
        let entries = vec![("a", 1), ("b", 2)];
        let page = page_by_key(entries.into_iter(), SELECTION, None, 2).expect("fresh token");
        assert!(
            page.next_token.is_none(),
            "an exactly-filled final page must not carry a token, got {:?}",
            page.next_token
        );
    }

    /// Following the token resumes strictly after the last item of the page before.
    #[test]
    fn the_token_resumes_after_the_last_item() {
        let entries = vec![("a", 1), ("b", 2), ("c", 3)];
        let first =
            page_by_key(entries.clone().into_iter(), SELECTION, None, 2).expect("fresh token");
        assert_eq!(first.items, vec![1, 2]);
        let token = first.next_token.expect("a further entry remains");
        let second =
            page_by_key(entries.into_iter(), SELECTION, Some(&token), 2).expect("own token");
        assert_eq!(second.items, vec![3], "the next page resumes after 'b'");
        assert!(second.next_token.is_none(), "and terminates");
    }

    /// A token this service did not mint is refused rather than silently restarting
    /// the listing from the top.
    #[test]
    fn a_foreign_token_is_refused() {
        let entries = vec![("a", 1)];
        for foreign in ["not-a-token", "zz", "6162", "", "."] {
            assert!(
                page_by_key(entries.clone().into_iter(), SELECTION, Some(foreign), 10).is_err(),
                "case {foreign:?}: a token this service did not mint must be refused"
            );
        }
    }

    /// A token names a position within ONE listing: presenting it to a listing with
    /// a different selection is refused, rather than silently skipping or repeating
    /// items.
    #[test]
    fn a_token_does_not_resume_a_differently_selected_listing() {
        let entries = vec![("a", 1), ("b", 2), ("c", 3)];
        let first =
            page_by_key(entries.clone().into_iter(), SELECTION, None, 1).expect("fresh token");
        let token = first.next_token.expect("further entries remain");
        assert!(
            page_by_key(
                entries.into_iter(),
                "prefix=b;scope=Local",
                Some(&token),
                1
            )
            .is_err(),
            "a token issued for one selection must not resume another"
        );
    }

    /// `Limit` resolves to a page size that is never zero and never larger than the
    /// server-side ceiling, so no single response can materialise the whole store.
    #[test]
    fn a_requested_page_size_is_clamped_to_the_server_ceiling() {
        let ceiling = i32::try_from(MAX_PAGE_SIZE).expect("the ceiling fits an i32");
        // Rows: absent, the published bounds of the clamp, one past each, and the
        // extremes of the modelled `__integer`.
        for (limit, expected) in [
            (None, Some(MAX_PAGE_SIZE)),
            (Some(1), Some(1)),
            (Some(ceiling - 1), Some(MAX_PAGE_SIZE - 1)),
            (Some(ceiling), Some(MAX_PAGE_SIZE)),
            (Some(ceiling + 1), Some(MAX_PAGE_SIZE)),
            (Some(i32::MAX), Some(MAX_PAGE_SIZE)),
            (Some(0), None),
            (Some(-1), None),
            (Some(i32::MIN), None),
        ] {
            let resolved = requested_page_size(limit).ok();
            assert_eq!(
                resolved, expected,
                "case limit={limit:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// The status a binding reports is one the model accepts, and it is the clock —
    /// not a read — that moves it: in progress until the instant generation is due,
    /// complete from then on, whoever asks and however often.
    #[test]
    fn a_code_binding_status_follows_the_clock() {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let binding = CodeBinding {
            language: "Java8".to_string(),
            schema_version: 3,
            created_date: created,
            generation_due: created + CODE_GENERATION_DURATION,
        };

        // Rows: the put instant, just before the due instant, the due instant itself,
        // and long after it.
        for (offset, expected_status, expected_modified) in [
            (Duration::ZERO, CODE_GENERATION_IN_PROGRESS, created),
            (
                CODE_GENERATION_DURATION - Duration::from_millis(1),
                CODE_GENERATION_IN_PROGRESS,
                created,
            ),
            (
                CODE_GENERATION_DURATION,
                CODE_GENERATION_COMPLETE,
                created + CODE_GENERATION_DURATION,
            ),
            (
                Duration::from_secs(3600),
                CODE_GENERATION_COMPLETE,
                created + CODE_GENERATION_DURATION,
            ),
        ] {
            let report = binding.report_at(created + offset);
            assert!(
                crate::ops::types::CODE_GENERATION_STATUS_VALUES.contains(&report.status),
                "case +{offset:?}: {:?} is not a modelled CodeGenerationStatus",
                report.status
            );
            assert_eq!(
                report.status, expected_status,
                "case +{offset:?}: expected status {expected_status}, got {}",
                report.status
            );
            assert_eq!(
                report.last_modified, expected_modified,
                "case +{offset:?}: expected last-modified {expected_modified:?}, got {:?}",
                report.last_modified
            );
            assert_eq!(
                report.creation_date, created,
                "case +{offset:?}: the creation date never moves"
            );
            assert_eq!(
                report.schema_version, "3",
                "case +{offset:?}: expected the stored version in decimal, got {}",
                report.schema_version
            );
        }
    }

    /// The content budget counts every version of every schema in every registry —
    /// everything a caller's `Content` makes this service retain.
    #[test]
    fn the_content_budget_counts_every_stored_version() {
        let mut state = ComAmazonawsSchemasSchemasState::default();
        assert_eq!(
            stored_content_bytes(&state),
            0,
            "an empty store retains nothing"
        );

        let now = std::time::SystemTime::now();
        for (registry_name, contents) in [("r1", ["ab", "cde"]), ("r2", ["f", "ghij"])] {
            let mut registry = Registry {
                name: registry_name.to_string(),
                arn: registry_arn(DEFAULT_REGION, registry_name),
                description: None,
                tags: BTreeMap::new(),
                schemas: BTreeMap::new(),
            };
            for (index, content) in contents.iter().enumerate() {
                let name = format!("s{index}");
                registry.schemas.insert(
                    name.clone(),
                    Schema {
                        arn: schema_arn(DEFAULT_REGION, registry_name, &name),
                        name,
                        description: None,
                        r#type: "OpenApi3".to_string(),
                        tags: BTreeMap::new(),
                        latest_version: 1,
                        last_modified: now,
                        code_bindings: BTreeMap::new(),
                        versions: std::iter::once((
                            1,
                            SchemaVersion {
                                version: 1,
                                content: (*content).to_string(),
                                r#type: "OpenApi3".to_string(),
                                description: None,
                                created_date: now,
                            },
                        ))
                        .collect(),
                    },
                );
            }
            state.registries.insert(registry_name.to_string(), registry);
        }

        // 2 + 3 + 1 + 4, summed across both registries.
        assert_eq!(
            stored_content_bytes(&state),
            10,
            "every stored version's content counts towards the budget"
        );
    }
}
