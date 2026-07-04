#!/bin/sh

set -eu

die() {
    printf 'miri-test.sh: %s\n' "$*" >&2
    exit 1
}

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd -P)
repo_root=$(CDPATH= cd "$script_dir/.." && pwd -P)

command -v cargo >/dev/null 2>&1 || die "cargo not found"
command -v make >/dev/null 2>&1 || die "make not found"

cd "$repo_root"

printf '%s\n' "Building crate..."
cargo build

target_dir=$(
    cargo metadata --format-version=1 --no-deps |
        sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'
)

[ -n "$target_dir" ] || die "could not read target_directory from cargo metadata"
[ -d "$target_dir" ] || die "target directory does not exist: $target_dir"

work_dir="$target_dir/miri-libffi-build"
libffi_so=

if [ -d "$work_dir" ]; then
    libffi_so=$(
        find "$work_dir" -path '*/.libs/libffi.so' -printf '%T@ %p\n' |
            sort -nr |
            sed -n '1s/^[^ ]* //p'
    )
fi

if [ -n "$libffi_so" ]; then
    printf 'Using existing libffi: %s\n' "$libffi_so"
else
    libffi_src=$(
        find "$target_dir" -type d -name libffi-build -printf '%T@ %p\n' |
            sort -nr |
            sed -n '1s/^[^ ]* //p'
    )

    [ -n "$libffi_src" ] || die "could not find libffi-build under $target_dir"
    [ -f "$libffi_src/configure" ] || die "configure not found in $libffi_src"

    printf 'Using libffi source: %s\n' "$libffi_src"
    printf 'Preparing libffi work directory: %s\n' "$work_dir"

    rm -rf "$work_dir"
    mkdir -p "$work_dir"
    cp -R "$libffi_src/." "$work_dir"

    printf '%s\n' "Configuring libffi..."
    (
        cd "$work_dir"
        sh ./configure --disable-docs
    )

    printf '%s\n' "Building libffi..."
    (
        cd "$work_dir"
        make
    )

    libffi_so=$(
        find "$work_dir" -path '*/.libs/libffi.so' -printf '%T@ %p\n' |
            sort -nr |
            sed -n '1s/^[^ ]* //p'
    )
fi

[ -n "$libffi_so" ] || die "could not find libffi.so under $work_dir"

printf 'Running Miri with libffi: %s\n' "$libffi_so"
MIRIFLAGS="${MIRIFLAGS:+$MIRIFLAGS }-Zmiri-native-lib=$libffi_so" cargo +nightly miri test --lib
