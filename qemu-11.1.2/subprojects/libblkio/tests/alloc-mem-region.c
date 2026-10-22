// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "alloc-mem-region-XXXXXX";

/*
 * Alloc a memory region, and write/read a well-known pattern.
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

    buf_size = sysconf(_SC_PAGESIZE);

    /* Initialize pattern buffer */
    pattern = malloc(buf_size);
    assert(pattern);
    memset(pattern, 'A', buf_size);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    /* Initialize the memory region with invalid values */
    mem_region = (struct blkio_mem_region) {
        .addr = NULL,
        .iova = UINT64_MAX,
        .len = 0,
        .fd_offset = -1,
        .fd = -1,
        .flags = UINT32_MAX,
    };

    /* Set up I/O buffer */
    ok(blkio_alloc_mem_region(b, &mem_region, buf_size));
    assert(mem_region.addr != NULL);
    assert(mem_region.len >= buf_size);
    assert(mem_region.iova == 0);
    assert(mem_region.flags == 0);

    buf = mem_region.addr;
    memcpy(buf, pattern, buf_size);

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));
    err(blkio_map_mem_region(b, &mem_region), -EINVAL);

    q = blkio_get_queue(b, 0);
    assert(q);

    blkioq_write(q, TEST_FILE_OFFSET, buf, buf_size, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.ret == 0);

    memset(buf, 0, buf_size);

    /* Try to unmap and map again the memory region */
    blkio_unmap_mem_region(b, &mem_region);
    blkio_unmap_mem_region(b, &mem_region);
    ok(blkio_map_mem_region(b, &mem_region));
    err(blkio_map_mem_region(b, &mem_region), -EINVAL);

    blkioq_read(q, TEST_FILE_OFFSET, buf, buf_size, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.ret == 0);
    assert(memcmp(buf, pattern, buf_size) == 0);

    blkio_destroy(&b);
    return 0;
}
