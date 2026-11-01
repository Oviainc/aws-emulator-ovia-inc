// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-inspectorscan-inspectorscan service.
//!
//! `ScanSbom` is this service's only operation, and it answers from the advisory
//! data held here: the seeded [`VulnRecord`] set plus the `(package_type,
//! package_name)` lookup index built over it. A finding in a scan response is
//! always a projection of one of these records onto one component of the
//! submitted SBOM — never a literal in the handler.

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

/// The most completed scans the audit store keeps.
///
/// The store grows once per DISTINCT submitted document, and the digest that
/// keys it is chosen entirely by the caller, so it is capped and evicted
/// oldest-first rather than left to accumulate for the life of the process.
pub const MAX_SCANS: usize = 256;

/// The most finding ids one [`ScanRecord`] retains. A scan may report one
/// finding per (component, advisory) pair, so the per-record size is bounded
/// here as well as the record count.
pub const MAX_RECORDED_FINDING_IDS: usize = 512;

/// The most characters of a submitted `serialNumber` one [`ScanRecord`]
/// retains. The value is caller-chosen, so it is bounded before it is stored;
/// a well-formed CycloneDX serial number is a 45-character `urn:uuid:` string.
pub const MAX_RECORDED_SERIAL_NUMBER_LEN: usize = 128;

/// One advisory in the vulnerability database: a CVE as it applies to one
/// package in one ecosystem.
///
/// `affected` is the release interval the advisory applies to, held as DATA
/// rather than as a comparator expression to parse: see [`VersionRange`].
#[derive(Debug, Clone)]
pub struct VulnRecord {
    /// The vulnerability id, e.g. `CVE-2021-44228`.
    pub cve_id: String,
    /// The advisory's publisher, as CycloneDX `vulnerability.source.name`.
    pub source: String,
    /// The advisory's canonical URL, as CycloneDX `vulnerability.source.url`.
    pub source_url: Option<String>,
    /// Package name in the ecosystem's own convention: `group:artifact` for
    /// maven, the module path for golang, the plain package name elsewhere.
    pub package_name: String,
    /// The purl ecosystem this advisory applies to (`npm`, `pypi`, `maven`,
    /// `golang`, `deb`, `rpm`).
    pub package_type: String,
    /// The affected releases, as the interval the advisory states.
    pub affected: VersionRange,
    /// The first version carrying the fix, when one has shipped.
    pub fixed_in_version: Option<String>,
    /// The NVD CVSS v3.1 base score and its vector.
    pub cvss_v3_score: f64,
    pub cvss_v3_vector: String,
    /// NVD's qualitative severity, uppercase (`CRITICAL` … `LOW`).
    pub nvd_severity: String,
    /// Amazon Inspector's own (environment-adjusted) CVSS v3.1 score, vector
    /// and severity. Reported beside the NVD rating, which is why the two can
    /// differ — exactly the discrepancy the operation's documentation warns of.
    pub inspector_score: f64,
    pub inspector_cvss_vector: String,
    pub inspector_severity: String,
    /// CWE ids as integers: CycloneDX 1.5 types `vulnerability.cwes` as an
    /// array of integers, so the store keeps them in that form.
    pub cwes: Vec<u32>,
    pub description: String,
    /// Advisory reference URLs, as CycloneDX `vulnerability.advisories[].url`.
    pub advisories: Vec<String>,
    /// Advisory publication / last-modification instants, RFC 3339.
    pub published: String,
    pub modified: String,
}

/// An audit entry for one completed scan.
#[derive(Debug, Clone)]
pub struct ScanRecord {
    pub scan_id: String,
    pub received_at: SystemTime,
    pub output_format: OutputFormat,
    pub component_count: usize,
    pub sbom_serial_number: Option<String>,
    /// The vulnerability ids reported, in the order the response lists them.
    pub finding_ids: Vec<String>,
    /// Hex sha-256 of the canonical form of the submitted document. Two
    /// requests carrying the same SBOM share a digest, which is how a repeat of
    /// an `@idempotent` scan is recognised.
    pub sbom_digest: String,
}

