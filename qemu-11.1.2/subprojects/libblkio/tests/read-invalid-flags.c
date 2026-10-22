// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "read-invalid-flags-XXXXXX";

/*
 * Read with invalid request flags and expect an error.
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
    uint32_t invalid_flags = ~0; /* most bits are reserved and must be zero! */

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

    blkioq_read(q, TEST_FILE_OFFSET, buf, buf_size, NULL, invalid_flags);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.user_data == NULL);
    assert(completion.ret == -EINVAL);
    assert(completion.error_msg);
    assert(strcmp(completion.error_msg,
                  "unsupported bits in request flags") == 0);

    blkioq_read(q, TEST_FILE_OFFSET, buf, buf_size, NULL, BLKIO_REQ_NO_UNMAP);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.user_data == NULL);
    assert(completion.ret == -EINVAL);
    assert(completion.error_msg);
    assert(strcmp(completion.error_msg,
                  "BLKIO_REQ_NO_UNMAP is invalid for this request type") == 0);

    blkio_destroy(&b);
    return 0;
}
