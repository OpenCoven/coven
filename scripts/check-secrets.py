#!/usr/bin/env python3
"""Small repo-local secret guard for public-release checks.

The scanner intentionally prints rule names and file locations only. It never
prints matching values.
"""
from __future__ import annotations

import collections
import math
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXCLUDED_PARTS = {".git", "target", "node_modules", ".coven", ".comux", ".comux-hooks"}
EXCLUDED_PATHS = {"scripts/check-secrets.py", "scripts/check-secrets-test.py"}
LOCKFILE_NAMES = ("pnpm-lock.yaml", "package-lock.json", "yarn.lock")
LOCKFILE_PACKAGE_KEY = re.compile(r"^\s*(?:['\"]?/?@?[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)?(?:@[A-Za-z0-9][^:'\"]*)?['\"]?)\s*:\s*(?:\{\})?\s*$")
LOCKFILE_NODE_MODULE_KEY = re.compile(r'''^\s*["']?node_modules/(?:@?[A-Za-z0-9_.-]+/)?[A-Za-z0-9_.-]+["']?\s*:\s*\{?\s*$''')
LOCKFILE_PACKAGE_VERSION_ENTRY = re.compile(r"^\s*['\"]?@?[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)?['\"]?\s*:\s*\d+\.\d+\.\d+(?:[-+][A-Za-z0-9_.-]+)?\s*$")
LOCKFILE_INTEGRITY_LINE = re.compile(r'''["']?\bintegrity\b["']?\s*:\s*["']?(?:sha256|sha384|sha512)-[A-Za-z0-9+/=]+["']?''')
LOCKFILE_RESOLVED_LINE = re.compile(r'''["']?\bresolved\b["']?\s*:\s*["']?https://registry\.npmjs\.org/[A-Za-z0-9_+/@.,~%:-]+\.tgz["']?''')
SECRET_RULES: list[tuple[str, re.Pattern[str]]] = [
    ("private_key", re.compile(r"-----BEGIN (?:RSA |DSA |EC |OPENSSH |PGP )?PRIVATE KEY-----")),
    ("aws_access_key", re.compile(r"AKIA[0-9A-Z]{16}")),
    ("github_token", re.compile(r"gh[pousr]_[A-Za-z0-9_]{20,}")),
    ("openai_key", re.compile(r"sk-[A-Za-z0-9]{32,}")),
    ("anthropic_key", re.compile(r"sk-ant-[A-Za-z0-9_-]{20,}")),
    ("slack_token", re.compile(r"xox[baprs]-[A-Za-z0-9-]{20,}")),
    (
        "generic_assignment",
        re.compile(
            r"(?i)\b(api[_-]?key|secret|token|password|private[_-]?key)\b\s*[:=]\s*[\"']?[^\"'\s]{12,}"
        ),
    ),
]
GENERIC_ASSIGNMENT_SAFE_VALUE = re.compile(
    r'''(?ix)\b(?:api[_-]?key|secret|token|password|private[_-]?key)\b\s*[:=]\s*["']?'''
    r"(?:"
    r"<[-A-Za-z0-9_ .]{1,40}>"
    r"|your_[A-Za-z0-9_.-]{1,64}"
    r"|(?:placeholder|example|secret_value)(?:[-_][A-Za-z0-9_.-]{1,64})?"
    r"|op://[A-Za-z0-9_.@/%+-]{1,200}"
    r")"
    r'''["']?'''
)
RESERVED_EXAMPLE_URL = re.compile(
    r"(?i)https?://(?:[A-Za-z0-9-]+\.)*"
    r"(?:example\.(?:com|net|org)|[A-Za-z0-9-]+\.(?:example|invalid|localhost|test))"
    r"(?::[0-9]{1,5})?(?:/[^\s\"'<>]*)?"
)
ENV_SECRET_READ = re.compile(
    r'''(?ix)\b(?:api[_-]?key|secret|token|password|private[_-]?key)\b\s*[:=]\s*
    (?:
        os\.environ\.get\(
            \s*["'][A-Z0-9_]+["']
            (?:\s*,\s*(?:""|''|None))?
            \s*
        \)
        |std::env::var\(\s*["'][A-Z0-9_]+["']\s*\)
        |env::var\(\s*["'][A-Z0-9_]+["']\s*\)
        |process\.env\.[A-Z0-9_]+(?:\.trim\(\)|\?\.trim\(\)|!)?
    )'''
)
ENV_SECRET_REFERENCE = re.compile(
    r"(?i)\b(?:api[_-]?key|secret|token|password|private[_-]?key)\b\s*[:=]\s*[\"']?"
    r"(?:\$[A-Z0-9_]+|\$\{[A-Z0-9_]+(?:(?::?\?)[^}\"']*)?\})"
)
# A Rust `let` binding whose right-hand side begins with a call expression
# (identifier chain followed by `(`, e.g. `let token = text.split_whitespace()`).
# The secret-sounding name binds the RESULT of code that runs later; the line
# cannot contain a credential literal. A quoted or bare-blob RHS does not match
# this shape and still trips `generic_assignment`.
RUST_LET_CALL_BINDING = re.compile(
    r"^\s*let\s+(?:mut\s+)?[A-Za-z_][A-Za-z0-9_]*\s*(?::[^=]{1,64})?=\s*"
    r"[A-Za-z_][A-Za-z0-9_]*(?:(?:::|\.)[A-Za-z_][A-Za-z0-9_]*)*!?\("
)
QUOTED_TEXT = re.compile(r"'[^']*'|\"[^\"]*\"")
SAFE_SECRET_FIELD_REGEX_PATTERN = re.compile(
    r"""(?ix)
    ^(?P<paren>\()?
    (?:
        (?:api[_-]?key|apikey|key|secret|token|password|private[_-]?key|authorization)
        (?:\s*[:=]|\[=:\])?
        |bearer(?:\s+\[A-Za-z0-9\])?\s*
    )
    (?:
        \|
        (?:
            (?:api[_-]?key|apikey|key|secret|token|password|private[_-]?key|authorization)
            (?:\s*[:=]|\[=:\])?
            |bearer(?:\s+\[A-Za-z0-9\])?\s*
        )
    )+
    (?(paren)\)|)
    $
    """
)
SAFE_ASSIGNMENT_SUFFIX = re.compile(
    r'''(?x)
    (?:
        ["';,.:?)}\]`]+
        |</[A-Za-z][A-Za-z0-9-]*>
        |\.(?:strip|to_string|trim|unwrap_or_default)\(\)
    )*
    '''
)
SAFE_FALLBACK_VALUE = re.compile(
    r'''(?ix)(?:""|''|none|null|undefined)[;,.)}\]]*(?:\s*(?:\#|//).*)?'''
)
ASSIGNMENT_CONTINUATION_OPERATOR = re.compile(
    r"(?is)\s*(\+|\|\||\?\?|or\b)\s*(.*)"
)
ASSIGNMENT_TRAILING_CHUNK = re.compile(
    r'''\[[^\]\r\n]{1,80}\]\([^()\s]+\)'''
    r'''|(?:\#|//)[^\r\n]*'''
    r'''|"[^"]*"|'[^']*'|`[^`]*`|(?:\\.|[^\s])+'''
)
ASSIGNMENT_CONTEXT_IDENTIFIER = re.compile(
    r"[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+"
)
ASSIGNMENT_CONTEXT_ENV_REFERENCE = re.compile(
    r"\$(?:[A-Z][A-Z0-9_]*|\{[A-Z][A-Z0-9_]*\})"
)
ASSIGNMENT_CONTEXT_ENV_PATH = re.compile(
    r"(?:~|\$[A-Z][A-Z0-9_]*|\$\{[A-Z][A-Z0-9_]*\})"
    r"(?:/[A-Za-z0-9_.-]{1,64})+"
)
ASSIGNMENT_CONTEXT_MARKDOWN_LINK = re.compile(
    r"\[([^\]\r\n]{1,80})\]\(([^()\s]+)\)"
)
ASSIGNMENT_CONTEXT_PROSE = re.compile(
    r"[A-Za-z][A-Za-z ,.:;()'/-]{0,160}"
)
ASSIGNMENT_CONTEXT_DOC_PATH = re.compile(
    r"(?:[A-Za-z0-9_.-]{1,64}/)*"
    r"[A-Za-z0-9_.-]{1,64}\."
    r"(?:md|txt|rst|adoc|html?)"
)
ASSIGNMENT_CONTEXT_PATH = re.compile(
    r"(?:(?:/(?:etc|usr|opt|var|tmp|home|Users))"
    r"|(?:(?:\./|\.\./)?(?:docs|src|scripts|crates|packages|skills)))"
    r"(?:/[A-Za-z0-9_.-]{1,64})+"
)
OPEN_COVEN_REPO_ROOT_URL = re.compile(
    r"https?://github\.com/OpenCoven/coven/?"
)
SAFE_URL_FRAGMENT = re.compile(
    r"#(?:L[0-9]{1,7}(?:-L[0-9]{1,7})?"
    r"|[A-Za-z][A-Za-z0-9_.-]{0,63})$"
)
ENTROPY_TOKEN = re.compile(r"\b[A-Za-z0-9_+/@.-]{32,}\b")
ORDERED_ALPHABET_FIXTURES = {
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV",
}
KNOWN_PUBLIC_DOCUMENTATION_TOKENS = {
    "support-dev.discord.com/hc/en-us/articles/6207308062871-What-are-Privileged-Intents",
}
KNOWN_FAKE_PRIVATE_KEY_FIXTURE = re.compile(
    r"-----BEGIN PRIVATE KEY-----\\n(?:fake){3,}\\n-----END PRIVATE KEY-----"
)


