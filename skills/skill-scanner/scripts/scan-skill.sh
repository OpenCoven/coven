#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: scan-skill.sh /absolute/path/to/skill [report-dir]" >&2
  exit 64
fi

SKILL_DIR="$1"
SKILL_NAME="$(basename "$SKILL_DIR")"
REPORT_DIR="${2:-$(dirname "$SKILL_DIR")/.scan-reports/$SKILL_NAME}"
SDK_CLI="/Users/buns/Documents/GitHub/BunsDev/codeql-sdk/dist/cli.js"
PACKAGER="$HOME/.nvm/versions/node/v24.13.0/lib/node_modules/openclaw/skills/skill-creator/scripts/package_skill.py"
DIST_DIR="${REPORT_DIR}/dist"

if [[ ! -f "$SKILL_DIR/SKILL.md" ]]; then
  echo "Missing SKILL.md: $SKILL_DIR" >&2
  exit 65
fi

if [[ ! -f "$SDK_CLI" ]]; then
  echo "Missing codeql-sdk CLI: $SDK_CLI" >&2
  exit 66
fi

mkdir -p "$REPORT_DIR" "$DIST_DIR"

if ! command -v codeql >/dev/null 2>&1 && [[ -z "${CODEQL_PATH:-}" ]]; then
  cat >&2 <<'ERR'
CodeQL CLI not found. codeql-sdk is installed, but its runtime dependency is missing.
Install on macOS with:
  brew install --cask codeql
Or set CODEQL_PATH=/path/to/codeql and rerun.
ERR
  exit 67
fi

openclaw skills check >/tmp/openclaw-skills-check.log
python3 "$PACKAGER" "$SKILL_DIR" "$DIST_DIR" >"$REPORT_DIR/package-validator.log"
set +e
node "$SDK_CLI" audit "$SKILL_DIR" --format json --output "$REPORT_DIR/codeql-sdk-results.json" --fail-on-high >"$REPORT_DIR/codeql-sdk-json.log" 2>&1
JSON_STATUS=$?
set -e

if [[ $JSON_STATUS -ne 0 ]]; then
  if grep -qi "CodeQL did not detect any code" "$REPORT_DIR/codeql-sdk-json.log"; then
    cat >"$REPORT_DIR/codeql-sdk-not-applicable.txt" <<'EOF'
CodeQL SDK scan was attempted but is not applicable: CodeQL did not detect any analyzable JavaScript/TypeScript/HTML/YAML/etc source for its configured JavaScript database.
This is common for markdown-only skills. Treat this as not-applicable, not as a security pass.
EOF
    echo "CodeQL SDK scan not applicable: no analyzable source code detected"
    echo "Report dir: $REPORT_DIR"
    exit 0
  fi
  cat "$REPORT_DIR/codeql-sdk-json.log" >&2
  exit $JSON_STATUS
fi

node "$SDK_CLI" audit "$SKILL_DIR" --format sarif --output "$REPORT_DIR/codeql-sdk-results.sarif"

echo "Skill scan complete"
echo "Report dir: $REPORT_DIR"
