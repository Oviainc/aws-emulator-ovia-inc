// SPDX-License-Identifier: CC0-1.0

//! Compiling a CloudWatch Logs Insights QL query string, which is what decides whether
//! `StartQuery` accepts one or refuses it with `MalformedQueryException` ("The query string
//! is not valid. Details about this error are displayed in a QueryCompileError object").
//!
//! A query is a PIPELINE: commands separated by `|`, each reading the records the previous
//! one produced. This module parses that pipeline far enough to answer the one question the
//! API asks of it — does the query compile? — and to say WHERE it stopped when it does not,
//! which is the `QueryCompileError$location` the error carries.
//!
//! What it judges, and nothing else:
//!
//! * the pipeline's shape: a query with no command at all, a stage that is empty, or a `|`
//!   followed by a word that is no command of the language ([`COMMANDS`]), is not a query;
//! * the command of each stage: it has to be one of the commands the query language defines;
//! * the operand of each stage: a command the language writes with one
//!   ([`COMMANDS_TAKING_AN_OPERAND`]) is not a command without it, so a query that ends — or
//!   reaches the next stage separator — before the operand it owes does not compile;
//! * the regular expression a `parse` or `filter` command carries: it is compiled with the
//!   `regex` crate, so an expression that is not a regular expression at all — an unclosed
//!   group, a dangling quantifier — is refused where it stands;
//! * the ephemeral fields the pipeline defines: a command may not DEFINE a field name an
//!   earlier command of the same pipeline already defined — whether it defines it with an
//!   `as` clause, with a named capture group of a `parse` expression, or by listing it in a
//!   `fields` command — because the two definitions would name one field;
//! * a `limit` command's count, which is a positive integer;
//! * the lookup table each `lookup` command reads, which that command's first argument names
//!   and which the caller resolves against the account's lookup tables
//!   ([`CompiledQuery::lookup_tables`]).
//!
//! It judges nothing about the DATA: which fields a record carries, and whether a filter
//! expression selects anything, are questions about log events rather than about the query
//! string.
//!
//! WHAT BOUNDS IT: the only input is the query string, which the model constrains to
//! `@length(0..10000)` (`QueryString`) and the request walk refuses outside that before any
//! handler runs — so every scan here is linear or near-linear in at most ten thousand
//! characters, with no recursion anywhere and no state that outlives one call. The one thing
//! a query could otherwise grow without a bound of its own is the regular expression it
//! carries, which is compiled under an explicit size limit ([`MAX_COMPILED_REGEX_BYTES`]) and
//! never run over any data.
//!
//! [`COMMANDS`] is the closed set of commands the query language defines today. A query
//! using a command the language adds later is refused here until that command is added to
//! that one list — which is the deliberate trade: a word that is no command is the most
//! common malformed query there is, and refusing it is what `MalformedQueryException` is for.

use std::collections::BTreeSet;

/// Why a query string does not compile, and where in the string the compiler stopped.
///
/// The offsets are CHARACTER offsets into the query string, which is what
/// `QueryCompileErrorLocation`'s `startCharOffset` / `endCharOffset` carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryCompileFailure {
    /// What is wrong, as the compile error reports it.
    pub message: String,
    /// The first character of the text at fault.
    pub start_char_offset: usize,
    /// One past its last character.
    pub end_char_offset: usize,
}

/// One stage of a compiled pipeline: the command it runs and the text it runs it over.
///
/// The commands of a pipeline decide whether a query COMPILES, which is all `StartQuery`
/// asks; running one also needs the arguments each command was given, so a stage carries
/// both and whoever EXECUTES the pipeline reads it here rather than splitting the query
/// string a second time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledStage {
    /// The stage's command, lowercased — one of [`COMMANDS`] for a query that compiled.
    pub command: String,
    /// Everything after the command, exactly as the query string wrote it.
    pub arguments: String,
}

