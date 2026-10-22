// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "flush-XXXXXX";

/*
 * Verify that blkioq_flush() succeeds.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_completion completion;

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    q = blkio_get_queue(b, 0);
    assert(q);

    blkioq_flush(q, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.ret == 0);

    blkio_destroy(&b);
    return 0;
}
