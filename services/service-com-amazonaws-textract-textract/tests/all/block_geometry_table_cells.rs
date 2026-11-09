// SPDX-License-Identifier: CC0-1.0

//! The TABLE CELLS a page's own gaps produce — and the words they do not swallow — end to
//! end over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: Textract.<Operation>`).
//!
//! THE RULE UNDER TEST: a document is read off its pixels, and nothing on a page says
//! where one word ends and the next begins except the GAP between their ink. So the gaps
//! of one line are clustered against each other: the lowest cluster boundary is the line's
//! word break, and a further boundary above it — a gap distinctly wider than the line's
//! own word gaps — is a COLUMN break, which the block layer reads as a table row (`TABLE`
//! and one `CELL` per column). A line whose gaps form one cluster above the ink is a line
//! of words, with no table in it.
//!
//! The tables span what the rule generalises over: the SIZE the page is drawn at (which
//! scales every gap in it), the stroke weight, how many words stand in a cell, how many
//! columns the row has, and — as the negative control — the same words drawn with word
//! gaps only.
//!
//! The last test drives the block layer DIRECTLY, over content a page cannot draw: a row
//! whose middle cell holds no words at all. A page shows a column break as a gap in its
//! ink and two gaps running together are one gap, so no raster reaches that layout — but
//! the layer still has to lay such a row out, and this is where that is pinned.

use image::ImageFormat;
use serde_json::json;
use service_com_amazonaws_textract_textract::ops::types::{Block, BoundingBox};
use service_com_amazonaws_textract_textract::state;

use crate::document_extraction::{blocks_of, call, harness, of_type};
use crate::raster_documents::{raster_document, render, texts_of, Paper};

/// The lines every row below is drawn from: mixed case, digits, punctuation and words of
/// several lengths, so a line's gaps really do fall into clusters.
fn sample_lines() -> Vec<&'static str> {
    vec![
        "SIMPARITY LEDGER",
        "Account Number: 48213",
        "Total Due 1250.00",
    ]
}

/// The words of `lines`, in reading order: what the recogniser must come back with.
fn drawn_words(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| line.split_whitespace())
        .map(ToString::to_string)
        .collect()
}