/// What a query string compiled to: the facts `StartQuery` needs about an accepted query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledQuery {
    /// The command of each stage of the pipeline, in order, lowercased.
    pub commands: Vec<String>,
    /// The same stages, each with the arguments its command was given, for a caller that
    /// RUNS the pipeline rather than only accepting it.
    pub stages: Vec<CompiledStage>,
    /// The ephemeral fields the pipeline defines, in no particular order.
    pub defined_fields: BTreeSet<String>,
    /// The lookup tables the pipeline's `lookup` commands read, each named once and in the
    /// order the pipeline names them. The query string says only which table a stage reads;
    /// whether the account HOLDS that table is a question about state, so the caller resolves
    /// these names against the stored lookup tables.
    pub lookup_tables: Vec<String>,
}

impl CompiledQuery {
    /// Whether the query selects the log groups to search itself: "Or the queryString must
    /// include a SOURCE command to select log groups for the query" (`StartQuery`).
    pub fn selects_its_own_sources(&self) -> bool {
        self.commands.iter().any(|command| command == SOURCE)
    }
}

/// The commands the query language defines, each spelled as a query spells it.
///
/// SOURCE from `StartQuery`'s own documentation ("the queryString must include a SOURCE
/// command to select log groups for the query"); the rest from the CloudWatch Logs Insights
/// query syntax `StartQuery` refers to ("For more information, see CloudWatch Logs Insights
/// Query Syntax"), which defines the query commands `display`, `fields`, `filter`, `stats`,
/// `sort`, `limit`, `parse`, `pattern`, `diff`, `dedup` and `unmask`.
pub const COMMANDS: [&str; 13] = [
    SOURCE, "display", "fields", "filter", "stats", "sort", "limit", "parse", "pattern", "diff",
    "dedup", "unmask", LOOKUP,
];

/// The command that selects the log groups a query reads, as `StartQuery` spells it.
pub const SOURCE: &str = "source";

/// The command that creates ephemeral fields from a regular expression or a glob pattern.
const PARSE: &str = "parse";

/// The command that selects records, which can match them against a regular expression
/// (`filter @message like /pattern/`).
const FILTER: &str = "filter";

/// The command that gets specific fields, or defines a new ephemeral field.
const FIELDS: &str = "fields";

/// The command that bounds how many records the pipeline returns.
const LIMIT: &str = "limit";

/// The command that enriches each record of the pipeline from a lookup table — the reference
/// data `CreateLookupTable` uploads — by joining the table's rows on a key of the record:
/// `lookup <lookupTableName> <keyField>[ AS <tableKeyField>] [OUTPUT <tableField>[ AS
/// <newField>], …]`.
const LOOKUP: &str = "lookup";

/// The keyword that opens the list of lookup-table columns a `lookup` command joins onto each
/// record.
const OUTPUT: &str = "output";

/// The keyword that names the field a command's expression is assigned to.
const AS: &str = "as";

/// One stage of the pipeline: its text and where that text starts.
#[derive(Debug, Clone)]
struct Stage {
    text: String,
    start_char_offset: usize,
}

