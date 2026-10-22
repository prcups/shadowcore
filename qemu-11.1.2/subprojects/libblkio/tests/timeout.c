// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <limits.h>
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "timeout-XXXXXX";

/*
 * Returns true if the timeout expired, false if the request completed before
 * the timeout.
 */
static bool test_timeout(struct blkioq *q, void *buf, size_t len,
                         struct timespec timeout)
{
    struct blkio_completion completion;
    struct timespec ts = timeout;
    bool expired = false;
    int ret;

    blkioq_write(q, 0, buf, len, NULL, 0);
    ret = blkioq_do_io_interruptible(q, &completion, 1, 1, &ts, NULL);
    if (ret == -ETIME) {
        expired = true;

        /* Now wait for the request to complete */
        ret = blkioq_do_io_interruptible(q, &completion, 1, 1, NULL, NULL);
    }

    /* It's also possible for the request to succeed */
    assert(ret == 1);
    assert(completion.ret == 0);
    return expired;
}

/*
 * Verify that a non-NULL blkioq_do_io_interruptible() timeout works
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkio_mem_region mem_region;
    void *buf;
    uint64_t buf_size;
    struct blkioq *q;
    int req_alignment;

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    /* Set up I/O buffer */
    ok(blkio_get_int(b, "request-alignment", &req_alignment));
    ok(blkio_get_uint64(b, "mem-region-alignment", &buf_size));
    if (buf_size < req_alignment) {
        buf_size = req_alignment;
    }

    ok(blkio_alloc_mem_region(b, &mem_region, buf_size));
    buf = mem_region.addr;
    memset(buf, 0x55, buf_size);

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

    /*
     * Timers are affected by the system load and scheduler. Run this many
     * times in the hope of hitting the timeout expired code path. There is no
     * assert(3) here since timeouts racy and this can fail under CI. Let's
     * just aim for code coverage and showing there is no panic in the code.
     */
    int i;
    for (i = 0; i < 1000; i++) {
        if (test_timeout(q, buf, buf_size,
                    (struct timespec){
                        .tv_sec = 0,
                        .tv_nsec = 1,
                    })) {
            break;
        }
    }

    /* Check a very long timeout doesn't expire */
    struct timespec very_long_timeout = (struct timespec){
        .tv_sec = LONG_MAX,
        .tv_nsec = 0,
    };
    assert(!test_timeout(q, buf, buf_size, very_long_timeout));

    blkio_destroy(&b);
    return 0;
}
