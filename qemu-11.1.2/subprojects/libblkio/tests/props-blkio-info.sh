#!/bin/bash
set -eu

if [[ "$#" -le 2 || "$1" != "--path" || $(($# % 2)) -ne 0 ]]; then
    echo "usage: $0 --path /dev/ng0n1 <property> <value> <property> <value>..."
    exit 2
fi

path="$2"
shift 2

test_property() {
    OUT=$(./examples/blkio-info nvme-io_uring path="${path}" $1)
    if [ "$OUT" != "$2" ]; then
        echo "Got '${OUT}', expected '$2' for '$1' property."
        exit 1
    fi
}

while [ "$#" -gt 0 ]; do
    test_property "$1" "$2"
    shift 2
done

