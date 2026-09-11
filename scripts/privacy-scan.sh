#!/usr/bin/env bash
# Pre-release / pre-push privacy scan.
#
# Checks, without printing any sensitive *value*:
#   1. Git author/committer identity of commits about to be pushed (must be anonymous).
#   2. Filenames of staged/changed/untracked files for secrets / diagnostic / private-data patterns.
#   3. Content of text blobs being committed for high-confidence secret patterns.
#
# Exit 0 = clean. Exit 1 = finding (release/push must stop). Only the *kind* of
# finding and its location (file path / commit hash) is reported — never the
# matched secret itself.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

FAIL=0
note() { printf '%s\n' "$1"; }
finding() { printf 'PRIVACY-FINDING: %s\n' "$1"; FAIL=1; }

ALLOWED_NAME="tkseraph"
ALLOWED_EMAIL="tkseraph@users.noreply.github.com"

# --- 1. Identity of HEAD + any unpushed commits -----------------------------
# Range: upstream..HEAD if upstream exists, else just HEAD.
if git rev-parse --abbrev-ref --symbolic-full-name '@{u}' >/dev/null 2>&1; then
  RANGE="@{u}..HEAD"
else
  RANGE="HEAD"
fi
while IFS= read -r line; do
  # line = "<hash> <name> <email> <cname> <cemail>"
  h="${line%% *}"; rest="${line#* }"
  an="${rest%% *}"; rest="${rest#* }"
  ae="${rest%% *}"; rest="${rest#* }"
  ce="${rest##* }"
  if [ "$an" != "$ALLOWED_NAME" ] || [ "$ae" != "$ALLOWED_EMAIL" ] || [ "$ce" != "$ALLOWED_EMAIL" ]; then
    finding "commit $h has non-anonymous author/committer identity"
  fi
done < <(git log --format='%H %an %ae %cn %ce' $RANGE 2>/dev/null || true)

# --- 2. Filenames ------------------------------------------------------------
# Combine staged + unstaged + untracked (exclude ignored). bash 3.2 compatible.
FILES_FILE="$(mktemp -t privacy-scan-files)"
trap 'rm -f "$FILES_FILE"' EXIT
{ git diff --cached --name-only; git diff --name-only; git ls-files --others --exclude-standard; } | sort -u > "$FILES_FILE"
BAD_NAME_RE='(^|/)(\.env|\.env\.[^/]*|.*\.(p12|pem|key|mobileprovision|certSigningRequest)|id_rsa|id_ed25519|credentials|.*\.(db|db-wal|db-shm))$'
while IFS= read -r f; do
  [ -z "$f" ] && continue
  if printf '%s' "$f" | grep -Eq "$BAD_NAME_RE"; then
    finding "filename matches secret/private-data pattern: $f"
  fi
done < "$FILES_FILE"

# --- 3. Content scan of committed text blobs ---------------------------------
# Only scan files git considers text and that are in the index/HEAD diff.
CONTENT_RE='(BEGIN [A-Z ]*PRIVATE KEY|AKIA[0-9A-Z]{16}|-----BEGIN RSA|xox[baprs]-|ghp_[A-Za-z0-9]{36}|AIza[0-9A-Za-z_-]{35})'
FILE_COUNT=0
while IFS= read -r f; do
  [ -z "$f" ] && continue
  [ -f "$f" ] || continue
  # The scanner lists its own patterns; never flag itself.
  [ "$f" = "scripts/privacy-scan.sh" ] && continue
  FILE_COUNT=$((FILE_COUNT + 1))
  # skip binary
  if file -b --mime "$f" 2>/dev/null | grep -q 'charset=binary'; then continue; fi
  if grep -Eq "$CONTENT_RE" "$f" 2>/dev/null; then
    finding "high-confidence secret pattern in: $f"
  fi
done < "$FILES_FILE"

if [ "$FAIL" -eq 1 ]; then
  note "Privacy scan FAILED — resolve findings above before push/release."
  exit 1
fi
note "Privacy scan clean ($FILE_COUNT files checked)."
exit 0
