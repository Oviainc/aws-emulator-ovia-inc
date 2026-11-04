// SPDX-License-Identifier: CC0-1.0

//! @op AmendBenefitApplication
//! Applies a list of field amendments to one stored benefit application in
//! [`crate::state`], bumps its revision and `UpdatedAt` where that changed the record,
//! and records the amendment.
//!
//! The operation answers in this order, which is the order AWS reports:
//!
//! 1. request-shape validation — the centralized walk, before this handler runs;
//! 2. the partner-entitlement gate — a caller holding no active AWS partner benefit is
//!    refused with `AccessDeniedException` (see [`crate::state::ENTITLEMENT_REQUIRED`]),
//!    so it never learns whether the application it named exists;
//! 3. the `ClientToken` replay check — a retry of an amendment already applied changes
//!    nothing and answers success, while the same token on a request naming different
//!    amendments, reason, revision or application is refused with `ConflictException` (see
//!    [`crate::state::ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::replayed_application_write`]);
//! 4. the application's status — AWS has finished with a terminal application
//!    ([`crate::state::TERMINAL_STATUSES`]), so amending one is `ConflictException`;
//! 5. optimistic concurrency — the request's `Revision` must be the application's
//!    current one, else `ConflictException`;
//! 6. the amendments themselves, applied to a copy so that a request carrying one that
//!    cannot be applied leaves the stored application exactly as it was.
//!
//! The revision is the record's optimistic-concurrency token, so it moves only when the
//! amendments actually CHANGED the stored record
//! ([`crate::state::BenefitApplication::revise`]): an amendment naming the value a field
//! already holds is accepted and answers with the record left as it was.
//!
//! ## What a `FieldPath` may name
//!
//! Every member a caller supplies and `UpdateBenefitApplication` also writes: `Name`,
//! `Description`, `BenefitApplicationDetails` followed by a `.`-separated path into that
//! document, and a caller-supplied member of one element of `PartnerContacts` or
//! `FileDetails`. `NewValue` is one string and a list is not, so the path below a list
//! member addresses ONE element — its 0-based position, as a JSON path indexes a list,
//! written either as a subscript on that list member's name (`PartnerContacts[0].Email`)
//! or as a segment of its own (`FileDetails.1.FileURI`) — followed by the member of that
//! element to write. Each path may also be written in JSON-path form (`$.Name`).
//!
//! Everything else on the record is service-owned — its identity, its status, the programs
//! of the benefit it requests, and the members of a stored attachment that follow the URI
//! or record who attached it and when — so a path naming one is refused with the modelled
//! `ValidationException`, as is a `NewValue` the member the path names is not modelled to
//! accept: `Amendment$NewValue` is an unconstrained `String`, so the target member's own
//! constraints are enforced here, by the same validating walk the centralized request
//! validation runs.
//!
//! The members the model leaves unconstrained carry this service's own bounds, so no
//! request and no sequence of requests can grow one record without limit: at least
//! [`crate::state::MIN_AMENDMENTS_PER_REQUEST`] and at most
//! [`crate::state::MAX_AMENDMENTS_PER_REQUEST`] amendments per request, a `FieldPath` of
//! at most [`crate::state::MAX_FIELD_PATH_LEN`] characters and
//! [`crate::state::MAX_DETAILS_DEPTH`] segments, a details value — and a value written to
//! a member the model leaves unconstrained ([`CONTACT_UNCONSTRAINED_MEMBERS`],
//! [`FILE_UNCONSTRAINED_MEMBERS`]) — of at most
//! [`crate::state::MAX_DETAILS_VALUE_LEN`], an `AmendmentReason` and a `ClientToken`
//! within the lengths the model gives them
//! ([`crate::state::MAX_AMENDMENT_REASON_LEN`], [`crate::state::MAX_CLIENT_TOKEN_LEN`] —
//! the two strings the record keeps), and an amended document still within
//! [`crate::state::details_requirement`]'s depth, member-count and byte bounds.
//!
//! The document the amendments produce is judged against the request schema the
//! application's benefit declares, exactly as a document supplied to the create or the
//! update is: an amended document that contradicts the schema is refused with the modelled
//! `ValidationException` and nothing is persisted — see
//! [`crate::state::benefit_schema_refusal`]. `Amendment$NewValue` is modelled as a String
//! and is written as one, so a path naming a field the schema declares with another JSON
//! type cannot be satisfied by any amendment: such a request is refused rather than
//! answered by writing text where the schema says number. The caller changes that field
//! through `UpdateBenefitApplication`, whose `BenefitApplicationDetails` is a `Document`
//! and so can carry the declared type.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `AmendBenefitApplication::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `AmendBenefitApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json")
//! ```
//!
//! Modifies an existing benefit application by applying amendments to specific fields while
//! maintaining revision control.
//!
//! ## Input
//!
//! * `AmendmentReason` (String) **required** — A descriptive reason explaining why the
//!   benefit application is being amended.
//! * `Amendments` (AmendmentList) **required** — A list of specific field amendments to apply
//!   to the benefit application.
//! * `Catalog` (CatalogName, pattern ^[A-Za-z0-9_-]+$) **required** — The catalog identifier
//!   that specifies which benefit catalog the application belongs to.
//! * `ClientToken` (String) **required** — A unique, case-sensitive identifier to ensure
//!   idempotent processing of the amendment request.
//! * `Identifier` (BenefitApplicationIdentifier, pattern ^(arn:.+|benappl-[0-9a-z]{14})$)
//!   **required** — The unique identifier of the benefit application to be amended.
//! * `Revision` (String) **required** — The current revision number of the benefit
//!   application to ensure optimistic concurrency control.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Thrown when the caller does not have sufficient
//!   permissions to perform the requested operation.
//! * `ConflictException` (HTTP 409) — Thrown when the request conflicts with the current
//!   state of the resource, such as attempting to modify a resource that has been changed by
//!   another process.
//! * `InternalServerException` (HTTP 500) — Thrown when an unexpected error occurs on the
//!   server side during request processing.
//! * `ResourceNotFoundException` (HTTP 404) — Thrown when the requested resource cannot be
//!   found or does not exist.
//! * `ThrottlingException` (HTTP 429) — Thrown when the request rate exceeds the allowed
//!   limits and the request is being throttled.
//! * `ValidationException` (HTTP 400) — Thrown when the request contains invalid parameters
//!   or fails input validation requirements.

