#!/usr/bin/env bash
# Unit tests for scripts/check_submissions.sh's path/login rules, each
# driven against a throwaway git repository holding fake, minimal paths.
#
# `cargo xtask audit report` itself is not exercised for real here — a fake
# `cargo` shim placed ahead of the real one on PATH stands in, controlled by
# $FAKE_CARGO_EXIT, so these cases stay fast (no compiled xtask needed) and
# test only what this script's own control flow does with that command's
# exit code. The command's real behavior (parsing an actual verdict file)
# is xtask's own concern and is covered by its Rust tests.
#
# Run directly: scripts/tests/check_submissions_test.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/../check_submissions.sh"

failures=0
case_output="$(mktemp)"
trap 'rm -f "$case_output"' EXIT

setup_fake_cargo() {
    local bindir="$1"
    mkdir -p "$bindir"
    cat >"$bindir/cargo" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = "xtask" ]; then
    exit "${FAKE_CARGO_EXIT:-0}"
fi
exit 0
EOF
    chmod +x "$bindir/cargo"
}

new_repo() {
    local dir="$1"
    git init -q "$dir"
    (
        cd "$dir"
        git config user.email test@example.com
        git config user.name test
        echo placeholder >README.md
        git add README.md
        git commit -q -m base
    )
}

run_case() {
    local name="$1"
    shift
    if "$@" >"$case_output" 2>&1; then
        echo "ok: $name"
    else
        echo "FAIL: $name"
        sed 's/^/    /' "$case_output"
        failures=$((failures + 1))
    fi
}

# Every case_* function below follows the same shape: build a throwaway repo
# under a fresh $tmp, run the script against it inside a subshell (so `set
# -e` cannot abort the whole test run on an expected non-zero exit), record
# pass/fail into $ok, clean up $tmp unconditionally, then return $ok — never
# a `trap ... RETURN`, which in bash fires on *every* function return for
# the rest of the script, not just the one that set it, and reliably breaks
# on the second case's cleanup.

case_valid_submission_passes() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/alice
        printf '[meta]\nseed = 7\n' >audit/submissions/alice/7.toml
        echo report >audit/submissions/alice/7-report.txt
        git add audit/submissions
        git commit -q -m submission
        PATH="$bindir:$PATH" "$script" "$base" alice
    ); then
        ok=0
    else
        ok=1
    fi
    rm -rf "$tmp"
    return "$ok"
}

case_bad_path_fails() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/alice
        echo x >audit/submissions/alice/notes.md
        git add audit/submissions
        git commit -q -m bad
        PATH="$bindir:$PATH" "$script" "$base" alice
    ); then
        ok=1 # the script must have rejected this path; success is failure
    else
        ok=0
    fi
    rm -rf "$tmp"
    return "$ok"
}

case_wrong_login_folder_fails() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/bob
        printf '[meta]\nseed = 7\n' >audit/submissions/bob/7.toml
        git add audit/submissions
        git commit -q -m submission
        PATH="$bindir:$PATH" "$script" "$base" alice
    ); then
        ok=1
    else
        ok=0
    fi
    rm -rf "$tmp"
    return "$ok"
}

case_maintainer_login_bypasses_folder_check() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/bob
        printf '[meta]\nseed = 7\n' >audit/submissions/bob/7.toml
        git add audit/submissions
        git commit -q -m submission
        PATH="$bindir:$PATH" "$script" "$base" sadigaxund
    ); then
        ok=0
    else
        ok=1
    fi
    rm -rf "$tmp"
    return "$ok"
}

case_a_toml_that_fails_report_fails() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/alice
        echo 'not valid toml' >audit/submissions/alice/7.toml
        git add audit/submissions
        git commit -q -m submission
        FAKE_CARGO_EXIT=1 PATH="$bindir:$PATH" "$script" "$base" alice
    ); then
        ok=1
    else
        ok=0
    fi
    rm -rf "$tmp"
    return "$ok"
}

case_sadigaxunds_legacy_seed5_shape_passes_for_sadigaxund() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/sadigaxund/5
        echo 'help text' >audit/submissions/sadigaxund/5/ar.txt
        git add audit/submissions
        git commit -q -m legacy-capture
        PATH="$bindir:$PATH" "$script" "$base" sadigaxund
    ); then
        ok=0
    else
        ok=1
    fi
    rm -rf "$tmp"
    return "$ok"
}

case_same_seed5_shape_under_another_login_still_fails() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/alice/5
        echo 'help text' >audit/submissions/alice/5/ar.txt
        git add audit/submissions
        git commit -q -m legacy-capture
        PATH="$bindir:$PATH" "$script" "$base" alice
    ); then
        ok=1
    else
        ok=0
    fi
    rm -rf "$tmp"
    return "$ok"
}

# The skip is keyed on the author, not the folder alone: a non-maintainer
# author cannot dodge the checks just by writing into sadigaxund's folder.
case_sadigaxund_folder_still_fails_for_a_different_author() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        mkdir -p audit/submissions/sadigaxund/5
        echo 'help text' >audit/submissions/sadigaxund/5/ar.txt
        git add audit/submissions
        git commit -q -m legacy-capture
        PATH="$bindir:$PATH" "$script" "$base" alice
    ); then
        ok=1
    else
        ok=0
    fi
    rm -rf "$tmp"
    return "$ok"
}

case_no_changes_under_submissions_passes() {
    local tmp bindir base ok
    tmp="$(mktemp -d)"
    new_repo "$tmp/repo"
    bindir="$tmp/bin"
    setup_fake_cargo "$bindir"
    if (
        cd "$tmp/repo"
        base="$(git rev-parse HEAD)"
        echo more >>README.md
        git commit -q -am unrelated
        PATH="$bindir:$PATH" "$script" "$base" alice
    ); then
        ok=0
    else
        ok=1
    fi
    rm -rf "$tmp"
    return "$ok"
}

run_case "a well-formed submission under the author's own login passes" case_valid_submission_passes
run_case "a path that does not match <login>/<seed>.toml or -report.txt fails" case_bad_path_fails
run_case "a folder that does not match the PR author's login fails" case_wrong_login_folder_fails
run_case "the maintainer login bypasses the folder/login match" case_maintainer_login_bypasses_folder_check
run_case "a .toml that fails cargo xtask audit report fails" case_a_toml_that_fails_report_fails
run_case "a PR with no changes under audit/submissions passes trivially" case_no_changes_under_submissions_passes
run_case "sadigaxund's legacy seed-5 capture shape passes for sadigaxund" case_sadigaxunds_legacy_seed5_shape_passes_for_sadigaxund
run_case "the same seed-5 shape under another login still fails" case_same_seed5_shape_under_another_login_still_fails
run_case "the sadigaxund folder still fails for a different author" case_sadigaxund_folder_still_fails_for_a_different_author

if [ "$failures" -ne 0 ]; then
    echo "$failures case(s) failed"
    exit 1
fi
echo "all check_submissions.sh cases passed"
