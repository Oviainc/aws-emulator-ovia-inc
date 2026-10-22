// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codecatalyst-codecatalyst service.
//!
//! The shape of the store mirrors the API's own containment: a space holds
//! projects, a project holds source repositories. Every map is a
//! [`BTreeMap`] so the list operations enumerate in a stable, sorted order and
//! a pagination cursor over that order resumes deterministically.
//!
//! CodeCatalyst models NO `CreateSpace` operation — a space is created out of
//! band (through the console / an identity flow), so the emulator seeds one
//! ambient space at state init. Everything else is created through the API.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::SystemTime;

/// The ambient space the emulator starts with. The API models no `CreateSpace`,
/// so without a seeded space every space-scoped operation would be unreachable.
/// The name satisfies `NameString` (len 3..63, pattern
/// `^[a-zA-Z0-9]+(?:[-_\.][a-zA-Z0-9]+)*$`).
pub const DEFAULT_SPACE_NAME: &str = "my-space";

/// The Region reported for the seeded space. CodeCatalyst spaces are hosted in
/// `us-west-2`, and the value has to satisfy `RegionString` (len 3..16, pattern
/// `^(us(?:-gov)?|af|ap|ca|cn|eu|sa)-(central|(?:north|south)?(?:east|west)?)-(\d+)$`).
pub const DEFAULT_SPACE_REGION: &str = "us-west-2";

/// A CodeCatalyst space and the projects created in it.
#[derive(Debug, Clone)]
pub struct Space {
    pub name: String,
    pub region_name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// Keyed by the project's `name` (the derived, API-visible identifier).
    pub projects: BTreeMap<String, Project>,
}

/// A project in a space, and the source repositories created under it.
#[derive(Debug, Clone)]
pub struct Project {
    pub space_name: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// Keyed by the repository's `name`.
    pub source_repositories: BTreeMap<String, SourceRepository>,
}

/// A source repository under a project.
#[derive(Debug, Clone)]
pub struct SourceRepository {
    pub space_name: String,
    pub project_name: String,
    pub name: String,
    pub description: Option<String>,
    pub created_time: SystemTime,
    pub last_updated_time: SystemTime,
}

/// Per-service state for com-amazonaws-codecatalyst-codecatalyst. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsCodecatalystCodecatalystState {
    /// Keyed by space name.
    pub spaces: BTreeMap<String, Space>,
}

impl Default for ComAmazonawsCodecatalystCodecatalystState {
    /// Seeds the one ambient space (see [`DEFAULT_SPACE_NAME`]). This runs once,
    /// when the engine first hands out this service's store, so a read of the
    /// seeded space is a read — nothing a later caller sees changes because of it.
    fn default() -> Self {
        let space = Space {
            name: DEFAULT_SPACE_NAME.to_string(),
            region_name: DEFAULT_SPACE_REGION.to_string(),
            display_name: Some(DEFAULT_SPACE_NAME.to_string()),
            description: None,
            projects: BTreeMap::new(),
        };
        Self {
            spaces: BTreeMap::from([(space.name.clone(), space)]),
        }
    }
}

impl ComAmazonawsCodecatalystCodecatalystState {
    /// The space named `space_name`, or `None` when no such space exists.
    pub fn space(&self, space_name: &str) -> Option<&Space> {
        self.spaces.get(space_name)
    }

    /// Mutable access to the space named `space_name`.
    pub fn space_mut(&mut self, space_name: &str) -> Option<&mut Space> {
        self.spaces.get_mut(space_name)
    }

    /// The project `project_name` in `space_name`, or `None` when either the
    /// space or the project does not exist.
    pub fn project(&self, space_name: &str, project_name: &str) -> Option<&Project> {
        self.space(space_name)?.projects.get(project_name)
    }

    /// Mutable access to the project `project_name` in `space_name`.
    pub fn project_mut(&mut self, space_name: &str, project_name: &str) -> Option<&mut Project> {
        self.space_mut(space_name)?.projects.get_mut(project_name)
    }
}

/// The project `name` CodeCatalyst derives from a `displayName`.
///
/// `ProjectDisplayName` (len 3..63, pattern `^[a-zA-Z0-9]+(?:[-_\. ][a-zA-Z0-9]+)*$`)
/// admits the space as a separator; `NameString` (len 3..63, pattern
/// `^[a-zA-Z0-9]+(?:[-_\.][a-zA-Z0-9]+)*$`) does not, and the hyphen is the
/// separator both admit. Replacing each separating space with a hyphen is
/// therefore a total map from one shape's value space into the other's: it
/// preserves length, so a display name inside 3..63 derives a name inside 3..63,
/// and leaves every name that is already a `NameString` unchanged.
pub fn derived_project_name(display_name: &str) -> String {
    display_name.replace(' ', "-")
}

/// Encode a pagination cursor: the last key a page returned, hex-encoded so the
/// token is opaque to the caller (it is a cursor, not a resource name).
pub fn encode_cursor(last_key: &str) -> String {
    hex::encode(last_key)
}