/// Compile `query`, answering what it compiled to or why it does not compile.
///
/// This is the whole of the `MalformedQueryException` decision for a Logs Insights QL query
/// string: a query this answers `Ok` for is accepted, and one it answers `Err` for is
/// refused with the failure's message and location in the `QueryCompileError`.
pub fn compile(query: &str) -> Result<CompiledQuery, QueryCompileFailure> {
    let stages = split_pipeline(query);
    if stages.is_empty() {
        return Err(QueryCompileFailure {
            message: "The query string is empty.".to_string(),
            start_char_offset: 0,
            end_char_offset: query.chars().count(),
        });
    }
    let mut compiled = CompiledQuery::default();
    for stage in &stages {
        let trimmed = stage.text.trim();
        if trimmed.is_empty() {
            // A stage with no command in it is the compiler running out of input where a
            // command was required, so it stopped at the END of what it had read: the
            // location is the zero-width position there rather than the blank run it
            // scanned, which is what lets the refusal name the end of the query as the
            // symbol it stopped on.
            let at = stage.start_char_offset + stage.text.chars().count();
            return Err(QueryCompileFailure {
                message: "The query has an empty command.".to_string(),
                start_char_offset: at,
                end_char_offset: at,
            });
        }
        let command_offset = stage.start_char_offset + leading_blank_chars(&stage.text);
        let command: String = trimmed
            .chars()
            .take_while(|character| !character.is_whitespace())
            .collect();
        let lowercased = command.to_lowercase();
        if !COMMANDS.contains(&lowercased.as_str()) {
            return Err(QueryCompileFailure {
                message: format!("The command {command} is not a valid query command."),
                start_char_offset: command_offset,
                end_char_offset: command_offset + command.chars().count(),
            });
        }
        let arguments = &trimmed[command.len()..];
        let arguments_offset = command_offset + command.chars().count();
        // A `|` left INSIDE a stage separates this command from a word that is no command of
        // the query language — the split above only separates stages at a command — so the
        // word after it is what the query got wrong.
        if let Some(failure) = unseparated_command(arguments, arguments_offset) {
            return Err(failure);
        }
        // A command of the language is the command WORD plus what it acts on, so a stage
        // holding the word alone is a command the query never finished writing: the compiler
        // stops where the operand was owed ([`missing_operand_failure`]) rather than accepting
        // a stage that commands nothing.
        if COMMANDS_TAKING_AN_OPERAND.contains(&lowercased.as_str()) && arguments.trim().is_empty()
        {
            // The command is named as the QUERY spells it, the way the unknown-command
            // failure above names the word it stopped on.
            return Err(missing_operand_failure(query, &command, arguments_offset));
        }
        if lowercased == PARSE || lowercased == FILTER {
            check_regex_pattern(&lowercased, arguments, arguments_offset)?;
        }
        if lowercased == LIMIT {
            check_limit_count(arguments, arguments_offset)?;
        }
        // A `lookup` command reads one lookup table, which its first argument names. The name
        // is collected here and resolved against state by the caller, so a table this
        // account does hold makes the query a query of it.
        if lowercased == LOOKUP {
            let table = lookup_table_name(arguments, arguments_offset)?;
            if !compiled.lookup_tables.contains(&table) {
                compiled.lookup_tables.push(table);
            }
        }
        // A `fields` command DEFINES the fields it lists — "use the fields command to get
        // specific fields, or to create a new ephemeral field" — so a field an earlier
        // command of the same pipeline already defined cannot be listed by it: the two
        // definitions would name one field. Only `fields` defines what it names this way; a
        // command that merely READS a field (`display`, `stats`, `sort`, `filter`) names it
        // without defining anything.
        if lowercased == FIELDS {
            for token in tokens(arguments) {
                let name = token.text.trim_matches(',');
                if is_field_name(name) && compiled.defined_fields.contains(name) {
                    return Err(QueryCompileFailure {
                        message: format!("The field {name} is already defined."),
                        start_char_offset: arguments_offset + token.offset,
                        end_char_offset: arguments_offset
                            + token.offset
                            + token.text.chars().count(),
                    });
                }
            }
        }
        for (field, offset) in defined_fields(&lowercased, arguments, arguments_offset) {
            if !compiled.defined_fields.insert(field.clone()) {
                return Err(QueryCompileFailure {
                    message: format!("The field {field} is already defined."),
                    start_char_offset: offset,
                    end_char_offset: offset + field.chars().count(),
                });
            }
        }
        compiled.stages.push(CompiledStage {
            command: lowercased.clone(),
            arguments: arguments.to_string(),
        });
        compiled.commands.push(lowercased);
    }
    Ok(compiled)
}

