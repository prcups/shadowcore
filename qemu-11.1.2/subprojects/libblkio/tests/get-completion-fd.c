// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <unistd.h>
#include <fcntl.h>
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
};

static char filename[] = "get_completion_fd-XXXXXX";

/*
 * Wait for completion by reading from the completion fd.
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
    uint64_t efd_value;
    int completion_fd;
    int status_flag;
    int n;

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    /* Set up I/O buffer */
    buf_size = sysconf(_SC_PAGESIZE);
    ok(blkio_alloc_mem_region(b, &mem_region, buf_size));
    buf = mem_region.addr;

    ok(blkio_set_int(b, "num-queues", 1));
    ok(blkio_start(b));

    ok(blkio_map_mem_region(b, &mem_region));

    q = blkio_get_queue(b, 0);
    assert(q);

    blkioq_set_completion_fd_enabled(q, true);

    completion_fd = blkioq_get_completion_fd(q);
    assert(completion_fd >= 0);

    status_flag = fcntl(completion_fd, F_GETFL);
    assert(status_flag >= 0);

    /* completion fd must be initialized in non-blocking mode */
    assert(status_flag & O_NONBLOCK);

    /* Switch to blocking mode for read(2) below */
    assert(fcntl(completion_fd, F_SETFL, status_flag & ~O_NONBLOCK) == 0);

    blkioq_read(q, TEST_FILE_OFFSET, buf, buf_size, NULL, 0);
    assert(blkioq_do_io(q, NULL, 0, 0, NULL) == 0);

    do {
        assert(read(completion_fd, &efd_value, sizeof(efd_value)) == sizeof(efd_value));
        n = blkioq_do_io(q, &completion, 0, 1, NULL);
    } while (n == 0);

    assert(n == 1);
    assert(completion.ret == 0);

    blkio_destroy(&b);
    return 0;
}