#[allow(unused_imports)]
use super::types::*;

use std::collections::HashMap;

use ::aws_smithy_types::Document;

/// The benefit-application member `Name` amends, as the model spells it on the wire.
const FIELD_NAME: &str = "Name";
/// The benefit-application member `Description` amends.
const FIELD_DESCRIPTION: &str = "Description";
/// The benefit-application member a `BenefitApplicationDetails...` path amends.
const FIELD_DETAILS: &str = "BenefitApplicationDetails";
/// The benefit-application member a `PartnerContacts...` path amends.
const FIELD_PARTNER_CONTACTS: &str = "PartnerContacts";
/// The benefit-application member a `FileDetails...` path amends.
const FIELD_FILE_DETAILS: &str = "FileDetails";

/// The members of a benefit application an amendment may write.
///
/// They are the members a caller supplies and `UpdateBenefitApplication` also writes
/// (`Name`, `Description`, `BenefitApplicationDetails`, `PartnerContacts`, `FileDetails` —
/// that operation's own input); everything else on the record is service-owned (its
/// identity, its status, the programs of the benefit it requests) and no amendment reaches
/// it.
const AMENDABLE_FIELDS: &[&str] = &[
    FIELD_NAME,
    FIELD_DESCRIPTION,
    FIELD_DETAILS,
    FIELD_PARTNER_CONTACTS,
    FIELD_FILE_DETAILS,
];

/// The two amendable fields whose shape is a LIST of structures, so a path below one
/// addresses an element of it. They are the only fields a subscript is read on.
const LIST_FIELDS: &[&str] = &[FIELD_PARTNER_CONTACTS, FIELD_FILE_DETAILS];

/// The members of one `PartnerContacts` element an amendment may write, as the model
/// spells them: every member of the modelled `Contact`, which is a structure the caller
/// supplies whole (`smithy_query(query="shape",shape="Contact")`).
const MEMBER_EMAIL: &str = "Email";
const MEMBER_FIRST_NAME: &str = "FirstName";
const MEMBER_LAST_NAME: &str = "LastName";
const MEMBER_BUSINESS_TITLE: &str = "BusinessTitle";
const MEMBER_PHONE: &str = crate::state::CONTACT_PHONE_MEMBER;
const CONTACT_MEMBERS: &[&str] = &[
    MEMBER_EMAIL,
    MEMBER_FIRST_NAME,
    MEMBER_LAST_NAME,
    MEMBER_BUSINESS_TITLE,
    MEMBER_PHONE,
];

/// The members of one `FileDetails` element an amendment may write: the members of the
/// modelled `FileInput` a caller supplies for an attachment
/// (`smithy_query(query="shape",shape="FileInput")`). The rest of a stored `FileDetail` is
/// the service's own — `FileName` and `FileType` follow the URI, `CreatedBy` and
/// `CreatedAt` record who attached the file and when (see [`crate::state::file_detail`]).
const MEMBER_FILE_URI: &str = "FileURI";
const MEMBER_BUSINESS_USE_CASE: &str = crate::state::FILE_BUSINESS_USE_CASE_MEMBER;
const FILE_MEMBERS: &[&str] = &[MEMBER_FILE_URI, MEMBER_BUSINESS_USE_CASE];

