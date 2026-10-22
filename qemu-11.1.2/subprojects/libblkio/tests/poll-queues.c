// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <fcntl.h>
#include <sys/mman.h>
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "poll-queues-XXXXXX";

/*
 * Verify that poll queues behave as expected, as compared to regular queues.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_mem_region mem_region;
    struct blkio_completion completion;
    void *buf;
    size_t size;
    struct iovec iovec;
    int ret;
    bool supports_poll_queues;

    parse_generic_opts(&opts, argc, argv);

    size = sysconf(_SC_PAGESIZE);

    create(&b, &opts, filename, TEST_FILE_SIZE);

    if (driver_is_io_uring(opts.driver)) {
        ok(blkio_set_bool(b, "direct", true));
    }

    ok(blkio_connect(b));

    ok(blkio_get_bool(b, "supports-poll-queues", &supports_poll_queues));
    skip_if(!supports_poll_queues);

    /* Set up I/O buffer */
    ok(blkio_alloc_mem_region(b, &mem_region, size));
    buf = mem_region.addr;

    iovec = (struct iovec) {
        .iov_base = buf,
        .iov_len = size,
    };

    ok(blkio_set_int(b, "num-queues", 0));
    ok(blkio_set_int(b, "num-poll-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    assert(!blkio_get_queue(b, 0));
    q = blkio_get_poll_queue(b, 0);
    assert(q);

    // ensure "app-level" polling using blkioq_do_io(min_completion = 0) works

    blkioq_read(q, TEST_FILE_OFFSET, buf, size, NULL, 0);

    do {
        ret = blkioq_do_io(q, &completion, 0, 1, NULL);
    } while (ret == 0);
    assert(ret == 1);

    // ensure read/readv/write/writev succeed

    blkioq_read(q, TEST_FILE_OFFSET, buf, size, NULL, 0);
    blkioq_readv(q, TEST_FILE_OFFSET, &iovec, 1, NULL, 0);
    blkioq_write(q, TEST_FILE_OFFSET, buf, size, NULL, 0);
    blkioq_writev(q, TEST_FILE_OFFSET, &iovec, 1, NULL, 0);

    for (int i = 0; i < 4; ++i) {
        assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
        assert(completion.ret == 0);
    }

    // ensure flush/write_zeroes/discard either succeed or fail with -ENOTSUP

    blkioq_flush(q, NULL, 0);
    blkioq_write_zeroes(q, TEST_FILE_OFFSET, size, NULL, 0);
    blkioq_discard(q, TEST_FILE_OFFSET, size, NULL, 0);

    for (int i = 0; i < 3; ++i) {
        assert(blkioq_do_io(q, &completion, 1, 1, NULL) == 1);
        assert(completion.ret == 0 || completion.ret == -ENOTSUP);
    }

    blkio_destroy(&b);
    return 0;
}