def sh(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True, stderr=subprocess.DEVNULL)


def entropy(value: str) -> float:
    if not value:
        return 0.0
    counts = collections.Counter(value)
    return -sum((count / len(value)) * math.log2(count / len(value)) for count in counts.values())


def is_lockfile_path(path: str) -> bool:
    normalized = path.replace("\\", "/")
    return any(normalized == lockfile or normalized.endswith(f"/{lockfile}") for lockfile in LOCKFILE_NAMES)


def is_excluded_path(path: str) -> bool:
    normalized = path.replace("\\", "/")
    return normalized in EXCLUDED_PATHS


def match_is_within(
    inner: re.Match[str], outer: re.Match[str], *, require_end: bool = True
) -> bool:
    if require_end:
        return outer.start() <= inner.start() and inner.end() <= outer.end()
    return outer.start() <= inner.start() < outer.end()


def assignment_is_covered_by_safe_match(
    assignment: re.Match[str], safe_match: re.Match[str]
) -> bool:
    if assignment.start() < safe_match.start() or assignment.start() >= safe_match.end():
        return False
    if assignment.end() <= safe_match.end():
        return True
    suffix = assignment.string[safe_match.end() : assignment.end()]
    return bool(SAFE_ASSIGNMENT_SUFFIX.fullmatch(suffix))


