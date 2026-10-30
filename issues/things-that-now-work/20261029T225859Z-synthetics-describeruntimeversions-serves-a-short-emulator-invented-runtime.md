Issue-Source: trace-sweep-divergence
Issue-Trace: synthetics-catalog-describe-runtime-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: synthetics-catalog-describe-runtime-versions synthetics-canary-create-validation-errors
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|DescribeRuntimeVersions|2xx->2xx|$.RuntimeVersions[*]
Issue-Commit: dbd8b6da40b3283b718ba9768f41bda1f397bd18

# synthetics: DescribeRuntimeVersions serves a short emulator-invented runtime catalog instead of the published runtime versions

> _Note: this issue was identified against the code base at commit `dbd8b6da40b3283b718ba9768f41bda1f397bd18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRuntimeVersions

## Description
One fault, observed from two fixtures (counted as two causes below because the replay clustered them separately; both are closed by the same change).

=== Cause 1 of 2: the published runtime catalog's contents, size and per-entry fields do not match the service's catalog ===
AWS: Smithy: DescribeRuntimeVersions (POST /runtime-versions), output RuntimeVersions -> RuntimeVersionList of RuntimeVersion {VersionName (String 1..1024), Description (String 1..1024), ReleaseDate (timestamp), DeprecationDate (timestamp)}; paginated (NextToken/MaxResults, default and max page 100). The model fixes the SHAPE but not the catalog's contents, so for the contents this is recorded behavior only: the operation reports the service's published list of canary runtime versions — in the recording, ~80 entries ordered newest-first, beginning with the current nodejs and python runtimes (syn-nodejs-5.3, syn-nodejs-playwright-9.0, syn-nodejs-puppeteer-18.0, syn-python-selenium-13.0, ...), each entry carrying VersionName, a Description of the form "Security fixes. Dependencies: <dependency list>" for a patch release, a ReleaseDate, a DeprecationDate (null while not deprecated) and a Language member ('nodejs' | 'python'). Hypothesis: the on-disk Smithy model predates the Language member, which is why it is not in RuntimeVersion above; the recording shows it on every entry.
Emulator: serves a small hard-coded catalog of its own (src/state.rs RUNTIME_VERSION_CATALOG, read by src/ops/describe_runtime_versions.rs) whose entries are older/other versions in a different order — the first entries replayed as syn-python-selenium-3.0, syn-nodejs-puppeteer-6.2, syn-nodejs-puppeteer-7.0, syn-python-selenium-4.0 with emulator-authored Descriptions ("Python canary runtime: scripts are written against the Selenium WebDriver API and run on a bundled headless Chromium."), no Language member, and far fewer entries than the recording's ~80 (every index from 0 to 79 diverged, including indices the emulator does not serve at all).
Why: DescribeRuntimeVersions is how a client discovers which RuntimeVersion strings CreateCanary will accept, and this emulator's CreateCanary validates against that very same catalog ("the one catalog, so the names this operation reports and the names a canary may be created on cannot disagree"). A client that pins a current runtime version, or that picks the newest non-deprecated entry from this list, gets a name the emulator does not know and cannot create a canary; code reading entry['Language'] to split nodejs from python runtimes fails with a missing key; and pagination over ~80 entries (NextToken past the first page) is never exercised because the catalog is far shorter than one page.
Fix: the catalog data itself — RUNTIME_VERSION_CATALOG in src/state.rs (and the RuntimeVersion projection that renders it, which must also emit Language; the service's Smithy model needs the member before it can be serialized). The paging logic in src/ops/describe_runtime_versions.rs is not the defect.
Traces: synthetics-catalog-describe-runtime-versions#task#1.

=== Cause 2 of 2: same catalog divergence observed on the first step of the canary-validation fixture ===
AWS: as cause 1 — the same operation, with the recording's first entry being the then-current nodejs patch release (VersionName syn-nodejs-5.2, Description "Security fixes. Dependencies: Node JS 22.x", Language 'nodejs', DeprecationDate null).
Emulator: answers 200 with its own catalog's first entry (syn-python-selenium-3.0) instead, i.e. neither the version name, the description nor the ordering matches, and Language is absent.
Why: as cause 1 — this is the discovery call a client makes before CreateCanary, so the runtime name it then uses is one the emulator rejects.
Fix: as cause 1 (same RUNTIME_VERSION_CATALOG); listed separately only so a fixer re-replays this fixture too.
Traces: synthetics-canary-create-validation-errors#task#1.

The individual version names, descriptions and release timestamps quoted here are the recording's catalog snapshot; the requirement is that the operation reports the service's published catalog — ordered newest-first, with each entry's VersionName/Description/ReleaseDate/DeprecationDate/Language — rather than a list authored by the emulator.

## Evidence
Corpus simparity-wire-aws-v1, 33 steps, 19 passed, score 57.6%. Both clusters are 200/200 body mismatches on $.RuntimeVersions.

Cause 1 — fixture synthetics-catalog-describe-runtime-versions, step task#1, DescribeRuntimeVersions, expected 200 / actual 200. Divergent keys: $.RuntimeVersions[0] .. $.RuntimeVersions[79] (all 80). Samples:
  [0] expected {'VersionName': 'syn-nodejs-5.3', 'Description': 'Security fixes. Dependencies: Node JS 22.x', 'Language': 'nodejs', 'ReleaseDate': 1790553600.0, 'DeprecationDate': None} / got {'VersionName': 'syn-python-selenium-3.0', 'Description': 'Python canary runtime: scripts are written against the Selenium WebDriver API and run on a bundled headless Chromium.', 'ReleaseDate': 1698796800}
  [1] expected {'VersionName': 'syn-nodejs-playwright-9.0', ... 'Language': 'nodejs'} / got {'VersionName': 'syn-nodejs-puppeteer-6.2', ...}
  [2] expected {'VersionName': 'syn-nodejs-puppeteer-18.0', ...} / got {'VersionName': 'syn-nodejs-puppeteer-7.0', ...}
  [3] expected {'VersionName': 'syn-python-selenium-13.0', 'Language': 'python', ...} / got {'VersionName': 'syn-python-selenium-4.0', ...}
  Note no served entry carries Language, and indices beyond the emulator's short catalog are reported 'missing'.

Cause 2 — fixture synthetics-canary-create-validation-errors, step task#1, DescribeRuntimeVersions, expected 200 / actual 200:
  $.RuntimeVersions[0]: missing (expected {'VersionName': 'syn-nodejs-5.2', 'Description': 'Security fixes. Dependencies: Node JS 22.x', 'Language': 'nodejs', 'ReleaseDate': 1784073600.0, 'DeprecationDate': None}, got {'VersionName': 'syn-python-selenium-3.0', ...})

Source: src/state.rs RUNTIME_VERSION_CATALOG, paged by src/ops/describe_runtime_versions.rs (lines ~195-212) and shared with CreateCanary's RuntimeVersion validation per that file's module docs. Model: RuntimeVersion has no Language member on disk, while every recorded entry carries one.

## Steps to Reproduce
Call DescribeRuntimeVersions with no NextToken/MaxResults and compare the RuntimeVersions list against the service's published catalog: the emulator answers a short list whose first entries are syn-python-selenium-3.0 / syn-nodejs-puppeteer-6.2 / syn-nodejs-puppeteer-7.0 with emulator-authored Descriptions and no Language member, where the recording returns ~80 newest-first entries (syn-nodejs-5.x, syn-nodejs-playwright-9.0, syn-nodejs-puppeteer-18.0, syn-python-selenium-13.0, ...) each with Language and a 'Security fixes. Dependencies: ...' description, plus a NextToken when more than one page remains. Then CreateCanary with RuntimeVersion set to a current published version name (e.g. the newest entry the real service lists) -> the emulator rejects it because its catalog does not contain that name.