/// The member of a `Contact` the model constrains not at all: `Phone` targets
/// `ContactPhone`, which carries no `@length`
/// (`smithy_query(query="shape",shape="Contact")`). A value written to it therefore
/// carries this service's own bound on a persisted caller value, exactly as a leaf of the
/// details document does ([`crate::state::MAX_DETAILS_VALUE_LEN`]), so no amendment can
/// grow one record without limit — the SAME bound the create and the update apply to the
/// member ([`crate::state::unconstrained_members_refusal`]).
const CONTACT_UNCONSTRAINED_MEMBERS: &[&str] = &[MEMBER_PHONE];

/// The member of a `FileInput` the model constrains not at all, for the same reason:
/// `BusinessUseCase` targets a bare `String`
/// (`smithy_query(query="shape",shape="FileInput")`).
const FILE_UNCONSTRAINED_MEMBERS: &[&str] = &[MEMBER_BUSINESS_USE_CASE];

/// The longest value `Name` accepts: the modelled `BenefitApplicationName`
/// `@length(min: 1, max: 255)`. `Amendment$NewValue` is an unconstrained `String`, so
/// the target member's own constraint is enforced here.
const NAME_MAX_LEN: usize = 255;

/// The longest value `Description` accepts: the modelled
/// `BenefitApplicationDescription` `@length(min: 1, max: 1000)`.
const DESCRIPTION_MAX_LEN: usize = 1000;

/// The shortest value either string member accepts, from those same `@length` traits.
const MIN_LEN: usize = 1;

/// Why one amendment cannot be applied: the input member to report, and the constraint
/// clause it failed.
struct AmendmentRefusal {
    member: String,
    clause: String,
}

impl AmendmentRefusal {
    fn new(member: String, clause: String) -> Self {
        Self { member, clause }
    }
}

/// The input member one amendment arrives on, as the wire spells it.
const AMENDMENTS_MEMBER: &str = "Amendments";

/// The path of one member of `Amendments[position]`, spelled the way this service's
/// validation messages spell a member path — through the one construction that spells a
/// nested member of a list element ([`crate::state::element_member_path`]), e.g.
/// `amendments.1.member.fieldPath`.
fn member_path(position: usize, member: &str) -> String {
    crate::state::element_member_path(AMENDMENTS_MEMBER, position, member)
}

/// The segments of one `FieldPath`: a field name, optionally written in JSON-path form
/// (`$.Name`), with `.`-separated member names below it. A list element's position is
/// written either as a subscript on the field name (`PartnerContacts[0]`) or as a segment
/// of its own (`PartnerContacts.0`) — a JSON path indexes a list in brackets, so both
/// spellings address the same element and the walk below reads one grammar.
///
/// A subscript is read on the FIELD NAME of a LIST member ([`LIST_FIELDS`]) and nowhere
/// else, because that is where a list is addressed: on any other field name, and on every
/// segment below a field name, the brackets are part of the name — a key of the details
/// document is a name the CALLER chose and is addressed verbatim, brackets included.
///
/// `None` for a path this service cannot read: one with an empty segment, one whose
/// subscript is unclosed, or one with more than [`crate::state::MAX_DETAILS_DEPTH`]
/// segments. `FieldPath` is an unconstrained `String`, so that bound is what keeps the
/// walk below from recursing without limit; its LENGTH is bounded separately by
/// [`field_path_length_clause`], which states the length requirement it failed instead of
/// folding a legal-but-long path in with a path that names nothing.
fn path_segments(field_path: &str) -> Option<Vec<&str>> {
    let mut steps = field_path
        .strip_prefix("$.")
        .unwrap_or(field_path)
        .split('.');
    let head = steps
        .next()
        .expect("`split('.')` yields at least one segment");
    let mut segments: Vec<&str> = Vec::new();
    match head.split_once('[') {
        Some((field, subscript)) if LIST_FIELDS.contains(&field) => {
            segments.push(field);
            segments.push(subscript.strip_suffix(']')?);
        }
        _ => segments.push(head),
    }
    segments.extend(steps);
    (segments.iter().all(|segment| !segment.is_empty())
        && segments.len() <= crate::state::MAX_DETAILS_DEPTH)
    .then_some(segments)
}