def safe_assignment_continuation_is_syntax(
    path: str, line: str, safe_match: re.Match[str]
) -> bool:
    tail = line[safe_match.end() :]
    immediate_suffix = re.match(r"\S*", tail)
    assert immediate_suffix is not None
    if not SAFE_ASSIGNMENT_SUFFIX.fullmatch(immediate_suffix.group(0)):
        return False
    remainder = tail[immediate_suffix.end() :]
    continuation = ASSIGNMENT_CONTINUATION_OPERATOR.fullmatch(remainder)
    if continuation is not None:
        operator, fallback = continuation.groups()
        return operator != "+" and bool(SAFE_FALLBACK_VALUE.fullmatch(fallback))
    trailing_start = safe_match.end() + immediate_suffix.end()
    return all(
        assignment_trailing_chunk_is_accounted_for(path, line, chunk_match)
        for chunk_match in ASSIGNMENT_TRAILING_CHUNK.finditer(
            line, trailing_start
        )
    )


def has_unsafe_safe_assignment_continuation(path: str, line: str) -> bool:
    return any(
        not safe_assignment_continuation_is_syntax(path, line, safe_match)
        for pattern in (
            GENERIC_ASSIGNMENT_SAFE_VALUE,
            ENV_SECRET_READ,
            ENV_SECRET_REFERENCE,
        )
        for safe_match in pattern.finditer(line)
    )


def is_safe_secret_field_regex_assignment(
    path: str, line: str, assignment: re.Match[str]
) -> bool:
    return any(
        match_is_within(assignment, quoted)
        and bool(
            SAFE_SECRET_FIELD_REGEX_PATTERN.fullmatch(
                quoted.group(0)[1:-1]
            )
        )
        and safe_assignment_continuation_is_syntax(path, line, quoted)
        for quoted in QUOTED_TEXT.finditer(line)
    )


