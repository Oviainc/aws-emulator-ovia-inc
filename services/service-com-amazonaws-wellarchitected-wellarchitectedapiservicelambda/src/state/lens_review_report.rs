// SPDX-License-Identifier: CC0-1.0

//! The REPORT of ONE lens review of a workload: the document `GetLensReviewReport` hands
//! back.
//!
//! The report is a `LensReviewReport`, whose `Base64String` is "the Base64-encoded string
//! representation of a lens review report" that "can be used to create a PDF file"
//! (`smithy_query(query="shape", shape="Base64String")`) — so what the member carries is a
//! PDF DOCUMENT, written by the `pdf-writer` crate so a caller that decodes it and saves it
//! opens a document rather than bytes shaped like one.
//!
//! Everything the document says is DERIVED from the reviewed workload's stored state on
//! every read — the workload's own members, the lens the catalog resolves for the version
//! the review is against, the review's notes, and the `Risk` roll-ups counted over the
//! lens's and each pillar's questions from the workload's stored answers. Nothing about a
//! review is stored twice, so a report read after a write reports that write, and a report
//! of the workload a milestone recorded reports the review that milestone froze.
//!
//! The projection lives beside the record types for the reason
//! [`WorkloadRecord::to_lens_review`] does: a response built from a stored record is built
//! in one place, so the review a caller READS and the review a report DESCRIBES cannot
//! disagree. The handler resolves the workload, the milestone and the lens and then reads
//! this.

use super::*;

use crate::ops::types::LensReviewReport;

/// The column the report's body text is wrapped to: the body is set in [`FONT_SIZE`]
/// Helvetica on the A4 page ([`PAGE_WIDTH`]), which fits about this many characters between
/// the margins.
const REPORT_WIDTH: usize = 92;

/// What the document prints for a member the stored review carries no value for — a lens
/// this emulator holds no content for has no name, version or currency here, and a
/// workload records no review owner until one is set.
const UNRECORDED: &str = "(not recorded)";

/// The `LensReviewReport` of ONE lens review of `workload`.
///
/// `lens` is the lens as the request named it — an alias or an ARN — which is the spelling
/// the report echoes, exactly as [`WorkloadRecord::to_lens_review`] echoes it; `reviewed`
/// is what the lens catalog says about the lens for the version this review is against
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_at`]),
/// and `content` the content of the lenses the workload reviews
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_content`]),
/// which the roll-ups in the document are counted over. `milestone_number` is the
/// milestone the read was scoped to, when it was scoped to one.
pub fn lens_review_report(
    workload: &WorkloadRecord,
    lens: &str,
    reviewed: &ReviewedLens,
    content: &ReviewedLensContent,
    milestone_number: Option<i32>,
) -> LensReviewReport {
    let document = report_document(workload, lens, reviewed, content, milestone_number);
    LensReviewReport {
        lens_alias: Some(lens.to_string()),
        lens_arn: Some(reviewed.lens_arn.clone()),
        base64_string: Some(::aws_smithy_types::base64::encode(&document)),
    }
}

/// The report document itself: the lines [`report_lines`] derives from the stored review,
/// set on as many pages as they take.
fn report_document(
    workload: &WorkloadRecord,
    lens: &str,
    reviewed: &ReviewedLens,
    content: &ReviewedLensContent,
    milestone_number: Option<i32>,
) -> Vec<u8> {
    pdf_document(&report_lines(
        workload,
        lens,
        reviewed,
        content,
        milestone_number,
    ))
}

/// What the report SAYS, one line per entry: the workload under review, the lens the
/// review is of, the review's own notes, and the risk roll-up of the review as a whole and
/// of each pillar the lens is organized into.
///
/// Derived in full from the arguments, so the same review always reads the same way and a
/// report taken before and after a write differs exactly where the review does.
fn report_lines(
    workload: &WorkloadRecord,
    lens: &str,
    reviewed: &ReviewedLens,
    content: &ReviewedLensContent,
    milestone_number: Option<i32>,
) -> Vec<String> {
    let written = workload.lens_review_record(lens);
    let mut lines = vec![
        "AWS Well-Architected Tool - lens review report".to_string(),
        String::new(),
        format!(
            "Workload: {} ({})",
            workload.workload_name, workload.workload_id
        ),
        format!("Workload ARN: {}", workload.workload_arn),
        format!("Environment: {}", workload.environment),
        format!("Owner account: {}", workload.owner),
        format!(
            "Review owner: {}",
            workload.review_owner.as_deref().unwrap_or(UNRECORDED)
        ),
    ];
    // Named only when the read was scoped to a milestone: a report of the live review is a
    // report of no milestone.
    if let Some(number) = milestone_number {
        lines.push(format!("Milestone number: {number}"));
    }
    lines.push(format!("Last updated: {}", at(workload.updated_at)));
    lines.push(String::new());
    lines.push(format!(
        "Lens: {} ({})",
        reviewed.lens_name.as_deref().unwrap_or(UNRECORDED),
        lens_alias(lens)
    ));
    lines.push(format!("Lens ARN: {}", reviewed.lens_arn));
    lines.push(format!(
        "Lens version: {}",
        reviewed.lens_version.as_deref().unwrap_or(UNRECORDED)
    ));
    lines.push(format!(
        "Lens status: {}",
        reviewed.lens_status.as_deref().unwrap_or(UNRECORDED)
    ));

    lines.push(String::new());
    lines.push("Risk counts for this lens review".to_string());
    lines.extend(counted(&workload.lens_risk_counts(lens, content)));

    if let Some(notes) = written.and_then(|written| written.notes.as_deref()) {
        lines.push(String::new());
        lines.push("Lens review notes".to_string());
        lines.extend(body(notes));
    }

    for (pillar_id, pillar_name) in workload.lens_review_pillars(lens, content) {
        lines.push(String::new());
        lines.push(format!(
            "Pillar: {} ({pillar_id})",
            pillar_name.as_deref().unwrap_or(UNRECORDED)
        ));
        lines.extend(counted(&workload.pillar_risk_counts(
            lens,
            &pillar_id,
            content,
        )));
        if let Some(notes) = written.and_then(|written| written.pillar_notes.get(&pillar_id)) {
            lines.push("  Notes".to_string());
            lines.extend(body(notes).into_iter().map(|line| format!("  {line}")));
        }
    }

    lines
}