/// The `outputFormat` member's modelled values.
///
/// The wire spellings are exactly `crate::ops::types::OUTPUT_FORMAT_VALUES`,
/// which is generated from the model's `OutputFormat` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    CycloneDx15,
    Inspector,
    InspectorAlt,
}

impl OutputFormat {
    /// The format for a wire value, or `None` when the model declares no such
    /// member value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "CYCLONE_DX_1_5" => Some(Self::CycloneDx15),
            "INSPECTOR" => Some(Self::Inspector),
            "INSPECTOR_ALT" => Some(Self::InspectorAlt),
            _ => None,
        }
    }

    /// The wire spelling of this format.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::CycloneDx15 => "CYCLONE_DX_1_5",
            Self::Inspector => "INSPECTOR",
            Self::InspectorAlt => "INSPECTOR_ALT",
        }
    }
}

/// The key the scan audit store is addressed by: the submitted document's
/// digest and the format it was reported in.
pub type ScanKey = (String, OutputFormat);

/// Per-service state for com-amazonaws-inspectorscan-inspectorscan. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsInspectorscanInspectorscanState {
    /// The advisory data every finding is projected from.
    pub vulnerability_db: Vec<VulnRecord>,
    /// `(package_type, folded package_name)` -> indices into
    /// `vulnerability_db`. The per-component lookup path; build it with
    /// [`build_index`] so the keys agree with [`Self::matching_advisories`].
    pub index: HashMap<(String, String), Vec<usize>>,
    /// Completed scans, keyed for O(1) idempotent-repeat resolution. Private:
    /// the cap and the eviction order are invariants of this pair, so records
    /// go in through [`Self::record_scan`] and come out through
    /// [`Self::scan_for`] / [`Self::scans`].
    scans: HashMap<ScanKey, ScanRecord>,
    /// The keys of `scans`, oldest first — the eviction order that keeps the
    /// store at [`MAX_SCANS`].
    scan_order: VecDeque<ScanKey>,
    /// Monotonic counter backing generated `scan_id` values, so two distinct
    /// scans are distinguishable in state. Private for the same reason as
    /// `scans`: it is part of the scan-identity invariant, so it moves only
    /// through [`Self::next_scan_id`] and is read through
    /// [`Self::next_serial`].
    next_serial: u64,
}

impl Default for ComAmazonawsInspectorscanInspectorscanState {
    fn default() -> Self {
        let vulnerability_db = seed_vulnerability_db();
        let index = build_index(&vulnerability_db);
        Self {
            vulnerability_db,
            index,
            scans: HashMap::new(),
            scan_order: VecDeque::new(),
            next_serial: 1,
        }
    }
}

impl ComAmazonawsInspectorscanInspectorscanState {
    /// Every advisory that applies to `package_name` in `package_type` at
    /// `version`, as indices into [`Self::vulnerability_db`].
    ///
    /// An empty result is the normal answer for a package with no known
    /// advisory, and for a version that is already past the affected range.
    pub fn matching_advisories(
        &self,
        package_type: &str,
        package_name: &str,
        version: &str,
    ) -> Vec<usize> {
        let Some(candidates) = self.index.get(&lookup_key(package_type, package_name)) else {
            return Vec::new();
        };
        candidates
            .iter()
            .copied()
            // `get`, not `[]`: a caller that reseeds `vulnerability_db` without
            // rebuilding `index` should drop a stale candidate, not panic.
            .filter(|&i| {
                self.vulnerability_db
                    .get(i)
                    .is_some_and(|record| version_in_range(version, &record.affected))
            })
            .collect()
    }

    /// Does the advisory data hold any rule for this package coordinate?
    ///
    /// This is what a scan reports a component as SKIPPED by: the store holds
    /// nothing about the package, so the scan reached no conclusion about that
    /// component at all. It is a different answer from
    /// [`Self::matching_advisories`] being empty, which means a rule was
    /// applied and the resolved version fell outside every affected range —
    /// i.e. the component was ruled on and is clean.
    pub fn has_supported_rules(&self, package_type: &str, package_name: &str) -> bool {
        self.index
            .get(&lookup_key(package_type, package_name))
            .is_some_and(|candidates| !candidates.is_empty())
    }

