#!/bin/bash
set -e

FILE="blkio-info-file"

dd if=/dev/urandom of="${FILE}" bs=1M count=2 conv=fsync

OUT=$(./examples/blkio-info io_uring path="${FILE}" capacity)
if [ "$OUT" != "2097152" ]; then
    exit 1
fi

OUT=$(./examples/blkio-info io_uring path="${FILE}" read-only)
if [ "$OUT" != "false" ]; then
    exit 1
fi

OUT=$(./examples/blkio-info io_uring path="${FILE}" path)
if [ "$OUT" != "${FILE}" ]; then
    exit 1
fi

rm "${FILE}"
