#!/bin/bash

set -e

SRC_FILE="blkio-copy-file-src"
DST_FILE="blkio-copy-file-dst"

dd if=/dev/urandom of="${SRC_FILE}" bs=1M count=2 conv=fsync
dd if=/dev/urandom of="${DST_FILE}" bs=1M count=2 conv=fsync

./examples/blkio-copy --input-blkio=io_uring path="${SRC_FILE}" \
    --output-file="${DST_FILE}"
if ! cmp -s "${SRC_FILE}" "${DST_FILE}"; then
    echo "files are different"
    exit 1
fi

dd if=/dev/urandom of="${SRC_FILE}" bs=1M count=2 conv=fsync
dd if=/dev/urandom of="${DST_FILE}" bs=1M count=2 conv=fsync

./examples/blkio-copy --input-blkio=io_uring path="${SRC_FILE}" \
    --output-blkio=io_uring path="${DST_FILE}"
if ! cmp -s "${SRC_FILE}" "${DST_FILE}"; then
    echo "files are different"
    exit 1
fi

dd if=/dev/urandom of="${SRC_FILE}" bs=1M count=2 conv=fsync
dd if=/dev/urandom of="${DST_FILE}" bs=1M count=2 conv=fsync

./examples/blkio-copy --input-file="${SRC_FILE}" \
    --output-blkio=io_uring path="${DST_FILE}"
if ! cmp -s "${SRC_FILE}" "${DST_FILE}"; then
    echo "files are different"
    exit 1
fi

rm "${SRC_FILE}" "${DST_FILE}"
