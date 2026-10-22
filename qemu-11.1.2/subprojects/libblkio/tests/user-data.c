// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
    NUM_REQS = 128,
};

static char filename[] = "user-data-XXXXXX";

/*
 * Submit many requests and check that the user_data returned in the completions
 * is as expected.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_mem_region mem_region;
    struct blkio_completion completion;
    void *buf;
    size_t buf_size;

    parse_generic_opts(&opts, argc, argv);

    buf_size = sysconf(_SC_PAGESIZE);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    /* Set up I/O buffer */
    ok(blkio_alloc_mem_region(b, &mem_region, buf_size));
    buf = mem_region.addr;

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

    for (size_t i = 0; i < NUM_REQS; ++i) {
        switch (i % 3) {
        case 0:
            blkioq_flush(q, (void *)i, 0);
            break;
        case 1:
            blkioq_write(q, TEST_FILE_OFFSET, buf, buf_size, (void *)i, 0);
            break;
        case 2:
            blkioq_write(q, TEST_FILE_OFFSET, buf, buf_size, (void *)i,
                         BLKIO_REQ_FUA);
            break;
        }
    }

    {
        bool seen[NUM_REQS] = { 0 };

        for (size_t i = 0; i < NUM_REQS; ++i) {
            size_t user_data;

            assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
            assert(completion.ret == 0);

            user_data = (size_t)completion.user_data;
            assert(user_data < NUM_REQS);
            assert(!seen[user_data]);

            seen[user_data] = true;
        }
    }

    /* no more outstanding requests */
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == -EINVAL);

    blkio_destroy(&b);
    return 0;
}