/// The element of a list member a `FieldPath` addresses and the member of that element it
/// writes, from the path below the list member.
///
/// `rest` is that path: the element's position, 0-based as a JSON path indexes a list, then
/// the name of the member to write. `None` for a path of any other shape — `NewValue` is
/// one string, so a path that does not address one element and one of its members names
/// nothing an amendment can write. The position is a decimal written canonically, so one
/// element has ONE spelling: a sign, a leading zero or any other text names no element.
fn list_element_path<'p>(rest: &[&'p str]) -> Option<(usize, &'p str)> {
    let [position, member] = rest else {
        return None;
    };
    let canonical = position.bytes().all(|byte| byte.is_ascii_digit())
        && (position.len() == 1 || !position.starts_with('0'));
    canonical
        .then(|| position.parse().ok())
        .flatten()
        .map(|index| (index, *member))
}

/// The requirement `value` fails as the member `member` of the modelled structure
/// `schema`, or `None` when the shape accepts it.
///
/// `Amendment$NewValue` is an unconstrained `String`, so the constraints of the member it
/// is written to are enforced here — by the SAME validating walk the centralized request
/// validation runs (`aws_smithy_schema_server::constraints::validate` over the shape's own
/// schema), so a value an amendment writes into a member must satisfy exactly what a value
/// the create or the update supplied for it must. Only the member the amendment wrote is
/// reported on, and the requirement is worded by [`crate::service::constraint_requirement`]
/// — the words every refusal of this service uses — and echoes nothing of the value.
fn member_requirement(
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    schema: &::aws_smithy_schema::Schema,
    member: &str,
) -> Option<String> {
    let path = ::aws_smithy_schema_server::constraints::path::member(member);
    ::aws_smithy_schema_server::constraints::validate(value, schema)
        .into_iter()
        .filter(|violation| violation.path == path)
        .find_map(|violation| crate::service::constraint_requirement(&violation.kind))
}

/// The slot of `contact` the element member `member` writes, or `None` when `member` names
/// none of [`CONTACT_MEMBERS`].
fn contact_slot<'c>(contact: &'c mut Contact, member: &str) -> Option<&'c mut Option<String>> {
    match member {
        MEMBER_EMAIL => Some(&mut contact.email),
        MEMBER_FIRST_NAME => Some(&mut contact.first_name),
        MEMBER_LAST_NAME => Some(&mut contact.last_name),
        MEMBER_BUSINESS_TITLE => Some(&mut contact.business_title),
        MEMBER_PHONE => Some(&mut contact.phone),
        _ => None,
    }
}

/// The caller-supplied content of one stored attachment — the `FileInput` an amendment
/// writes into, and which the rest of the stored record follows (see
/// [`crate::state::file_detail`]).
fn file_input(file: &FileDetail) -> FileInput {
    FileInput {
        file_uri: file.file_uri.clone(),
        business_use_case: file.business_use_case.clone(),
    }
}

/// The requirement a `FieldPath` longer than [`crate::state::MAX_FIELD_PATH_LEN`] fails,
/// or `None` for one within it.
///
/// `FieldPath` is an unconstrained `String`, so this bound is the handler's own and goes
/// through the one length check of this service ([`crate::state::length_clause`]): it is
/// counted in CHARACTERS, like every other length this service enforces and like the
/// centralized walk counts a `@length` on a string, so a path's room does not depend on
/// the script its caller-chosen keys are written in, and it is reported as the length
/// requirement it is.
fn field_path_length_clause(field_path: &str) -> Option<String> {
    crate::state::length_clause(field_path, 0, crate::state::MAX_FIELD_PATH_LEN)
}

/// The requirement a `FieldPath` below a list member states: it addresses one element of
/// the list and names the member of that element to write.
fn element_path_requirement(field: &str) -> String {
    format!(
        "Member must name a member of one element of {field}, as that element's 0-based \
         position followed by the member name"
    )
}

/// The requirement a `FieldPath` fails when it addresses an element the list does not
/// hold. It states how many elements the RECORD holds — the service's own data — and
/// echoes nothing of the path the request carried.
fn element_count_requirement(field: &str, count: usize) -> String {
    format!("Member must name a 0-based element position within the {count} elements {field} holds")
}

/// The requirement a `FieldPath` fails when it names a member of a list element that is
/// not one an amendment may write.
fn element_member_requirement(field: &str, members: &[&str]) -> String {
    format!(
        "Member must name one of the amendable members of a {field} element ({})",
        members.join(", ")
    )
}

