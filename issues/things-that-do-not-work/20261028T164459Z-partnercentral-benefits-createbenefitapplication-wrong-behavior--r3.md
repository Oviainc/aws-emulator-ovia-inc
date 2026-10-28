Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0b60124a277040298c4e92ee09b21d27ce53d94f

# FileDetail's FileName bound is counted in BYTES, so a multi-byte file name the modelled length admits is silently dropped (with its FileType)

_Filed: 2026-10-28T16:44:59Z_
_Operation: CreateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0b60124a277040298c4e92ee09b21d27ce53d94f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::file_detail` — the one construction `CreateBenefitApplication` and `UpdateBenefitApplication` both build a stored `FileDetail` with — derives `FileName` from the supplied `FileURI` through `state::file_name_of`, which enforces the modelled `FileDetail$FileName` `@length(min: 1, max: 255)` itself, by hand, with `name.len()`: a BYTE count.

This crate states, in its own words, that a length it enforces itself is counted in CHARACTERS and that such a bound may not be counted two ways: `state::length_clause` is documented as "The one length check every handler of this service makes for itself goes through, so a bound a handler enforces cannot be worded — or counted — two ways … the length is counted in CHARACTERS, as the walk counts a `@length` on a string", and its body is `value.chars().count()`. The crate's own persisted-string suite declares the same rule over the values a write persists ("the character width of the value (ASCII and multi-byte, since a `@length` counts characters)").

`file_name_of` is the one enforcement of a modelled `@length` in this crate that does not go through that helper, and it counts the other way. The consequence is observable: a `FileURI` whose last path segment is, say, 101 CJK characters (101 characters — well inside the modelled 1..255 — but ~303 bytes) makes `file_name_of` return `None`, so the stored `FileDetail` carries NO `FileName` even though the member admits that name, and because `file_type_of` starts by calling `file_name_of`, the derived `FileType` is dropped with it (an `…\u{4e2d}…\u{4e2d}.pdf` attachment reports neither name nor `application/pdf`). Nothing is refused either: the request is served 200 and the record simply loses the two derived members.

## Evidence

The hand-rolled bound, counted in bytes — src/state.rs:2374-2385:
    /// The last path segment of `uri`, which is the file's name, with any query or fragment
    /// removed. `None` when the URI names no segment, or one outside the modelled
    /// `FileDetail$FileName` length (1..255).
    pub fn file_name_of(uri: &str) -> Option<String> {
        let path = uri
            .split(['?', '#'])
            .next()
            .unwrap_or(uri)
            .trim_end_matches('/');
        let name = path.rsplit('/').next()?;
        (1..=255).contains(&name.len()).then(|| name.to_string())
    }

The rule this crate states for exactly this kind of bound — src/state.rs:2892-2903:
    /// Whether `value` satisfies the `@length(min: min, max: max)` of the member it is
    /// written to or stored in.
    ///
    /// The one length check every handler of this service makes for itself goes through, so a
    /// bound a handler enforces cannot be worded — or counted — two ways. … and the length is
    /// counted in CHARACTERS, as the walk counts a `@length` on a string.
    pub fn length_clause(value: &str, min: usize, max: usize) -> Option<String> {
        let length = value.chars().count();

The same rule declared over the wire by the crate's own suite — tests/all/persisted_string_bounds.rs:17-19:
    //! `CancelBenefitApplication`, whose `Reason` the model bounds), the position relative to
    //! each published bound (at it, one past it, and at the minimum) and the character width
    //! of the value (ASCII and multi-byte, since a `@length` counts characters).

The member whose bound is being enforced, from the model (`smithy_query(query="shape",shape="FileDetail")`):
    "FileName": {"target": "String", "length": {"min": 1, "max": 255}, "doc": "<p>The original name of the uploaded file.</p>"}

The second member lost with it — src/state.rs:2389-2391:
    pub fn file_type_of(uri: &str) -> Option<String> {
        let name = file_name_of(uri)?;
        let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();

The paths that store it — src/ops/create_benefit_application.rs:474-479:
                file_details: input
                    .file_details
                    .unwrap_or_default()
                    .iter()
                    .map(|file| crate::state::file_detail(file, &principal, now))
                    .collect(),
and src/ops/update_benefit_application.rs:401-407 (`application.file_details = files.iter().map(|file| crate::state::file_detail(file, &principal, now)).collect();`).

Test coverage: tests/all/benefit_application_lifecycle.rs:747-784 (`file_details_derive_the_file_name_and_type_from_the_uri`) is the only suite over this derivation and every one of its 13 rows is a short ASCII name ("plan.pdf", "REPORT.PDF", "no-extension", …), so neither the 255 boundary nor the counting unit is pinned, and no comment states a reason for counting bytes here.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_application_lifecycle.rs:90 does.
2. POST / with X-Amz-Target: PartnerCentralBenefitsService.CreateBenefitApplication and body {"Catalog":"AWS","ClientToken":"tok-1","BenefitIdentifier":"<an ACTIVE ben-… of the catalog>","FileDetails":[{"FileURI":"s3://b/<100 CJK characters>.pdf","BusinessUseCase":"evidence"}]} — the last path segment is 104 characters (inside the modelled FileDetail$FileName 1..255) but about 304 bytes.
3. GetBenefitApplication for the returned Id and read FileDetails[0]: FileName is absent and FileType is absent, although the URI round-trips and the modelled member admits a 104-character name.
4. Repeat step 2 with an ASCII name of the same character count (e.g. 100 'a' characters + ".pdf"): FileName is reported and FileType is "application/pdf" — the same character width answers two different ways depending on the bytes per character.
5. Contrast with how this crate counts the same kind of bound elsewhere: send RecallBenefitApplication with a 1000-character multi-byte Reason (tests/all/persisted_string_bounds.rs rows) and observe it is accepted, because state::length_clause counts characters.