/// The commands that take an OPERAND, each spelled as [`COMMANDS`] spells it.
///
/// Source: the CloudWatch Logs Insights query syntax `StartQuery` refers to, the same
/// document [`COMMANDS`] is read off — which writes each of these as the command word
/// followed by what it acts on: the log groups `SOURCE` selects, the fields `display`,
/// `fields` and `dedup` name, the expression `filter` selects records by, the aggregation
/// `stats` computes, the field `sort` orders by, the pattern `parse` extracts with, the field
/// `pattern` clusters and the field `unmask` reveals. None of those operands is optional, so
/// a stage holding one of these words alone commands nothing.
///
/// Three commands of [`COMMANDS`] are deliberately absent: `diff`, which that syntax writes
/// on its own (it compares the stage's records with the previous period and takes no
/// operand), and `limit` and `lookup`, whose own required argument is judged by the check
/// that reads it ([`check_limit_count`], [`lookup_table_name`]) and reported in the terms
/// that check states.
const COMMANDS_TAKING_AN_OPERAND: [&str; 10] = [
    SOURCE, "display", FIELDS, FILTER, "stats", "sort", PARSE, "pattern", "dedup", "unmask",
];

/// The failure a command whose operand the query never wrote is refused with: the compiler
/// stopped where that operand was owed, which is `at`.
///
/// What stands at `at` decides the span. When nothing but blank space follows it in the whole
/// query, the compiler ran out of INPUT: the span is the zero-width position at the END of the
/// query string, which is what names the end of the query as the symbol it stopped on — the
/// convention the empty-stage branch of [`compile`] reports an exhausted query with, so one
/// query that ends early and another that ends early further along are placed the same way,
/// each at its own end. When a symbol DOES stand there — the `|` opening the next stage of a
/// pipeline whose earlier command is the incomplete one — that symbol is what the query got
/// wrong, so the span covers it, and ONE token of it: a token is a run of word characters
/// ([`is_word_character`]), and any other character is a token of its own, so a separator
/// written against the next command (`fields |sort x`) is named as the one character it is.
fn missing_operand_failure(query: &str, command: &str, at: usize) -> QueryCompileFailure {
    let message = format!("The {command} command requires an argument.");
    let characters: Vec<char> = query.chars().collect();
    let mut index = at.min(characters.len());
    while characters
        .get(index)
        .is_some_and(|character| character.is_whitespace())
    {
        index += 1;
    }
    let symbol: String = match characters.get(index) {
        None => String::new(),
        Some(first) if !is_word_character(*first) => first.to_string(),
        Some(_) => characters[index..]
            .iter()
            .take_while(|character| is_word_character(**character))
            .collect(),
    };
    if symbol.is_empty() {
        return QueryCompileFailure {
            message,
            start_char_offset: characters.len(),
            end_char_offset: characters.len(),
        };
    }
    QueryCompileFailure {
        message,
        start_char_offset: index,
        end_char_offset: index + symbol.chars().count(),
    }
}

/// Whether `character` is part of a WORD of the query language — a command, a field name, a
/// number — rather than a token of its own. The word characters are the ones a field name is
/// written from ([`is_field_name`]), so a name and the command before it tokenize alike.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '@' | '_' | '.')
}

/// How many characters of leading blank space `text` opens with.
fn leading_blank_chars(text: &str) -> usize {
    text.chars()
        .take_while(|character| character.is_whitespace())
        .count()
}

/// Split `query` into its pipeline stages.
///
/// A pipeline stage BEGINS with a command, so a `|` separates two stages exactly when a
/// command follows it. Every other `|` belongs to the command it sits inside — the
/// alternation of a regular expression (`filter @message like /WARN|ERROR/`) is written with
/// one, and so is a quoted string — and is left where it is. A `|` that is followed by a word
/// which is no command of the language is then the query's own mistake, reported by
/// [`unseparated_command`] against the stage it was left in.
fn split_pipeline(query: &str) -> Vec<Stage> {
    split_pipeline_of(query, &COMMANDS)
}

