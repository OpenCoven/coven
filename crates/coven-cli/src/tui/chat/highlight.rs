//! Per-language code-block tokenizers. Each `tokenize_*` walks one line of
//! source and emits typed tokens; `highlight_line` then maps token roles to
//! brand-tinted styles via `theme::SYNTAX_*`. Block comments (`/* … */`)
//! and Python triple-strings (`""" … """`, `''' … '''`) are tracked across
//! lines via `TokenizerState`, which the caller threads through every line
//! of a fenced block. Other multi-line constructs — Rust raw strings
//! spanning lines, JS template literals with embedded newlines, bash
//! heredocs — aren't tracked: unterminated strings color to EOL and resume
//! fresh on the next line. Good enough for chat output, with no external
//! deps.

use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

use crate::theme::{
    self, SYNTAX_ATTRIBUTE, SYNTAX_COMMENT, SYNTAX_KEYWORD, SYNTAX_NUMBER, SYNTAX_STRING,
};

/// Language picked from the opening fence tag (e.g. ```` ```rust ````).
/// `tokenizer_for` is the single entry point that maps a fence tag to a
/// variant; unknown tags return `None` and the caller falls back to plain
/// rendering.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Lang {
    Rust,
    Js,
    Python,
    Bash,
}

/// Semantic role of a token. Drives both the production `Span` styling and
/// any out-of-band consumers (preview tooling, future export paths) without
/// re-running classification heuristics.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Default,
    Keyword,
    String,
    Number,
    Comment,
    Attribute,
}

/// One tokenized chunk of a code line. `text` is an owned copy; the
/// tokenizers are byte-walkers but the chunks they emit are sliced out
/// of the input by index.
#[derive(Debug, Clone)]
pub(super) struct Token {
    pub text: String,
    pub role: Role,
}

/// Triple-quote flavor for Python multi-line strings — `'''…'''` vs
/// `"""…"""`. Tracked so the closing delimiter has to match the opener.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TripleKind {
    Single,
    Double,
}

impl TripleKind {
    fn marker(self) -> &'static str {
        match self {
            TripleKind::Single => "'''",
            TripleKind::Double => "\"\"\"",
        }
    }
}

/// Cross-line state carried by the caller between successive `tokenize` /
/// `highlight_line` calls on the lines of one fenced code block. Reset
/// to `default()` at every opening fence so two adjacent blocks never
/// bleed state into each other.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TokenizerState {
    /// True when the prior line ended inside an unterminated `/* …`
    /// block comment, so the next line should color from byte 0 up
    /// through the closing `*/` (or to EOL again if still unclosed).
    pub in_block_comment: bool,
    /// Some(kind) when the prior line ended inside an unterminated Python
    /// triple-quoted string. The next line colors from byte 0 up through
    /// the matching `'''`/`"""` (or stays in string-mode to EOL).
    pub in_python_triple: Option<TripleKind>,
}

/// Map a fence language tag (everything after ```` ``` ```` up to the first
/// whitespace) to a known tokenizer. Case-insensitive; common aliases
/// collapsed to the same variant.
pub(super) fn tokenizer_for(tag: &str) -> Option<Lang> {
    match tag.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some(Lang::Rust),
        "javascript" | "js" | "jsx" | "typescript" | "ts" | "tsx" => Some(Lang::Js),
        "python" | "py" => Some(Lang::Python),
        "bash" | "sh" | "shell" | "zsh" => Some(Lang::Bash),
        _ => None,
    }
}

/// Tokenize one line of source. Returns a flat sequence of `Token`s whose
/// concatenated text equals the input. `state` carries cross-line context
/// (currently just "are we still inside a `/* … */` block comment?") and
/// is mutated to reflect the state at end-of-line.
pub(super) fn tokenize(line: &str, lang: Lang, state: &mut TokenizerState) -> Vec<Token> {
    match lang {
        Lang::Rust => tokenize_rust(line, state),
        Lang::Js => tokenize_js(line, state),
        Lang::Python => tokenize_python(line, state),
        Lang::Bash => tokenize_bash(line, state),
    }
}

/// Tokenize one line of source into styled spans. Thin wrapper over
/// `tokenize` that resolves each token role to its brand-mapped style.
pub(super) fn highlight_line<'a>(
    line: &str,
    lang: Lang,
    default_style: Style,
    state: &mut TokenizerState,
) -> Vec<Span<'a>> {
    tokenize(line, lang, state)
        .into_iter()
        .map(|tok| Span::styled(tok.text, style_for(tok.role, default_style)))
        .collect()
}

pub(super) fn style_for(role: Role, default_style: Style) -> Style {
    match role {
        Role::Default => default_style,
        Role::Keyword => kw_style(),
        Role::String => str_style(),
        Role::Number => num_style(),
        Role::Comment => com_style(),
        Role::Attribute => attr_style(),
    }
}

// ── Shared style builders ──────────────────────────────────────────────────

fn kw_style() -> Style {
    theme::ratatui_style(SYNTAX_KEYWORD).add_modifier(Modifier::BOLD)
}
fn str_style() -> Style {
    theme::ratatui_style(SYNTAX_STRING)
}
fn num_style() -> Style {
    theme::ratatui_style(SYNTAX_NUMBER)
}
fn com_style() -> Style {
    theme::ratatui_style(SYNTAX_COMMENT).add_modifier(Modifier::ITALIC)
}
fn attr_style() -> Style {
    theme::ratatui_style(SYNTAX_ATTRIBUTE)
}

// ── Rust ───────────────────────────────────────────────────────────────────

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "yield",
];

