// SPDX-License-Identifier: CC0-1.0

//! A speech mark: one line of the `application/x-json-stream` a `json` request carries.
//!
//! The four mark types the model declares describe the SAME utterance the audio formats
//! carry, so they all come off the timeline [`super::plan`] builds and are produced by
//! [`super::audio::speech_marks_stream`]. A `sentence` and a `word` mark cover a stretch
//! of `Text` and report its byte offsets; a `viseme` and an `ssml` mark are points in
//! time and report none.

/// One speech mark, as Amazon Polly's `application/x-json-stream` carries it: one JSON
/// object per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechMark {
    /// Milliseconds from the start of the stream.
    pub time: u64,
    /// `"sentence"`, `"word"`, `"viseme"` or `"ssml"` — one of the values
    /// `SpeechMarkType` enumerates.
    pub mark_type: &'static str,
    /// The byte offsets in `Text` of the stretch this mark covers, for the two mark
    /// types that cover one; `None` for a mark that is a point in time.
    pub span: Option<(usize, usize)>,
    /// What the mark carries: the text a `sentence` or `word` covers, the mouth shape of
    /// a `viseme`, or the name of an `ssml` mark.
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::super::audio::speech_marks_stream;
    use super::*;

    /// RULE: a mark that covers a stretch of the text reports its offsets, and one that
    /// is a point in time reports none.
    #[test]
    fn only_a_covering_mark_reports_offsets() {
        let marks = [
            SpeechMark {
                time: 0,
                mark_type: "word",
                span: Some((0, 5)),
                value: "Hello".to_string(),
            },
            SpeechMark {
                time: 7,
                mark_type: "viseme",
                span: None,
                value: "k".to_string(),
            },
        ];
        let stream = String::from_utf8(speech_marks_stream(&marks)).expect("the stream is UTF-8");
        let lines: Vec<&str> = stream.lines().collect();
        assert_eq!(lines.len(), 2, "one line a mark, got {lines:?}");
        let word: serde_json::Value =
            serde_json::from_str(lines[0]).expect("each line is a JSON object");
        assert_eq!(
            (word["start"].as_u64(), word["end"].as_u64()),
            (Some(0), Some(5)),
            "a word mark reports the offsets of the text it covers, got {word}"
        );
        let viseme: serde_json::Value =
            serde_json::from_str(lines[1]).expect("each line is a JSON object");
        assert!(
            viseme.get("start").is_none() && viseme.get("end").is_none(),
            "a viseme mark covers no stretch of the text, so it reports no offsets, got \
             {viseme}"
        );
        assert_eq!(
            viseme["value"].as_str(),
            Some("k"),
            "a viseme mark carries its mouth shape, got {viseme}"
        );
    }
}