/// [`split_pipeline`] over the command vocabulary `commands`, which is what a pipeline's
/// stage separators are recognised by. The Logs Insights QL commands are [`COMMANDS`];
/// another pipelined query language passes its own.
fn split_pipeline_of(query: &str, commands: &[&str]) -> Vec<Stage> {
    let characters: Vec<char> = query.chars().collect();
    let mut stages: Vec<Stage> = Vec::new();
    let mut current = String::new();
    let mut stage_start = 0usize;
    let mut index = 0usize;
    while index < characters.len() {
        let character = characters[index];
        // A quoted string travels whole: a `|` inside one is part of the string, and so is a
        // word that looks like a command.
        if character == '"' || character == '\'' {
            let (literal, next) = scan_quoted(&characters, index, character);
            current.push_str(&literal);
            index = next;
            continue;
        }
        if character == '|' && command_follows(&characters, index + 1, commands) {
            stages.push(Stage {
                text: current.clone(),
                start_char_offset: stage_start,
            });
            current.clear();
            stage_start = index + 1;
            index += 1;
            continue;
        }
        current.push(character);
        index += 1;
    }
    stages.push(Stage {
        text: current,
        start_char_offset: stage_start,
    });
    // A query of nothing but blank space holds no command at all, which the caller reports.
    if stages.len() == 1 && stages[0].text.trim().is_empty() {
        return Vec::new();
    }
    stages
}

/// Whether a command of the language begins at `from`, after any blank space: the test a `|`
/// is a stage separator by.
fn command_follows(characters: &[char], from: usize, commands: &[&str]) -> bool {
    let mut index = from;
    while characters.get(index).is_some_and(|character| character.is_whitespace()) {
        index += 1;
    }
    let word: String = characters[index.min(characters.len())..]
        .iter()
        .take_while(|character| character.is_alphanumeric() || **character == '_')
        .collect();
    commands.contains(&word.to_lowercase().as_str())
}

/// The stages of a pipelined query written in the command vocabulary `commands`, each as
/// the command it names and the arguments it was given — with nothing judged about either.
///
/// This is the reading an EXECUTOR needs for a query language whose syntax this service does
/// not compile (the OpenSearch languages `StartQuery` also accepts, whose pipelines are
/// separated by `|` the same way), so one splitter serves every pipeline and a `|` inside a
/// quoted string or a regular-expression literal stays where it is in all of them.
///
/// A stage's command is a WORD, so it ends at the first character that is not part of one:
/// `source=logGroups(…)` names the command `source` and is given the rest, whatever
/// whitespace the query string left around the `=`. The command is lowercased, because a
/// command names itself however it is cased; the arguments are handed on EXACTLY as written,
/// because they carry names and values whose case is their own.
pub fn split_stages(query: &str, commands: &[&str]) -> Vec<CompiledStage> {
    split_pipeline_of(query, commands)
        .into_iter()
        .filter_map(|stage| {
            let trimmed = stage.text.trim().to_string();
            if trimmed.is_empty() {
                return None;
            }
            let command: String = trimmed
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect();
            let arguments: String = trimmed.chars().skip(command.chars().count()).collect();
            Some(CompiledStage {
                command: command.to_lowercase(),
                arguments,
            })
        })
        .collect()
}

/// The `|` a stage still holds, and the word after it, when that word is no command of the
/// language — the only way a `|` survives [`split_pipeline`].
///
/// A `|` inside a regular-expression literal is part of that expression, so the scan skips a
/// `/…/` literal; a `/` with no closing `/` in the same stage is a division operator rather
/// than the start of one.
fn unseparated_command(arguments: &str, arguments_offset: usize) -> Option<QueryCompileFailure> {
    let characters: Vec<char> = arguments.chars().collect();
    let mut index = 0usize;
    while index < characters.len() {
        let character = characters[index];
        if character == '"' || character == '\'' {
            let (literal, next) = scan_quoted(&characters, index, character);
            let _ = literal;
            index = next;
            continue;
        }
        if character == '/' {
            if let Some(close) = closing_slash(&characters, index) {
                index = close + 1;
                continue;
            }
        }
        if character == '|' {
            let mut after = index + 1;
            while characters
                .get(after)
                .is_some_and(|character| character.is_whitespace())
            {
                after += 1;
            }
            let word: String = characters[after.min(characters.len())..]
                .iter()
                .take_while(|character| !character.is_whitespace())
                .collect();
            let (start, end) = if word.is_empty() {
                // Nothing follows the separator: the compiler ran out of input where a
                // command was required, so it stopped at the end of what it had read (see
                // the empty-stage branch of `compile`).
                (characters.len(), characters.len())
            } else {
                (after, after + word.chars().count())
            };
            return Some(QueryCompileFailure {
                message: if word.is_empty() {
                    "The query has an empty command.".to_string()
                } else {
                    format!("The command {word} is not a valid query command.")
                },
                start_char_offset: arguments_offset + start,
                end_char_offset: arguments_offset + end,
            });
        }
        index += 1;
    }
    None
}

