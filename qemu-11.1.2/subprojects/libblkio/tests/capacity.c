// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "capacity-XXXXXX";

int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    uint64_t capacity;

    parse_generic_opts(&opts, argc, argv);

    create(&b, &opts, filename, TEST_FILE_SIZE);

    err(blkio_get_uint64(b, "capacity", &capacity), -ENODEV);
    err(blkio_set_uint64(b, "capacity", 4096), -EACCES);

    ok(blkio_connect(b));

    if (!opts.path) {
        // file was created by create()
        ok(blkio_get_uint64(b, "capacity", &capacity));
        assert(capacity == TEST_FILE_SIZE);
    }

    err(blkio_set_uint64(b, "capacity", 4096), -EACCES);

    if (!opts.path) {
        // file was created by create()
        ok(blkio_get_uint64(b, "capacity", &capacity));
        assert(capacity == TEST_FILE_SIZE);
    }

    blkio_destroy(&b);
    assert(!b);

    return 0;
}