def is_known_safe_generic_assignment(
    path: str, line: str, assignment: re.Match[str]
) -> bool:
    for pattern in (
        GENERIC_ASSIGNMENT_SAFE_VALUE,
        ENV_SECRET_READ,
        ENV_SECRET_REFERENCE,
    ):
        if any(
            assignment_is_covered_by_safe_match(assignment, match)
            and safe_assignment_continuation_is_syntax(path, line, match)
            for match in pattern.finditer(line)
        ):
            return True

    if any(
        assignment_is_covered_by_safe_match(assignment, match)
        and safe_assignment_continuation_is_syntax(path, line, match)
        for match in RESERVED_EXAMPLE_URL.finditer(line)
    ):
        return True

    rust_binding = RUST_LET_CALL_BINDING.match(line)
    if rust_binding and match_is_within(
        assignment, rust_binding, require_end=False
    ):
        return True

    if is_safe_secret_field_regex_assignment(path, line, assignment):
        return True

    return False


def is_known_safe_lockfile_token(
    path: str, line: str, token_match: re.Match[str]
) -> bool:
    if not is_lockfile_path(path):
        return False

    for pattern in (LOCKFILE_INTEGRITY_LINE, LOCKFILE_RESOLVED_LINE):
        if any(
            match_is_within(token_match, safe_match)
            for safe_match in pattern.finditer(line)
        ):
            return True

    stripped = line.strip()
    return bool(
        LOCKFILE_NODE_MODULE_KEY.fullmatch(stripped)
        or LOCKFILE_PACKAGE_KEY.fullmatch(stripped)
        or LOCKFILE_PACKAGE_VERSION_ENTRY.fullmatch(stripped)
    )


def is_local_path_like_token(token: str) -> bool:
    normalized = token.strip("/")
    parts = normalized.split("/")
    if len(parts) < 4:
        return False
    if any(
        len(part) > 64
        or not re.fullmatch(r"[A-Za-z0-9_.-]+", part)
        for part in parts
    ):
        return False
    if parts[0] in {"Users", "home", "private", "var", "tmp", "Volumes"}:
        return True
    if parts[0:2] == ["Documents", "GitHub"]:
        return True
    return ".worktrees" in parts or "worktrees" in parts


def is_public_repo_url_like_token(token: str) -> bool:
    normalized = token.strip("/")
    if not re.fullmatch(
        r"github\.com/OpenCoven/[A-Za-z0-9_.-]+/"
        r"[A-Za-z0-9_+/@.,~%:-]+",
        normalized,
    ):
        return False
    if any(len(part) > 64 for part in normalized.split("/")):
        return False
    if "/blob/" in normalized or "/tree/" in normalized:
        return True
    # Release-artifact URLs (…/releases/download/<tag>/<artifact>): tags and
    # artifact filenames are short path segments; cap each so a token-like
    # blob smuggled into a fake artifact name still trips the entropy rule.
    if "/releases/download/" in normalized:
        return all(len(part) <= 64 for part in normalized.split("/"))
    return False


def is_opencoven_repo_relative_path_token(token: str) -> bool:
    normalized = token.strip("/")
    if not normalized.startswith("OpenCoven/coven/"):
        return False
    # Keep this allowlist tight: only permit path-ish characters (no `+`/`@`) and
    # reject mixed-case-within-a-segment / extremely long segments that look token-like.
    if not re.fullmatch(r"OpenCoven/coven/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+){1,}", normalized):
        return False
    for part in normalized.split("/")[2:]:
        if len(part) > 64:
            return False
        letters = "".join(ch for ch in part if ch.isalpha())
        if letters and not (letters.islower() or letters.isupper()):
            return False
    return True


def is_github_advisory_url_like_token(token: str) -> bool:
    normalized = token.strip("/")
    return bool(
        re.fullmatch(
            r"(?i)github\.com/advisories/"
            r"GHSA-[23456789cfghjmpqrvwx]{4}"
            r"-[23456789cfghjmpqrvwx]{4}"
            r"-[23456789cfghjmpqrvwx]{4}",
            normalized,
        )
    )


def is_github_commit_url_like_token(token: str) -> bool:
    normalized = token.strip("/")
    return bool(re.fullmatch(r"github\.com/[^/\s]+/[^/\s]+/commit/[0-9a-f]{32,64}", normalized))