/// The index of the `/` that closes the literal opening at `open`, or `None` when nothing in
/// the same text closes it.
fn closing_slash(characters: &[char], open: usize) -> Option<usize> {
    let mut index = open + 1;
    while index < characters.len() {
        match characters[index] {
            '\\' => index += 2,
            '/' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

/// Scan the quoted string that opens at `start`, answering its text (delimiters included) and
/// the index just past it. A string nothing closes runs to the end of the query, which is
/// where the command carrying it is judged.
fn scan_quoted(characters: &[char], start: usize, delimiter: char) -> (String, usize) {
    let mut literal = String::from(delimiter);
    let mut index = start + 1;
    while index < characters.len() {
        let character = characters[index];
        literal.push(character);
        // An escaped delimiter is part of the string, so the escape and what follows it are
        // taken together.
        if character == '\\' {
            if let Some(escaped) = characters.get(index + 1) {
                literal.push(*escaped);
                index += 2;
                continue;
            }
        }
        if character == delimiter {
            return (literal, index + 1);
        }
        index += 1;
    }
    (literal, characters.len())
}

/// Check the regular expression a command carries: an expression has to be an expression.
///
/// The expression is compiled with the `regex` crate, so a literal that is not an expression
/// at all — an unclosed group, a dangling quantifier — is refused where it stands. A
/// construct that engine does not IMPLEMENT (a look-around, say) is not evidence that the
/// query is malformed, so it is accepted: this judges the expression's syntax, not the
/// feature set of one engine.
///
/// A `parse` command's pattern is required, so an UNCLOSED literal there is still its
/// pattern and is judged as one; in any other command a `/` that nothing closes is a division
/// operator rather than the start of an expression.
fn check_regex_pattern(
    command: &str,
    arguments: &str,
    arguments_offset: usize,
) -> Result<(), QueryCompileFailure> {
    let Some(literal) = regex_literal(arguments, arguments_offset) else {
        return Ok(());
    };
    if !literal.closed && command != PARSE {
        return Ok(());
    }
    // The expression comes from the request, so what compiling it may allocate is bounded
    // here rather than left at the engine's default: the expression is only COMPILED — to
    // decide whether it is an expression at all — and never run over any data.
    let compiled = ::regex::RegexBuilder::new(&literal.pattern)
        .size_limit(MAX_COMPILED_REGEX_BYTES)
        .build();
    if let Err(error) = compiled {
        let unsupported_construct = error.to_string().contains("not supported");
        if !unsupported_construct {
            // The text at fault is the literal and the slashes that delimit it: two of them
            // for a literal a `/` closed, one for a literal that runs to the end of the
            // command.
            let slashes = if literal.closed { 2 } else { 1 };
            return Err(QueryCompileFailure {
                message: format!(
                    "The regular expression /{}/ is not valid.",
                    literal.pattern
                ),
                start_char_offset: literal.offset,
                end_char_offset: literal.offset + literal.pattern.chars().count() + slashes,
            });
        }
    }
    Ok(())
}

/// How large a compiled expression this module will build from a request's query string.
/// THE EMULATOR'S OWN BOUND: the query string is already `@length(0..10000)`, and this caps
/// what compiling the expression inside it may allocate.
const MAX_COMPILED_REGEX_BYTES: usize = 1 << 20;

/// A regular-expression literal a command carries.
struct RegexLiteral {
    /// The expression itself, without its slashes.
    pattern: String,
    /// The character offset of its opening slash.
    offset: usize,
    /// Whether a `/` closed it.
    closed: bool,
}

/// The regular-expression literal inside `arguments`, or `None` when the command carries
/// none — a glob pattern in quotes, or no pattern at all.
fn regex_literal(arguments: &str, arguments_offset: usize) -> Option<RegexLiteral> {
    let characters: Vec<char> = arguments.chars().collect();
    let open = characters.iter().position(|character| *character == '/')?;
    let mut pattern = String::new();
    let mut index = open + 1;
    while index < characters.len() {
        let character = characters[index];
        if character == '\\' {
            if let Some(escaped) = characters.get(index + 1) {
                pattern.push(character);
                pattern.push(*escaped);
                index += 2;
                continue;
            }
        }
        if character == '/' {
            return Some(RegexLiteral {
                pattern,
                offset: arguments_offset + open,
                closed: true,
            });
        }
        pattern.push(character);
        index += 1;
    }
    Some(RegexLiteral {
        pattern,
        offset: arguments_offset + open,
        closed: false,
    })
}

/// The lookup table a `lookup` command reads: its first argument, which the command requires
/// (`lookup <lookupTableName> <keyField> …`), so a `lookup` naming none is not a `lookup`.
///
/// A name may be written quoted, which the quotes are not part of.
fn lookup_table_name(
    arguments: &str,
    arguments_offset: usize,
) -> Result<String, QueryCompileFailure> {
    let missing = || QueryCompileFailure {
        message: "The lookup command must name a lookup table.".to_string(),
        start_char_offset: arguments_offset,
        end_char_offset: arguments_offset + arguments.chars().count(),
    };
    let token = tokens(arguments).into_iter().next().ok_or_else(missing)?;
    let name = token
        .text
        .trim_matches(|character| matches!(character, '"' | '\'' | ','));
    if name.is_empty() {
        return Err(missing());
    }
    Ok(name.to_string())
}

/// The text of a `lookup` command's `OUTPUT` list and the character offset that text starts
/// at, or `None` when the command carries no `OUTPUT` clause — which is optional, a `lookup`
/// without one joining every column of the table.
fn output_clause(arguments: &str, arguments_offset: usize) -> Option<(String, usize)> {
    let keyword = tokens(arguments)
        .into_iter()
        .find(|token| token.text.to_lowercase() == OUTPUT)?;
    let after = keyword.offset + keyword.text.chars().count();
    let text: String = arguments.chars().skip(after).collect();
    Some((text, arguments_offset + after))
}

/// Check a `limit` command's count: "limit — Specify the number of log events returned", so
/// the argument is one positive integer and nothing else.
fn check_limit_count(
    arguments: &str,
    arguments_offset: usize,
) -> Result<(), QueryCompileFailure> {
    let trimmed = arguments.trim();
    let offset = arguments_offset + leading_blank_chars(arguments);
    let valid = trimmed
        .parse::<u64>()
        .is_ok_and(|count| count > 0);
    if valid {
        return Ok(());
    }
    Err(QueryCompileFailure {
        message: format!("The limit {trimmed} is not a positive number of log events."),
        start_char_offset: offset,
        end_char_offset: offset + trimmed.chars().count(),
    })
}

/// The ephemeral fields the stage DEFINES, each with the character offset of its name.
///
/// A field is defined by an `as <name>` clause — which `fields`, `parse`, `stats` and
/// `pattern` all take — and by a named capture group of a `parse` command's regular
/// expression, which is how that command names the fields it extracts.
fn defined_fields(
    command: &str,
    arguments: &str,
    arguments_offset: usize,
) -> Vec<(String, usize)> {
    let mut defined: Vec<(String, usize)> = Vec::new();
    if command == PARSE {
        if let Some(literal) = regex_literal(arguments, arguments_offset) {
            defined.extend(capture_group_names(&literal.pattern, literal.offset + 1));
        }
    }
    // A `lookup` command's two halves name different things. Before `OUTPUT` an `as` clause
    // renames the record's key to a COLUMN OF THE TABLE, which defines no field of the
    // pipeline; inside the `OUTPUT` list an `as` clause does name a new ephemeral field. The
    // columns the `OUTPUT` list itself names are the table's own, joined onto each record, so
    // a pipeline that already names one of them is not defining it twice.
    if command == LOOKUP {
        if let Some((output, offset)) = output_clause(arguments, arguments_offset) {
            defined.extend(alias_names(&output, offset));
        }
        return defined;
    }
    defined.extend(alias_names(arguments, arguments_offset));
    defined
}

/// The named capture groups of `literal`, each with the character offset of its name.
///
/// A named group is written `(?<name>` — the form `StartQuery`'s own `parse` examples use —
/// or `(?P<name>`, the form the Python flavour spells the same group with; a group with no
/// name defines no field.
fn capture_group_names(literal: &str, literal_offset: usize) -> Vec<(String, usize)> {
    let characters: Vec<char> = literal.chars().collect();
    let mut names: Vec<(String, usize)> = Vec::new();
    let mut index = 0usize;
    while index < characters.len() {
        if characters[index] == '\\' {
            index += 2;
            continue;
        }
        // The group opens with `(?` and then either `<` or `P<`, so the name starts two or
        // three characters past the parenthesis.
        let opens_group = characters[index] == '(' && characters.get(index + 1) == Some(&'?');
        let name_at = if !opens_group {
            None
        } else if characters.get(index + 2) == Some(&'<') {
            Some(index + 3)
        } else if characters.get(index + 2) == Some(&'P')
            && characters.get(index + 3) == Some(&'<')
        {
            Some(index + 4)
        } else {
            None
        };
        let Some(name_at) = name_at else {
            index += 1;
            continue;
        };
        let name: String = characters[name_at.min(characters.len())..]
            .iter()
            .take_while(|character| **character != '>')
            .collect();
        if !name.is_empty() {
            names.push((name.clone(), literal_offset + name_at));
        }
        index = name_at + name.chars().count();
    }
    names
}

/// The names the `as` clauses of `arguments` assign to, each with the character offset of the
/// name.
///
/// One clause may name several fields, as a glob `parse` does (`parse @message "* *" as a,
/// b`), so the names of a comma-separated list are all defined by it.
fn alias_names(arguments: &str, arguments_offset: usize) -> Vec<(String, usize)> {
    let mut names: Vec<(String, usize)> = Vec::new();
    let mut after_as = false;
    for token in tokens(arguments) {
        let token_offset = arguments_offset + token.offset;
        if token.text.to_lowercase() == AS {
            after_as = true;
            continue;
        }
        if !after_as {
            continue;
        }
        // The list ends at the first token that is not a field name or a separator.
        let name = token.text.trim_matches(',');
        if name.is_empty() {
            continue;
        }
        if is_field_name(name) {
            names.push((name.to_string(), token_offset));
        }
        // A token that does not end in a comma ends the list it belongs to.
        if !token.text.ends_with(',') {
            after_as = false;
        }
    }
    names
}

/// One whitespace-separated token of a command's arguments, and where it starts.
struct Token {
    text: String,
    offset: usize,
}

/// The whitespace-separated tokens of `arguments`, each with its character offset inside
/// them.
fn tokens(arguments: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();
    let mut start = 0usize;
    for (index, character) in arguments.chars().enumerate() {
        if character.is_whitespace() {
            if !current.is_empty() {
                tokens.push(Token {
                    text: current.clone(),
                    offset: start,
                });
                current.clear();
            }
            continue;
        }
        if current.is_empty() {
            start = index;
        }
        current.push(character);
    }
    if !current.is_empty() {
        tokens.push(Token {
            text: current,
            offset: start,
        });
    }
    tokens
}

/// Whether `candidate` is a field name: a name, or a `@`-prefixed system field, and not an
/// expression or a punctuation token.
fn is_field_name(candidate: &str) -> bool {
    let mut characters = candidate.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first == '@' || first.is_alphabetic() || first == '_')
        && candidate
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '@' | '_' | '.'))
}
