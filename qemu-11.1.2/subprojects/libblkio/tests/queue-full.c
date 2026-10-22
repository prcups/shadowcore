// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_NUM_ENTRIES = 16,
    MAX_COMPLETIONS = 8,
    // NUM_REQS is defined as such to ensure we reproduce the blkio_do_io() hang
    // reported in https://gitlab.com/libblkio/libblkio/-/issues/38
    NUM_REQS = TEST_NUM_ENTRIES * 2 + MAX_COMPLETIONS + 1,
};

static char filename[] = "queue-full-XXXXXX";

/*
 * Verify that more than num-entries requests can be added.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_completion completions[MAX_COMPLETIONS];

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_set_int(b, "num-queues", 1));
    if (driver_is_io_uring(opts.driver)) {
        ok(blkio_set_int(b, "num-entries", TEST_NUM_ENTRIES));
    } else if (driver_is_virtio_blk(opts.driver)) {
        ok(blkio_set_int(b, "queue-size", TEST_NUM_ENTRIES));
    } else {
        skip();
    }
    ok(blkio_start(b));

    q = blkio_get_queue(b, 0);
    assert(q);

    for (int i = 0; i < NUM_REQS; i++) {
        blkioq_flush(q, NULL, 0);
    }

    for (int i = 0; i < NUM_REQS; ) {
        int num = blkioq_do_io(q, completions, 1, MAX_COMPLETIONS, NULL);
        assert(num >= 1);
        i += num;
    }

    blkio_destroy(&b);
    return 0;
}
