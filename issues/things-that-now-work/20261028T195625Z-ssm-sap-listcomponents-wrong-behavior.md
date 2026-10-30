Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c6f8380220d2b80f1162c28669d8bc5048f04f8c

# ListComponents orders pages by a key joined with '/', so the listing is not in (ApplicationId, ComponentId) order

_Filed: 2026-10-28T19:56:25Z_
_Operation: ListComponents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c6f8380220d2b80f1162c28669d8bc5048f04f8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListComponents` builds the key it sorts and pages by as `format!("{}/{}", application.id, component.id)`. `/` is byte 0x2F, which is HIGHER than `-` (0x2D) and `.` (0x2E) — two characters the modelled `ApplicationId` pattern `^[\w\d\.-]+$` admits — so ordering those composite strings is NOT the same as ordering by (application id, component id). When one application id is a prefix of another followed by `-` or `.`, the longer id sorts FIRST, and the unnarrowed listing reports its components before the shorter id's.

The crate states and solves exactly this problem for the sibling database listing: `ApplicationDatabase::page_key` joins the two ids with a separator chosen to be "a byte lower than any the modelled `ApplicationId` ... patterns admit, so ordering these keys as strings is the same as ordering by (application id, database id)". `ListComponents` ignores that rule, so the two listings of the same Region disagree about which application comes first, and `ListComponents` contradicts its own module doc, which promises `<ApplicationId>/<ComponentId>` order.

Paging itself stays self-consistent (the handler sorts by the same key it tokenises), so nothing is skipped or repeated — the defect is that the reported order, and therefore how items are grouped into pages, is not the order the handler documents.

## Evidence

The key `ListComponents` sorts and pages by — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_components.rs:209-220:
```
        candidates.extend(applications.iter().flat_map(|application| {
            application.components.iter().map(move |component| {
                (
                    format!("{}/{}", application.id, component.id),
                    (application.id.as_str(), component),
                )
            })
        }));
        ...
        candidates.sort_by(|(left, _), (right, _)| left.cmp(right));
```
and the order that handler's own module doc promises — src/ops/list_components.rs:5-9: "Reports one `ComponentSummary` per component of the registered applications in the request's Region ... in `<ApplicationId>/<ComponentId>` order".

The rule the same crate applies to the same kind of key for databases — src/state.rs:1117-1129:
```
    /// The key a database listing is ordered and paged by: this database's application
    /// id and its own id, in that order.
    ///
    /// The two are joined by a byte lower than any the modelled `ApplicationId`
    /// (`^[\w\d\.-]+$`) and `DatabaseId` (`[\w\d]+$`) patterns admit, so ordering these
    /// keys as strings is the same as ordering by (application id, database id) ...
    pub fn page_key(&self) -> String {
        format!(
            "{}{DATABASE_PAGE_KEY_SEPARATOR}{}",
            self.application_id, self.database_id
        )
```
with `const DATABASE_PAGE_KEY_SEPARATOR: char = '\u{1}';` (src/state.rs:1133).

Test coverage does not pin the documented order: tests/all/component_listing.rs:462-476 builds its expectation with the implementation's own key (`format!("{}/{}", summary.application_id, summary.component_id)` then `all.sort()`), so it mirrors the defect instead of checking (ApplicationId, ComponentId) order, and the ids in `CASES` ("HDB-TEST", "A", "hana_99", "abap.prod_01" — src/../tests/all/component_listing.rs:270-299) are never prefix-related, so the divergence is never exercised. By contrast tests/all/list_databases.rs:443-445 deliberately uses "Two application ids that differ in a character lower than a letter, so the order of the listing is the order of (application id, database id) rather than of some other joining of the two".

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"app","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/hdb"}]} — expect 200; the registration derives component "HDB-HANA".
2. POST /register-application with {"ApplicationId":"app-2","ApplicationType":"HANA","Instances":["i-0123456789abcdef1"],"Sid":"QAS","SapInstanceNumber":"01","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/qas"}]} — expect 200; derives component "QAS-HANA".
3. POST /list-components with {} (no ApplicationId) — expected, per the handler's own doc, `<ApplicationId>/<ComponentId>` order: app/HDB-HANA then app-2/QAS-HANA. Actual: app-2/QAS-HANA first, because "app-2/QAS-HANA" < "app/HDB-HANA" as strings ('-' 0x2D < '/' 0x2F).
4. POST /list-components with {"MaxResults":1} — the first page holds app-2's component rather than app's, and the token resumes at app's.
5. POST /list-databases with {} over the same two applications — it reports app's database first, i.e. the two listings of one Region disagree about which application comes first.