/// One risk roll-up as the document shows it: one indented line per `Risk` bucket, in the
/// bucket order the roll-up is held in, so every bucket the enum publishes is in the
/// report whether anything counts under it or not.
fn counted(counts: &BTreeMap<String, i32>) -> Vec<String> {
    counts
        .iter()
        .map(|(risk, count)| format!("  {risk}: {count}"))
        .collect()
}

/// A stored note as the document shows it: the note's own line breaks kept, and each of
/// its lines wrapped to [`REPORT_WIDTH`] so a note as long as `Notes` admits stays inside
/// the page's margins.
fn body(text: &str) -> Vec<String> {
    text.split('\n')
        .flat_map(|line| {
            let wrapped = ::textwrap::wrap(line, REPORT_WIDTH);
            if wrapped.is_empty() {
                // `wrap` answers nothing for an empty line, which is still a line of the
                // note.
                vec![String::new()]
            } else {
                wrapped.into_iter().map(|line| line.into_owned()).collect()
            }
        })
        .collect()
}

/// An instant as the document prints it: RFC 3339 in UTC, the same rendering the wire
/// gives a modelled timestamp, so the report and the response agree about when the review
/// last changed.
fn at(instant: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| UNRECORDED.to_string())
}

/// Point size of the body text, and the baseline-to-baseline distance that goes with it.
const FONT_SIZE: f32 = 11.0;

/// See [`FONT_SIZE`].
const LEADING: f32 = 15.0;

/// The ISO 216 A4 page box in PDF points (1/72 inch).
const PAGE_WIDTH: f32 = 595.28;

/// See [`PAGE_WIDTH`].
const PAGE_HEIGHT: f32 = 841.89;

/// Where the first line of a page sits: the left margin, and the baseline of the top line.
const TEXT_LEFT: f32 = 56.0;

/// See [`TEXT_LEFT`].
const TEXT_TOP: f32 = 785.0;

/// How many lines fit on one page: the body runs from [`TEXT_TOP`] down to the bottom
/// margin, one [`LEADING`] apart.
const LINES_PER_PAGE: usize = 48;

/// The object id of the first page; the fixed objects (the catalog, the page tree and the
/// font) take the ids before it, and each page takes two — itself and its content stream.
const FIRST_PAGE_ID: i32 = 4;

/// A PDF document showing `lines`, one per line, in the Base-14 Helvetica face every PDF
/// reader carries (so nothing is embedded and the document stands alone), broken over as
/// many pages as the lines take.
fn pdf_document(lines: &[String]) -> Vec<u8> {
    use pdf_writer::{Finish, Name, Pdf, Rect, Ref};

    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let font_id = Ref::new(3);
    let font_name = Name(b"F1");
    let page_box = Rect::new(0.0, 0.0, PAGE_WIDTH, PAGE_HEIGHT);

    // A document has at least one page, so a report with nothing to say is still a
    // document a reader opens.
    let pages: Vec<&[String]> = if lines.is_empty() {
        vec![&[]]
    } else {
        lines.chunks(LINES_PER_PAGE).collect()
    };
    // Every page's own object id, so the page tree can name its kids before the pages
    // themselves are written.
    let page_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(FIRST_PAGE_ID + 2 * index as i32))
        .collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    for (page_id, shown) in page_ids.iter().zip(&pages) {
        let content_id = Ref::new(page_id.get() + 1);
        let mut page = pdf.page(*page_id);
        page.parent(page_tree_id);
        page.media_box(page_box);
        page.contents(content_id);
        page.resources().fonts().pair(font_name, font_id);
        page.finish();
        pdf.stream(content_id, &page_content(shown, font_name));
    }
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    pdf.finish()
}

/// One page's content stream: `lines` set from the top margin down, one [`LEADING`] apart.
fn page_content(lines: &[String], font_name: pdf_writer::Name<'_>) -> pdf_writer::Buf {
    use pdf_writer::{Content, Str};

    let mut content = Content::new();
    content.begin_text();
    content.set_font(font_name, FONT_SIZE);
    // The first move is from the page origin to the top-left text start; every later one
    // steps down one line.
    content.next_line(TEXT_LEFT, TEXT_TOP);
    for line in lines {
        content.show(Str(&shown(line)));
        content.next_line(0.0, -LEADING);
    }
    content.end_text();
    content.finish()
}

/// One line as the page can SHOW it.
///
/// The Base-14 face the document names is encoded one byte per glyph, so a character
/// outside printable US-ASCII has no glyph in it; such a character is shown as `?` rather
/// than written as a byte that would render as an unrelated glyph. Every value the report
/// derives from a stored review is readable this way, and a name carrying anything else
/// still produces a document a reader opens.
fn shown(line: &str) -> Vec<u8> {
    line.chars()
        .map(|character| match character {
            ' '..='~' => character as u8,
            _ => b'?',
        })
        .collect()
}