fn tokenize_rust(line: &str, state: &mut TokenizerState) -> Vec<Token> {
    let bytes = line.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut buf = String::new();
    let mut i = resume_block_comment(line, state, &mut out);

    while i < bytes.len() {
        if line[i..].starts_with("//") {
            flush_buf(&mut buf, &mut out);
            out.push(tok(&line[i..], Role::Comment));
            return out;
        }
        if line[i..].starts_with("/*") {
            flush_buf(&mut buf, &mut out);
            i = consume_block_comment(line, i, &mut out, state);
            continue;
        }
        if bytes[i] == b'#' && (line[i + 1..].starts_with('[') || line[i + 1..].starts_with("![")) {
            flush_buf(&mut buf, &mut out);
            if let Some(end_off) = line[i..].find(']') {
                let span_end = i + end_off + 1;
                out.push(tok(&line[i..span_end], Role::Attribute));
                i = span_end;
            } else {
                out.push(tok(&line[i..], Role::Attribute));
                return out;
            }
            continue;
        }
        if bytes[i] == b'"' {
            flush_buf(&mut buf, &mut out);
            let end = scan_dq_string(line, i);
            out.push(tok(&line[i..end], Role::String));
            i = end;
            continue;
        }
        if bytes[i] == b'\'' {
            flush_buf(&mut buf, &mut out);
            let (end, is_lifetime) = scan_rust_quote(line, i);
            let role = if is_lifetime {
                Role::Attribute
            } else {
                Role::String
            };
            out.push(tok(&line[i..end], role));
            i = end;
            continue;
        }
        if bytes[i].is_ascii_digit() {
            flush_buf(&mut buf, &mut out);
            let end = scan_number(line, i);
            out.push(tok(&line[i..end], Role::Number));
            i = end;
            continue;
        }
        if is_ident_start(bytes[i]) {
            flush_buf(&mut buf, &mut out);
            let mut j = i + 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            let ident = &line[i..j];
            let role = if RUST_KEYWORDS.contains(&ident) {
                Role::Keyword
            } else {
                Role::Default
            };
            out.push(tok(ident, role));
            i = j;
            continue;
        }
        push_one_char(line, &mut i, &mut buf);
    }
    flush_buf(&mut buf, &mut out);
    out
}

// ── JavaScript / TypeScript ────────────────────────────────────────────────

const JS_KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "set",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

fn tokenize_js(line: &str, state: &mut TokenizerState) -> Vec<Token> {
    let bytes = line.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut buf = String::new();
    let mut i = resume_block_comment(line, state, &mut out);

    while i < bytes.len() {
        if line[i..].starts_with("//") {
            flush_buf(&mut buf, &mut out);
            out.push(tok(&line[i..], Role::Comment));
            return out;
        }
        if line[i..].starts_with("/*") {
            flush_buf(&mut buf, &mut out);
            i = consume_block_comment(line, i, &mut out, state);
            continue;
        }
        let b = bytes[i];
        if b == b'"' || b == b'\'' || b == b'`' {
            flush_buf(&mut buf, &mut out);
            let end = scan_js_string(line, i, b);
            out.push(tok(&line[i..end], Role::String));
            i = end;
            continue;
        }
        if b.is_ascii_digit() {
            flush_buf(&mut buf, &mut out);
            let end = scan_number(line, i);
            out.push(tok(&line[i..end], Role::Number));
            i = end;
            continue;
        }
        if is_ident_start(b) {
            flush_buf(&mut buf, &mut out);
            let mut j = i + 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            let ident = &line[i..j];
            let role = if JS_KEYWORDS.contains(&ident) {
                Role::Keyword
            } else {
                Role::Default
            };
            out.push(tok(ident, role));
            i = j;
            continue;
        }
        push_one_char(line, &mut i, &mut buf);
    }
    flush_buf(&mut buf, &mut out);
    out
}

// ── Python ─────────────────────────────────────────────────────────────────

const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "case", "class",
    "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
    "import", "in", "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise", "return",
    "try", "while", "with", "yield",
];

fn tokenize_python(line: &str, state: &mut TokenizerState) -> Vec<Token> {
    let bytes = line.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut buf = String::new();
    let mut i = resume_triple_string(line, state, &mut out);

    while i < bytes.len() {
        // `#` line comment — Python has no block comment.
        if bytes[i] == b'#' {
            flush_buf(&mut buf, &mut out);
            out.push(tok(&line[i..], Role::Comment));
            return out;
        }
        // String literal — possibly with a single-char prefix (r/R/b/B/f/F/u/U)
        // or two-char combo (rb, Rb, fr, …). Always cap the prefix at two
        // ASCII letters so we don't mis-eat identifiers.
        if let Some(quote_offset) = python_string_prefix_len(bytes, i) {
            let quote_at = i + quote_offset;
            if quote_at < bytes.len() && (bytes[quote_at] == b'"' || bytes[quote_at] == b'\'') {
                flush_buf(&mut buf, &mut out);
                let qb = bytes[quote_at];
                let (kind, is_triple) = if line[quote_at..].starts_with("\"\"\"") {
                    (Some(TripleKind::Double), true)
                } else if line[quote_at..].starts_with("'''") {
                    (Some(TripleKind::Single), true)
                } else {
                    (None, false)
                };
                if is_triple {
                    let kind = kind.unwrap();
                    let after_open = quote_at + 3;
                    // Look for matching closing triple on this same line.
                    if let Some(rel) = line[after_open..].find(kind.marker()) {
                        let close_end = after_open + rel + 3;
                        out.push(tok(&line[i..close_end], Role::String));
                        i = close_end;
                    } else {
                        out.push(tok(&line[i..], Role::String));
                        state.in_python_triple = Some(kind);
                        return out;
                    }
                } else {
                    let end = scan_js_string(line, quote_at, qb);
                    out.push(tok(&line[i..end], Role::String));
                    i = end;
                }
                continue;
            }
        }
        // Decorator: `@name` (or `@name.method`) — color the whole leading
        // identifier-and-dots run as attribute. Heuristic: only at start of
        // line (modulo indent) treat as decorator. Off-leading `@` (rare in
        // Python pre-3.5) falls through as default.
        if bytes[i] == b'@' && line[..i].chars().all(char::is_whitespace) {
            flush_buf(&mut buf, &mut out);
            let mut j = i + 1;
            while j < bytes.len() && (is_ident_continue(bytes[j]) || bytes[j] == b'.') {
                j += 1;
            }
            out.push(tok(&line[i..j], Role::Attribute));
            i = j;
            continue;
        }
        if bytes[i].is_ascii_digit() {
            flush_buf(&mut buf, &mut out);
            let end = scan_number(line, i);
            // Python supports `1j`/`1J` complex literal suffix; scan_number's
            // generic ident-continue tail already grabs it.
            out.push(tok(&line[i..end], Role::Number));
            i = end;
            continue;
        }
        if is_ident_start(bytes[i]) {
            flush_buf(&mut buf, &mut out);
            let mut j = i + 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            let ident = &line[i..j];
            let role = if PYTHON_KEYWORDS.contains(&ident) {
                Role::Keyword
            } else {
                Role::Default
            };
            out.push(tok(ident, role));
            i = j;
            continue;
        }
        push_one_char(line, &mut i, &mut buf);
    }
    flush_buf(&mut buf, &mut out);
    out
}