_GITHUB_ACTION_SHA_REF = re.compile(
    r"^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+@[0-9a-f]{40}$"
)


def is_github_action_sha_ref_token(token: str) -> bool:
    """Whether `token` is a GitHub Actions `uses:` reference pinned to a 40-char
    commit SHA, e.g. ``actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd``.
    SHA-pinned refs are an OpenSSF best practice (they prevent action authors
    from silently moving a version tag onto a malicious commit) but the trailing
    40-hex SHA otherwise pushes the workflow line over the entropy threshold.
    """
    return bool(_GITHUB_ACTION_SHA_REF.match(token))


_MACOS_LIBRARY_PATH_TOKEN = re.compile(
    r"(?:Users/[A-Za-z0-9_.-]+/)?Library/"
    r"(?:LaunchAgents|LaunchDaemons|Preferences|Caches|Logs)"
    r"(?:/[A-Za-z0-9_.-]+)+"
)


def is_macos_library_path_token(token: str) -> bool:
    """Whether `token` is a macOS `Library/...` well-known path such as
    ``Library/LaunchAgents/dev.opencoven.hub.plist``. These show up in
    launchd/service documentation and are never credentials, but reverse-DNS
    plist names push the token over the entropy threshold. The subdirectory
    list is a closed set and every segment is capped so a token-like blob
    appended to a path still trips the entropy rule.
    """
    if not _MACOS_LIBRARY_PATH_TOKEN.fullmatch(token):
        return False
    return all(len(part) <= 64 for part in token.split("/"))


_APPLE_DTD_URL_TOKEN = re.compile(r"www\.apple\.com/DTDs/[A-Za-z0-9_.-]{1,64}\.dtd")


def is_apple_dtd_url_token(token: str) -> bool:
    """Whether `token` is the Apple property-list DTD system identifier
    (``www.apple.com/DTDs/PropertyList-1.0.dtd``) that appears in every plist
    XML doctype. It is public boilerplate, not a secret, but the mixed-case
    host/path combination exceeds the entropy threshold.
    """
    return bool(_APPLE_DTD_URL_TOKEN.fullmatch(token))


def is_ordered_alphabet_fixture_token(token: str) -> bool:
    return token in ORDERED_ALPHABET_FIXTURES


def is_discord_support_article_token(token: str) -> bool:
    return token in KNOWN_PUBLIC_DOCUMENTATION_TOKENS


def is_known_fake_private_key_match(
    line: str, private_key_match: re.Match[str]
) -> bool:
    return any(
        match_is_within(private_key_match, fixture_match)
        for fixture_match in KNOWN_FAKE_PRIVATE_KEY_FIXTURE.finditer(line)
    )


def is_programming_identifier_token(token: str) -> bool:
    """Whether `token` looks like a snake_case / SCREAMING_SNAKE_CASE identifier
    (optionally suffixed with a `.method` call), a workflow-style relative file
    path (e.g. `github/workflows/release-npm.yml`), or a Rust target triple
    (`target/x86_64-pc-windows-msvc/release`). None of these shapes are ever a
    credential.

    The guard keeps the rest of the high-entropy path strict by requiring the
    token to be composed only of `[A-Za-z0-9_./-]`, to be split into at least
    three segments by `_`/`.`/`/`/`-`, for at least one segment to contain
    letters (so a token of pure-digit segments still trips the entropy rule),
    and for every letter-bearing segment to be uniformly single-case while
    allowing digits inside those identifier/path/triple segments. Real API tokens
    lack separators, mix case within a segment, or contain base64-only characters
    such as `+`/`=`, so they continue to fail at least one of these checks and
    trip the entropy rule as before.
    """
    if not re.fullmatch(r"[A-Za-z0-9_./-]+", token):
        return False
    segments = [seg for seg in re.split(r"[._/-]", token) if seg]
    if len(segments) < 3:
        return False
    has_letter_segment = False
    for seg in segments:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9]*", seg):
            return False
        if len(seg) > 24:
            return False
        letters = "".join(ch for ch in seg if ch.isalpha())
        if letters:
            has_letter_segment = True
            if not (letters.islower() or letters.isupper()):
                return False
    return has_letter_segment


def is_assignment_context_identifier(chunk: str) -> bool:
    if not ASSIGNMENT_CONTEXT_IDENTIFIER.fullmatch(chunk):
        return False
    for part in chunk.split("_"):
        if len(part) > 32:
            return False
    return True


