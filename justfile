# herdr-review dev tasks — run `just <task>` (https://github.com/casey/just)

# default: list tasks
default:
    @just --list

# format the code
fmt:
    cargo fmt --all

# check formatting (CI parity)
fmt-check:
    cargo fmt --all --check

# lint with clippy, warnings as errors (CI parity)
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# run the test suite, cut off from the herdr it may run inside: a test that reaches `herdr`
# gets a binary that refuses, never a live agent pane
test:
    env -u HERDR_WORKSPACE_ID -u HERDR_PANE_ID HERDR_BIN_PATH=false cargo test --all-features

# build (debug)
build:
    cargo build

# run reviewr in the current repo
run:
    cargo run

# build release and install the binary into bin/ for `herdr plugin link`
install:
    cargo build --release
    mkdir -p bin
    ./scripts/swap-binary.sh target/release/herdr-reviewr bin/herdr-reviewr

# build release and swap it into the GitHub-installed plugin for local QA (docs/qa-install.md)
qa-install:
    cargo build --release
    ./scripts/qa-install.sh

# restore the released binary and manifest the last `just qa-install` replaced
qa-restore:
    #!/usr/bin/env sh
    set -eu
    root="$(ls -d "$HOME"/.config/herdr/plugins/github/persiyanov.reviewr-* | head -1)"
    ./scripts/swap-binary.sh "$root/bin/herdr-reviewr.release-backup" "$root/bin/herdr-reviewr"
    echo "restored release binary at $root/bin/herdr-reviewr"
    if [ -f "$root/herdr-plugin.toml.release-backup" ]; then
        cp "$root/herdr-plugin.toml.release-backup" "$root/herdr-plugin.toml.staging"
        mv "$root/herdr-plugin.toml.staging" "$root/herdr-plugin.toml"
        echo "restored release manifest at $root/herdr-plugin.toml"
    fi

# PTY smoke test of the editor path against a real release binary
smoke-edit:
    cargo build --release
    python3 scripts/smoke_edit_file.py --binary target/release/herdr-reviewr

# everything CI runs, locally
ci: fmt-check lint test
    cargo build --release
