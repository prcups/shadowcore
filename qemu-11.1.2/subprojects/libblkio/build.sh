#!/bin/sh
set -e

cd $(dirname $0)
src=$(pwd)
cd - >/dev/null

if [ ! -d build ]; then
    # configure a debug build (unoptimized and with debug info) for development
    meson setup build --buildtype=debug
else
    # If using containerized build we must reconfigure inside the container.
    meson setup --reconfigure build --buildtype=debug
fi

if command -v rustfmt >/dev/null; then
    cargo fmt --all -- --check
fi

meson compile -C build

wrapper=
if command -v kcov >/dev/null; then
    rm -rf build/kcov-runs
    mkdir build/kcov-runs

    wrapper="--wrapper \"$src/kcov-wrapper.sh\""
fi

if ! meson test \
    --suite generic --suite io_uring+parallel --suite examples \
    -C build $wrapper
then
    # meson test output is lacking, show the full test log on failure
    if command -v kcov >/dev/null; then
        cat build/meson-logs/testlog-kcov-wrapper.sh.txt
    else
        cat build/meson-logs/testlog.txt
    fi
    exit 1
fi

if command -v kcov >/dev/null; then
    rm -rf build/kcov-output
    kcov --merge build/kcov-output build/kcov-runs/*

    # Print the overall code coverage percentage so GitLab CI can parse it
    grep '^\s*"percent_covered"' build/kcov-output/kcov-merged/coverage.json
fi