def is_known_safe_assignment_context_url(url: str) -> bool:
    fragment = SAFE_URL_FRAGMENT.search(url)
    base_url = url[: fragment.start()] if fragment else url
    without_scheme = re.sub(r"(?i)^https?://", "", base_url)
    return bool(
        RESERVED_EXAMPLE_URL.fullmatch(base_url)
        or OPEN_COVEN_REPO_ROOT_URL.fullmatch(base_url)
        or is_public_repo_url_like_token(without_scheme)
        or is_github_advisory_url_like_token(without_scheme)
        or is_github_commit_url_like_token(without_scheme)
        or is_apple_dtd_url_token(without_scheme)
        or is_discord_support_article_token(without_scheme)
    )


def is_known_safe_markdown_target(target: str) -> bool:
    if is_known_safe_assignment_context_url(target):
        return True
    if SAFE_URL_FRAGMENT.fullmatch(target):
        return True
    fragment = SAFE_URL_FRAGMENT.search(target)
    base_target = target[: fragment.start()] if fragment else target
    return bool(
        ASSIGNMENT_CONTEXT_PATH.fullmatch(base_target)
        or ASSIGNMENT_CONTEXT_DOC_PATH.fullmatch(base_target)
    )


def is_likely_passphrase_prose(text: str) -> bool:
    words = re.findall(r"[A-Za-z]+", text)
    if len(words) == 1:
        return len(words[0]) >= 20
    return len(words) >= 4 and all(
        len(word) >= 4 and word.islower() for word in words
    )


def is_known_safe_assignment_context_prose(
    text: str, *, allow_empty: bool = False
) -> bool:
    normalized = text.strip()
    if not normalized:
        return allow_empty
    return bool(
        ASSIGNMENT_CONTEXT_PROSE.fullmatch(normalized)
        and not is_likely_passphrase_prose(normalized)
    )


def normalize_assignment_context_chunk(chunk: str) -> str:
    core = chunk.rstrip(".,;:")
    wrapper_pairs = (
        ('"', '"'),
        ("'", "'"),
        ("`", "`"),
        ("(", ")"),
        ("[", "]"),
        ("{", "}"),
        ("<", ">"),
    )
    while len(core) >= 2:
        for opener, closer in wrapper_pairs:
            if core.startswith(opener) and core.endswith(closer):
                core = core[1:-1].strip().rstrip(".,;:")
                break
        else:
            return core
    return core


def is_known_safe_assignment_context_chunk(chunk: str) -> bool:
    markdown_link = ASSIGNMENT_CONTEXT_MARKDOWN_LINK.fullmatch(chunk)
    if markdown_link:
        label, target = markdown_link.groups()
        if is_known_safe_assignment_context_prose(
            label
        ) and is_known_safe_markdown_target(target):
            return True
    if chunk.startswith("#"):
        return is_known_safe_assignment_context_prose(
            chunk[1:], allow_empty=True
        )
    if chunk.startswith("//"):
        return is_known_safe_assignment_context_prose(
            chunk[2:], allow_empty=True
        )
    contains_quote = any(quote in chunk for quote in "\"'`")
    core = normalize_assignment_context_chunk(chunk)
    if not core:
        return True
    if (
        is_known_safe_assignment_context_url(core)
        or ASSIGNMENT_CONTEXT_PATH.fullmatch(core)
        or is_assignment_context_identifier(core)
        or ASSIGNMENT_CONTEXT_ENV_REFERENCE.fullmatch(core)
        or ASSIGNMENT_CONTEXT_ENV_PATH.fullmatch(core)
        or is_local_path_like_token(core)
        or is_opencoven_repo_relative_path_token(core)
        or is_github_action_sha_ref_token(core)
        or is_macos_library_path_token(core)
        or is_ordered_alphabet_fixture_token(core)
    ):
        return True
    return not contains_quote and len(core) < 12


