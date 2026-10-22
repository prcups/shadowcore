// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <sys/mman.h>
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "write-zeroes-XXXXXX";

/*
 * Write a well-known pattern at an offset in a file, then overwrite it with
 * zeroes.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_mem_region mem_region;
    struct blkio_completion completion;
    void *buf;     /* I/O buffer */
    void *pattern; /* reference data at TEST_FILE_OFFSET */
    size_t buf_size;

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    /* Set up I/O buffer */
    buf_size = 3 * sysconf(_SC_PAGESIZE);
    ok(blkio_alloc_mem_region(b, &mem_region, buf_size));
    buf = mem_region.addr;

    /* Initialize pattern buffer */
    pattern = malloc(buf_size);
    assert(pattern);
    memset(pattern, 'A', buf_size);

    memcpy(buf, pattern, buf_size);

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

    blkioq_write(q, TEST_FILE_OFFSET, buf, buf_size, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.ret == 0);

    blkioq_write_zeroes(q, TEST_FILE_OFFSET, buf_size / 3, NULL, 0);

    blkioq_write_zeroes(q, TEST_FILE_OFFSET + buf_size / 3, buf_size / 3,
                        NULL, BLKIO_REQ_FUA);

    blkioq_write_zeroes(q, TEST_FILE_OFFSET + 2 * (buf_size / 3), buf_size / 3,
                        NULL, BLKIO_REQ_NO_UNMAP);

    for (int i = 0; i < 3; ++i) {
        assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
        skip_if(completion.ret == -ENOTSUP);
        assert(completion.ret == 0);
    }

    memset(pattern, '\0', buf_size);

    blkioq_read(q, TEST_FILE_OFFSET, buf, buf_size, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.ret == 0);

    assert(memcmp(buf, pattern, buf_size) == 0);

    blkio_destroy(&b);
    return 0;
}
