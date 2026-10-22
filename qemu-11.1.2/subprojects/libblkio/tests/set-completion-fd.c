// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <fcntl.h>
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
    TEST_FILE_OFFSET = 0x10000,
    NUM_REQS = 128,
};

static char filename[] = "set-completion-fd-XXXXXX";

int process_completions(struct blkioq *q, struct blkio_completion *completion)
{
    int ret;
    int n = 0;

    /* Suppress completion fd notifications while we process completions */
    blkioq_set_completion_fd_enabled(q, false);

    do {
        ret = blkioq_do_io(q, completion, 0, 1, NULL);
        if (ret == 0) {
            blkioq_set_completion_fd_enabled(q, true);

            /* Re-check for completions to avoid race */
            ret = blkioq_do_io(q, completion, 0, 1, NULL);
            if (ret == 1) {
                blkioq_set_completion_fd_enabled(q, false);
            }
        }
        assert(ret >= 0);
        if (ret == 1) {
            assert(completion->ret == 0);
            n += 1;
        }
    } while (ret == 1);

    return n;
}

int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkioq *q;
    struct blkio_mem_region mem_region;
    struct blkio_completion completion;
    void *buf; /* I/O buffer */
    size_t buf_size;
    int completion_fd;
    int status_flag;
    int n = 0;

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

    /* Switch to blocking mode */
    assert(fcntl(completion_fd, F_SETFL, status_flag & ~O_NONBLOCK) == 0);

    for (size_t i = 0; i < NUM_REQS; ++i) {
        blkioq_write(q, TEST_FILE_OFFSET, buf, buf_size, (void *)i,
                     BLKIO_REQ_FUA);
    }

    /* Submit requests but do not wait */
    assert(blkioq_do_io(q, NULL, 0, 0, NULL) == 0);

    char event_data[8];

    do {
        read(completion_fd, event_data, sizeof(event_data));
        n += process_completions(q, &completion);
    } while (n < NUM_REQS);

    blkio_destroy(&b);
    return 0;
}