    /// The scan already recorded for this document and output format, if any.
    /// `ScanSbom` is `@idempotent`, so a repeat of a scan answers from the
    /// record the first one wrote instead of minting a second identity.
    ///
    /// HORIZON: the store holds the most recent [`MAX_SCANS`] distinct
    /// (document, format) pairs, so a repeat resolves only while the first
    /// scan's record is still held — past that, an evicted document is
    /// indistinguishable from one never scanned and the caller mints a fresh
    /// identity. [`Self::record_scan`] keeps the order last-touch, so the
    /// documents a caller keeps re-submitting are the last to be evicted.
    ///
    /// O(1): the store is a map, so the per-request cost under the state lock
    /// does not grow with how many scans have been served.
    pub fn scan_for(&self, sbom_digest: &str, output_format: OutputFormat) -> Option<&ScanRecord> {
        self.scans
            .get(&(sbom_digest.to_string(), output_format))
    }

    /// Record a completed scan, replacing the record for the same document and
    /// format when there is one, and evicting the oldest records once the store
    /// is at [`MAX_SCANS`].
    ///
    /// Replacing on a repeat is what keeps the store and the wire response in
    /// agreement: the repeat's report is recomputed against the advisory data
    /// as it stands now, so the record it leaves behind describes that report.
    /// The scan's identity (`scan_id`, `received_at`) is carried over by the
    /// caller, so a repeat does not mint a second identity.
    ///
    /// Both caller-chosen strings a record retains are bounded here, at the
    /// growth site: the submitted serial number to
    /// [`MAX_RECORDED_SERIAL_NUMBER_LEN`] characters, and the reported finding
    /// ids to [`MAX_RECORDED_FINDING_IDS`] entries.
    pub fn record_scan(&mut self, mut record: ScanRecord) {
        record.sbom_serial_number = record
            .sbom_serial_number
            .map(|serial| serial.chars().take(MAX_RECORDED_SERIAL_NUMBER_LEN).collect());
        record.finding_ids.truncate(MAX_RECORDED_FINDING_IDS);

        let key = (record.sbom_digest.clone(), record.output_format);
        if self.scans.insert(key.clone(), record).is_some() {
            // A repeat: move the key to the back so the eviction order is
            // LAST-TOUCH rather than first-insert. A document a caller keeps
            // re-submitting is then the last to lose its record, which is what
            // keeps the `@idempotent` repeat resolvable for the documents
            // actually in use (see [`Self::scan_for`] on the horizon).
            self.scan_order.retain(|recorded| recorded != &key);
        }
        self.scan_order.push_back(key);
        while self.scan_order.len() > MAX_SCANS {
            if let Some(evicted) = self.scan_order.pop_front() {
                self.scans.remove(&evicted);
            }
        }
    }

    /// The recorded scans, oldest first.
    pub fn scans(&self) -> Vec<&ScanRecord> {
        self.scan_order
            .iter()
            .filter_map(|key| self.scans.get(key))
            .collect()
    }

    /// How many scans the audit store currently holds (at most [`MAX_SCANS`]).
    pub fn scan_count(&self) -> usize {
        self.scans.len()
    }

    /// Mint the next scan id and advance the counter.
    pub fn next_scan_id(&mut self) -> String {
        let serial = self.next_serial;
        self.next_serial += 1;
        format!("scan-{serial:012}")
    }

    /// The serial the next minted `scan_id` will carry — read-only; the counter
    /// advances only through [`Self::next_scan_id`].
    pub fn next_serial(&self) -> u64 {
        self.next_serial
    }
}

/// Is a package name case-insensitive in this ecosystem?
///
/// purl lowercases the TYPE unconditionally; name case-folding is per type.
/// npm, PyPI and the OS package ecosystems name packages case-insensitively,
/// while a maven `group:artifact` coordinate and a Go module path are
/// case-SENSITIVE, so folding them would match coordinates the real matcher
/// does not.
fn name_is_case_insensitive(package_type: &str) -> bool {
    matches!(
        package_type,
        "npm" | "pypi" | "deb" | "rpm" | "apk" | "gem" | "nuget" | "composer" | "cargo" | "hex"
    )
}