/// The requirement a value fails against this service's own bound on a member the model
/// leaves unconstrained, or `None` for a member the model bounds itself — there the
/// modelled constraint is the only one, so the two cannot disagree. `unconstrained` is the
/// set for the shape the member belongs to
/// ([`CONTACT_UNCONSTRAINED_MEMBERS`], [`FILE_UNCONSTRAINED_MEMBERS`]).
fn unconstrained_member_clause(
    unconstrained: &[&str],
    member: &str,
    value: &str,
) -> Option<String> {
    if !unconstrained.contains(&member) {
        return None;
    }
    crate::state::unconstrained_member_clause(value)
}

/// Write `value` at `path` inside `document`, creating the objects the path needs.
///
/// An empty path replaces `document` itself, so a `FieldPath` that names the details
/// member alone sets the whole document to the new value. `Err(depth)` reports the
/// 0-based path segment whose parent is not an object, so nothing below it can be
/// written.
fn set_document_member(document: &mut Document, path: &[&str], value: &str) -> Result<(), usize> {
    let Some((head, rest)) = path.split_first() else {
        *document = Document::String(value.to_string());
        return Ok(());
    };
    let Document::Object(members) = document else {
        return Err(0);
    };
    let child = members
        .entry((*head).to_string())
        .or_insert_with(|| Document::Object(HashMap::new()));
    set_document_member(child, rest, value).map_err(|depth| depth + 1)
}

/// One refusal of an input member, as this operation's modelled `ValidationException`.
///
/// Built by the one construction every member refusal of this crate goes through
/// ([`crate::state::member_validation_exception`], which renders the message with
/// [`crate::service::member_clause`] and [`crate::service::validation_envelope`] — the
/// same two helpers the centralized walk's refusals go through): it names the member path
/// and the requirement it failed, and echoes NOTHING of the rejected value.
fn refused(member: &str, clause: &str) -> AmendBenefitApplicationError {
    AmendBenefitApplicationErrorModeled::ValidationException(
        crate::state::member_validation_exception(
            member,
            clause,
            crate::state::VALIDATION_REASON_FIELD,
        ),
    )
    .into()
}

/// The number of amendments one request may carry, as the clause states it — a length
/// bound on the list, worded like any other.
///
/// BOTH of its ends go through the one length wording of this crate
/// ([`crate::state::count_length_clause`], which words a length with
/// [`crate::service::constraint_requirement`]), so the two bounds of this one member are
/// stated in one vocabulary and a caller parses them the same way.
fn amendments_length_clause(count: usize) -> Option<String> {
    crate::state::count_length_clause(
        count,
        crate::state::MIN_AMENDMENTS_PER_REQUEST,
        crate::state::MAX_AMENDMENTS_PER_REQUEST,
    )
}

