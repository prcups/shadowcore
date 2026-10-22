#!/bin/bash
set -e

FILE="blkio-bench-file"
RUNTIME=1

dd if=/dev/urandom of="${FILE}" bs=1M count=2 conv=fsync

OUT=$(./examples/blkio-bench io_uring path="${FILE}" --runtime="${RUNTIME}" \
    | grep kiops | wc -l)
if [ "$OUT" != "2" ]; then
    exit 1
fi

OUT=$(./examples/blkio-bench io_uring path="${FILE}" --runtime="${RUNTIME}" \
    --poll \
    | grep kiops | wc -l)
if [ "$OUT" != "2" ]; then
    exit 1
fi

OUT=$(./examples/blkio-bench io_uring path="${FILE}" --runtime="${RUNTIME}" --poll \
    --readwrite=write \
    | grep kiops | wc -l)
if [ "$OUT" != "2" ]; then
    exit 1
fi

rm "${FILE}"
