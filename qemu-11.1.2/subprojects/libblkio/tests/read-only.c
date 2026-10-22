// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "read-only-XXXXXX";

/*
 * Ensure that write/discard operations are not allowed on read-only devices.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    bool read_only;
    struct blkio_mem_region mem_region;
    struct blkio_completion completion;
    void *buf;
    size_t size;
    struct iovec iovec;

    parse_generic_opts(&opts, argc, argv);

    size = sysconf(_SC_PAGESIZE);

    create(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_set_bool(b, "read-only", true));

    ok(blkio_connect(b));

    ok(blkio_get_bool(b, "read-only", &read_only));
    assert(read_only);

    /* Set up I/O buffer */
    ok(blkio_alloc_mem_region(b, &mem_region, size));
    buf = mem_region.addr;

    iovec = (struct iovec) {
        .iov_base = buf,
        .iov_len = size,
    };

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

#define should_succeed(call) \
    call; \
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1); \
    assert(completion.ret == 0 || completion.ret == size || \
           completion.ret == -ENOTSUP);

#define should_fail(call) \
    call; \
    assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1); \
    assert(completion.ret == -EBADF || completion.ret == -ENOTSUP);

    /* read */

    should_succeed(blkioq_read(q, 0, buf, size, NULL, 0));

    /* write */

    should_fail(blkioq_write(q, 0, buf, size, NULL, 0));
    should_fail(blkioq_write(q, 0, buf, size, NULL, BLKIO_REQ_FUA));

    /* readv */

    should_succeed(blkioq_readv(q, 0, &iovec, 1, NULL, 0));

    /* writev */

    should_fail(blkioq_writev(q, 0, &iovec, 1, NULL, 0));
    should_fail(blkioq_writev(q, 0, &iovec, 1, NULL, BLKIO_REQ_FUA));

    /* write_zeroes */

    should_fail(blkioq_write_zeroes(q, 0, size, NULL, 0));
    should_fail(blkioq_write_zeroes(q, 0, size, NULL, BLKIO_REQ_NO_UNMAP));
    should_fail(blkioq_write_zeroes(q, 0, size, NULL, BLKIO_REQ_NO_FALLBACK));
    should_fail(blkioq_write_zeroes(q, 0, size, NULL, BLKIO_REQ_NO_UNMAP |
                                                      BLKIO_REQ_NO_FALLBACK));

    /* discard */

    should_fail(blkioq_discard(q, 0, size, NULL, 0));

    /* flush */

    should_succeed(blkioq_flush(q, NULL, 0));

    blkio_destroy(&b);
    return 0;
}