def is_known_safe_entropy_token(
    path: str, line: str, token_match: re.Match[str]
) -> bool:
    token = token_match.group(0)
    return bool(
        re.fullmatch(r"[0-9a-f]{32,64}", token)
        or is_known_safe_lockfile_token(path, line, token_match)
        or is_local_path_like_token(token)
        or is_public_repo_url_like_token(token)
        or is_opencoven_repo_relative_path_token(token)
        or is_github_advisory_url_like_token(token)
        or is_github_commit_url_like_token(token)
        or is_github_action_sha_ref_token(token)
        or is_macos_library_path_token(token)
        or is_apple_dtd_url_token(token)
        or is_ordered_alphabet_fixture_token(token)
        or is_discord_support_article_token(token)
        or is_programming_identifier_token(token)
    )


def is_high_entropy_finding(
    path: str, line: str, token_match: re.Match[str]
) -> bool:
    token = token_match.group(0)
    return not is_known_safe_entropy_token(
        path, line, token_match
    ) and entropy(token) >= 4.3


def assignment_trailing_chunk_is_accounted_for(
    path: str, line: str, chunk_match: re.Match[str]
) -> bool:
    chunk = chunk_match.group(0)
    if is_known_safe_assignment_context_chunk(chunk):
        return True
    if any(
        name != "generic_assignment" and pattern.search(chunk)
        for name, pattern in SECRET_RULES
    ):
        return True
    return any(
        is_high_entropy_finding(path, line, token_match)
        for token_match in ENTROPY_TOKEN.finditer(
            line, chunk_match.start(), chunk_match.end()
        )
    )


def scan_text(text: str, path: str) -> list[tuple[str, int, str]]:
    hits: list[tuple[str, int, str]] = []
    for line_number, line in enumerate(text.splitlines(), 1):
        for name, pattern in SECRET_RULES:
            matches = list(pattern.finditer(line))
            if name == "private_key":
                matches = [
                    match
                    for match in matches
                    if not is_known_fake_private_key_match(line, match)
                ]
            if name == "generic_assignment":
                unsafe_continuation = has_unsafe_safe_assignment_continuation(
                    path, line
                )
                if not matches:
                    if unsafe_continuation:
                        hits.append((path, line_number, name))
                    continue
                if not unsafe_continuation and all(
                    is_known_safe_generic_assignment(path, line, match)
                    for match in matches
                ):
                    continue
                hits.append((path, line_number, name))
                continue
            if not matches:
                continue
            hits.append((path, line_number, name))
        for match in ENTROPY_TOKEN.finditer(line):
            if is_high_entropy_finding(path, line, match):
                hits.append((path, line_number, "high_entropy"))
    return hits


def scan_bytes(data: bytes, path: str) -> list[tuple[str, int, str]]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return []
    return scan_text(text, path)


def tracked_file_hits() -> list[tuple[str, int, str]]:
    files = sh("git", "ls-files").splitlines()
    hits: list[tuple[str, int, str]] = []
    for rel in files:
        if is_excluded_path(rel):
            continue
        path = ROOT / rel
        if any(part in EXCLUDED_PARTS for part in path.relative_to(ROOT).parts):
            continue
        if path.is_file():
            hits.extend(scan_bytes(path.read_bytes(), rel))
    return hits


def history_blob_hits(ref: str = "HEAD") -> list[tuple[str, str, int, str]]:
    rows = sh("git", "rev-list", "--objects", ref).splitlines()
    hits: list[tuple[str, str, int, str]] = []
    seen: set[str] = set()
    for row in rows:
        parts = row.split(" ", 1)
        sha = parts[0]
        rel = parts[1] if len(parts) > 1 else "<unknown>"
        if is_excluded_path(rel):
            continue
        if sha in seen:
            continue
        seen.add(sha)
        if any(part in EXCLUDED_PARTS for part in pathlib.PurePosixPath(rel).parts):
            continue
        if sh("git", "cat-file", "-t", sha).strip() != "blob":
            continue
        data = subprocess.check_output(["git", "cat-file", "-p", sha], cwd=ROOT)
        for path, line, rule in scan_bytes(data, rel):
            hits.append((sha[:12], path, line, rule))
    return hits


def main() -> int:
    current = tracked_file_hits()
    history = history_blob_hits()
    if current or history:
        print("Secret guard found possible sensitive values. Values are intentionally redacted.", file=sys.stderr)
        for path, line, rule in current:
            print(f"current:{path}:{line}:{rule}", file=sys.stderr)
        if history:
            print(f"history findings: {len(history)} entries (details redacted)", file=sys.stderr)
        return 1
    print("Secret guard passed: no current-tree or history findings.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