/// Number of bytes between `i` and the quote character of a Python string
/// literal, accounting for an optional 1- or 2-char prefix (`r`, `b`, `f`,
/// `u`, `rb`, `fr`, …). Returns `Some(0)` for an un-prefixed quote and
/// `None` when there's no string starting here.
fn python_string_prefix_len(bytes: &[u8], i: usize) -> Option<usize> {
    if i >= bytes.len() {
        return None;
    }
    let b = bytes[i];
    if b == b'"' || b == b'\'' {
        return Some(0);
    }
    if !is_python_string_prefix_char(b) {
        return None;
    }
    // Single-char prefix?
    if i + 1 < bytes.len() && (bytes[i + 1] == b'"' || bytes[i + 1] == b'\'') {
        // Reject identifiers like `r_squared` — only treat as a prefix when
        // the byte before is not ident-continue (so it isn't part of a
        // bigger identifier) AND the quote follows directly.
        if i == 0 || !is_ident_continue(bytes[i - 1]) {
            return Some(1);
        }
        return None;
    }
    // Two-char prefix?
    if i + 2 < bytes.len()
        && is_python_string_prefix_char(bytes[i + 1])
        && (bytes[i + 2] == b'"' || bytes[i + 2] == b'\'')
        && (i == 0 || !is_ident_continue(bytes[i - 1]))
    {
        return Some(2);
    }
    None
}

fn is_python_string_prefix_char(b: u8) -> bool {
    matches!(b, b'r' | b'R' | b'b' | b'B' | b'f' | b'F' | b'u' | b'U')
}

/// If we entered this line still inside a Python triple-quoted string,
/// scan forward for the matching closing triple. Emit one String token for
/// the swallowed range and clear the state when we close it; otherwise
/// stay inside and color the whole line. Returns the byte index where
/// regular tokenization should resume.
fn resume_triple_string(line: &str, state: &mut TokenizerState, out: &mut Vec<Token>) -> usize {
    let Some(kind) = state.in_python_triple else {
        return 0;
    };
    let marker = kind.marker();
    if let Some(rel) = line.find(marker) {
        let close_end = rel + 3;
        out.push(tok(&line[..close_end], Role::String));
        state.in_python_triple = None;
        close_end
    } else {
        out.push(tok(line, Role::String));
        line.len()
    }
}

// ── Bash / shell ───────────────────────────────────────────────────────────

const BASH_KEYWORDS: &[&str] = &[
    "alias", "break", "case", "continue", "declare", "do", "done", "elif", "else", "esac", "eval",
    "exec", "exit", "export", "fi", "for", "function", "getopts", "if", "in", "let", "local",
    "read", "readonly", "return", "select", "set", "shift", "source", "test", "then", "time",
    "trap", "umask", "unalias", "unset", "until", "while",
];

fn tokenize_bash(line: &str, _state: &mut TokenizerState) -> Vec<Token> {
    let bytes = line.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < bytes.len() {
        // `#` line comment — covers shebang `#!/...` too.
        if bytes[i] == b'#' {
            flush_buf(&mut buf, &mut out);
            out.push(tok(&line[i..], Role::Comment));
            return out;
        }
        let b = bytes[i];
        // Strings. Single-quotes are fully literal in bash (no `\` escapes);
        // double-quotes honor backslash escapes. We don't try to tokenize
        // `$var` interpolation *inside* a double-quoted string — the whole
        // span renders as one String token.
        if b == b'\'' {
            flush_buf(&mut buf, &mut out);
            let end = scan_bash_single_string(line, i);
            out.push(tok(&line[i..end], Role::String));
            i = end;
            continue;
        }
        if b == b'"' {
            flush_buf(&mut buf, &mut out);
            let end = scan_dq_string(line, i);
            out.push(tok(&line[i..end], Role::String));
            i = end;
            continue;
        }
        // Variable reference: `$NAME`, `$1`, `${NAME}`, `$?`, `$#`, `$@`.
        // Color as Attribute so it shares the lifetime/decorator family.
        if b == b'$' && i + 1 < bytes.len() {
            flush_buf(&mut buf, &mut out);
            let end = scan_bash_var(line, i);
            out.push(tok(&line[i..end], Role::Attribute));
            i = end;
            continue;
        }
        if b.is_ascii_digit() {
            flush_buf(&mut buf, &mut out);
            let end = scan_number(line, i);
            out.push(tok(&line[i..end], Role::Number));
            i = end;
            continue;
        }
        if is_ident_start(b) {
            flush_buf(&mut buf, &mut out);
            let mut j = i + 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            let ident = &line[i..j];
            let role = if BASH_KEYWORDS.contains(&ident) {
                Role::Keyword
            } else {
                Role::Default
            };
            out.push(tok(ident, role));
            i = j;
            continue;
        }
        push_one_char(line, &mut i, &mut buf);
    }
    flush_buf(&mut buf, &mut out);
    out
}

