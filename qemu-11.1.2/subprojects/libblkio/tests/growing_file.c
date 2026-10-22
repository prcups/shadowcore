// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 0,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "write-XXXXXX";

/*
 * Check that "capacity" is properly updated when we query it.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_mem_region mem_region;
    struct blkio_completion completion;
    void *buf;     /* I/O buffer */
    size_t buf_size;
    uint64_t capacity;
    bool can_grow;

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_get_bool(b, "can-grow", &can_grow));
    skip_if(!can_grow);

    /* Set up I/O buffer */
    buf_size = sysconf(_SC_PAGESIZE);
    ok(blkio_alloc_mem_region(b, &mem_region, buf_size));
    buf = mem_region.addr;

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

    ok(blkio_get_uint64(b, "capacity", &capacity));
    assert(capacity == TEST_FILE_SIZE);

    blkioq_write(q, TEST_FILE_OFFSET, buf, buf_size, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.ret == 0);

    ok(blkio_get_uint64(b, "capacity", &capacity));
    assert(capacity == TEST_FILE_OFFSET + buf_size);

    blkio_destroy(&b);
    return 0;
}
