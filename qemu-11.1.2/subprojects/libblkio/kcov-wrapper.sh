#!/bin/sh
test_name=$(basename $1_$3)
exec kcov --exclude-path=/usr/include,/usr/src/libblkio/.cargo,/usr/src/libblkio/tests kcov-runs/$test_name "$@"