/// Apply one amendment to `application`, or report why it cannot be applied.
///
/// The caller applies amendments to a COPY of the stored application, so a refusal here
/// leaves the store untouched however many amendments were applied before it.
fn apply_amendment(
    application: &mut crate::state::BenefitApplication,
    position: usize,
    amendment: &Amendment,
) -> Result<(), AmendmentRefusal> {
    let unamendable = || {
        AmendmentRefusal::new(
            member_path(position, "fieldPath"),
            format!(
                "Member must name one of the amendable fields ({}), a `.`-separated path \
                 into {FIELD_DETAILS}, or a member of one element of \
                 {FIELD_PARTNER_CONTACTS} or {FIELD_FILE_DETAILS}",
                AMENDABLE_FIELDS.join(", ")
            ),
        )
    };
    let refused_path = |requirement: String| {
        AmendmentRefusal::new(member_path(position, "fieldPath"), requirement)
    };
    let refused_value = |requirement: String| {
        AmendmentRefusal::new(member_path(position, "newValue"), requirement)
    };

    if let Some(clause) = field_path_length_clause(&amendment.field_path) {
        return Err(refused_path(clause));
    }
    let segments = path_segments(&amendment.field_path).ok_or_else(unamendable)?;
    let (head, rest) = segments
        .split_first()
        .expect("`split('.')` yields at least one segment");

    match *head {
        FIELD_NAME if rest.is_empty() => {
            if let Some(clause) =
                crate::state::length_clause(&amendment.new_value, MIN_LEN, NAME_MAX_LEN)
            {
                return Err(refused_value(clause));
            }
            application.name = Some(amendment.new_value.clone());
        }
        FIELD_DESCRIPTION if rest.is_empty() => {
            if let Some(clause) = crate::state::length_clause(
                &amendment.new_value,
                MIN_LEN,
                DESCRIPTION_MAX_LEN,
            ) {
                return Err(refused_value(clause));
            }
            application.description = Some(amendment.new_value.clone());
        }
        FIELD_DETAILS => {
            // The details member is modelled as a `Document`, so a path may reach into
            // it; a member the document does not hold yet is created, which is what
            // lets an amendment add a field as well as change one. A path naming the
            // member ALONE (no segment below it) sets the whole document to the new
            // value, which is deliberate: the member is a `Document`, and a string is
            // one. The model constrains a leaf of the document not at all, so the one
            // bound on a value written here is this service's own.
            // A leaf of the document carries no modelled length at all (and an empty
            // string is a valid member of a document), so the only bound on a value
            // written here is this service's own maximum.
            if let Some(clause) = crate::state::length_clause(
                &amendment.new_value,
                0,
                crate::state::MAX_DETAILS_VALUE_LEN,
            ) {
                return Err(refused_value(clause));
            }
            let document = application
                .benefit_application_details
                .get_or_insert_with(|| Document::Object(HashMap::new()));
            set_document_member(document, rest, &amendment.new_value).map_err(|depth| {
                refused_path(format!(
                    "Member must name a member of an object: segment {} of the path names a \
                     value that is not an object",
                    depth + 1
                ))
            })?;
        }
        FIELD_PARTNER_CONTACTS => {
            // A contact is a structure the caller supplies whole, so an amendment names
            // the element and the member of it to write; the element has to be one the
            // record holds, since the MEMBERSHIP of the list is what the update rewrites.
            let (index, member) = list_element_path(rest)
                .ok_or_else(|| refused_path(element_path_requirement(FIELD_PARTNER_CONTACTS)))?;
            let count = application.partner_contacts.len();
            let contact = application
                .partner_contacts
                .get_mut(index)
                .ok_or_else(|| {
                    refused_path(element_count_requirement(FIELD_PARTNER_CONTACTS, count))
                })?;
            let mut amended = contact.clone();
            let slot = contact_slot(&mut amended, member).ok_or_else(|| {
                refused_path(element_member_requirement(
                    FIELD_PARTNER_CONTACTS,
                    CONTACT_MEMBERS,
                ))
            })?;
            *slot = Some(amendment.new_value.clone());
            if let Some(requirement) = member_requirement(&amended, &Contact::SCHEMA, member) {
                return Err(refused_value(requirement));
            }
            if let Some(clause) = unconstrained_member_clause(
                CONTACT_UNCONSTRAINED_MEMBERS,
                member,
                &amendment.new_value,
            ) {
                return Err(refused_value(clause));
            }
            *contact = amended;
        }
        FIELD_FILE_DETAILS => {
            // An attachment's caller-supplied content is its `FileURI` and its
            // `BusinessUseCase`; the rest of the stored record follows them.
            let (index, member) = list_element_path(rest)
                .ok_or_else(|| refused_path(element_path_requirement(FIELD_FILE_DETAILS)))?;
            let count = application.file_details.len();
            let file = application
                .file_details
                .get_mut(index)
                .ok_or_else(|| refused_path(element_count_requirement(FIELD_FILE_DETAILS, count)))?;
            let mut supplied = file_input(file);
            match member {
                MEMBER_FILE_URI => supplied.file_uri = amendment.new_value.clone(),
                MEMBER_BUSINESS_USE_CASE => {
                    supplied.business_use_case = Some(amendment.new_value.clone());
                }
                _ => {
                    return Err(refused_path(element_member_requirement(
                        FIELD_FILE_DETAILS,
                        FILE_MEMBERS,
                    )))
                }
            }
            if let Some(requirement) = member_requirement(&supplied, &FileInput::SCHEMA, member) {
                return Err(refused_value(requirement));
            }
            if let Some(clause) = unconstrained_member_clause(
                FILE_UNCONSTRAINED_MEMBERS,
                member,
                &amendment.new_value,
            ) {
                return Err(refused_value(clause));
            }
            // `FileName` and `FileType` follow the URI, exactly as they do for an
            // attachment the create or the update stored
            // ([`crate::state::file_detail`]), so a write that moved the URI derives them
            // again — and a write to anything else leaves every member the URI governs
            // exactly as it was. `CreatedBy` and `CreatedAt` record WHO attached the file
            // and WHEN rather than what was attached, so no amendment touches them.
            if supplied.file_uri != file.file_uri {
                file.file_name = crate::state::file_name_of(&supplied.file_uri);
                file.file_type = crate::state::file_type_of(&supplied.file_uri);
                file.file_uri = supplied.file_uri;
            }
            file.business_use_case = supplied.business_use_case;
        }
        _ => return Err(unamendable()),
    }
    Ok(())
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AmendBenefitApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AmendBenefitApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.partnercentralbenefits#AmendBenefitApplication")]
#[derive(Debug)]
pub enum AmendBenefitApplicationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::amend_benefit_application::AmendBenefitApplicationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AmendBenefitApplicationError = crate::scaffolded::Scaffolded<AmendBenefitApplicationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AmendBenefitApplicationErrorModeled> for AmendBenefitApplicationError {
    fn from(modeled: AmendBenefitApplicationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AmendBenefitApplication` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AmendBenefitApplication<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AmendBenefitApplication<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AmendBenefitApplication<Eng>
{
    type Input = AmendBenefitApplicationInput;
    type Output = AmendBenefitApplicationOutput;
    type Error = AmendBenefitApplicationError;

    const NAME: &'static str = "AmendBenefitApplication";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::AMEND_BENEFIT_APPLICATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::AMEND_BENEFIT_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // The catalog this write would land in is resolved FIRST, and the
        // partner-entitlement gate applies to a request whose catalog resolved — see
        // [`crate::state::write_request_refusal`]. It takes the store lock itself, so it
        // runs before this handler takes one.
        match crate::state::write_request_refusal(&*self.engine, &input.catalog) {
            Some(crate::state::WriteRequestRefusal::MissingCatalog(missing)) => {
                return Err(
                    AmendBenefitApplicationErrorModeled::ResourceNotFoundException(missing).into(),
                )
            }
            Some(crate::state::WriteRequestRefusal::NotEntitled(denied)) => {
                return Err(
                    AmendBenefitApplicationErrorModeled::AccessDeniedException(denied).into(),
                )
            }
            None => {}
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(AmendBenefitApplicationErrorModeled::InternalServerException(
                InternalServerException {
                    message: crate::state::STORE_UNAVAILABLE.to_string(),
                },
            )
            .into());
        };

        // An amendment request has to name what it amends, and no more fields than this
        // service applies in one request: the model requires the list but constrains no
        // length, so both bounds live here — ahead of the store lock, so the loop that
        // applies them runs under the lock with a cited bound.
        if let Some(clause) = amendments_length_clause(input.amendments.len()) {
            return Err(refused(AMENDMENTS_MEMBER, &clause));
        }

        // The two caller-chosen strings this operation PERSISTS, checked at the door of
        // the store whose byte accounting cites these bounds. The model constrains both
        // (`AmendmentReason` `@length(max: 1000)`, `ClientToken` `@length(min: 1, max:
        // 64)`), so the centralized walk refuses a longer one first; this is the
        // enforcement the store itself relies on, not a restatement of the walk's job.
        if let Some(clause) = crate::state::length_clause(
            &input.amendment_reason,
            0,
            crate::state::MAX_AMENDMENT_REASON_LEN,
        ) {
            return Err(refused("amendmentReason", &clause));
        }
        if let Some(clause) = crate::state::length_clause(
            &input.client_token,
            crate::state::MIN_CLIENT_TOKEN_LEN,
            crate::state::MAX_CLIENT_TOKEN_LEN,
        ) {
            return Err(refused("clientToken", &clause));
        }

        let now = crate::state::now_millis();

        let mut store = state.lock();

        // The catalog the request names is resolved FIRST: an application lives inside a
        // catalog, so a catalog that does not exist is the missing resource and is what
        // the answer names — see [`crate::state::catalog_refusal`].
        if let Some(missing) = store.catalog_refusal(&input.catalog) {
            return Err(
                AmendBenefitApplicationErrorModeled::ResourceNotFoundException(missing).into(),
            );
        }

        // The application's own benefit governs the document the amendments produce, the
        // way it governs the one the create stores. The schema is read — and cloned —
        // BEFORE the application is borrowed for writing.
        let benefit_schema =
            store.benefit_schema_for_application(&input.catalog, &input.identifier);

        // The identity of this REQUEST, which is what the `ClientToken` below is judged
        // against: every member the request carries other than the token itself. The
        // `Identifier` member is covered by the APPLICATION it names, not by the spelling
        // it was written with.
        let fingerprint = crate::state::request_fingerprint(vec![
            (
                "Catalog",
                Some(crate::state::fingerprint_value(&input.catalog)),
            ),
            (
                "Identifier",
                Some(crate::state::fingerprint_value(
                    &store.application_identity(&input.catalog, &input.identifier),
                )),
            ),
            (
                "Revision",
                Some(crate::state::fingerprint_value(&input.revision)),
            ),
            (
                "AmendmentReason",
                Some(crate::state::fingerprint_value(&input.amendment_reason)),
            ),
            (
                "Amendments",
                Some(crate::state::fingerprint_value(&input.amendments)),
            ),
        ]);

        // A retry of an amendment that already succeeded applies nothing a second time,
        // while a request carrying that token with a different reason, revision,
        // amendment list or application is a DIFFERENT request and is refused rather than
        // answered success — the token is looked up wherever in the store it was spent. It
        // is checked BEFORE the revision, because the first call moved the revision the
        // retry still carries.
        match store.replayed_application_write(Self::NAME, &input.client_token, &fingerprint) {
            Err(conflict) => {
                return Err(AmendBenefitApplicationErrorModeled::ConflictException(conflict).into())
            }
            Ok(Some(_)) => return Ok(AmendBenefitApplicationOutput {}),
            Ok(None) => {}
        }

        // The application and the refusal a missing one is answered with are ONE
        // resolution — see [`crate::state::require_application_mut`].
        let application = store
            .require_application_mut(&input.catalog, &input.identifier)
            .map_err(AmendBenefitApplicationErrorModeled::ResourceNotFoundException)?;

        // AWS has finished with a terminal application (`APPROVED`, `REJECTED`,
        // `CANCELED`), so there is nothing left to amend: the change is refused and
        // nothing is persisted — see [`crate::state::TERMINAL_STATUSES`]. An amendment of
        // a submitted application is exactly what this operation is for, so the
        // non-terminal statuses are all accepted. Checked after the retry above, which
        // changes nothing at all, and before the revision, which the status refusal does
        // not depend on.
        if application.is_terminal() {
            return Err(AmendBenefitApplicationErrorModeled::ConflictException(
                ConflictException {
                    message: crate::state::status_conflict_message(
                        &application.id,
                        &application.status,
                        "amended",
                    ),
                },
            )
            .into());
        }

        if application.revision_text() != input.revision {
            return Err(AmendBenefitApplicationErrorModeled::ConflictException(
                ConflictException {
                    message: crate::state::revision_conflict_message(
                        &application.id,
                        &application.revision_text(),
                        &input.revision,
                    ),
                },
            )
            .into());
        }

        // Every amendment is applied to a copy, in request order, so two amendments of
        // one path leave the last one's value and a request carrying one that cannot be
        // applied changes nothing at all.
        let mut amended = application.clone();
        for (position, amendment) in input.amendments.iter().enumerate() {
            if let Err(refusal) = apply_amendment(&mut amended, position, amendment) {
                return Err(refused(&refusal.member, &refusal.clause));
            }
        }

        // An amendment can nest the details document deeper, so the bound the create and
        // the update enforce on a supplied document is enforced on the amended one too,
        // and refused exactly as they refuse it.
        if let Some(refusal) = crate::state::details_refusal(
            crate::state::DETAILS_MEMBER,
            amended.benefit_application_details.as_ref(),
        ) {
            return Err(AmendBenefitApplicationErrorModeled::ValidationException(refusal).into());
        }

        // The amended document has to satisfy the benefit's request schema just as a
        // supplied one does: an amendment can write a field the schema declares, so the
        // rule the create applies to the document it stores is applied to this one.
        if let Some(refusal) = crate::state::benefit_schema_refusal(
            benefit_schema.as_ref(),
            amended.benefit_application_details.as_ref(),
        ) {
            return Err(AmendBenefitApplicationErrorModeled::ValidationException(refusal).into());
        }

        // The revision and `UpdatedAt` move only when the record actually changed, so an
        // amendment naming the value a field already holds reports the revision the
        // application already had — the same rule the writes beside this one apply (see
        // [`crate::state::BenefitApplication::revise`]). The amendable fields
        // ([`AMENDABLE_FIELDS`]) are the only ones `apply_amendment` writes, and each is
        // compared the way the write beside this one compares it: a contact through the
        // canonical document of the modelled shape
        // ([`crate::state::same_value`]), a stored attachment through its
        // caller-supplied content ([`crate::state::same_file_details`]), which is all an
        // amendment can change because the rest of the record follows it.
        if amended.name != application.name
            || amended.description != application.description
            || amended.benefit_application_details != application.benefit_application_details
            || !crate::state::same_value(&application.partner_contacts, &amended.partner_contacts)
            || !crate::state::same_file_details(
                &application.file_details,
                &amended.file_details.iter().map(file_input).collect::<Vec<_>>(),
            )
        {
            amended.revise(now);
        }
        amended.record_write(crate::state::AppliedWrite {
            operation: Self::NAME,
            client_token: input.client_token.clone(),
            request_fingerprint: fingerprint,
            reason: Some(input.amendment_reason.clone()),
            fields: input
                .amendments
                .iter()
                .map(|amendment| amendment.field_path.clone())
                .collect(),
            applied_at_millis: amended.updated_at_millis,
            revision: amended.revision,
        });
        *application = amended;

        // The model binds no member on this operation's output: what the amendment did
        // is read back through `GetBenefitApplication`, whose `Revision` is the one this
        // call produced.
        Ok(AmendBenefitApplicationOutput {})
    }
}
