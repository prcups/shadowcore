// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "writev-XXXXXX";

/*
 * Write a well-known pattern at an offset in a file.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_mem_region mem_region;
    struct blkio_completion completion;
    void *buf;     /* I/O buffer */
    struct iovec iovecs[3];
    int iovcnt = sizeof(iovecs) / sizeof(iovecs[0]);
    void *pattern; /* reference data at TEST_FILE_OFFSET */
    size_t buf_size;
    size_t sub_buf_size;
    int fd, max_segments;

    parse_generic_opts(&opts, argc, argv);

    sub_buf_size = sysconf(_SC_PAGESIZE);
    buf_size = iovcnt * sysconf(_SC_PAGESIZE);

    /* Initialize pattern buffer */
    pattern = malloc(buf_size);
    assert(pattern);
    memset(pattern, 'A', sub_buf_size);
    memset(pattern + sub_buf_size, 'B', sub_buf_size);
    memset(pattern + 2 * sub_buf_size, 'C', sub_buf_size);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_get_int(b, "max-segments", &max_segments));
    skip_if(max_segments < iovcnt);

    ok(blkio_get_int(b, "fd", &fd));
    assert(fd >= 0);

    /* Set up I/O buffer */
    ok(blkio_alloc_mem_region(b, &mem_region, buf_size));
    buf = mem_region.addr;

    iovecs[0] = (struct iovec){
        .iov_base = buf,
        .iov_len = sub_buf_size,
    };
    iovecs[1] = (struct iovec){
        .iov_base = buf + sub_buf_size,
        .iov_len = sub_buf_size,
    };
    iovecs[2] = (struct iovec){
        .iov_base = buf + 2 * sub_buf_size,
        .iov_len = sub_buf_size,
    };

    memcpy(buf, pattern, buf_size);

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

    blkioq_writev(q, TEST_FILE_OFFSET, iovecs, iovcnt, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    assert(completion.ret == 0);

    memset(buf, 0, buf_size);
    assert(pread_full(fd, buf, buf_size, TEST_FILE_OFFSET) == buf_size);
    assert(memcmp(buf, pattern, buf_size) == 0);

    blkio_destroy(&b);
    return 0;
}