#[tokio::test]
async fn words_are_split_on_the_lines_own_word_gaps_however_it_is_drawn() {
    let router = harness();

    // Every gap on a page scales with the size the text is drawn at, and a heavier stroke
    // narrows the gaps inside a word while leaving the gaps between words — so if the
    // lowest cluster boundary were not the word boundary, these rows would disagree with
    // each other about where the words are.
    let papers: Vec<(&str, Paper)> = vec![
        ("at the font's own size", Paper { scale: 1, ..Paper::plain() }),
        ("twice the font's size", Paper { scale: 2, ..Paper::plain() }),
        ("four times the font's size", Paper { scale: 4, ..Paper::plain() }),
        ("six times the font's size", Paper { scale: 6, ..Paper::plain() }),
        (
            "far from the page's corner",
            Paper { scale: 5, margin: 140, ..Paper::plain() },
        ),
    ];

    for (name, paper) in papers {
        let page = render(&sample_lines(), paper);
        let (status, body) = call(
            &router,
            "DetectDocumentText",
            &json!({ "Document": raster_document(&page, ImageFormat::Png) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a drawn page must be read, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        assert_eq!(
            texts_of(&blocks, "WORD"),
            drawn_words(&sample_lines()),
            "case {name:?}: the words read must be the words drawn"
        );
        assert!(
            of_type(&blocks, "CELL").is_empty() && of_type(&blocks, "TABLE").is_empty(),
            "case {name:?}: a page of ordinary lines holds no table, got {blocks:?}"
        );
    }
}

#[tokio::test]
async fn a_gap_wider_than_the_lines_word_gaps_is_a_column_break() {
    let router = harness();

    // (case, the lines as they are DRAWN, the columns each table row holds)
    //
    // The wide gap is what makes a row a row, so the rows differ along how many columns
    // there are, how many words stand in a cell, and the size the page is drawn at. The
    // last row is the negative control: the same words with word gaps only.
    let cases: Vec<(&str, Vec<&'static str>, usize, usize)> = vec![
        (
            "two columns",
            vec![
                "Item Code      Amount Due",
                "Widget Mk2      1250.00",
            ],
            2,
            3,
        ),
        (
            "three columns, one of them several words",
            vec![
                "Item Code      Qty      Unit Price",
                "Widget Mk2      12      1250.00 USD",
            ],
            3,
            4,
        ),
        (
            "the same row drawn larger",
            vec![
                "Item Code      Amount Due",
                "Widget Mk2      1250.00",
            ],
            2,
            5,
        ),
        (
            "word gaps only, which are not columns",
            vec!["Item Code Amount Due", "Widget Mk2 1250.00"],
            0,
            3,
        ),
    ];

    for (name, lines, columns, scale) in cases {
        let page = render(&lines, Paper { scale, ..Paper::plain() });
        let (status, body) = call(
            &router,
            "AnalyzeDocument",
            &json!({
                "Document": raster_document(&page, ImageFormat::Png),
                "FeatureTypes": ["TABLES"],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a drawn page must be read, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        let tables = of_type(&blocks, "TABLE");
        if columns == 0 {
            assert!(
                tables.is_empty() && of_type(&blocks, "CELL").is_empty(),
                "case {name:?}: a line of words carries no table, got {blocks:?}"
            );
            // The positive half of the control: the words themselves still came back.
            assert_eq!(
                texts_of(&blocks, "WORD"),
                drawn_words(&lines),
                "case {name:?}: the words read must be the words drawn"
            );
            continue;
        }
        assert_eq!(
            tables.len(),
            1,
            "case {name:?}: the page's one table must be reported once, got {blocks:?}"
        );
        // One CELL per column of every row of the table.
        assert_eq!(
            of_type(&blocks, "CELL").len(),
            columns * lines.len(),
            "case {name:?}: expected {columns} cells in each of the {} rows, got {:?}",
            lines.len(),
            texts_of(&blocks, "CELL")
        );
        assert_eq!(
            texts_of(&blocks, "WORD"),
            drawn_words(&lines),
            "case {name:?}: a column break splits the row, it does not change its words"
        );
    }
}

/// RULE: a cell's box is the box of the words read IN it, so a cell that holds no words
/// has nothing of its own that was measured: it spans the gap its neighbours leave, from
/// the right edge of the nearest populated cell before it to the left edge of the nearest
/// one after it, and it names no children.
///
/// Driven through `state::blocks` rather than over the wire, because no page of pixels can
/// draw an empty cell between two populated ones (two column gaps running together are one
/// gap). The rows differ along WHERE the empty cells sit and how many run together.
#[test]
fn a_table_cell_holding_no_words_is_bounded_by_the_cells_around_it() {
    // (case, the row as the content holds it, the 1-based columns holding no words)
    let cases: Vec<(&str, &str, &[usize])> = vec![
        ("one empty cell between two populated", "Widget\t\t1250.00", &[2]),
        ("two empty cells running together", "Widget\t\t\t1250.00", &[2, 3]),
        ("an empty cell before every populated one", "\tWidget\t1250.00", &[1]),
        ("two empty cells before every populated one", "\t\tWidget", &[1, 2]),
        ("an empty cell at the start and another inside", "\tWidget\t\t1250.00", &[1, 3]),
    ];

    for (case, row, empty) in cases {
        // Content with no measured geometry: the layout is the character grid the row's own
        // columns place it on, which is what makes the cell boxes comparable here.
        let content = state::DocumentContent {
            pages: vec![format!("Item\tQty\tPrice\n{row}\n")],
            confidence: 90.0,
            measured: Vec::new(),
        };
        let blocks = state::blocks(
            "a-document-key",
            &content,
            state::FeatureSet {
                forms: false,
                tables: true,
            },
        );
        let mut cells: Vec<&Block> = blocks
            .iter()
            .filter(|block| {
                block.block_type.as_deref() == Some("CELL") && block.row_index == Some(2)
            })
            .collect();
        cells.sort_by_key(|cell| cell.column_index.unwrap_or_default());
        assert_eq!(
            cells.len(),
            row.split('\t').count(),
            "case {case:?}: expected one CELL per cell of {row:?}, got {cells:?}"
        );

        let boxes: Vec<BoundingBox> = cells
            .iter()
            .map(|cell| {
                cell.geometry
                    .as_ref()
                    .and_then(|geometry| geometry.bounding_box.clone())
                    .unwrap_or_else(|| panic!("case {case:?}: a CELL must carry a BoundingBox"))
            })
            .collect();
        let right = |bbox: &BoundingBox| bbox.left + bbox.width;
        let populated: Vec<usize> = (1..=cells.len())
            .filter(|column| !empty.contains(column))
            .collect();
        assert!(
            !populated.is_empty(),
            "case {case:?}: the row must hold at least one populated cell"
        );

        for &column in empty {
            let own = &boxes[column - 1];
            assert!(
                own.width >= -1e-6,
                "case {case:?}: the empty cell at column {column} must not be reported inside \
                 out, got {own:?}"
            );
            if let Some(&before) = populated.iter().rev().find(|before| **before < column) {
                assert!(
                    (own.left - right(&boxes[before - 1])).abs() < 1e-6,
                    "case {case:?}: the empty cell at column {column} must start where column \
                     {before} ends ({:?}), got {own:?}",
                    boxes[before - 1]
                );
            }
            if let Some(&after) = populated.iter().find(|after| **after > column) {
                assert!(
                    (right(own) - boxes[after - 1].left).abs() < 1e-6,
                    "case {case:?}: the empty cell at column {column} must end where column \
                     {after} starts ({:?}), got {own:?}",
                    boxes[after - 1]
                );
            }
            assert!(
                cells[column - 1].relationships.is_none(),
                "case {case:?}: the empty cell at column {column} holds no words, so it must \
                 name no children, got {:?}",
                cells[column - 1]
            );
        }

        // The positive control on the same row: a populated cell spans its own words.
        for &column in &populated {
            assert!(
                cells[column - 1]
                    .relationships
                    .as_ref()
                    .is_some_and(|relationships| !relationships.is_empty()),
                "case {case:?}: the cell at column {column} must name the words read in it, \
                 got {:?}",
                cells[column - 1]
            );
            assert!(
                boxes[column - 1].width > 0.0,
                "case {case:?}: the cell at column {column} holds words, so it must have a \
                 width, got {:?}",
                boxes[column - 1]
            );
        }
    }
}
