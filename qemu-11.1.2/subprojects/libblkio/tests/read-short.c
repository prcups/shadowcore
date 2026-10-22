// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <sys/mman.h>
#include <stdbool.h>
#include "util.h"

enum {
    TEST_FILE_SIZE = (2 * 1024 * 1024) + 421,
    TEST_FILE_OFFSET = (2 * 1024 * 1024) + 42,
};

#define TEST_PATTERN_GOOD_SIZE (TEST_FILE_SIZE - TEST_FILE_OFFSET)

static char filename[] = "read-short-XXXXXX";

/*
 * Test "short reads" reading after EOF with both blkioq_readv() and
 * blkioq_read().
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
    int mfd;

    parse_generic_opts(&opts, argc, argv);

    /* Set up I/O buffer */
    sub_buf_size = sysconf(_SC_PAGESIZE);
    buf_size = iovcnt * sub_buf_size;
    mfd = memfd_create("buf", MFD_CLOEXEC);
    assert(mfd >= 0);
    assert(ftruncate(mfd, buf_size) == 0);
    buf = mmap(NULL, buf_size, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
    assert(buf != MAP_FAILED);

    mem_region = (struct blkio_mem_region) {
        .addr = buf,
        .len = buf_size,
        .fd_offset = 0,
        .fd = mfd,
    };

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

    /* Initialize pattern buffer */
    pattern = malloc(buf_size);
    assert(pattern);
    memset(pattern, 'A', sub_buf_size);
    memset(pattern + sub_buf_size, 'B', sub_buf_size);
    memset(pattern + 2 * sub_buf_size, 'C', sub_buf_size);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_get_int(b, "max-segments", &max_segments));
    ok(blkio_get_int(b, "fd", &fd));
    assert(fd >= 0);

    assert(pwrite_full(fd, pattern, TEST_PATTERN_GOOD_SIZE , TEST_FILE_OFFSET)
            == TEST_PATTERN_GOOD_SIZE);

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));
    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

    /* Short reads should have 0s after EOF */
    memset(pattern + TEST_PATTERN_GOOD_SIZE, 0,
           buf_size - TEST_PATTERN_GOOD_SIZE);

    if (iovcnt <= max_segments) {
        /* Initialize I/O buffer */
        memset(buf, 'X', buf_size);

        blkioq_readv(q, TEST_FILE_OFFSET, iovecs, iovcnt, NULL, 0);
        assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
        assert(completion.ret == 0);
        assert(memcmp(buf, pattern, buf_size) == 0);
    }

    /* Initialize I/O buffer */
    memset(buf, 'X', buf_size);

    blkioq_read(q, TEST_FILE_OFFSET, buf, buf_size, NULL, 0);
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
    skip_if(completion.ret == -ENOTSUP);
    assert(completion.ret == 0);
    assert(memcmp(buf, pattern, buf_size) == 0);

    blkio_destroy(&b);
    return 0;
}
