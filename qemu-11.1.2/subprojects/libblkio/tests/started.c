// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "started-XXXXXX";

int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;

    parse_generic_opts(&opts, argc, argv);

    create(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_connect(b));

    /* Calling it twice works */
    ok(blkio_connect(b));

    ok(blkio_start(b));

    /* Calling it twice works */
    ok(blkio_start(b));

    err(blkio_connect(b), -EBUSY);

    blkio_destroy(&b);
    assert(!b);

    return 0;
}