/// Decode a pagination cursor produced by [`encode_cursor`]. `None` when the
/// token was not produced by this service, which a caller is told about with a
/// modelled `ValidationException` rather than a silently empty page.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// The most entries ANY list response from this service carries, whatever the
/// caller asks for.
///
/// `ListProjects.maxResults` is modelled `@range(min: 1, max: 100)`
/// (`smithy_query(query="operation",operation="ListProjects")`), so 100 is the
/// service's own published page maximum. It is applied as a SERVER-SIDE cap to
/// every listing here — including `ListSpaces`, which models no `maxResults` at
/// all — so a request can never make one response proportional to the whole
/// store: a caller that wants more follows the `nextToken`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most projects one space holds, and the most source repositories one
/// project holds — named `EMULATOR_` because they are this emulator's own
/// resource guard, NOT an AWS-published quota.
///
/// Both are the EMULATOR'S OWN resource guard, not an AWS-published quota: the
/// create operations are the only caller-driven growth sites in this service's
/// store, so without a cap one client could grow it for the life of the process.
/// A create past the cap is refused with the modelled
/// `ServiceQuotaExceededException` (402), which is what the real service answers
/// when "one or more resources has reached its limits". The value is set above
/// [`MAX_PAGE_SIZE`] so paging is still exercised across several pages, and it
/// also bounds the work a filtered listing can be asked to do (see
/// [`MAX_SCANNED_ENTRIES`]).
pub const EMULATOR_MAX_PROJECTS_PER_SPACE: usize = 200;
pub const EMULATOR_MAX_SOURCE_REPOSITORIES_PER_PROJECT: usize = 200;

/// The most entries one listing request may WALK, however selective the
/// caller's filter is.
///
/// A filtered page has to look at entries it discards, so the scan — unlike the
/// response — is not bounded by the page size. This budget bounds it: when it
/// runs out the page is returned short, with a cursor at the last entry
/// EXAMINED, and the caller follows that cursor for the rest. Set to ten pages'
/// worth, so a selective filter still makes progress in one call.
pub const MAX_SCANNED_ENTRIES: usize = MAX_PAGE_SIZE as usize * 10;

/// One page of a sorted map, and the cursor that resumes after it.
pub struct Page<'a, V> {
    /// The values on this page, in key order.
    pub items: Vec<&'a V>,
    /// `Some` only when the page was truncated — i.e. at least one more entry
    /// follows it. A returned token resumes strictly after the last item here.
    pub next_token: Option<String>,
}

/// Take the page of `map` that starts after the cursor `after` (decoded by
/// [`decode_cursor`]) and holds at most `max_results` entries.
///
/// The page size is the caller's `max_results` CLAMPED into `1..=MAX_PAGE_SIZE`,
/// and `MAX_PAGE_SIZE` when the caller names none — so neither an absent, a
/// negative nor an oversized `maxResults` can turn one request into a response
/// (or a scan) proportional to everything stored. A token is emitted ONLY when
/// entries remain after the page, so a caller that paged to the end gets `None`
/// and stops.
pub fn page<'a, V>(
    map: &'a BTreeMap<String, V>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<'a, V> {
    page_where(map, after, max_results, |_| true)
}

/// [`page`] over the entries `keep` accepts. The cursor runs over the key order
/// of the WALKED entries, so a token resumes where this call stopped looking,
/// whether it stopped because the page filled or because the scan budget ran
/// out.
///
/// Two independent bounds hold per request: at most `max_results` (clamped into
/// `1..=MAX_PAGE_SIZE`) entries are RETURNED, and at most
/// [`MAX_SCANNED_ENTRIES`] entries are WALKED — so `keep` is called at most
/// `MAX_SCANNED_ENTRIES` times however selective the filter is. A page that
/// stops on the scan budget comes back SHORT with a cursor, which is how a
/// filtered listing makes progress without doing work proportional to the whole
/// store in one call.
pub fn page_where<'a, V>(
    map: &'a BTreeMap<String, V>,
    after: Option<&str>,
    max_results: Option<i32>,
    mut keep: impl FnMut(&V) -> bool,
) -> Page<'a, V> {
    let requested = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let limit = usize::try_from(requested).unwrap_or(1);
    let start = match after {
        Some(key) => Bound::Excluded(key),
        None => Bound::Unbounded,
    };
    let mut rest = map.range::<str, _>((start, Bound::Unbounded));

    let mut items = Vec::new();
    let mut last_key: Option<&str> = None;
    let mut walked = 0usize;
    let mut stopped_early = false;
    for (key, value) in rest.by_ref() {
        walked += 1;
        if keep(value) {
            items.push(value);
        }
        last_key = Some(key.as_str());
        if items.len() >= limit || walked >= MAX_SCANNED_ENTRIES {
            stopped_early = true;
            break;
        }
    }
    // A cursor only where this call stopped with entries still behind it: a
    // listing that reached the end of the map ends with `None`, so a caller that
    // follows the token always terminates.
    let next_token = match (stopped_early && rest.next().is_some(), last_key) {
        (true, Some(key)) => Some(encode_cursor(key)),
        _ => None,
    };
    Page { items, next_token }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodecatalystCodecatalystState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodecatalystCodecatalystState>)
}