/// The index key for a package coordinate: the type always folded, the name
/// folded only where its ecosystem is case-insensitive.
fn lookup_key(package_type: &str, package_name: &str) -> (String, String) {
    let package_type = package_type.to_ascii_lowercase();
    let package_name = if name_is_case_insensitive(&package_type) {
        package_name.to_ascii_lowercase()
    } else {
        package_name.to_string()
    };
    (package_type, package_name)
}

/// Build the advisory lookup index over `db`, keyed exactly as
/// [`ComAmazonawsInspectorscanInspectorscanState::matching_advisories`] looks
/// up — through [`lookup_key`], so the two cannot disagree.
pub fn build_index(db: &[VulnRecord]) -> HashMap<(String, String), Vec<usize>> {
    let mut index: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, record) in db.iter().enumerate() {
        index
            .entry(lookup_key(&record.package_type, &record.package_name))
            .or_default()
            .push(i);
    }
    index
}

/// A package release as the three numbers an advisory interval is stated with.
///
/// Ordering is the derived one — by `major`, then `minor`, then `patch` — which
/// is how releases compare: the field order IS the precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl ReleaseVersion {
    /// The release `major.minor.patch`.
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

/// The releases one advisory applies to, as the two bounds an advisory states:
/// the first affected release and the first release carrying the fix.
///
/// Held as DATA, not as a comparator expression: an advisory's affected range is
/// two release numbers, so the store keeps those two numbers and the match is a
/// comparison. There is no range syntax to parse, and so no way for a malformed
/// expression to widen or narrow what an advisory covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionRange {
    /// The first affected release, inclusive. `None` when every release up to
    /// the upper bound is affected.
    pub from_including: Option<ReleaseVersion>,
    /// The first release carrying the fix, exclusive. `None` when no fixed
    /// release has shipped, i.e. everything from the lower bound on is
    /// affected.
    pub before_excluding: Option<ReleaseVersion>,
}

impl VersionRange {
    /// Everything below `fixed` is affected — the shape of an advisory that
    /// names only the release that fixed it.
    pub const fn before(fixed: ReleaseVersion) -> Self {
        Self {
            from_including: None,
            before_excluding: Some(fixed),
        }
    }

    /// `introduced` up to but not including `fixed` — the shape of an advisory
    /// that names both bounds.
    pub const fn between(introduced: ReleaseVersion, fixed: ReleaseVersion) -> Self {
        Self {
            from_including: Some(introduced),
            before_excluding: Some(fixed),
        }
    }

    /// Everything from `introduced` on is affected — the shape of an advisory
    /// with no fix yet.
    pub const fn from(introduced: ReleaseVersion) -> Self {
        Self {
            from_including: Some(introduced),
            before_excluding: None,
        }
    }
}

/// Does `version` fall inside the release interval `range`?
///
/// `version` is first reduced to its upstream release numbers by
/// [`normalize_version`], because a package version as it appears in an SBOM
/// carries decorations an advisory's bounds never do (a `v` prefix, a Debian
/// epoch, a distribution release suffix). The lower bound is inclusive and the
/// upper bound exclusive, as an advisory states them: the fixed release is not
/// affected.
///
/// A range with NEITHER bound bounds nothing and so matches nothing: reading it
/// as "no constraint" would make such an advisory apply to every release ever
/// published.
///
/// APPROXIMATION, and it changes which advisories match. Matching is on the
/// UPSTREAM release only, so:
///
/// * an OS package carrying a backported fix in its distribution release
///   (`openssl 3.0.2-0ubuntu1.10`) still matches an interval written against the
///   upstream version it was branched from, where dpkg/rpm epoch-version-release
///   ordering would clear it;
/// * a pre-release (`2.15.0-rc1`) is compared as its release (`2.15.0`), so it
///   escapes an interval whose upper bound is that release;
/// * a version with no leading release number at all (`latest`, a git hash)
///   matches nothing, and the component is reported clean.
///
/// Tests should assert finding presence on upstream release numbers rather than
/// on these forms.
pub fn version_in_range(version: &str, range: &VersionRange) -> bool {
    let Some(parsed) = normalize_version(version) else {
        return false;
    };
    if range.from_including.is_none() && range.before_excluding.is_none() {
        return false;
    }
    range
        .from_including
        .is_none_or(|introduced| parsed >= introduced)
        && range
            .before_excluding
            .is_none_or(|fixed| parsed < fixed)
}

