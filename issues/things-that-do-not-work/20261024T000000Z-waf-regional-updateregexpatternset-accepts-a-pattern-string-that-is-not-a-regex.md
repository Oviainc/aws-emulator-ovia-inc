Issue-Source: fixer-filed-gap
Issue-Severity: MEDIUM
Issue-Shape-Id: com.amazonaws.wafregional#AWSWAF_Regional_20161128

# waf-regional UpdateRegexPatternSet accepts a RegexPatternString that is not a regular expression

## Operation
UpdateRegexPatternSet

## Description
`UpdateRegexPatternSet` models `WAFInvalidRegexPatternException` (HTTP 400) — "The regular
expression (regex) you specified in `RegexPatternString` is invalid" — and the handler at
`services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/update_regex_pattern_set.rs`
never produces it: the only constraints applied to a pattern string are the modelled ones
(`RegexPatternString`: `length 1..512`, pattern `.*`), which any text satisfies. So an
`INSERT` of a string that is not a regular expression at all — `[a-z`, `(abc`, `*bad`,
`a{3,1}`, a trailing backslash — is answered HTTP 200, stored, and reported by a later
`GetRegexPatternSet` as a live pattern, where AWS refuses the update.

Deciding whether a pattern string is a regular expression means COMPILING caller text,
which needs a regex engine (a crate, and therefore a `Cargo.lock` change) and a bound on
the work one caller-chosen pattern may buy. That is why the handler that implemented the
INSERT/DELETE semantics left this refusal out rather than guessing at it; this report is
the record of the gap.

What a fix needs:
* a maintained, LINEAR-TIME engine on this untrusted-input path — the `regex` crate, whose
  RE2 syntax is also the syntax AWS WAF documents for a pattern string (no look-around, no
  backreferences) — pinned in the service crate's `Cargo.toml`;
* an explicit compile budget rather than the crate's defaults (`RegexBuilder::size_limit`
  / `dfa_size_limit`), because the pattern comes from the request;
* the compile run BEFORE the store's lock is taken, over the already-bounded `Updates`
  list (`MAX_REGEX_PATTERN_SET_UPDATES_PER_REQUEST`), so no caller-chosen compile cost is
  ever paid inside the critical section;
* `Err(UpdateRegexPatternSetErrorModeled::WAFInvalidRegexPatternException(..))` for a
  pattern that does not compile, with the stored set and the request's `ChangeToken` left
  exactly as they were;
* an integration test at `tests/all/regex_pattern_set_updates.rs` asserting the refusal
  over a table of malformed pattern strings and of constructs that syntax does not have.

The same gap applies to `UpdateRegexMatchSet`, which models the same exception and is still
a scaffold answering 501.

## Evidence
`src/ops/update_regex_pattern_set.rs` declares `WAFInvalidRegexPatternException` in
`UpdateRegexPatternSetErrorModeled` and constructs it nowhere; the handler's own comment
says so ("Whether the string is a regular expression AWS would compile is what the
operation's `WAFInvalidRegexPatternException` answers … that refusal is left to a change of
its own"). `tests/all/regex_pattern_set_updates.rs` therefore asserts the INSERT/DELETE
semantics and the other modelled refusals, but no invalid-pattern refusal.

## Steps to Reproduce
1. Boot the emulator built from the checkout.
2. `CreateRegexPatternSet` with any `Name` and a fresh `ChangeToken`; note the
   `RegexPatternSetId`.
3. `UpdateRegexPatternSet` for that id with
   `Updates=[{Action: INSERT, RegexPatternString: "[a-z"}]` and a fresh `ChangeToken`.
   Observe HTTP 200 where AWS answers 400 `WAFInvalidRegexPatternException`.
4. `GetRegexPatternSet` for that id: the uncompilable string is reported in
   `RegexPatternSet.RegexPatternStrings`.