/// Scan a bash single-quoted string. Single quotes are literal — `\` does
/// not escape; the only terminator is another `'`.
fn scan_bash_single_string(line: &str, start: usize) -> usize {
    let bytes = line.as_bytes();
    let mut j = start + 1;
    while j < bytes.len() {
        if bytes[j] == b'\'' {
            return j + 1;
        }
        j += 1;
    }
    j
}

/// Scan a bash variable reference starting at `$`. Handles `$NAME`, `$1`,
/// `${...}`, and special variables `$?`, `$#`, `$@`, `$*`, `$$`, `$!`, `$-`.
fn scan_bash_var(line: &str, start: usize) -> usize {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut j = start + 1;
    if j >= n {
        return j;
    }
    if bytes[j] == b'{' {
        j += 1;
        while j < n && bytes[j] != b'}' {
            j += 1;
        }
        if j < n {
            j += 1;
        }
        return j;
    }
    if matches!(bytes[j], b'?' | b'#' | b'@' | b'*' | b'$' | b'!' | b'-') {
        return j + 1;
    }
    while j < n && is_ident_continue(bytes[j]) {
        j += 1;
    }
    j
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn tok(text: &str, role: Role) -> Token {
    Token {
        text: text.to_string(),
        role,
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn flush_buf(buf: &mut String, out: &mut Vec<Token>) {
    if !buf.is_empty() {
        out.push(Token {
            text: std::mem::take(buf),
            role: Role::Default,
        });
    }
}

fn push_one_char(line: &str, i: &mut usize, buf: &mut String) {
    let ch = line[*i..].chars().next().expect("non-empty remainder");
    let ch_len = ch.len_utf8();
    buf.push_str(&line[*i..*i + ch_len]);
    *i += ch_len;
}

fn consume_block_comment(
    line: &str,
    start: usize,
    out: &mut Vec<Token>,
    state: &mut TokenizerState,
) -> usize {
    let after = &line[start + 2..];
    if let Some(end) = after.find("*/") {
        let span_end = start + 2 + end + 2;
        out.push(tok(&line[start..span_end], Role::Comment));
        span_end
    } else {
        // No closing `*/` on this line — flag the state so the next line
        // resumes inside the comment, and color the rest of this line.
        out.push(tok(&line[start..], Role::Comment));
        state.in_block_comment = true;
        line.len()
    }
}

/// If we entered this line still inside a `/* … */` block comment, emit
/// a Comment token from byte 0 up to (and including) the closing `*/`,
/// or the entire line if it never closes. Returns the byte index where
/// regular tokenization should resume.
fn resume_block_comment(line: &str, state: &mut TokenizerState, out: &mut Vec<Token>) -> usize {
    if !state.in_block_comment {
        return 0;
    }
    if let Some(end) = line.find("*/") {
        out.push(tok(&line[..end + 2], Role::Comment));
        state.in_block_comment = false;
        end + 2
    } else {
        out.push(tok(line, Role::Comment));
        line.len()
    }
}

fn scan_dq_string(line: &str, start: usize) -> usize {
    let bytes = line.as_bytes();
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if j + 1 < bytes.len() => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    j
}

fn scan_js_string(line: &str, start: usize, delim: u8) -> usize {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut j = start + 1;
    while j < n {
        let c = bytes[j];
        if c == b'\\' && j + 1 < n {
            j += 2;
            continue;
        }
        if c == delim {
            return j + 1;
        }
        j += 1;
    }
    j
}

/// Disambiguate `'…` between a char literal and a Rust lifetime. Returns
/// `(end_index, is_lifetime)`. Char literals stop after the closing `'`;
/// lifetimes stop at the first non-ident byte after the leading `'`.
fn scan_rust_quote(line: &str, start: usize) -> (usize, bool) {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut j = start + 1;
    if j >= n {
        return (n, false);
    }
    if bytes[j] == b'\\' {
        j += 1;
        while j < n && bytes[j] != b'\'' {
            j += 1;
        }
        if j < n {
            j += 1;
        }
        return (j, false);
    }
    let id_start = j;
    while j < n && is_ident_continue(bytes[j]) {
        j += 1;
    }
    let id_len = j - id_start;
    if id_len > 0 && (j >= n || bytes[j] != b'\'') {
        return (j, true);
    }
    if id_len == 0 {
        if let Some(ch) = line[id_start..].chars().next() {
            j = id_start + ch.len_utf8();
        }
    }
    if j < n && bytes[j] == b'\'' {
        j += 1;
    }
    (j, false)
}

/// Scan a numeric literal starting at `start`. Handles `0x` / `0o` / `0b`
/// prefixes, fractional and exponent parts, embedded `_` separators, and a
/// trailing type-suffix ident run (`u32`, `f64`, etc.) without validating
/// the suffix — invalid suffixes still color cleanly as numbers, which is
/// fine for display.
fn scan_number(line: &str, start: usize) -> usize {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut j = start;

    if j + 1 < n
        && bytes[j] == b'0'
        && matches!(bytes[j + 1], b'x' | b'X' | b'o' | b'O' | b'b' | b'B')
    {
        j += 2;
        while j < n && (bytes[j].is_ascii_hexdigit() || bytes[j] == b'_') {
            j += 1;
        }
    } else {
        while j < n && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
            j += 1;
        }
        if j + 1 < n && bytes[j] == b'.' && bytes[j + 1].is_ascii_digit() {
            j += 1;
            while j < n && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
                j += 1;
            }
        }
        if j < n && (bytes[j] == b'e' || bytes[j] == b'E') {
            j += 1;
            if j < n && (bytes[j] == b'+' || bytes[j] == b'-') {
                j += 1;
            }
            while j < n && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
                j += 1;
            }
        }
    }
    while j < n && is_ident_continue(bytes[j]) {
        j += 1;
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn spans_of(line: &str, lang: Lang) -> Vec<(String, Style)> {
        let mut state = TokenizerState::default();
        highlight_line(line, lang, Style::default(), &mut state)
            .into_iter()
            .map(|s| (s.content.to_string(), s.style))
            .collect()
    }

    fn roles_of(line: &str, lang: Lang) -> Vec<(String, Role)> {
        let mut state = TokenizerState::default();
        tokenize(line, lang, &mut state)
            .into_iter()
            .map(|t| (t.text, t.role))
            .collect()
    }

    /// Tokenize a full multi-line source, threading state across lines the
    /// way the render loop does. Returns one role-vector per line.
    fn roles_per_line(source: &str, lang: Lang) -> Vec<Vec<(String, Role)>> {
        let mut state = TokenizerState::default();
        source
            .lines()
            .map(|line| {
                tokenize(line, lang, &mut state)
                    .into_iter()
                    .map(|t| (t.text, t.role))
                    .collect()
            })
            .collect()
    }

    fn find_span<'a>(spans: &'a [(String, Style)], needle: &str) -> &'a (String, Style) {
        spans
            .iter()
            .find(|(c, _)| c == needle)
            .unwrap_or_else(|| panic!("missing span {needle:?} in {spans:?}"))
    }

    fn find_role<'a>(toks: &'a [(String, Role)], needle: &str) -> &'a (String, Role) {
        toks.iter()
            .find(|(c, _)| c == needle)
            .unwrap_or_else(|| panic!("missing token {needle:?} in {toks:?}"))
    }

    #[test]
    fn tokenizer_for_recognizes_rust_and_js_aliases() {
        assert_eq!(tokenizer_for("rust"), Some(Lang::Rust));
        assert_eq!(tokenizer_for("rs"), Some(Lang::Rust));
        assert_eq!(tokenizer_for("RUST"), Some(Lang::Rust));
        assert_eq!(tokenizer_for("js"), Some(Lang::Js));
        assert_eq!(tokenizer_for("javascript"), Some(Lang::Js));
        assert_eq!(tokenizer_for("ts"), Some(Lang::Js));
        assert_eq!(tokenizer_for("tsx"), Some(Lang::Js));
        assert_eq!(tokenizer_for("typescript"), Some(Lang::Js));
        assert_eq!(tokenizer_for(""), None);
        // Languages handled by other tokenizers / unknown tags fall through.
        assert_eq!(tokenizer_for("cobol"), None);
    }

    #[test]
    fn rust_keyword_and_string_get_distinct_roles_and_styles() {
        let toks = roles_of(r#"let x = "hi";"#, Lang::Rust);
        assert_eq!(find_role(&toks, "let").1, Role::Keyword);
        assert_eq!(find_role(&toks, "\"hi\"").1, Role::String);

        let spans = spans_of(r#"let x = "hi";"#, Lang::Rust);
        assert_eq!(find_span(&spans, "let").1, kw_style());
        assert_eq!(find_span(&spans, "\"hi\"").1, str_style());
    }

    #[test]
    fn rust_numbers_with_suffix_and_underscores_render_as_one_token() {
        let toks = roles_of("let n = 1_000_u32;", Lang::Rust);
        assert_eq!(find_role(&toks, "1_000_u32").1, Role::Number);
    }

    #[test]
    fn rust_hex_number_renders_as_number_token() {
        let toks = roles_of("0xFF_AA", Lang::Rust);
        assert_eq!(find_role(&toks, "0xFF_AA").1, Role::Number);
    }

    #[test]
    fn rust_float_renders_as_number_token() {
        let toks = roles_of("3.14_f64", Lang::Rust);
        assert_eq!(find_role(&toks, "3.14_f64").1, Role::Number);
    }

    #[test]
    fn rust_method_call_dot_is_not_consumed_into_number() {
        let toks = roles_of("2.into()", Lang::Rust);
        assert_eq!(find_role(&toks, "2").1, Role::Number);
        assert_eq!(find_role(&toks, "into").1, Role::Default);
    }

    #[test]
    fn rust_line_comment_colors_to_end_of_line() {
        let toks = roles_of("let x = 1; // trailing", Lang::Rust);
        assert_eq!(find_role(&toks, "// trailing").1, Role::Comment);
    }

    #[test]
    fn rust_block_comment_with_close_only_colors_inside() {
        let toks = roles_of("a /* b */ c", Lang::Rust);
        assert_eq!(find_role(&toks, "/* b */").1, Role::Comment);
        find_role(&toks, "a");
        find_role(&toks, "c");
    }

    #[test]
    fn rust_unterminated_block_comment_colors_to_eol() {
        let toks = roles_of("ok /* never closes", Lang::Rust);
        assert_eq!(find_role(&toks, "/* never closes").1, Role::Comment);
    }

    #[test]
    fn block_comment_spans_multiple_lines_when_state_threads_across_lines() {
        // Three-line block comment: open on line 1, body on line 2, close
        // on line 3. With state threaded, every byte of all three lines
        // (after the opening `let x = 1;`) should be Comment-tagged.
        let source = "let x = 1; /* opens\n   still inside\n   and closes */ let y = 2;";
        let per_line = roles_per_line(source, Lang::Rust);
        assert_eq!(per_line.len(), 3);

        // Line 1: tail from `/* opens` is one Comment token.
        assert_eq!(find_role(&per_line[0], "/* opens").1, Role::Comment);

        // Line 2: every token is Comment, because we entered already inside.
        assert!(
            per_line[1].iter().all(|(_, r)| *r == Role::Comment),
            "all of line 2 should be comment, got {:?}",
            per_line[1]
        );

        // Line 3: leading text up through `*/` is Comment; after the close
        // we drop back into regular tokenization and pick up `let` again.
        assert_eq!(find_role(&per_line[2], "   and closes */").1, Role::Comment);
        assert_eq!(find_role(&per_line[2], "let").1, Role::Keyword);
        assert_eq!(find_role(&per_line[2], "2").1, Role::Number);
    }

    #[test]
    fn block_comment_left_unterminated_keeps_state_set_across_remaining_lines() {
        let source = "/* opens\n   never closes";
        let per_line = roles_per_line(source, Lang::Rust);
        assert_eq!(per_line.len(), 2);
        for line in &per_line {
            assert!(
                line.iter().all(|(_, r)| *r == Role::Comment),
                "every line should be comment, got {:?}",
                line
            );
        }
    }

    #[test]
    fn block_comment_state_resets_between_separate_tokenize_calls() {
        // Calling `tokenize` with a fresh state must not pick up state from
        // a prior call that ended inside a block comment. (This is the
        // contract the render loop relies on when a new fence opens.)
        let mut state = TokenizerState::default();
        let _ = tokenize("/* unterminated", Lang::Rust, &mut state);
        assert!(state.in_block_comment, "expected state to be sticky");

        // A fresh state starts clean even if the same comment text appears.
        let mut fresh = TokenizerState::default();
        let toks: Vec<_> = tokenize("let z = 3;", Lang::Rust, &mut fresh)
            .into_iter()
            .map(|t| (t.text, t.role))
            .collect();
        assert_eq!(find_role(&toks, "let").1, Role::Keyword);
        assert!(!fresh.in_block_comment);
    }

    #[test]
    fn block_comment_in_typescript_spans_multiple_lines() {
        let source = "/* outer\n   notes */ const x = 1;";
        let per_line = roles_per_line(source, Lang::Js);
        assert_eq!(per_line.len(), 2);
        assert_eq!(find_role(&per_line[1], "   notes */").1, Role::Comment);
        assert_eq!(find_role(&per_line[1], "const").1, Role::Keyword);
    }

    #[test]
    fn rust_lifetime_is_styled_as_attribute_not_string() {
        let toks = roles_of("fn f<'a>(x: &'a str) {}", Lang::Rust);
        assert_eq!(find_role(&toks, "'a").1, Role::Attribute);
    }

    #[test]
    fn rust_char_literal_is_styled_as_string() {
        let toks = roles_of("let c = 'x';", Lang::Rust);
        assert_eq!(find_role(&toks, "'x'").1, Role::String);
    }

    #[test]
    fn rust_char_literal_with_escape_is_styled_as_string() {
        let toks = roles_of(r"let c = '\n';", Lang::Rust);
        assert_eq!(find_role(&toks, r"'\n'").1, Role::String);
    }

    #[test]
    fn rust_attribute_brackets_are_styled_as_attribute() {
        let toks = roles_of("#[derive(Debug)]", Lang::Rust);
        assert_eq!(find_role(&toks, "#[derive(Debug)]").1, Role::Attribute);
    }

    #[test]
    fn rust_string_with_escaped_quote_stays_intact() {
        let toks = roles_of(r#"let s = "a\"b";"#, Lang::Rust);
        assert_eq!(find_role(&toks, r#""a\"b""#).1, Role::String);
    }

    #[test]
    fn js_template_literal_is_styled_as_string() {
        let toks = roles_of("const s = `hi ${name}`;", Lang::Js);
        assert_eq!(find_role(&toks, "`hi ${name}`").1, Role::String);
    }

    #[test]
    fn js_keywords_render_with_keyword_role() {
        let toks = roles_of("const x = await f();", Lang::Js);
        assert_eq!(find_role(&toks, "const").1, Role::Keyword);
        assert_eq!(find_role(&toks, "await").1, Role::Keyword);
    }

    #[test]
    fn js_number_with_exponent_and_dollar_ident_split_cleanly() {
        let toks = roles_of("let $x = 1.5e-3;", Lang::Js);
        assert_eq!(find_role(&toks, "$x").1, Role::Default);
        assert_eq!(find_role(&toks, "1.5e-3").1, Role::Number);
    }

    #[test]
    fn js_line_comment_colors_to_end_of_line() {
        let toks = roles_of("return 0; // last", Lang::Js);
        assert_eq!(find_role(&toks, "// last").1, Role::Comment);
    }

    #[test]
    fn default_style_is_applied_to_non_token_runs() {
        let default_style = Style::default().add_modifier(Modifier::REVERSED);
        let mut state = TokenizerState::default();
        let spans: Vec<_> = highlight_line("foo(bar)", Lang::Rust, default_style, &mut state);
        let plain: Vec<_> = spans
            .iter()
            .filter(|s| s.content == "(" || s.content == ")")
            .collect();
        assert!(
            !plain.is_empty(),
            "expected punctuation to land in default-styled run"
        );
        for span in plain {
            assert_eq!(span.style, default_style);
        }
    }

    #[test]
    fn tokenizer_for_recognizes_python_and_bash_aliases() {
        assert_eq!(tokenizer_for("python"), Some(Lang::Python));
        assert_eq!(tokenizer_for("py"), Some(Lang::Python));
        assert_eq!(tokenizer_for("PYTHON"), Some(Lang::Python));
        assert_eq!(tokenizer_for("bash"), Some(Lang::Bash));
        assert_eq!(tokenizer_for("sh"), Some(Lang::Bash));
        assert_eq!(tokenizer_for("shell"), Some(Lang::Bash));
        assert_eq!(tokenizer_for("zsh"), Some(Lang::Bash));
    }

    // ── Python ─────────────────────────────────────────────────────────────

    #[test]
    fn python_keywords_and_strings_get_distinct_roles() {
        let toks = roles_of("def greet(name): return \"hi\"", Lang::Python);
        assert_eq!(find_role(&toks, "def").1, Role::Keyword);
        assert_eq!(find_role(&toks, "return").1, Role::Keyword);
        assert_eq!(find_role(&toks, "\"hi\"").1, Role::String);
    }

    #[test]
    fn python_line_comment_colors_to_end_of_line() {
        let toks = roles_of("x = 1  # trailing", Lang::Python);
        assert_eq!(find_role(&toks, "# trailing").1, Role::Comment);
    }

    #[test]
    fn python_decorator_at_start_of_line_is_attribute() {
        let toks = roles_of("    @functools.cache", Lang::Python);
        assert_eq!(find_role(&toks, "@functools.cache").1, Role::Attribute);
    }

    #[test]
    fn python_string_prefixes_are_part_of_the_string_token() {
        // `f"…"` and `rb"…"` — the prefix bytes belong to the string span,
        // not the surrounding default text.
        let toks = roles_of("x = f\"hi {name}\"", Lang::Python);
        assert_eq!(find_role(&toks, "f\"hi {name}\"").1, Role::String);

        let toks = roles_of("x = rb\"raw bytes\"", Lang::Python);
        assert_eq!(find_role(&toks, "rb\"raw bytes\"").1, Role::String);
    }

    #[test]
    fn python_identifiers_starting_with_string_prefix_letters_are_not_strings() {
        // `r_squared` starts with `r` but is just an identifier — the
        // prefix detector must NOT mis-eat the leading `r`.
        let toks = roles_of("r_squared = 1", Lang::Python);
        assert_eq!(find_role(&toks, "r_squared").1, Role::Default);
        assert_eq!(find_role(&toks, "1").1, Role::Number);
    }

    #[test]
    fn python_complex_literal_with_j_suffix_renders_as_one_number() {
        let toks = roles_of("z = 3.5j", Lang::Python);
        assert_eq!(find_role(&toks, "3.5j").1, Role::Number);
    }

    #[test]
    fn python_triple_double_string_spans_multiple_lines() {
        // Open on line 1, body on line 2, close on line 3. With state
        // threaded, every byte after the opening triple-quote and through
        // the closing one is one continuous String role.
        let source = "msg = \"\"\"opens\n   still inside\n   and closes\"\"\" + x";
        let per_line = roles_per_line(source, Lang::Python);
        assert_eq!(per_line.len(), 3);
        assert_eq!(find_role(&per_line[0], "\"\"\"opens").1, Role::String);
        assert!(
            per_line[1].iter().all(|(_, r)| *r == Role::String),
            "line 2 should be entirely string, got {:?}",
            per_line[1]
        );
        assert_eq!(
            find_role(&per_line[2], "   and closes\"\"\"").1,
            Role::String
        );
        // After the close we drop back into normal tokenization.
        assert_eq!(find_role(&per_line[2], "x").1, Role::Default);
    }

    #[test]
    fn python_triple_single_does_not_close_on_triple_double() {
        // `'''` and `"""` must not cross-close — the closer has to match
        // the opener's flavor. Here the inner `"""` should stay part of
        // the single-quoted triple string.
        let source = "doc = '''first\n   \"\"\" still inside\n   end'''";
        let per_line = roles_per_line(source, Lang::Python);
        assert_eq!(per_line.len(), 3);
        assert!(
            per_line[1].iter().all(|(_, r)| *r == Role::String),
            "mid-triple-string line should be entirely string, got {:?}",
            per_line[1]
        );
        assert_eq!(find_role(&per_line[2], "   end'''").1, Role::String);
    }

    #[test]
    fn python_triple_string_on_single_line_does_not_set_state() {
        let mut state = TokenizerState::default();
        let _ = tokenize("x = \"\"\"all on one line\"\"\"", Lang::Python, &mut state);
        assert_eq!(
            state.in_python_triple, None,
            "single-line triple string should not leave state armed"
        );
    }

    // ── Bash ───────────────────────────────────────────────────────────────

    #[test]
    fn bash_keywords_and_strings_get_distinct_roles() {
        let toks = roles_of("if [ \"$x\" = \"y\" ]; then echo hi; fi", Lang::Bash);
        assert_eq!(find_role(&toks, "if").1, Role::Keyword);
        assert_eq!(find_role(&toks, "then").1, Role::Keyword);
        assert_eq!(find_role(&toks, "fi").1, Role::Keyword);
        assert_eq!(find_role(&toks, "\"y\"").1, Role::String);
    }

    #[test]
    fn bash_line_comment_colors_to_end_of_line() {
        let toks = roles_of("echo hello  # greet", Lang::Bash);
        assert_eq!(find_role(&toks, "# greet").1, Role::Comment);
    }

    #[test]
    fn bash_single_quoted_string_treats_backslash_as_literal() {
        // In bash, single quotes are FULLY literal — `\'` does NOT escape.
        // The first `'` after the opener is always the terminator.
        let toks = roles_of("echo 'a\\b'c", Lang::Bash);
        assert_eq!(find_role(&toks, "'a\\b'").1, Role::String);
        assert_eq!(find_role(&toks, "c").1, Role::Default);
    }

    #[test]
    fn bash_variables_are_styled_as_attribute() {
        let toks = roles_of("path=$HOME; echo ${USER} status=$?", Lang::Bash);
        assert_eq!(find_role(&toks, "$HOME").1, Role::Attribute);
        assert_eq!(find_role(&toks, "${USER}").1, Role::Attribute);
        assert_eq!(find_role(&toks, "$?").1, Role::Attribute);
    }

    #[test]
    fn bash_shebang_is_treated_as_a_comment() {
        let toks = roles_of("#!/usr/bin/env bash", Lang::Bash);
        assert_eq!(find_role(&toks, "#!/usr/bin/env bash").1, Role::Comment);
    }

    #[test]
    fn bash_state_threading_is_a_no_op_block_comment_flag_stays_false() {
        // Bash has no `/* … */`, so tokenizing it must never set the C-style
        // block-comment flag — otherwise switching languages mid-fence would
        // bleed comment styling across blocks.
        let mut fresh = TokenizerState::default();
        let _ = tokenize("echo /* not a comment */", Lang::Bash, &mut fresh);
        assert!(!fresh.in_block_comment);
    }

    /// Visual smoke test. Ignored by default; run with
    /// `cargo test -p coven-cli preview_chat_highlight_to_truecolor_stdout -- --ignored --nocapture`
    /// to dump TrueColor ANSI to stdout. Uses the brand RGB tokens directly
    /// so the preview is faithful regardless of the per-process `theme::mode`
    /// cache (which collapses to NoColor under `cargo test`'s piped stdout).
    #[test]
    #[ignore]
    fn preview_chat_highlight_to_truecolor_stdout() {
        use crate::theme::brand;

        fn fg(c: crate::theme::Rgb) -> String {
            format!("\x1b[38;2;{};{};{}m", c.r, c.g, c.b)
        }
        let reset = "\x1b[0m";
        let bold = "\x1b[1m";
        let italic = "\x1b[3m";

        let samples = [
            (
                "rust",
                concat!(
                    "// renders an agent reply containing a Rust fenced block\n",
                    "#[derive(Debug)]\n",
                    "fn main() -> Result<()> {\n",
                    "    let port: u16 = 0xFF_u16;\n",
                    "    let label = \"coven\";\n",
                    "    let lifetime: &'static str = label;\n",
                    "    println!(\"{label} on {port}\"); // formatted\n",
                    "    Ok(())\n",
                    "}\n",
                ),
            ),
            (
                "ts",
                concat!(
                    "// TypeScript sample with template literal + number suffix\n",
                    "const greet = (name: string): string => `hi ${name}`;\n",
                    "let attempts = 1.5e-3; // tiny\n",
                    "/* block\n   comment */\n",
                    "export const PI = 3.14;\n",
                ),
            ),
            (
                "python",
                concat!(
                    "# Python sample with decorator + triple-string\n",
                    "import functools\n",
                    "\n",
                    "@functools.cache\n",
                    "def greet(name: str) -> str:\n",
                    "    \"\"\"Return a greeting.\n",
                    "    Supports multi-line docstring.\n",
                    "    \"\"\"\n",
                    "    return f\"hi {name}\"  # interpolated\n",
                    "\n",
                    "z = 3.5j\n",
                ),
            ),
            (
                "bash",
                concat!(
                    "#!/usr/bin/env bash\n",
                    "# bash sample with vars + control flow\n",
                    "set -euo pipefail\n",
                    "name=${1:-world}\n",
                    "for i in 1 2 3; do\n",
                    "    echo \"hello $name #$i\"\n",
                    "done\n",
                ),
            ),
        ];

        // Frame matches what append_agent_content_lines emits for code rows:
        // two-space gutter + bar prefix + tokenized content.
        let gutter_fg = fg(brand::TEXT_FAINT);
        let text_fg = fg(brand::TEXT);

        let kw_fg = fg(brand::PURPLE_2);
        let str_fg = fg(brand::SUCCESS);
        let num_fg = fg(brand::ACCENT_BLUE);
        let com_fg = fg(brand::TEXT_FAINT);
        let attr_fg = fg(brand::PURPLE_3);

        for (tag, source) in samples {
            let lang = tokenizer_for(tag).expect("known fence tag");
            let mut state = TokenizerState::default();
            println!(
                "\n{}{} fence ```{}``` (TrueColor preview){}",
                bold, attr_fg, tag, reset
            );
            for line in source.lines() {
                print!("{}  \u{2502} {}", gutter_fg, reset);
                for token in tokenize(line, lang, &mut state) {
                    let (open, close) = match token.role {
                        Role::Default => (text_fg.clone(), reset.to_string()),
                        Role::Keyword => (format!("{kw_fg}{bold}"), reset.to_string()),
                        Role::String => (str_fg.clone(), reset.to_string()),
                        Role::Number => (num_fg.clone(), reset.to_string()),
                        Role::Comment => (format!("{com_fg}{italic}"), reset.to_string()),
                        Role::Attribute => (attr_fg.clone(), reset.to_string()),
                    };
                    print!("{open}{}{close}", token.text);
                }
                println!();
            }
        }
    }
}