/// Reduce an SBOM package version to the `major.minor.patch` release the
/// advisory bounds are written against.
///
/// Drops a Debian-style `epoch:` prefix, an optional `v`/`V` prefix, and
/// everything from the first component that is not a plain number (a `-1ubuntu1`
/// package release, an `.el8` distribution tag, an `-rc1` pre-release tag), then
/// pads to three components. `None` when no leading release number is present.
pub fn normalize_version(version: &str) -> Option<ReleaseVersion> {
    let trimmed = version.trim();
    // A Debian epoch (`1:3.0.6-1`) orders versions across upstream renumberings
    // and is not part of the upstream version an advisory range names.
    let without_epoch = match trimmed.split_once(':') {
        Some((epoch, rest)) if !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit()) => {
            rest
        }
        _ => trimmed,
    };
    let without_v = without_epoch
        .strip_prefix('v')
        .or_else(|| without_epoch.strip_prefix('V'))
        .unwrap_or(without_epoch);

    let mut parts: Vec<u64> = Vec::new();
    for component in without_v.split('.') {
        let numeric: String = component.chars().take_while(char::is_ascii_digit).collect();
        if numeric.is_empty() {
            break;
        }
        let Ok(value) = numeric.parse::<u64>() else {
            break;
        };
        parts.push(value);
        if numeric.len() != component.len() || parts.len() == 3 {
            // The component carried a non-numeric tail (`7-1ubuntu1`), or we
            // already have major.minor.patch: stop here.
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    while parts.len() < 3 {
        parts.push(0);
    }
    Some(ReleaseVersion::new(parts[0], parts[1], parts[2]))
}

/// The advisory fixture this emulator scans against.
///
/// Real, published CVEs across the ecosystems Amazon Inspector's SBOM scanner
/// covers (npm, PyPI, Maven, Go, and OS packages), with their NVD CVSS v3.1
/// base scores and vectors. The Inspector rating is the environment-adjusted
/// score Inspector reports beside the NVD one.
fn seed_vulnerability_db() -> Vec<VulnRecord> {
    vec![
        VulnRecord {
            cve_id: "CVE-2021-23337".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2021-23337".to_string()),
            package_name: "lodash".to_string(),
            package_type: "npm".to_string(),
            affected: VersionRange::before(ReleaseVersion::new(4, 17, 21)),
            fixed_in_version: Some("4.17.21".to_string()),
            cvss_v3_score: 7.2,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:H/UI:N/S:U/C:H/I:H/A:H".to_string(),
            nvd_severity: "HIGH".to_string(),
            inspector_score: 7.2,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:H/UI:N/S:U/C:H/I:H/A:H".to_string(),
            inspector_severity: "HIGH".to_string(),
            cwes: vec![77],
            description: "lodash versions prior to 4.17.21 are vulnerable to command injection \
                          via the template function."
                .to_string(),
            advisories: vec![
                "https://github.com/lodash/lodash/commit/3469357cff396a26c363f8c1b5a91dde28ba4b1c"
                    .to_string(),
            ],
            published: "2021-02-15T13:15:00Z".to_string(),
            modified: "2022-01-28T18:15:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2021-44906".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2021-44906".to_string()),
            package_name: "minimist".to_string(),
            package_type: "npm".to_string(),
            affected: VersionRange::before(ReleaseVersion::new(1, 2, 6)),
            fixed_in_version: Some("1.2.6".to_string()),
            cvss_v3_score: 9.8,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H".to_string(),
            nvd_severity: "CRITICAL".to_string(),
            inspector_score: 9.8,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H".to_string(),
            inspector_severity: "CRITICAL".to_string(),
            cwes: vec![1321],
            description: "minimist before 1.2.6 is vulnerable to prototype pollution.".to_string(),
            advisories: vec!["https://github.com/minimistjs/minimist/commit/c2b981977fa834b223b408cfb860f933c9811e4d".to_string()],
            published: "2022-03-17T16:15:00Z".to_string(),
            modified: "2023-11-07T03:39:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2021-33503".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2021-33503".to_string()),
            package_name: "urllib3".to_string(),
            package_type: "pypi".to_string(),
            affected: VersionRange::before(ReleaseVersion::new(1, 26, 5)),
            fixed_in_version: Some("1.26.5".to_string()),
            cvss_v3_score: 7.5,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H".to_string(),
            nvd_severity: "HIGH".to_string(),
            inspector_score: 7.5,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H".to_string(),
            inspector_severity: "HIGH".to_string(),
            cwes: vec![400],
            description: "urllib3 before 1.26.5 allows a denial of service via a crafted URL \
                          with many @ characters."
                .to_string(),
            advisories: vec!["https://github.com/urllib3/urllib3/commit/2d4a3fee6de2fa45eb82169361918f759269b4ec".to_string()],
            published: "2021-06-29T11:15:00Z".to_string(),
            modified: "2023-11-07T03:35:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2020-14343".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2020-14343".to_string()),
            package_name: "pyyaml".to_string(),
            package_type: "pypi".to_string(),
            affected: VersionRange::before(ReleaseVersion::new(5, 4, 0)),
            fixed_in_version: Some("5.4".to_string()),
            cvss_v3_score: 9.8,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H".to_string(),
            nvd_severity: "CRITICAL".to_string(),
            inspector_score: 9.8,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H".to_string(),
            inspector_severity: "CRITICAL".to_string(),
            cwes: vec![20],
            description: "PyYAML before 5.4 allows arbitrary code execution when loading \
                          untrusted YAML with the full_load method or the FullLoader loader."
                .to_string(),
            advisories: vec!["https://github.com/yaml/pyyaml/pull/386".to_string()],
            published: "2021-02-09T22:15:00Z".to_string(),
            modified: "2023-11-07T03:17:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2021-44228".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2021-44228".to_string()),
            package_name: "org.apache.logging.log4j:log4j-core".to_string(),
            package_type: "maven".to_string(),
            affected: VersionRange::between(
                ReleaseVersion::new(2, 0, 0),
                ReleaseVersion::new(2, 15, 0),
            ),
            fixed_in_version: Some("2.15.0".to_string()),
            cvss_v3_score: 10.0,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H".to_string(),
            nvd_severity: "CRITICAL".to_string(),
            inspector_score: 10.0,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H".to_string(),
            inspector_severity: "CRITICAL".to_string(),
            cwes: vec![917, 502, 20],
            description: "Apache Log4j2 2.0-beta9 through 2.15.0 JNDI features used in \
                          configuration, log messages, and parameters do not protect against \
                          attacker controlled LDAP and other JNDI related endpoints."
                .to_string(),
            advisories: vec![
                "https://logging.apache.org/log4j/2.x/security.html".to_string(),
                "https://www.cisa.gov/uscert/apache-log4j-vulnerability-guidance".to_string(),
            ],
            published: "2021-12-10T10:15:00Z".to_string(),
            modified: "2023-11-07T03:39:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2020-36518".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2020-36518".to_string()),
            package_name: "com.fasterxml.jackson.core:jackson-databind".to_string(),
            package_type: "maven".to_string(),
            affected: VersionRange::between(
                ReleaseVersion::new(2, 0, 0),
                ReleaseVersion::new(2, 12, 6),
            ),
            fixed_in_version: Some("2.12.6".to_string()),
            cvss_v3_score: 7.5,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H".to_string(),
            nvd_severity: "HIGH".to_string(),
            inspector_score: 7.5,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H".to_string(),
            inspector_severity: "HIGH".to_string(),
            // NVD maps this advisory to uncontrolled recursion: the defect is
            // a stack exhaustion reached through deeply nested input.
            cwes: vec![674],
            description: "jackson-databind before 2.12.6 allows a Java StackOverflow exception \
                          and denial of service via a large depth of nested objects."
                .to_string(),
            advisories: vec![
                "https://github.com/FasterXML/jackson-databind/issues/2816".to_string()
            ],
            published: "2022-03-11T06:15:00Z".to_string(),
            modified: "2023-11-07T03:22:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2020-28483".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2020-28483".to_string()),
            package_name: "github.com/gin-gonic/gin".to_string(),
            package_type: "golang".to_string(),
            affected: VersionRange::before(ReleaseVersion::new(1, 7, 7)),
            fixed_in_version: Some("1.7.7".to_string()),
            cvss_v3_score: 7.5,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:H/A:N".to_string(),
            nvd_severity: "HIGH".to_string(),
            inspector_score: 7.5,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:H/A:N".to_string(),
            inspector_severity: "HIGH".to_string(),
            cwes: vec![444],
            description: "github.com/gin-gonic/gin before 1.7.7 allows header injection because \
                          it does not sanitize the X-Forwarded-For header."
                .to_string(),
            advisories: vec![
                "https://github.com/gin-gonic/gin/pull/2844".to_string(),
                "https://pkg.go.dev/vuln/GO-2021-0052".to_string(),
            ],
            published: "2021-01-05T18:15:00Z".to_string(),
            modified: "2023-11-07T03:21:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2022-3602".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2022-3602".to_string()),
            package_name: "openssl".to_string(),
            package_type: "deb".to_string(),
            affected: VersionRange::between(
                ReleaseVersion::new(3, 0, 0),
                ReleaseVersion::new(3, 0, 7),
            ),
            fixed_in_version: Some("3.0.7".to_string()),
            cvss_v3_score: 7.5,
            cvss_v3_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H".to_string(),
            nvd_severity: "HIGH".to_string(),
            inspector_score: 7.5,
            inspector_cvss_vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H".to_string(),
            inspector_severity: "HIGH".to_string(),
            cwes: vec![787],
            description: "A buffer overrun can be triggered in X.509 certificate verification, \
                          specifically in name constraint checking, in OpenSSL 3.0.0 through \
                          3.0.6."
                .to_string(),
            advisories: vec!["https://www.openssl.org/news/secadv/20221101.txt".to_string()],
            published: "2022-11-01T17:15:00Z".to_string(),
            modified: "2023-11-07T03:51:00Z".to_string(),
        },
        VulnRecord {
            cve_id: "CVE-2023-4911".to_string(),
            source: "NVD".to_string(),
            source_url: Some("https://nvd.nist.gov/vuln/detail/CVE-2023-4911".to_string()),
            package_name: "glibc".to_string(),
            package_type: "rpm".to_string(),
            affected: VersionRange::between(
                ReleaseVersion::new(2, 34, 0),
                ReleaseVersion::new(2, 38, 1),
            ),
            fixed_in_version: Some("2.38.1".to_string()),
            cvss_v3_score: 7.8,
            cvss_v3_vector: "CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:H".to_string(),
            nvd_severity: "HIGH".to_string(),
            // Locally exploitable only, so Inspector's adjusted score drops the
            // attack vector below NVD's for a packaged image.
            inspector_score: 7.0,
            inspector_cvss_vector: "CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:H/E:P".to_string(),
            inspector_severity: "HIGH".to_string(),
            cwes: vec![787],
            description: "A buffer overflow was discovered in the GNU C Library's dynamic \
                          loader while processing the GLIBC_TUNABLES environment variable."
                .to_string(),
            advisories: vec!["https://sourceware.org/bugzilla/show_bug.cgi?id=30976".to_string()],
            published: "2023-10-03T18:15:00Z".to_string(),
            modified: "2024-01-31T14:15:00Z".to_string(),
        },
    ]
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInspectorscanInspectorscanState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInspectorscanInspectorscanState>)
}
