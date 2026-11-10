#!/bin/sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
work=${CCVIEW_SHOT_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/ccview-shots.XXXXXX")}
demo="$work/demo"
rm -rf "$demo"
mkdir -p "$demo"

export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --release --manifest-path "$repo/Cargo.toml"
CARGO_TARGET_DIR="$work/capture-target" cargo build --release --offline --manifest-path "$here/capture/Cargo.toml"

cleanup() {
    if [ -f "$demo/pids" ]; then
        xargs kill < "$demo/pids" 2>/dev/null || true
    fi
}
trap cleanup EXIT

python3 "$here/build_demo.py" "$demo"
cp "$here/fake_harness.py" "$demo/fake_harness.py"
"$work/capture-target/release/capture" "$repo/target/release/ccview-rs" "$demo" "$here"
rm -f "$here"/*.txt
